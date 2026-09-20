use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::TaskUserInputKind;

pub const DEFAULT_BROWSER_BATCH_CONCURRENCY: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserBatchItemPhase {
    Queued,
    Running,
    Waiting,
    Completed,
    Failed,
    Cancelled,
}

impl BrowserBatchItemPhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserBatchItemSnapshot {
    pub item_id: String,
    pub alias: String,
    pub phase: BrowserBatchItemPhase,
    pub waiting_for_input: Option<TaskUserInputKind>,
    pub message: String,
    pub cancel_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserBatchSnapshot {
    pub task_id: String,
    pub max_concurrency: usize,
    pub items: Vec<BrowserBatchItemSnapshot>,
}

impl BrowserBatchSnapshot {
    pub fn is_terminal(&self) -> bool {
        !self.items.is_empty() && self.items.iter().all(|item| item.phase.is_terminal())
    }

    pub fn running_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.phase == BrowserBatchItemPhase::Running)
            .count()
    }

    pub fn queued_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.phase == BrowserBatchItemPhase::Queued)
            .count()
    }

    pub fn waiting_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.phase == BrowserBatchItemPhase::Waiting)
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserBatchItemLease {
    pub task_id: String,
    pub item_id: String,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserBatchError {
    EmptyTaskId,
    EmptyAlias,
    EmptyBatch,
    DuplicateAlias {
        alias: String,
    },
    BatchAlreadyExists {
        task_id: String,
    },
    BatchNotFound {
        task_id: String,
    },
    ItemNotFound {
        task_id: String,
        item_id: String,
    },
    InvalidTransition {
        item_id: String,
        from: BrowserBatchItemPhase,
        to: BrowserBatchItemPhase,
    },
    InputKindMismatch {
        item_id: String,
        expected: TaskUserInputKind,
        actual: TaskUserInputKind,
    },
}

#[derive(Debug, Clone)]
pub struct BrowserBatchCoordinator {
    max_concurrency: usize,
    inner: Arc<Mutex<BrowserBatchCoordinatorInner>>,
}

#[derive(Debug, Default)]
struct BrowserBatchCoordinatorInner {
    batches: BTreeMap<String, BrowserBatchEntry>,
}

#[derive(Debug)]
struct BrowserBatchEntry {
    items: BTreeMap<String, BrowserBatchItemSnapshot>,
}

impl Default for BrowserBatchCoordinator {
    fn default() -> Self {
        Self::new(DEFAULT_BROWSER_BATCH_CONCURRENCY)
    }
}

impl BrowserBatchCoordinator {
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            max_concurrency: max_concurrency.max(1),
            inner: Arc::new(Mutex::new(BrowserBatchCoordinatorInner::default())),
        }
    }

    pub fn register_batch<I, S>(
        &self,
        task_id: impl Into<String>,
        aliases: I,
    ) -> Result<BrowserBatchSnapshot, BrowserBatchError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let task_id = task_id.into().trim().to_string();
        if task_id.is_empty() {
            return Err(BrowserBatchError::EmptyTaskId);
        }
        let mut items = BTreeMap::new();
        let mut seen_aliases = BTreeSet::new();
        for (index, alias) in aliases.into_iter().enumerate() {
            let alias = alias.into().trim().to_string();
            if alias.is_empty() {
                return Err(BrowserBatchError::EmptyAlias);
            }
            if !seen_aliases.insert(alias.clone()) {
                return Err(BrowserBatchError::DuplicateAlias { alias });
            }
            let item_id = browser_batch_item_id(&task_id, index, &alias);
            items.insert(
                item_id.clone(),
                BrowserBatchItemSnapshot {
                    item_id,
                    alias,
                    phase: BrowserBatchItemPhase::Queued,
                    waiting_for_input: None,
                    message: "等待执行".to_string(),
                    cancel_requested: false,
                },
            );
        }
        if items.is_empty() {
            return Err(BrowserBatchError::EmptyBatch);
        }

        let mut inner = self.lock_inner();
        if inner.batches.contains_key(&task_id) {
            return Err(BrowserBatchError::BatchAlreadyExists { task_id });
        }
        inner
            .batches
            .insert(task_id.clone(), BrowserBatchEntry { items });
        snapshot_locked(&inner, &task_id, self.max_concurrency)
    }

    pub fn claim_available(&self) -> Vec<BrowserBatchItemLease> {
        let mut inner = self.lock_inner();
        let mut available = available_slots(&inner, self.max_concurrency);
        if available == 0 {
            return Vec::new();
        }

        let mut leases = Vec::new();
        for (task_id, batch) in &mut inner.batches {
            for item in batch.items.values_mut() {
                if available == 0 {
                    return leases;
                }
                if item.phase != BrowserBatchItemPhase::Queued || item.cancel_requested {
                    continue;
                }
                item.phase = BrowserBatchItemPhase::Running;
                item.waiting_for_input = None;
                item.message = "正在执行".to_string();
                leases.push(BrowserBatchItemLease {
                    task_id: task_id.clone(),
                    item_id: item.item_id.clone(),
                    alias: item.alias.clone(),
                });
                available -= 1;
            }
        }
        leases
    }

    pub fn claim_available_for(
        &self,
        task_id: &str,
    ) -> Result<Vec<BrowserBatchItemLease>, BrowserBatchError> {
        let mut inner = self.lock_inner();
        let mut available = available_slots(&inner, self.max_concurrency);
        let batch =
            inner
                .batches
                .get_mut(task_id)
                .ok_or_else(|| BrowserBatchError::BatchNotFound {
                    task_id: task_id.to_string(),
                })?;
        if available == 0 {
            return Ok(Vec::new());
        }

        let mut leases = Vec::new();
        for item in batch.items.values_mut() {
            if available == 0 {
                break;
            }
            if item.phase != BrowserBatchItemPhase::Queued || item.cancel_requested {
                continue;
            }
            item.phase = BrowserBatchItemPhase::Running;
            item.waiting_for_input = None;
            item.message = "正在执行".to_string();
            leases.push(BrowserBatchItemLease {
                task_id: task_id.to_string(),
                item_id: item.item_id.clone(),
                alias: item.alias.clone(),
            });
            available -= 1;
        }
        Ok(leases)
    }

    pub fn mark_waiting(
        &self,
        task_id: &str,
        item_id: &str,
        kind: TaskUserInputKind,
        message: impl Into<String>,
    ) -> Result<BrowserBatchSnapshot, BrowserBatchError> {
        self.transition_item(task_id, item_id, BrowserBatchItemPhase::Waiting, |item| {
            item.waiting_for_input = Some(kind);
            item.message = message.into();
        })
    }

    pub fn resume_waiting(
        &self,
        task_id: &str,
        item_id: &str,
        kind: TaskUserInputKind,
    ) -> Result<BrowserBatchSnapshot, BrowserBatchError> {
        let mut inner = self.lock_inner();
        let item = item_mut(&mut inner, task_id, item_id)?;
        if item.phase != BrowserBatchItemPhase::Waiting {
            return Err(BrowserBatchError::InvalidTransition {
                item_id: item_id.to_string(),
                from: item.phase,
                to: BrowserBatchItemPhase::Queued,
            });
        }
        let expected =
            item.waiting_for_input
                .ok_or_else(|| BrowserBatchError::InvalidTransition {
                    item_id: item_id.to_string(),
                    from: item.phase,
                    to: BrowserBatchItemPhase::Queued,
                })?;
        if expected != kind {
            return Err(BrowserBatchError::InputKindMismatch {
                item_id: item_id.to_string(),
                expected,
                actual: kind,
            });
        }
        item.phase = BrowserBatchItemPhase::Running;
        item.waiting_for_input = None;
        item.message = "已收到补充输入，正在继续".to_string();
        snapshot_locked(&inner, task_id, self.max_concurrency)
    }

    pub fn mark_completed(
        &self,
        task_id: &str,
        item_id: &str,
        message: impl Into<String>,
    ) -> Result<BrowserBatchSnapshot, BrowserBatchError> {
        self.transition_item(task_id, item_id, BrowserBatchItemPhase::Completed, |item| {
            item.waiting_for_input = None;
            item.message = message.into();
        })
    }

    pub fn mark_failed(
        &self,
        task_id: &str,
        item_id: &str,
        message: impl Into<String>,
    ) -> Result<BrowserBatchSnapshot, BrowserBatchError> {
        self.transition_owned_item(task_id, item_id, BrowserBatchItemPhase::Failed, |item| {
            item.waiting_for_input = None;
            item.message = message.into();
        })
    }

    pub fn request_item_cancel(
        &self,
        task_id: &str,
        item_id: &str,
    ) -> Result<BrowserBatchSnapshot, BrowserBatchError> {
        let mut inner = self.lock_inner();
        let item = item_mut(&mut inner, task_id, item_id)?;
        if item.phase.is_terminal() {
            return snapshot_locked(&inner, task_id, self.max_concurrency);
        }
        item.cancel_requested = true;
        if item.phase == BrowserBatchItemPhase::Queued {
            item.phase = BrowserBatchItemPhase::Cancelled;
            item.waiting_for_input = None;
            item.message = "已取消".to_string();
        } else if !item.phase.is_terminal() {
            item.message = "正在取消".to_string();
        }
        snapshot_locked(&inner, task_id, self.max_concurrency)
    }

    pub fn mark_cancelled(
        &self,
        task_id: &str,
        item_id: &str,
        message: impl Into<String>,
    ) -> Result<BrowserBatchSnapshot, BrowserBatchError> {
        self.transition_owned_item(task_id, item_id, BrowserBatchItemPhase::Cancelled, |item| {
            item.cancel_requested = true;
            item.waiting_for_input = None;
            item.message = message.into();
        })
    }

    pub fn snapshot(&self, task_id: &str) -> Result<BrowserBatchSnapshot, BrowserBatchError> {
        let inner = self.lock_inner();
        snapshot_locked(&inner, task_id, self.max_concurrency)
    }

    pub fn remove_terminal_batch(
        &self,
        task_id: &str,
    ) -> Result<BrowserBatchSnapshot, BrowserBatchError> {
        let mut inner = self.lock_inner();
        let snapshot = snapshot_locked(&inner, task_id, self.max_concurrency)?;
        if !snapshot.is_terminal() {
            return Err(BrowserBatchError::InvalidTransition {
                item_id: task_id.to_string(),
                from: BrowserBatchItemPhase::Running,
                to: BrowserBatchItemPhase::Completed,
            });
        }
        inner.batches.remove(task_id);
        Ok(snapshot)
    }

    pub fn remove_batch(&self, task_id: &str) -> Result<BrowserBatchSnapshot, BrowserBatchError> {
        let mut inner = self.lock_inner();
        let snapshot = snapshot_locked(&inner, task_id, self.max_concurrency)?;
        inner.batches.remove(task_id);
        Ok(snapshot)
    }

    fn transition_item<F>(
        &self,
        task_id: &str,
        item_id: &str,
        next: BrowserBatchItemPhase,
        update: F,
    ) -> Result<BrowserBatchSnapshot, BrowserBatchError>
    where
        F: FnOnce(&mut BrowserBatchItemSnapshot),
    {
        let mut inner = self.lock_inner();
        let item = item_mut(&mut inner, task_id, item_id)?;
        if item.phase != BrowserBatchItemPhase::Running {
            return Err(BrowserBatchError::InvalidTransition {
                item_id: item_id.to_string(),
                from: item.phase,
                to: next,
            });
        }
        item.phase = next;
        update(item);
        snapshot_locked(&inner, task_id, self.max_concurrency)
    }

    fn transition_owned_item<F>(
        &self,
        task_id: &str,
        item_id: &str,
        next: BrowserBatchItemPhase,
        update: F,
    ) -> Result<BrowserBatchSnapshot, BrowserBatchError>
    where
        F: FnOnce(&mut BrowserBatchItemSnapshot),
    {
        let mut inner = self.lock_inner();
        let item = item_mut(&mut inner, task_id, item_id)?;
        if !matches!(
            item.phase,
            BrowserBatchItemPhase::Running | BrowserBatchItemPhase::Waiting
        ) {
            return Err(BrowserBatchError::InvalidTransition {
                item_id: item_id.to_string(),
                from: item.phase,
                to: next,
            });
        }
        item.phase = next;
        update(item);
        snapshot_locked(&inner, task_id, self.max_concurrency)
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, BrowserBatchCoordinatorInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn item_mut<'a>(
    inner: &'a mut BrowserBatchCoordinatorInner,
    task_id: &str,
    item_id: &str,
) -> Result<&'a mut BrowserBatchItemSnapshot, BrowserBatchError> {
    inner
        .batches
        .get_mut(task_id)
        .ok_or_else(|| BrowserBatchError::BatchNotFound {
            task_id: task_id.to_string(),
        })?
        .items
        .get_mut(item_id)
        .ok_or_else(|| BrowserBatchError::ItemNotFound {
            task_id: task_id.to_string(),
            item_id: item_id.to_string(),
        })
}

fn snapshot_locked(
    inner: &BrowserBatchCoordinatorInner,
    task_id: &str,
    max_concurrency: usize,
) -> Result<BrowserBatchSnapshot, BrowserBatchError> {
    let batch = inner
        .batches
        .get(task_id)
        .ok_or_else(|| BrowserBatchError::BatchNotFound {
            task_id: task_id.to_string(),
        })?;
    Ok(BrowserBatchSnapshot {
        task_id: task_id.to_string(),
        max_concurrency,
        items: batch.items.values().cloned().collect(),
    })
}

fn available_slots(inner: &BrowserBatchCoordinatorInner, max_concurrency: usize) -> usize {
    let active_sessions = inner
        .batches
        .values()
        .flat_map(|batch| batch.items.values())
        .filter(|item| {
            matches!(
                item.phase,
                BrowserBatchItemPhase::Running | BrowserBatchItemPhase::Waiting
            )
        })
        .count();
    max_concurrency.saturating_sub(active_sessions)
}

fn browser_batch_item_id(task_id: &str, index: usize, alias: &str) -> String {
    format!("{task_id}:{index:08}:{alias}")
}
