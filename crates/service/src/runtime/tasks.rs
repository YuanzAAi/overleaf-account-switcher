use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use overleaf_core::{card::looks_like_payment_card_number, TaskEvent, TaskPhase, TaskStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const HIDDEN_TASK_RESULT_VALUE: &str = "[hidden]";
const HIDDEN_PATH_VALUE: &str = "[path hidden]";
const TASK_STARTED_MESSAGE: &str = "任务开始";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTaskPhase {
    Pending,
    Running,
    WaitingForUser,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskProgress {
    pub current: u32,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskLogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskLogEntry {
    pub sequence: u64,
    pub level: TaskLogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskUserInputKind {
    EmailCode,
    NewRegistrationCredentials,
    NewBrowserCredentials,
    CaptchaCompleted,
}

impl TaskUserInputKind {
    pub const ALL: [Self; 4] = [
        Self::EmailCode,
        Self::NewRegistrationCredentials,
        Self::NewBrowserCredentials,
        Self::CaptchaCompleted,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmailCode => "email_code",
            Self::NewRegistrationCredentials => "new_registration_credentials",
            Self::NewBrowserCredentials => "new_browser_credentials",
            Self::CaptchaCompleted => "captcha_completed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskUserInputReceipt {
    pub sequence: u64,
    pub kind: TaskUserInputKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskUserInput {
    pub sequence: u64,
    pub kind: TaskUserInputKind,
    pub item_id: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskWaitingItem {
    pub item_id: String,
    pub alias: String,
    pub kind: TaskUserInputKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskSnapshot {
    pub task_id: String,
    pub name: String,
    pub operation_kind: Option<TaskOperationKind>,
    pub retry_descriptor: Option<TaskRetryDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
    pub phase: ServiceTaskPhase,
    pub message: String,
    pub progress: Option<TaskProgress>,
    pub locked_aliases: Vec<String>,
    pub logs: Vec<TaskLogEntry>,
    pub submitted_inputs: Vec<TaskUserInputReceipt>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub failure_kind: Option<TaskFailureKind>,
    pub recovery_hint: Option<String>,
    pub waiting_for_input: Option<TaskUserInputKind>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waiting_items: Vec<TaskWaitingItem>,
    pub available_actions: Vec<TaskAvailableAction>,
    pub cancel_requested: bool,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskFailureKind {
    Retryable,
    NeedsUserInput,
    NeedsLogin,
    PageChanged,
    Fatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAvailableAction {
    Cancel,
    Retry,
    SubmitEmailCode,
    SubmitNewRegistrationCredentials,
    SubmitNewBrowserCredentials,
    MarkCaptchaCompleted,
}

impl TaskAvailableAction {
    pub const ALL: [Self; 6] = [
        Self::Cancel,
        Self::Retry,
        Self::SubmitEmailCode,
        Self::SubmitNewRegistrationCredentials,
        Self::SubmitNewBrowserCredentials,
        Self::MarkCaptchaCompleted,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cancel => "cancel",
            Self::Retry => "retry",
            Self::SubmitEmailCode => "submit_email_code",
            Self::SubmitNewRegistrationCredentials => "submit_new_registration_credentials",
            Self::SubmitNewBrowserCredentials => "submit_new_browser_credentials",
            Self::MarkCaptchaCompleted => "mark_captcha_completed",
        }
    }

    pub const fn input_kind(self) -> Option<TaskUserInputKind> {
        match self {
            Self::Cancel | Self::Retry => None,
            Self::SubmitEmailCode => Some(TaskUserInputKind::EmailCode),
            Self::SubmitNewRegistrationCredentials => {
                Some(TaskUserInputKind::NewRegistrationCredentials)
            }
            Self::SubmitNewBrowserCredentials => Some(TaskUserInputKind::NewBrowserCredentials),
            Self::MarkCaptchaCompleted => Some(TaskUserInputKind::CaptchaCompleted),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOperationKind {
    Generic,
    BrowserCurrentAccount,
    AccountImport,
    AccountManualCookieImport,
    AccountExport,
    AccountCredentialsAdd,
    AccountCredentialsRefresh,
    AccountRegistration,
    AccountGitTokenRefresh,
    AccountGitTokenGenerate,
    BrowserLogin,
    AccountSwitchPlan,
    AccountSwitchProjectPreview,
    AccountSwitchExecute,
    RemoteProjectCleanupPreview,
    AccountRemove,
    LocalPasswordUpdate,
    OverleafPasswordChange,
}

impl TaskOperationKind {
    pub const ALL: [Self; 18] = [
        Self::Generic,
        Self::BrowserCurrentAccount,
        Self::AccountImport,
        Self::AccountManualCookieImport,
        Self::AccountExport,
        Self::AccountCredentialsAdd,
        Self::AccountCredentialsRefresh,
        Self::AccountRegistration,
        Self::AccountGitTokenRefresh,
        Self::AccountGitTokenGenerate,
        Self::BrowserLogin,
        Self::AccountSwitchPlan,
        Self::AccountSwitchProjectPreview,
        Self::AccountSwitchExecute,
        Self::RemoteProjectCleanupPreview,
        Self::AccountRemove,
        Self::LocalPasswordUpdate,
        Self::OverleafPasswordChange,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::BrowserCurrentAccount => "browser_current_account",
            Self::AccountImport => "account_import",
            Self::AccountManualCookieImport => "account_manual_cookie_import",
            Self::AccountExport => "account_export",
            Self::AccountCredentialsAdd => "account_credentials_add",
            Self::AccountCredentialsRefresh => "account_credentials_refresh",
            Self::AccountRegistration => "account_registration",
            Self::AccountGitTokenRefresh => "account_git_token_refresh",
            Self::AccountGitTokenGenerate => "account_git_token_generate",
            Self::BrowserLogin => "browser_login",
            Self::AccountSwitchPlan => "account_switch_plan",
            Self::AccountSwitchProjectPreview => "account_switch_project_preview",
            Self::AccountSwitchExecute => "account_switch_execute",
            Self::RemoteProjectCleanupPreview => "remote_project_cleanup_preview",
            Self::AccountRemove => "account_remove",
            Self::LocalPasswordUpdate => "local_password_update",
            Self::OverleafPasswordChange => "overleaf_password_change",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskReplaySafety {
    Safe,
    RequiresConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskRetryDescriptor {
    pub operation_kind: TaskOperationKind,
    pub aliases: Vec<String>,
    pub replay_safety: TaskReplaySafety,
    #[serde(skip_serializing_if = "TaskRetryPayload::is_empty")]
    pub payload: TaskRetryPayload,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TaskRetryPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migrate_projects: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_skills: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_alias: Option<String>,
}

impl TaskRetryPayload {
    fn is_empty(&self) -> bool {
        self.migrate_projects.is_none()
            && self.sync_skills.is_none()
            && self.project_ids.is_none()
            && self.source_alias.is_none()
    }
}

#[derive(Debug, Clone, Default)]
pub struct TaskSnapshotFeed {
    snapshots: Arc<Mutex<Vec<TaskSnapshot>>>,
}

impl TaskSnapshotFeed {
    pub fn list_snapshots(&self) -> Vec<TaskSnapshot> {
        self.snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn replace(&self, snapshots: Vec<TaskSnapshot>) {
        *self
            .snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshots;
    }
}

#[derive(Debug)]
pub struct TaskStateStore {
    max_logs_per_task: usize,
    next_sequence: u64,
    tasks: BTreeMap<String, TaskSnapshot>,
    account_locks: BTreeMap<String, String>,
    user_inputs: BTreeMap<String, VecDeque<TaskUserInput>>,
    snapshot_feed: TaskSnapshotFeed,
}

impl Clone for TaskStateStore {
    fn clone(&self) -> Self {
        let cloned = Self {
            max_logs_per_task: self.max_logs_per_task,
            next_sequence: self.next_sequence,
            tasks: self.tasks.clone(),
            account_locks: self.account_locks.clone(),
            user_inputs: self.user_inputs.clone(),
            snapshot_feed: TaskSnapshotFeed::default(),
        };
        cloned.publish_snapshots();
        cloned
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskStateError {
    TaskAlreadyExists {
        task_id: String,
    },
    TaskNotFound {
        task_id: String,
    },
    InvalidProgress {
        current: u32,
        total: u32,
    },
    InvalidUserInput {
        message: String,
    },
    AccountAlreadyLocked {
        alias: String,
        task_id: String,
    },
    TerminalTask {
        task_id: String,
        phase: ServiceTaskPhase,
    },
}

impl TaskStateStore {
    pub fn new(max_logs_per_task: usize) -> Self {
        Self {
            max_logs_per_task,
            next_sequence: 1,
            tasks: BTreeMap::new(),
            account_locks: BTreeMap::new(),
            user_inputs: BTreeMap::new(),
            snapshot_feed: TaskSnapshotFeed::default(),
        }
    }

    pub fn snapshot_feed(&self) -> TaskSnapshotFeed {
        self.snapshot_feed.clone()
    }

    pub fn start_task(
        &mut self,
        task_id: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        self.start_task_with_locks_and_operation(task_id, name, Vec::<String>::new(), None)
    }

    pub fn start_task_with_operation(
        &mut self,
        task_id: impl Into<String>,
        name: impl Into<String>,
        operation_kind: Option<TaskOperationKind>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        self.start_task_with_locks_and_operation(
            task_id,
            name,
            Vec::<String>::new(),
            operation_kind,
        )
    }

    pub fn start_task_with_locks<I, S>(
        &mut self,
        task_id: impl Into<String>,
        name: impl Into<String>,
        locked_aliases: I,
    ) -> Result<TaskSnapshot, TaskStateError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.start_task_with_locks_and_operation(task_id, name, locked_aliases, None)
    }

    pub fn start_task_with_locks_and_operation<I, S>(
        &mut self,
        task_id: impl Into<String>,
        name: impl Into<String>,
        locked_aliases: I,
        operation_kind: Option<TaskOperationKind>,
    ) -> Result<TaskSnapshot, TaskStateError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.start_task_with_locks_operation_and_retry_payload(
            task_id,
            name,
            locked_aliases,
            operation_kind,
            TaskRetryPayload::default(),
        )
    }

    pub fn start_task_with_locks_operation_and_retry_payload<I, S>(
        &mut self,
        task_id: impl Into<String>,
        name: impl Into<String>,
        locked_aliases: I,
        operation_kind: Option<TaskOperationKind>,
        retry_payload: TaskRetryPayload,
    ) -> Result<TaskSnapshot, TaskStateError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let task_id = task_id.into();
        if self.tasks.contains_key(&task_id) {
            return Err(TaskStateError::TaskAlreadyExists { task_id });
        }

        let locked_aliases = normalize_locked_aliases(locked_aliases);
        for alias in &locked_aliases {
            if let Some(owner) = self.account_locks.get(alias) {
                return Err(TaskStateError::AccountAlreadyLocked {
                    alias: alias.clone(),
                    task_id: owner.clone(),
                });
            }
        }

        let name = sanitize_task_text(name.into());
        let mut snapshot = TaskSnapshot {
            task_id: task_id.clone(),
            name: name.clone(),
            operation_kind,
            retry_of: None,
            resolved_by: None,
            retry_descriptor: retry_descriptor_for_operation(
                operation_kind,
                &locked_aliases,
                retry_payload,
            ),
            phase: ServiceTaskPhase::Running,
            message: name,
            progress: None,
            locked_aliases: locked_aliases.clone(),
            logs: Vec::new(),
            submitted_inputs: Vec::new(),
            result: None,
            error: None,
            failure_kind: None,
            recovery_hint: None,
            waiting_for_input: None,
            waiting_items: Vec::new(),
            available_actions: Vec::new(),
            cancel_requested: false,
            last_sequence: 0,
        };
        refresh_available_actions(&mut snapshot);
        for alias in &locked_aliases {
            self.account_locks.insert(alias.clone(), task_id.clone());
        }
        self.tasks.insert(task_id.clone(), snapshot);
        self.append_log(&task_id, TaskLogLevel::Info, TASK_STARTED_MESSAGE)
    }

    pub fn apply_core_event(
        &mut self,
        task_id: impl AsRef<str>,
        event: TaskEvent,
    ) -> Result<TaskSnapshot, TaskStateError> {
        let task_id = task_id.as_ref();
        let result = match event {
            TaskEvent::Started { name } => {
                if self.tasks.contains_key(task_id) {
                    self.ensure_task_can_change_state(task_id)?;
                    let task = self.task_mut(task_id)?;
                    let name = sanitize_task_text(name);
                    task.name = name.clone();
                    task.phase = ServiceTaskPhase::Running;
                    task.message = name;
                    task.error = None;
                    task.failure_kind = None;
                    task.recovery_hint = None;
                    task.waiting_for_input = None;
                    task.waiting_items.clear();
                    refresh_available_actions(task);
                    self.append_log(task_id, TaskLogLevel::Info, TASK_STARTED_MESSAGE)
                } else {
                    self.start_task(task_id.to_string(), name)
                }
            }
            TaskEvent::Progress(status) => self.update_status(task_id, status),
            TaskEvent::Finished(status) => self.update_status(task_id, status),
        };
        if result.is_ok() {
            self.publish_snapshots();
        }
        result
    }

    pub fn set_progress(
        &mut self,
        task_id: impl AsRef<str>,
        current: u32,
        total: u32,
        message: impl Into<String>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        if total == 0 || current > total {
            return Err(TaskStateError::InvalidProgress { current, total });
        }

        self.ensure_task_can_change_state(task_id.as_ref())?;
        let sequence = self.next_task_sequence();
        let message = sanitize_task_text(message.into());
        let task = self.task_mut(task_id.as_ref())?;
        task.phase = ServiceTaskPhase::Running;
        task.message = message;
        task.progress = Some(TaskProgress { current, total });
        task.error = None;
        task.failure_kind = None;
        task.recovery_hint = None;
        task.waiting_for_input = None;
        refresh_available_actions(task);
        task.last_sequence = sequence;
        let snapshot = task.clone();
        self.publish_snapshots();
        Ok(snapshot)
    }

    pub fn set_progress_value(
        &mut self,
        task_id: impl AsRef<str>,
        current: u32,
        total: u32,
    ) -> Result<TaskSnapshot, TaskStateError> {
        if total == 0 || current > total {
            return Err(TaskStateError::InvalidProgress { current, total });
        }

        self.ensure_task_can_change_state(task_id.as_ref())?;
        let sequence = self.next_task_sequence();
        let task = self.task_mut(task_id.as_ref())?;
        task.progress = Some(TaskProgress { current, total });
        refresh_available_actions(task);
        task.last_sequence = sequence;
        let snapshot = task.clone();
        self.publish_snapshots();
        Ok(snapshot)
    }

    pub fn request_cancel(
        &mut self,
        task_id: impl AsRef<str>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        self.ensure_task_can_change_state(task_id.as_ref())?;
        let sequence = self.next_task_sequence();
        let task = self.task_mut(task_id.as_ref())?;
        task.cancel_requested = true;
        refresh_available_actions(task);
        task.last_sequence = sequence;
        let snapshot = task.clone();
        self.publish_snapshots();
        Ok(snapshot)
    }

    pub(crate) fn link_retry(&mut self, source: &str, retry: &str) -> Result<(), TaskStateError> {
        self.snapshot(source)?;
        let sequence = self.next_task_sequence();
        let task = self.task_mut(retry)?;
        task.retry_of = Some(source.to_string());
        task.last_sequence = sequence;
        self.resolve_retry_chain(retry);
        self.publish_snapshots();
        Ok(())
    }

    fn resolve_retry_chain(&mut self, retry: &str) {
        let Some(task) = self
            .tasks
            .get(retry)
            .filter(|task| task.phase == ServiceTaskPhase::Completed)
        else {
            return;
        };
        let mut source = task.retry_of.clone();
        while let Some(id) = source {
            let sequence = self.next_task_sequence();
            let Some(task) = self.tasks.get_mut(&id) else {
                break;
            };
            source = task.retry_of.clone();
            if task.phase == ServiceTaskPhase::Failed {
                task.resolved_by = Some(retry.to_string());
                task.last_sequence = sequence;
                refresh_available_actions(task);
            }
        }
    }

    pub fn complete_task(
        &mut self,
        task_id: impl AsRef<str>,
        message: impl Into<String>,
        result: Option<Value>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        let task_id = task_id.as_ref();
        self.ensure_task_can_change_state(task_id)?;
        let sequence = self.next_task_sequence();
        let max_logs_per_task = self.max_logs_per_task;
        let message = sanitize_task_text(message.into());
        let snapshot = {
            let task = self.task_mut(task_id)?;
            task.phase = ServiceTaskPhase::Completed;
            task.message = message.clone();
            task.result = result.map(sanitize_task_result_value);
            task.error = None;
            task.failure_kind = None;
            task.recovery_hint = None;
            task.waiting_for_input = None;
            task.waiting_items.clear();
            refresh_available_actions(task);
            task.last_sequence = sequence;
            push_log_entry(
                task,
                max_logs_per_task,
                sequence,
                TaskLogLevel::Info,
                format!("任务完成: {message}"),
            );
            task.clone()
        };
        self.release_locks_for_task(task_id);
        self.resolve_retry_chain(task_id);
        self.publish_snapshots();
        Ok(snapshot)
    }

    pub fn fail_task(
        &mut self,
        task_id: impl AsRef<str>,
        error: impl Into<String>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        self.fail_task_with_result(task_id, error, None)
    }

    pub fn fail_task_with_result(
        &mut self,
        task_id: impl AsRef<str>,
        error: impl Into<String>,
        result: Option<Value>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        let task_id = task_id.as_ref();
        self.ensure_task_can_change_state(task_id)?;
        let error = error.into();
        let safe_error = sanitize_task_text(error.clone());
        let sequence = self.next_task_sequence();
        let max_logs_per_task = self.max_logs_per_task;
        let failure_kind = classify_task_failure(&error);
        let snapshot = {
            let task = self.task_mut(task_id)?;
            task.phase = ServiceTaskPhase::Failed;
            task.message = safe_error.clone();
            task.failure_kind = Some(failure_kind);
            task.recovery_hint = Some(recovery_hint_for_failure_kind(failure_kind).to_string());
            task.error = Some(safe_error.clone());
            task.result = result.map(sanitize_task_result_value);
            task.waiting_for_input = None;
            task.waiting_items.clear();
            refresh_available_actions(task);
            task.last_sequence = sequence;
            push_log_entry(
                task,
                max_logs_per_task,
                sequence,
                TaskLogLevel::Error,
                format!("任务失败: {safe_error}"),
            );
            task.clone()
        };
        self.release_locks_for_task(task_id);
        self.publish_snapshots();
        Ok(snapshot)
    }

    pub fn cancel_task(
        &mut self,
        task_id: impl AsRef<str>,
        message: impl Into<String>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        let task_id = task_id.as_ref();
        self.ensure_task_can_change_state(task_id)?;
        let sequence = self.next_task_sequence();
        let max_logs_per_task = self.max_logs_per_task;
        let message = sanitize_task_text(message.into());
        let snapshot = {
            let task = self.task_mut(task_id)?;
            task.phase = ServiceTaskPhase::Cancelled;
            task.message = message.clone();
            task.failure_kind = None;
            task.recovery_hint = None;
            task.waiting_for_input = None;
            task.waiting_items.clear();
            task.cancel_requested = true;
            refresh_available_actions(task);
            task.last_sequence = sequence;
            push_log_entry(
                task,
                max_logs_per_task,
                sequence,
                TaskLogLevel::Warning,
                format!("任务取消: {message}"),
            );
            task.clone()
        };
        self.release_locks_for_task(task_id);
        self.publish_snapshots();
        Ok(snapshot)
    }

    pub fn append_log(
        &mut self,
        task_id: impl AsRef<str>,
        level: TaskLogLevel,
        message: impl Into<String>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        let sequence = self.next_task_sequence();
        let max_logs_per_task = self.max_logs_per_task;
        let task = self.task_mut(task_id.as_ref())?;
        task.last_sequence = sequence;
        push_log_entry(task, max_logs_per_task, sequence, level, message);
        let snapshot = task.clone();
        self.publish_snapshots();
        Ok(snapshot)
    }

    pub fn submit_user_input(
        &mut self,
        task_id: impl AsRef<str>,
        kind: TaskUserInputKind,
        value: impl Into<String>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        let task_id = task_id.as_ref();
        let value = value.into();
        let value = value.trim().to_string();
        if value.is_empty() {
            return Err(TaskStateError::InvalidUserInput {
                message: "empty user input".to_string(),
            });
        }

        let sequence = self.next_task_sequence();
        let snapshot = {
            let task = self.task_mut(task_id)?;
            if task.phase.is_terminal() {
                return Err(TaskStateError::InvalidUserInput {
                    message: "terminal task cannot receive user input".to_string(),
                });
            }
            if task.phase != ServiceTaskPhase::WaitingForUser {
                return Err(TaskStateError::InvalidUserInput {
                    message: "task is not waiting for user input".to_string(),
                });
            }
            let expected = task.waiting_for_input;
            let Some(expected) = expected else {
                return Err(TaskStateError::InvalidUserInput {
                    message: "task is not waiting for a supported user input".to_string(),
                });
            };
            if expected != kind {
                return Err(TaskStateError::InvalidUserInput {
                    message: format!("task is waiting for {expected:?}, not {kind:?}"),
                });
            }
            task.last_sequence = sequence;
            task.submitted_inputs.push(TaskUserInputReceipt {
                sequence,
                kind,
                item_id: None,
            });
            refresh_available_actions(task);
            task.clone()
        };
        self.user_inputs
            .entry(task_id.to_string())
            .or_default()
            .push_back(TaskUserInput {
                sequence,
                kind,
                item_id: None,
                value,
            });
        self.append_log(
            task_id,
            TaskLogLevel::Info,
            format!("User input received: {kind:?}"),
        )
        .or(Ok(snapshot))
    }

    pub fn submit_item_user_input(
        &mut self,
        task_id: impl AsRef<str>,
        item_id: impl AsRef<str>,
        kind: TaskUserInputKind,
        value: impl Into<String>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        let task_id = task_id.as_ref();
        let item_id = item_id.as_ref().trim();
        let value = value.into().trim().to_string();
        if item_id.is_empty() || value.is_empty() {
            return Err(TaskStateError::InvalidUserInput {
                message: "empty item id or user input".to_string(),
            });
        }

        let sequence = self.next_task_sequence();
        let snapshot = {
            let task = self.task_mut(task_id)?;
            if task.phase.is_terminal() {
                return Err(TaskStateError::InvalidUserInput {
                    message: "terminal task cannot receive user input".to_string(),
                });
            }
            let Some(position) = task
                .waiting_items
                .iter()
                .position(|item| item.item_id == item_id)
            else {
                return Err(TaskStateError::InvalidUserInput {
                    message: format!("task item is not waiting for input: {item_id}"),
                });
            };
            let expected = task.waiting_items[position].kind;
            if expected != kind {
                return Err(TaskStateError::InvalidUserInput {
                    message: format!("task item is waiting for {expected:?}, not {kind:?}"),
                });
            }
            task.waiting_items.remove(position);
            task.last_sequence = sequence;
            task.submitted_inputs.push(TaskUserInputReceipt {
                sequence,
                kind,
                item_id: Some(item_id.to_string()),
            });
            if task.waiting_items.is_empty() && task.phase == ServiceTaskPhase::WaitingForUser {
                task.phase = ServiceTaskPhase::Running;
                task.message = "已收到补充输入，正在继续".to_string();
            }
            refresh_available_actions(task);
            task.clone()
        };
        self.user_inputs
            .entry(task_id.to_string())
            .or_default()
            .push_back(TaskUserInput {
                sequence,
                kind,
                item_id: Some(item_id.to_string()),
                value,
            });
        self.append_log(
            task_id,
            TaskLogLevel::Info,
            format!("批次账号补充输入已接收: {item_id}"),
        )
        .or(Ok(snapshot))
    }

    pub fn wait_for_user_input(
        &mut self,
        task_id: impl AsRef<str>,
        kind: TaskUserInputKind,
        message: impl Into<String>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        let task_id = task_id.as_ref();
        self.ensure_task_can_change_state(task_id)?;
        let sequence = self.next_task_sequence();
        let message = sanitize_task_text(message.into());
        let snapshot = {
            let task = self.task_mut(task_id)?;
            task.phase = ServiceTaskPhase::WaitingForUser;
            task.message = message;
            task.error = None;
            task.failure_kind = None;
            task.recovery_hint = None;
            task.waiting_for_input = Some(kind);
            refresh_available_actions(task);
            task.last_sequence = sequence;
            task.clone()
        };
        self.publish_snapshots();
        Ok(snapshot)
    }

    pub fn wait_for_item_input(
        &mut self,
        task_id: impl AsRef<str>,
        item_id: impl Into<String>,
        alias: impl Into<String>,
        kind: TaskUserInputKind,
        message: impl Into<String>,
        all_remaining_items_waiting: bool,
    ) -> Result<TaskSnapshot, TaskStateError> {
        let task_id = task_id.as_ref();
        self.ensure_task_can_change_state(task_id)?;
        let item_id = item_id.into().trim().to_string();
        let alias = alias.into().trim().to_string();
        if item_id.is_empty() || alias.is_empty() {
            return Err(TaskStateError::InvalidUserInput {
                message: "empty batch item id or alias".to_string(),
            });
        }
        let sequence = self.next_task_sequence();
        let message = sanitize_task_text(message.into());
        let snapshot = {
            let task = self.task_mut(task_id)?;
            let waiting_item = TaskWaitingItem {
                item_id: item_id.clone(),
                alias,
                kind,
                message: message.clone(),
            };
            match task
                .waiting_items
                .iter()
                .position(|item| item.item_id == item_id)
            {
                Some(position) => task.waiting_items[position] = waiting_item,
                None => task.waiting_items.push(waiting_item),
            }
            if all_remaining_items_waiting {
                task.phase = ServiceTaskPhase::WaitingForUser;
                task.message = message;
            }
            task.error = None;
            task.failure_kind = None;
            task.recovery_hint = None;
            refresh_available_actions(task);
            task.last_sequence = sequence;
            task.clone()
        };
        self.publish_snapshots();
        Ok(snapshot)
    }

    pub fn take_user_input(
        &mut self,
        task_id: impl AsRef<str>,
        kind: TaskUserInputKind,
    ) -> Option<TaskUserInput> {
        let task_id = task_id.as_ref();
        let queue_is_empty;
        let input = {
            let queue = self.user_inputs.get_mut(task_id)?;
            let position = queue
                .iter()
                .position(|input| input.kind == kind && input.item_id.is_none())?;
            let input = queue.remove(position);
            queue_is_empty = queue.is_empty();
            input
        };
        if queue_is_empty {
            self.user_inputs.remove(task_id);
        }
        input
    }

    pub fn take_item_user_input(
        &mut self,
        task_id: impl AsRef<str>,
        item_id: impl AsRef<str>,
        kind: TaskUserInputKind,
    ) -> Option<TaskUserInput> {
        let task_id = task_id.as_ref();
        let item_id = item_id.as_ref();
        let queue_is_empty;
        let input = {
            let queue = self.user_inputs.get_mut(task_id)?;
            let position = queue.iter().position(|input| {
                input.kind == kind && input.item_id.as_deref() == Some(item_id)
            })?;
            let input = queue.remove(position);
            queue_is_empty = queue.is_empty();
            input
        };
        if queue_is_empty {
            self.user_inputs.remove(task_id);
        }
        input
    }

    pub fn discard_user_inputs(
        &mut self,
        task_id: impl AsRef<str>,
        kind: TaskUserInputKind,
    ) -> usize {
        let task_id = task_id.as_ref();
        let Some(queue) = self.user_inputs.get_mut(task_id) else {
            return 0;
        };
        let before = queue.len();
        queue.retain(|input| input.kind != kind || input.item_id.is_some());
        let removed = before - queue.len();
        if queue.is_empty() {
            self.user_inputs.remove(task_id);
        }
        removed
    }

    pub fn discard_item_user_inputs(
        &mut self,
        task_id: impl AsRef<str>,
        item_id: impl AsRef<str>,
        kind: TaskUserInputKind,
    ) -> usize {
        let task_id = task_id.as_ref();
        let item_id = item_id.as_ref();
        let Some(queue) = self.user_inputs.get_mut(task_id) else {
            return 0;
        };
        let before = queue.len();
        queue.retain(|input| input.kind != kind || input.item_id.as_deref() != Some(item_id));
        let removed = before - queue.len();
        if queue.is_empty() {
            self.user_inputs.remove(task_id);
        }
        removed
    }

    pub fn snapshot(&self, task_id: impl AsRef<str>) -> Result<TaskSnapshot, TaskStateError> {
        self.tasks
            .get(task_id.as_ref())
            .cloned()
            .ok_or_else(|| TaskStateError::TaskNotFound {
                task_id: task_id.as_ref().to_string(),
            })
    }

    pub fn list_snapshots(&self) -> Vec<TaskSnapshot> {
        self.tasks.values().cloned().collect()
    }

    pub fn list_snapshots_since(&self, since_sequence: u64) -> Vec<TaskSnapshot> {
        self.tasks
            .values()
            .filter(|snapshot| snapshot.last_sequence > since_sequence)
            .cloned()
            .collect()
    }

    pub fn clear_terminal_tasks(&mut self) -> Vec<String> {
        let task_ids: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_task_id, snapshot)| snapshot.phase.is_terminal())
            .map(|(task_id, _snapshot)| task_id.clone())
            .collect();
        for task_id in &task_ids {
            self.release_locks_for_task(task_id);
            self.tasks.remove(task_id);
            self.user_inputs.remove(task_id);
        }
        self.publish_snapshots();
        task_ids
    }

    pub fn account_lock_owner(&self, alias: impl AsRef<str>) -> Option<&str> {
        self.account_locks.get(alias.as_ref()).map(String::as_str)
    }

    fn update_status(
        &mut self,
        task_id: &str,
        status: TaskStatus,
    ) -> Result<TaskSnapshot, TaskStateError> {
        self.ensure_task_can_change_state(task_id)?;
        let sequence = self.next_task_sequence();
        let phase = ServiceTaskPhase::from(status.phase);
        let failure_kind = if phase == ServiceTaskPhase::Failed {
            Some(classify_task_failure(&status.message))
        } else {
            None
        };
        let safe_message = sanitize_task_text(status.message.clone());
        let max_logs_per_task = self.max_logs_per_task;
        let snapshot = {
            let task = self.task_mut(task_id)?;
            task.phase = phase;
            task.message = safe_message.clone();
            if let Some(kind) = failure_kind {
                task.failure_kind = Some(kind);
                task.recovery_hint = Some(recovery_hint_for_failure_kind(kind).to_string());
                task.error = Some(safe_message.clone());
                task.waiting_for_input = None;
            } else {
                task.failure_kind = None;
                task.recovery_hint = None;
                task.waiting_for_input = None;
                if task.phase != ServiceTaskPhase::Cancelled {
                    task.error = None;
                }
            }
            refresh_available_actions(task);
            task.last_sequence = sequence;
            if task.phase.is_terminal() {
                task.waiting_items.clear();
                let (level, prefix) = match task.phase {
                    ServiceTaskPhase::Completed => (TaskLogLevel::Info, "任务完成"),
                    ServiceTaskPhase::Failed => (TaskLogLevel::Error, "任务失败"),
                    ServiceTaskPhase::Cancelled => (TaskLogLevel::Warning, "任务取消"),
                    ServiceTaskPhase::Pending
                    | ServiceTaskPhase::Running
                    | ServiceTaskPhase::WaitingForUser => unreachable!("checked terminal phase"),
                };
                push_log_entry(
                    task,
                    max_logs_per_task,
                    sequence,
                    level,
                    format!("{prefix}: {safe_message}"),
                );
            }
            task.clone()
        };
        if snapshot.phase.is_terminal() {
            self.release_locks_for_task(task_id);
        }
        Ok(snapshot)
    }

    fn ensure_task_can_change_state(&self, task_id: &str) -> Result<(), TaskStateError> {
        let task = self
            .tasks
            .get(task_id)
            .ok_or_else(|| TaskStateError::TaskNotFound {
                task_id: task_id.to_string(),
            })?;
        if task.phase.is_terminal() {
            return Err(TaskStateError::TerminalTask {
                task_id: task_id.to_string(),
                phase: task.phase.clone(),
            });
        }
        Ok(())
    }

    fn next_task_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        sequence
    }

    fn publish_snapshots(&self) {
        self.snapshot_feed.replace(self.list_snapshots());
    }

    fn task_mut(&mut self, task_id: &str) -> Result<&mut TaskSnapshot, TaskStateError> {
        self.tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskStateError::TaskNotFound {
                task_id: task_id.to_string(),
            })
    }

    fn release_locks_for_task(&mut self, task_id: &str) {
        let locked_aliases = self
            .tasks
            .get(task_id)
            .map(|task| task.locked_aliases.clone())
            .unwrap_or_default();
        for alias in locked_aliases {
            if self.account_locks.get(&alias).map(String::as_str) == Some(task_id) {
                self.account_locks.remove(&alias);
            }
        }
    }
}

fn normalize_locked_aliases<I, S>(aliases: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for alias in aliases {
        let alias = alias.into();
        let alias = alias.trim();
        if !alias.is_empty() && seen.insert(alias.to_string()) {
            // 锁集合只负责去重；快照和重试载荷必须保留用户的选择顺序。
            normalized.push(alias.to_string());
        }
    }
    normalized
}

fn push_log_entry(
    task: &mut TaskSnapshot,
    max_logs_per_task: usize,
    sequence: u64,
    level: TaskLogLevel,
    message: impl Into<String>,
) {
    task.logs.push(TaskLogEntry {
        sequence,
        level,
        message: sanitize_task_text(message.into()),
    });
    if max_logs_per_task > 0 && task.logs.len() > max_logs_per_task {
        let excess = task.logs.len() - max_logs_per_task;
        task.logs.drain(0..excess);
    }
}

fn retry_descriptor_for_operation(
    operation_kind: Option<TaskOperationKind>,
    aliases: &[String],
    retry_payload: TaskRetryPayload,
) -> Option<TaskRetryDescriptor> {
    let operation_kind = operation_kind?;
    let replay_safety = match operation_kind {
        TaskOperationKind::BrowserCurrentAccount
        | TaskOperationKind::AccountGitTokenRefresh
        | TaskOperationKind::AccountSwitchPlan
        | TaskOperationKind::AccountSwitchProjectPreview
        | TaskOperationKind::RemoteProjectCleanupPreview => TaskReplaySafety::Safe,
        TaskOperationKind::AccountGitTokenGenerate
        | TaskOperationKind::BrowserLogin
        | TaskOperationKind::AccountSwitchExecute => TaskReplaySafety::RequiresConfirmation,
        _ => return None,
    };
    if operation_kind != TaskOperationKind::BrowserCurrentAccount && aliases.is_empty() {
        return None;
    }
    Some(TaskRetryDescriptor {
        operation_kind,
        aliases: aliases.to_vec(),
        replay_safety,
        payload: retry_payload,
    })
}

fn sanitize_task_result_value(value: Value) -> Value {
    match value {
        Value::Array(values) => {
            Value::Array(values.into_iter().map(sanitize_task_result_value).collect())
        }
        Value::Object(fields) => {
            let mut sanitized = Map::new();
            for (key, value) in fields {
                let value = if is_sensitive_task_result_key(&key)
                    || is_private_path_task_result_key(&key)
                {
                    Value::String(HIDDEN_TASK_RESULT_VALUE.to_string())
                } else if key == "project_id"
                    && value.as_str().is_some_and(|id| {
                        id.len() == 24 && id.bytes().all(|c| c.is_ascii_hexdigit())
                    })
                {
                    // 项目 ID 中的数字片段可能恰好通过卡号校验。
                    value
                } else {
                    sanitize_task_result_value(value)
                };
                sanitized.insert(key, value);
            }
            Value::Object(sanitized)
        }
        Value::String(text) if looks_like_sensitive_task_result_string(&text) => {
            Value::String(HIDDEN_TASK_RESULT_VALUE.to_string())
        }
        Value::String(text) => Value::String(sanitize_task_text(text)),
        other => other,
    }
}

pub(crate) fn sanitize_task_text(text: String) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return text;
    }

    let mut changed = false;
    let mut redact_next = false;
    let mut pending_assignment_key = false;
    let mut sanitized = Vec::with_capacity(tokens.len());
    for token in tokens {
        if redact_next {
            if is_task_text_separator_token(token) {
                sanitized.push(token.to_string());
                pending_assignment_key = false;
                continue;
            }
            sanitized.push(HIDDEN_TASK_RESULT_VALUE.to_string());
            changed = true;
            redact_next = false;
            pending_assignment_key = false;
            continue;
        }

        if pending_assignment_key {
            if is_task_text_separator_token(token) {
                sanitized.push(token.to_string());
                redact_next = true;
                pending_assignment_key = false;
                continue;
            }
            pending_assignment_key = false;
        }

        let (safe_token, should_redact_next) = sanitize_task_text_token(token);
        changed |= safe_token != token || should_redact_next;
        sanitized.push(safe_token);
        if should_redact_next {
            redact_next = true;
        } else if is_sensitive_assignment_text_key(token) {
            pending_assignment_key = true;
        }
    }

    if changed {
        sanitized.join(" ")
    } else {
        text
    }
}

fn sanitize_task_text_token(token: &str) -> (String, bool) {
    if let Some((safe_token, redact_next)) = sanitize_sensitive_assignment_token(token) {
        return (safe_token, redact_next);
    }
    if let Some(safe_token) = sanitize_private_path_token(token) {
        return (safe_token, false);
    }
    if looks_like_secret_text_token(token) || looks_like_payment_card_number(token) {
        return (HIDDEN_TASK_RESULT_VALUE.to_string(), false);
    }
    if is_sensitive_bare_text_key(token) {
        return (token.to_string(), true);
    }
    (token.to_string(), false)
}

fn sanitize_private_path_token(token: &str) -> Option<String> {
    let trimmed = token.trim_matches(|character: char| {
        matches!(character, '"' | '\'' | '`' | ',' | ';' | ')' | ']' | '}')
    });
    if trimmed.is_empty() || !looks_like_absolute_filesystem_path(trimmed) {
        return None;
    }
    Some(HIDDEN_PATH_VALUE.to_string())
}

fn looks_like_absolute_filesystem_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        return true;
    }
    if value.starts_with("\\\\") {
        return true;
    }
    if value.starts_with('/')
        && value[1..].contains('/')
        && !value.starts_with("/accounts/")
        && !value.starts_with("/addresses/")
        && !value.starts_with("/browser/")
        && !value.starts_with("/cards/")
        && !value.starts_with("/config")
        && !value.starts_with("/runtime/")
        && !value.starts_with("/tasks")
        && !value.starts_with("/ui/")
    {
        return true;
    }
    false
}

fn is_task_text_separator_token(token: &str) -> bool {
    matches!(token.trim(), "=" | ":")
}

fn sanitize_sensitive_assignment_token(token: &str) -> Option<(String, bool)> {
    for separator in ['=', ':'] {
        let Some(index) = token.find(separator) else {
            continue;
        };
        let key = &token[..index];
        if !is_sensitive_assignment_text_key(key) {
            continue;
        }

        let value_start = index + separator.len_utf8();
        if token[value_start..].is_empty() {
            return Some((token.to_string(), true));
        }
        return Some((
            format!("{}{}", &token[..value_start], HIDDEN_TASK_RESULT_VALUE),
            false,
        ));
    }
    None
}

fn is_sensitive_task_result_key(key: &str) -> bool {
    let normalized = key.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    matches!(
        normalized.as_str(),
        "password"
            | "passwords"
            | "old_password"
            | "current_password"
            | "new_password"
            | "confirm_password"
            | "password_value"
            | "cookie"
            | "cookies"
            | "raw_cookie"
            | "cookie_value"
            | "session"
            | "session_cookie"
            | "overleaf_session"
            | "overleaf_session2"
            | "secret"
            | "token"
            | "tokens"
            | "git_token"
            | "token_value"
            | "visible_token"
            | "cvc"
            | "card_number"
            | "number"
            | "number_or_suffix"
    ) || normalized.ends_with("_password")
        || normalized.ends_with("_passwords")
        || normalized.ends_with("_cookie")
        || normalized.ends_with("_session")
        || normalized.ends_with("_secret")
        || normalized.ends_with("_token")
        || normalized.ends_with("_cvc")
        || normalized.ends_with("_card_number")
}

fn is_private_path_task_result_key(key: &str) -> bool {
    let normalized = key.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    matches!(
        normalized.as_str(),
        "path"
            | "paths"
            | "file"
            | "files"
            | "output_dir"
            | "workspace_dir"
            | "profile_dir"
            | "tmp_dir"
            | "chrome_executable"
            | "detected_chrome_executable"
            | "export_path"
            | "import_path"
    ) || normalized.ends_with("_path")
        || normalized.ends_with("_dir")
}

fn looks_like_sensitive_task_result_string(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("olp_")
        || trimmed.contains("overleaf_session2=")
        || looks_like_overleaf_session_secret(trimmed)
}

fn is_sensitive_assignment_text_key(key: &str) -> bool {
    let normalized = normalize_text_key(key);
    matches!(
        normalized.as_str(),
        "password"
            | "passwords"
            | "old_password"
            | "current_password"
            | "new_password"
            | "confirm_password"
            | "cookie"
            | "cookies"
            | "raw_cookie"
            | "session_cookie"
            | "overleaf_session"
            | "overleaf_session2"
            | "secret"
            | "token"
            | "tokens"
            | "git_token"
            | "visible_token"
            | "cvc"
            | "card_cvc"
            | "card_number"
    ) || normalized.ends_with("_password")
        || normalized.ends_with("_cookie")
        || normalized.ends_with("_session")
        || normalized.ends_with("_secret")
        || normalized.ends_with("_token")
        || normalized.ends_with("_cvc")
        || normalized.ends_with("_card_number")
}

fn is_sensitive_bare_text_key(key: &str) -> bool {
    let normalized = normalize_text_key(key);
    matches!(
        normalized.as_str(),
        "password"
            | "passwords"
            | "old_password"
            | "current_password"
            | "new_password"
            | "confirm_password"
            | "secret"
            | "cvc"
            | "card_cvc"
            | "card_number"
    ) || normalized.ends_with("_password")
        || normalized.ends_with("_secret")
        || normalized.ends_with("_cvc")
        || normalized.ends_with("_card_number")
}

fn normalize_text_key(key: &str) -> String {
    key.trim_matches(|character: char| {
        !character.is_ascii_alphanumeric() && character != '_' && character != '-'
    })
    .to_ascii_lowercase()
    .replace('-', "_")
}

fn looks_like_secret_text_token(token: &str) -> bool {
    let trimmed = token.trim_matches(|character: char| {
        !character.is_ascii_alphanumeric() && character != '_' && character != '='
    });
    trimmed.starts_with("olp_")
        || trimmed.contains("overleaf_session2=")
        || looks_like_overleaf_session_secret(trimmed)
}

fn looks_like_overleaf_session_secret(value: &str) -> bool {
    let trimmed = value.trim_matches(|character: char| {
        character.is_ascii_whitespace()
            || matches!(character, '"' | '\'' | '`' | ',' | ';' | ')' | ']' | '}')
    });
    let lower = trimmed.to_ascii_lowercase();
    trimmed.len() >= 8 && (lower.starts_with("s%3a") || lower.starts_with("s:"))
}

fn classify_task_failure(message: &str) -> TaskFailureKind {
    let normalized = message.to_ascii_lowercase();
    if contains_any(
        &normalized,
        &[
            "captcha",
            "recaptcha",
            "email code",
            "verification code",
            "user input",
            "验证码",
            "需要用户",
            "等待用户",
        ],
    ) {
        return TaskFailureKind::NeedsUserInput;
    }
    if contains_any(
        &normalized,
        &[
            "not logged in",
            "login required",
            "session expired",
            "missing saved cookie",
            "missing cookies",
            "invalid cookie",
            "unauthorized",
            "401",
            "403",
            "未登录",
            "重新登录",
            "cookie",
        ],
    ) {
        return TaskFailureKind::NeedsLogin;
    }
    if contains_any(
        &normalized,
        &[
            "page structure",
            "selector",
            "locator",
            "missing input",
            "missing button",
            "missing field",
            "element not found",
            "页面结构",
            "找不到",
            "按钮",
            "输入框",
        ],
    ) {
        return TaskFailureKind::PageChanged;
    }
    if contains_any(
        &normalized,
        &[
            "rate limit",
            "rate-limited",
            "too many requests",
            "429",
            "timeout",
            "timed out",
            "temporarily",
            "connection",
            "network",
            "限流",
            "超时",
            "网络",
            "连接",
            "临时",
        ],
    ) {
        return TaskFailureKind::Retryable;
    }
    TaskFailureKind::Fatal
}

fn recovery_hint_for_failure_kind(kind: TaskFailureKind) -> &'static str {
    match kind {
        TaskFailureKind::Retryable => {
            "Retry after a short wait. If this is a rate limit, keep the same account state and retry the task."
        }
        TaskFailureKind::NeedsUserInput => {
            "Start or resume the task when you can complete the required CAPTCHA, email code, or user action."
        }
        TaskFailureKind::NeedsLogin => {
            "Refresh the account Cookie or log in again before retrying this task."
        }
        TaskFailureKind::PageChanged => {
            "Overleaf page structure may have changed. Check the browser flow or selectors before retrying."
        }
        TaskFailureKind::Fatal => {
            "Review the error details and input data before retrying; this may require configuration or data changes."
        }
    }
}

fn refresh_available_actions(task: &mut TaskSnapshot) {
    task.available_actions = available_actions_for_task(task);
}

fn available_actions_for_task(task: &TaskSnapshot) -> Vec<TaskAvailableAction> {
    let mut actions = Vec::new();
    if task.resolved_by.is_some() {
        return actions;
    }
    if task.phase == ServiceTaskPhase::Failed && task.retry_descriptor.is_some() {
        actions.push(TaskAvailableAction::Retry);
        return actions;
    }
    if task.cancel_requested || task.phase.is_terminal() {
        return actions;
    }
    actions.push(TaskAvailableAction::Cancel);

    for item in &task.waiting_items {
        let action = available_action_for_user_input(item.kind);
        if !actions.contains(&action) {
            actions.push(action);
        }
    }

    if task.phase != ServiceTaskPhase::WaitingForUser {
        return actions;
    }

    if let Some(kind) = task.waiting_for_input {
        actions.push(available_action_for_user_input(kind));
    }
    actions
}

fn available_action_for_user_input(kind: TaskUserInputKind) -> TaskAvailableAction {
    match kind {
        TaskUserInputKind::EmailCode => TaskAvailableAction::SubmitEmailCode,
        TaskUserInputKind::NewRegistrationCredentials => {
            TaskAvailableAction::SubmitNewRegistrationCredentials
        }
        TaskUserInputKind::NewBrowserCredentials => {
            TaskAvailableAction::SubmitNewBrowserCredentials
        }
        TaskUserInputKind::CaptchaCompleted => TaskAvailableAction::MarkCaptchaCompleted,
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

impl Default for TaskStateStore {
    fn default() -> Self {
        Self::new(200)
    }
}

impl From<TaskPhase> for ServiceTaskPhase {
    fn from(value: TaskPhase) -> Self {
        match value {
            TaskPhase::Pending => Self::Pending,
            TaskPhase::Running => Self::Running,
            TaskPhase::WaitingForUser => Self::WaitingForUser,
            TaskPhase::Completed => Self::Completed,
            TaskPhase::Failed => Self::Failed,
            TaskPhase::Cancelled => Self::Cancelled,
        }
    }
}

impl ServiceTaskPhase {
    fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}
