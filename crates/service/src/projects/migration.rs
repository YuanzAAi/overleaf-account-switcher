use std::collections::{BTreeSet, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use overleaf_api::project_api::{
    ProjectApiClient, ProjectApiError, ProjectApiStatusKind, ProjectApiTransport,
    ProjectApiTransportErrorKind, ProjectTokenSet, ReqwestProjectApiTransport,
};
use overleaf_api::session::{OverleafSessionClient, ReqwestSessionTransport};
use overleaf_api::OVERLEAF_SESSION_COOKIE;
use overleaf_core::Project;
use overleaf_storage::{AccountStore, AccountsDocument, SecretBackend};
use overleaf_workflows::{
    audit_project_migrations, plan_project_migrations, resolve_join_attempt, ApiAttemptResult,
    JoinAttemptDecision, ProjectMigrationAudit, ProjectMigrationEvidence, ProjectMigrationPlan,
    ProjectMigrationStrategy,
};
use serde::Serialize;

use crate::account_secrets::{resolve_account_cookies, AccountSecretStoreError};

const MIGRATION_MAX_RETRIES: u32 = 5;
const TRANSIENT_STEP_MAX_RETRIES: u32 = 3;
const JOIN_REQUEST_LIMIT: usize = 10;
const JOIN_REQUEST_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectMigrationProgressLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMigrationProgressEvent {
    pub current: u32,
    pub total: u32,
    pub level: ProjectMigrationProgressLevel,
    pub message: String,
}

pub trait ProjectMigrationControl: Send + Sync {
    fn selected_project_ids(&self) -> Option<&[String]> {
        None
    }

    fn expected_source_alias(&self) -> Option<&str> {
        None
    }

    fn is_cancel_requested(&self) -> bool {
        false
    }

    fn on_progress(&self, _event: ProjectMigrationProgressEvent) {}

    fn on_wait_countdown(&self, _current: u32, _total: u32, _message: String) {}
}

#[derive(Debug, Default)]
struct NoopProjectMigrationControl;

impl ProjectMigrationControl for NoopProjectMigrationControl {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectMigrationReport {
    pub source_alias: String,
    pub target_alias: String,
    pub total_projects: usize,
    pub migrated_count: usize,
    pub skipped_existing_count: usize,
    pub failed_count: usize,
    pub warning_count: usize,
    pub audit_confirmed_count: usize,
    pub audit_missing_count: usize,
    pub remigration_attempted_count: usize,
    pub items: Vec<ProjectMigrationItem>,
    pub missing_projects: Vec<ProjectMigrationSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectMigrationPreviewReport {
    pub source_alias: String,
    pub target_alias: String,
    pub total_projects: usize,
    pub will_migrate_count: usize,
    pub skipped_existing_count: usize,
    pub items: Vec<ProjectMigrationPreviewItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectMigrationItem {
    pub project_id: String,
    pub project_name: String,
    pub strategy: ProjectMigrationStrategySummary,
    pub status: ProjectMigrationItemStatus,
    pub joined_project_id: Option<String>,
    pub cloned_project_id: Option<String>,
    pub error_message: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectMigrationPreviewItem {
    pub project_id: String,
    pub project_name: String,
    pub strategy: ProjectMigrationStrategySummary,
    pub status: ProjectMigrationPreviewStatus,
    pub target_project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectMigrationSummary {
    pub project_id: String,
    pub project_name: String,
    pub strategy: ProjectMigrationStrategySummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectMigrationStrategySummary {
    OwnedProject,
    LinkSharingCollaboration,
    CopyCollaboration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectMigrationItemStatus {
    Migrated,
    SkippedExisting,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectMigrationPreviewStatus {
    WillMigrate,
    AlreadyExists,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectMigrationError {
    Io {
        message: String,
    },
    NoCurrentAccount,
    SourceEqualsTarget {
        alias: String,
    },
    AccountAliasNotFound {
        alias: String,
    },
    MissingCookies {
        alias: String,
    },
    SecretReadFailed {
        alias: String,
        category: &'static str,
    },
    CsrfTokenFetchFailed {
        alias: String,
        message: String,
    },
    ProjectListFailed {
        alias: String,
        message: String,
    },
    AuditIncomplete {
        failed_count: usize,
        missing_count: usize,
    },
    Cancelled,
}

#[async_trait::async_trait]
pub trait ProjectMigrationExecutor {
    async fn preview_projects(
        &self,
        store: &AccountStore,
        target_alias: &str,
        secret_backend: &(dyn SecretBackend + Send + Sync),
    ) -> Result<ProjectMigrationPreviewReport, ProjectMigrationError>;

    async fn migrate_projects(
        &self,
        store: &AccountStore,
        target_alias: &str,
        secret_backend: &(dyn SecretBackend + Send + Sync),
    ) -> Result<ProjectMigrationReport, ProjectMigrationError>;

    async fn migrate_projects_with_control(
        &self,
        store: &AccountStore,
        target_alias: &str,
        secret_backend: &(dyn SecretBackend + Send + Sync),
        _control: &(dyn ProjectMigrationControl + Send + Sync),
    ) -> Result<ProjectMigrationReport, ProjectMigrationError> {
        self.migrate_projects(store, target_alias, secret_backend)
            .await
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReqwestProjectMigrationExecutor;

#[async_trait::async_trait]
impl ProjectMigrationExecutor for ReqwestProjectMigrationExecutor {
    async fn preview_projects(
        &self,
        store: &AccountStore,
        target_alias: &str,
        secret_backend: &(dyn SecretBackend + Send + Sync),
    ) -> Result<ProjectMigrationPreviewReport, ProjectMigrationError> {
        preview_project_migrations_with_reqwest_in_store(store, target_alias, secret_backend).await
    }

    async fn migrate_projects(
        &self,
        store: &AccountStore,
        target_alias: &str,
        secret_backend: &(dyn SecretBackend + Send + Sync),
    ) -> Result<ProjectMigrationReport, ProjectMigrationError> {
        migrate_projects_with_reqwest_in_store(store, target_alias, secret_backend).await
    }

    async fn migrate_projects_with_control(
        &self,
        store: &AccountStore,
        target_alias: &str,
        secret_backend: &(dyn SecretBackend + Send + Sync),
        control: &(dyn ProjectMigrationControl + Send + Sync),
    ) -> Result<ProjectMigrationReport, ProjectMigrationError> {
        migrate_projects_with_reqwest_in_store_controlled(
            store,
            target_alias,
            secret_backend,
            control,
        )
        .await
    }
}

pub async fn preview_project_migrations_with_reqwest_in_store(
    store: &AccountStore,
    target_alias: &str,
    secret_backend: &(dyn SecretBackend + Send + Sync),
) -> Result<ProjectMigrationPreviewReport, ProjectMigrationError> {
    let document = store.load().map_err(ProjectMigrationError::from_io)?;
    let source_alias = document
        .current
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ProjectMigrationError::NoCurrentAccount)?;
    let target_alias = target_alias.trim();
    if source_alias == target_alias {
        return Err(ProjectMigrationError::SourceEqualsTarget {
            alias: target_alias.to_string(),
        });
    }

    let source_client = reqwest_project_client(&document, source_alias, secret_backend).await?;
    let target_client = reqwest_project_client(&document, target_alias, secret_backend).await?;
    preview_project_migrations_between_clients(
        source_alias,
        target_alias,
        &source_client,
        &target_client,
    )
    .await
}

pub async fn migrate_projects_with_reqwest_in_store(
    store: &AccountStore,
    target_alias: &str,
    secret_backend: &(dyn SecretBackend + Send + Sync),
) -> Result<ProjectMigrationReport, ProjectMigrationError> {
    migrate_projects_with_reqwest_in_store_controlled(
        store,
        target_alias,
        secret_backend,
        &NoopProjectMigrationControl,
    )
    .await
}

pub async fn migrate_projects_with_reqwest_in_store_controlled(
    store: &AccountStore,
    target_alias: &str,
    secret_backend: &(dyn SecretBackend + Send + Sync),
    control: &(dyn ProjectMigrationControl + Send + Sync),
) -> Result<ProjectMigrationReport, ProjectMigrationError> {
    ensure_migration_not_cancelled(control)?;
    let document = store.load().map_err(ProjectMigrationError::from_io)?;
    let source_alias = document
        .current
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ProjectMigrationError::NoCurrentAccount)?;
    let target_alias = target_alias.trim();
    if control
        .expected_source_alias()
        .is_some_and(|expected| expected != source_alias)
    {
        return Err(ProjectMigrationError::Io {
            message: "当前账号已改变，请重新选择迁移项目".into(),
        });
    }
    if source_alias == target_alias {
        return Err(ProjectMigrationError::SourceEqualsTarget {
            alias: target_alias.to_string(),
        });
    }

    let source_client = reqwest_project_client(&document, source_alias, secret_backend).await?;
    let target_client = reqwest_project_client(&document, target_alias, secret_backend).await?;
    migrate_projects_between_clients_controlled(
        source_alias,
        target_alias,
        &source_client,
        &target_client,
        control,
    )
    .await
}

pub async fn preview_project_migrations_between_clients<S, T>(
    source_alias: &str,
    target_alias: &str,
    source_client: &ProjectApiClient<S>,
    target_client: &ProjectApiClient<T>,
) -> Result<ProjectMigrationPreviewReport, ProjectMigrationError>
where
    S: ProjectApiTransport,
    T: ProjectApiTransport,
{
    let source_projects = source_client.list_projects().await.map_err(|error| {
        ProjectMigrationError::ProjectListFailed {
            alias: source_alias.to_string(),
            message: error.to_string(),
        }
    })?;
    let target_projects = target_client.list_projects().await.map_err(|error| {
        ProjectMigrationError::ProjectListFailed {
            alias: target_alias.to_string(),
            message: error.to_string(),
        }
    })?;
    let plans = plan_project_migrations(&source_projects);
    Ok(project_migration_preview(
        source_alias,
        target_alias,
        &plans,
        &target_projects,
    ))
}

pub async fn migrate_projects_between_clients<S, T>(
    source_alias: &str,
    target_alias: &str,
    source_client: &ProjectApiClient<S>,
    target_client: &ProjectApiClient<T>,
) -> Result<ProjectMigrationReport, ProjectMigrationError>
where
    S: ProjectApiTransport,
    T: ProjectApiTransport,
{
    migrate_projects_between_clients_controlled(
        source_alias,
        target_alias,
        source_client,
        target_client,
        &NoopProjectMigrationControl,
    )
    .await
}

pub async fn migrate_projects_between_clients_controlled<S, T>(
    source_alias: &str,
    target_alias: &str,
    source_client: &ProjectApiClient<S>,
    target_client: &ProjectApiClient<T>,
    control: &(dyn ProjectMigrationControl + Send + Sync),
) -> Result<ProjectMigrationReport, ProjectMigrationError>
where
    S: ProjectApiTransport,
    T: ProjectApiTransport,
{
    ensure_migration_not_cancelled(control)?;
    let mut source_projects =
        list_projects_with_retry(source_client, source_alias, control, 0, 1).await?;
    if let Some(selected) = control.selected_project_ids() {
        if selected
            .iter()
            .any(|id| !source_projects.iter().any(|p| p.id == *id && p.is_active()))
        {
            return Err(ProjectMigrationError::Io {
                message: "部分所选项目已不在工作区，请重新选择迁移项目".into(),
            });
        }
        source_projects.retain(|project| selected.contains(&project.id));
    }
    ensure_migration_not_cancelled(control)?;
    let target_projects =
        list_projects_with_retry(target_client, target_alias, control, 0, 1).await?;
    let plans = plan_project_migrations(&source_projects);
    let total = plans.len().min(u32::MAX as usize) as u32;
    notify_migration_progress(
        control,
        0,
        total,
        ProjectMigrationProgressLevel::Info,
        format!("项目迁移准备完成：共 {} 个项目", plans.len()),
    );
    let mut existing_owned_names = active_owned_project_names(&target_projects);
    let join_requests = Mutex::new([VecDeque::new(), VecDeque::new()]);
    let uncertain_clones = Mutex::new(BTreeSet::new());
    let mut evidence = ProjectMigrationEvidence {
        target_projects,
        original_accessible: BTreeSet::new(),
        cloned_project_ids: BTreeSet::new(),
    };

    let mut report = ProjectMigrationReport {
        source_alias: source_alias.to_string(),
        target_alias: target_alias.to_string(),
        total_projects: plans.len(),
        migrated_count: 0,
        skipped_existing_count: 0,
        failed_count: 0,
        warning_count: 0,
        audit_confirmed_count: 0,
        audit_missing_count: 0,
        remigration_attempted_count: 0,
        items: Vec::with_capacity(plans.len()),
        missing_projects: Vec::new(),
    };

    for (index, plan) in plans.iter().enumerate() {
        ensure_migration_not_cancelled(control)?;
        let current = index.min(u32::MAX as usize) as u32;
        notify_migration_progress(
            control,
            current,
            total,
            ProjectMigrationProgressLevel::Info,
            format!(
                "正在迁移项目 {}/{}: {}",
                index + 1,
                plans.len(),
                plan.project_name
            ),
        );
        migrate_one_project(
            source_client,
            target_client,
            plan,
            &mut existing_owned_names,
            &mut evidence,
            &mut report,
            MigrationProgressContext {
                control,
                join_requests: &join_requests,
                uncertain_clones: &uncertain_clones,
                current,
                total,
                project_id: &plan.project_id,
                project_name: &plan.project_name,
            },
        )
        .await?;
        notify_project_result(control, &report, plan, index + 1, plans.len());
    }

    ensure_migration_not_cancelled(control)?;
    notify_migration_progress(
        control,
        total,
        total,
        ProjectMigrationProgressLevel::Info,
        "正在核验目标账号项目清单",
    );
    refresh_audit(
        target_client,
        target_alias,
        &plans,
        &mut evidence,
        &mut report,
        control,
        total,
    )
    .await?;
    let missing_once = report.missing_projects.clone();
    if !missing_once.is_empty() {
        existing_owned_names = active_owned_project_names(&evidence.target_projects);
        for missing in missing_once {
            ensure_migration_not_cancelled(control)?;
            let Some(plan) = plans
                .iter()
                .find(|plan| plan.project_id == missing.project_id)
            else {
                continue;
            };
            report.remigration_attempted_count += 1;
            notify_migration_progress(
                control,
                total,
                total,
                ProjectMigrationProgressLevel::Warning,
                format!("首次核验未确认项目，正在重试: {}", plan.project_name),
            );
            migrate_one_project(
                source_client,
                target_client,
                plan,
                &mut existing_owned_names,
                &mut evidence,
                &mut report,
                MigrationProgressContext {
                    control,
                    join_requests: &join_requests,
                    uncertain_clones: &uncertain_clones,
                    current: total,
                    total,
                    project_id: &plan.project_id,
                    project_name: &plan.project_name,
                },
            )
            .await?;
        }
        ensure_migration_not_cancelled(control)?;
        refresh_audit(
            target_client,
            target_alias,
            &plans,
            &mut evidence,
            &mut report,
            control,
            total,
        )
        .await?;
    }
    recount_migration_report(&mut report);
    notify_migration_progress(
        control,
        total,
        total,
        if report.audit_missing_count == 0 && report.failed_count == 0 {
            ProjectMigrationProgressLevel::Info
        } else {
            ProjectMigrationProgressLevel::Error
        },
        format!(
            "项目迁移核验完成：确认 {}，缺失 {}，失败 {}",
            report.audit_confirmed_count, report.audit_missing_count, report.failed_count
        ),
    );

    if report.failed_count > 0 || report.audit_missing_count > 0 {
        return Err(ProjectMigrationError::AuditIncomplete {
            failed_count: report.failed_count,
            missing_count: report.audit_missing_count,
        });
    }

    Ok(report)
}

fn project_migration_preview(
    source_alias: &str,
    target_alias: &str,
    plans: &[ProjectMigrationPlan],
    target_projects: &[Project],
) -> ProjectMigrationPreviewReport {
    let mut will_migrate_count = 0;
    let mut skipped_existing_count = 0;
    let evidence = ProjectMigrationEvidence {
        target_projects: target_projects.to_vec(),
        ..Default::default()
    };
    let items = plans
        .iter()
        .map(|plan| {
            let confirmed = overleaf_workflows::is_migration_confirmed(plan, &evidence);
            let target_project_id = if confirmed {
                target_projects
                    .iter()
                    .find(|project| !project.trashed && project.id == plan.project_id)
                    .map(|project| project.id.clone())
                    .or_else(|| owned_project_id_by_name(target_projects, &plan.project_name, None))
            } else {
                None
            };
            let status = if confirmed {
                skipped_existing_count += 1;
                ProjectMigrationPreviewStatus::AlreadyExists
            } else {
                will_migrate_count += 1;
                ProjectMigrationPreviewStatus::WillMigrate
            };
            ProjectMigrationPreviewItem {
                project_id: plan.project_id.clone(),
                project_name: plan.project_name.clone(),
                strategy: strategy_summary(plan.strategy),
                status,
                target_project_id,
            }
        })
        .collect();

    ProjectMigrationPreviewReport {
        source_alias: source_alias.to_string(),
        target_alias: target_alias.to_string(),
        total_projects: plans.len(),
        will_migrate_count,
        skipped_existing_count,
        items,
    }
}

async fn migrate_one_project<S, T>(
    source_client: &ProjectApiClient<S>,
    target_client: &ProjectApiClient<T>,
    plan: &ProjectMigrationPlan,
    existing_owned_names: &mut BTreeSet<String>,
    evidence: &mut ProjectMigrationEvidence,
    report: &mut ProjectMigrationReport,
    progress: MigrationProgressContext<'_>,
) -> Result<(), ProjectMigrationError>
where
    S: ProjectApiTransport,
    T: ProjectApiTransport,
{
    ensure_migration_not_cancelled(progress.control)?;
    // 链接共享协作按原 project_id 审计目标访问权限，不能被目标账号同名自有项目误跳过。
    if plan.strategy != ProjectMigrationStrategy::LinkSharingCollaboration
        && existing_owned_names.contains(&plan.project_name)
    {
        if plan.strategy == ProjectMigrationStrategy::OwnedProject
            && active_project_id_exists(&evidence.target_projects, &plan.project_id)
        {
            let existing_owned_id = owned_project_id_by_name(
                &evidence.target_projects,
                &plan.project_name,
                Some(&plan.project_id),
            );
            match leave_project_with_retry(target_client, &plan.project_id, progress).await {
                Ok(()) => {
                    evidence
                        .target_projects
                        .retain(|project| project.id != plan.project_id);
                    record_migration_item(
                        report,
                        item(
                            plan,
                            ProjectMigrationItemStatus::Migrated,
                            Some(plan.project_id.clone()),
                            existing_owned_id,
                            None,
                            Vec::new(),
                        ),
                    );
                }
                Err(error) => record_migration_item(
                    report,
                    item(
                        plan,
                        ProjectMigrationItemStatus::Failed,
                        Some(plan.project_id.clone()),
                        existing_owned_id,
                        Some(error),
                        Vec::new(),
                    ),
                ),
            }
            ensure_migration_not_cancelled(progress.control)?;
            return Ok(());
        }
        record_migration_item(
            report,
            item(
                plan,
                ProjectMigrationItemStatus::SkippedExisting,
                None,
                None,
                None,
                Vec::new(),
            ),
        );
        return Ok(());
    }

    if plan.strategy != ProjectMigrationStrategy::OwnedProject
        && active_project_id_exists(&evidence.target_projects, &plan.project_id)
    {
        record_migration_item(
            report,
            item(
                plan,
                ProjectMigrationItemStatus::SkippedExisting,
                Some(plan.project_id.clone()),
                None,
                None,
                Vec::new(),
            ),
        );
        return Ok(());
    }

    match migrate_plan(source_client, target_client, plan, evidence, progress).await {
        Ok(outcome) => {
            if outcome.cloned_project_id.is_some() {
                existing_owned_names.insert(plan.project_name.clone());
            }
            record_migration_item(
                report,
                item(
                    plan,
                    ProjectMigrationItemStatus::Migrated,
                    outcome.joined_project_id,
                    outcome.cloned_project_id,
                    None,
                    outcome.warnings,
                ),
            );
        }
        Err(error) => {
            ensure_migration_not_cancelled(progress.control)?;
            record_migration_item(
                report,
                item(
                    plan,
                    ProjectMigrationItemStatus::Failed,
                    None,
                    None,
                    Some(error),
                    Vec::new(),
                ),
            );
        }
    }
    Ok(())
}

fn record_migration_item(report: &mut ProjectMigrationReport, next_item: ProjectMigrationItem) {
    if let Some(existing) = report
        .items
        .iter_mut()
        .find(|item| item.project_id == next_item.project_id)
    {
        *existing = next_item;
    } else {
        report.items.push(next_item);
    }
}

fn recount_migration_report(report: &mut ProjectMigrationReport) {
    report.migrated_count = report
        .items
        .iter()
        .filter(|item| item.status == ProjectMigrationItemStatus::Migrated)
        .count();
    report.skipped_existing_count = report
        .items
        .iter()
        .filter(|item| item.status == ProjectMigrationItemStatus::SkippedExisting)
        .count();
    report.failed_count = report
        .items
        .iter()
        .filter(|item| item.status == ProjectMigrationItemStatus::Failed)
        .count();
    report.warning_count = report.items.iter().map(|item| item.warnings.len()).sum();
}

fn ensure_migration_not_cancelled(
    control: &(dyn ProjectMigrationControl + Send + Sync),
) -> Result<(), ProjectMigrationError> {
    if control.is_cancel_requested() {
        Err(ProjectMigrationError::Cancelled)
    } else {
        Ok(())
    }
}

fn notify_migration_progress(
    control: &(dyn ProjectMigrationControl + Send + Sync),
    current: u32,
    total: u32,
    level: ProjectMigrationProgressLevel,
    message: impl Into<String>,
) {
    let total = total.max(1);
    control.on_progress(ProjectMigrationProgressEvent {
        current: current.min(total),
        total,
        level,
        message: message.into(),
    });
}

fn notify_migration_wait_countdown(
    control: &(dyn ProjectMigrationControl + Send + Sync),
    current: u32,
    total: u32,
    message: impl Into<String>,
) {
    let total = total.max(1);
    control.on_wait_countdown(current.min(total), total, message.into());
}

fn notify_project_result(
    control: &(dyn ProjectMigrationControl + Send + Sync),
    report: &ProjectMigrationReport,
    plan: &ProjectMigrationPlan,
    index: usize,
    total: usize,
) {
    let Some(item) = report
        .items
        .iter()
        .find(|item| item.project_id == plan.project_id)
    else {
        return;
    };
    let (level, message) = match item.status {
        ProjectMigrationItemStatus::Migrated => (
            ProjectMigrationProgressLevel::Info,
            format!("项目迁移完成 {index}/{total}: {}", plan.project_name),
        ),
        ProjectMigrationItemStatus::SkippedExisting => (
            ProjectMigrationProgressLevel::Info,
            format!("项目已存在，跳过 {index}/{total}: {}", plan.project_name),
        ),
        ProjectMigrationItemStatus::Failed => (
            ProjectMigrationProgressLevel::Error,
            format!(
                "项目迁移失败 {index}/{total}: {}: {}",
                plan.project_name,
                item.error_message.as_deref().unwrap_or("unknown error")
            ),
        ),
    };
    notify_migration_progress(
        control,
        index.min(u32::MAX as usize) as u32,
        total.min(u32::MAX as usize) as u32,
        level,
        message,
    );
    if !item.warnings.is_empty() {
        notify_migration_progress(
            control,
            index.min(u32::MAX as usize) as u32,
            total.min(u32::MAX as usize) as u32,
            ProjectMigrationProgressLevel::Warning,
            format!(
                "项目迁移警告 {}: {}",
                plan.project_name,
                item.warnings.join("; ")
            ),
        );
    }
}

#[derive(Clone, Copy)]
struct MigrationProgressContext<'a> {
    control: &'a (dyn ProjectMigrationControl + Send + Sync),
    join_requests: &'a Mutex<[VecDeque<Instant>; 2]>,
    uncertain_clones: &'a Mutex<BTreeSet<String>>,
    current: u32,
    total: u32,
    project_id: &'a str,
    project_name: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MigrationOutcome {
    joined_project_id: Option<String>,
    cloned_project_id: Option<String>,
    warnings: Vec<String>,
}

async fn migrate_plan<S, T>(
    source_client: &ProjectApiClient<S>,
    target_client: &ProjectApiClient<T>,
    plan: &ProjectMigrationPlan,
    evidence: &mut ProjectMigrationEvidence,
    progress: MigrationProgressContext<'_>,
) -> Result<MigrationOutcome, String>
where
    S: ProjectApiTransport,
    T: ProjectApiTransport,
{
    match plan.strategy {
        ProjectMigrationStrategy::OwnedProject => {
            let token =
                enable_link_sharing_with_retry(source_client, &plan.project_id, progress).await?;
            let (token, is_read_only) = select_token(token)?;
            let joined = join_project_with_retry(
                target_client,
                &token,
                is_read_only,
                &plan.project_id,
                progress,
            )
            .await?
            .unwrap_or_else(|| plan.project_id.clone());
            evidence.original_accessible.insert(plan.project_id.clone());
            let cloned = clone_project_with_retry(
                target_client,
                &plan.project_id,
                &plan.project_name,
                Some(&plan.project_id),
                progress,
            )
            .await?;
            leave_project_with_retry(target_client, &plan.project_id, progress).await?;
            evidence
                .target_projects
                .retain(|project| project.id != plan.project_id);
            evidence.cloned_project_ids.insert(plan.project_id.clone());
            Ok(MigrationOutcome {
                joined_project_id: Some(joined),
                cloned_project_id: Some(cloned),
                warnings: Vec::new(),
            })
        }
        ProjectMigrationStrategy::LinkSharingCollaboration => {
            let token =
                get_project_tokens_with_retry(source_client, &plan.project_id, progress).await?;
            let (token, is_read_only) = select_token(token)?;
            let joined = join_project_with_retry(
                target_client,
                &token,
                is_read_only,
                &plan.project_id,
                progress,
            )
            .await?
            .unwrap_or_else(|| plan.project_id.clone());
            evidence.original_accessible.insert(plan.project_id.clone());
            Ok(MigrationOutcome {
                joined_project_id: Some(joined),
                cloned_project_id: None,
                warnings: Vec::new(),
            })
        }
        ProjectMigrationStrategy::CopyCollaboration => {
            if let Ok(token) = source_client.get_project_tokens(&plan.project_id).await {
                if let Ok((token, is_read_only)) = select_token(token) {
                    if let Ok(joined) = join_project_with_retry(
                        target_client,
                        &token,
                        is_read_only,
                        &plan.project_id,
                        progress,
                    )
                    .await
                    {
                        let joined = joined.unwrap_or_else(|| plan.project_id.clone());
                        evidence.original_accessible.insert(plan.project_id.clone());
                        return Ok(MigrationOutcome {
                            joined_project_id: Some(joined),
                            cloned_project_id: None,
                            warnings: Vec::new(),
                        });
                    }
                }
            }

            let members = source_client
                .get_project_members(&plan.project_id)
                .await
                .unwrap_or_default();
            let source_copy = clone_project_with_retry(
                source_client,
                &plan.project_id,
                &plan.project_name,
                Some(&plan.project_id),
                progress,
            )
            .await?;
            let copy_token =
                enable_link_sharing_with_retry(source_client, &source_copy, progress).await?;
            let (copy_token, is_read_only) = select_token(copy_token)?;
            let joined_copy = join_project_with_retry(
                target_client,
                &copy_token,
                is_read_only,
                &source_copy,
                progress,
            )
            .await?
            .unwrap_or_else(|| source_copy.clone());
            let cloned = clone_project_with_retry(
                target_client,
                &source_copy,
                &plan.project_name,
                Some(&source_copy),
                progress,
            )
            .await?;
            leave_project_with_retry(target_client, &source_copy, progress).await?;
            evidence
                .target_projects
                .retain(|project| project.id != source_copy);
            let _ = source_client.delete_project(&source_copy).await;
            let mut warnings = Vec::new();
            for member in members {
                if member.privileges == "owner" || member.member_type == "owner" {
                    continue;
                }
                if let Err(error) = invite_collaborator_with_retry(
                    target_client,
                    &cloned,
                    &member.email,
                    &member.privileges,
                    progress,
                )
                .await
                {
                    warnings.push(format!(
                        "failed to invite {} as {}: {}",
                        member.email, member.privileges, error
                    ));
                }
            }
            evidence.cloned_project_ids.insert(source_copy);
            Ok(MigrationOutcome {
                joined_project_id: Some(joined_copy),
                cloned_project_id: Some(cloned),
                warnings,
            })
        }
    }
}

async fn enable_link_sharing_with_retry<T>(
    client: &ProjectApiClient<T>,
    project_id: &str,
    progress: MigrationProgressContext<'_>,
) -> Result<ProjectTokenSet, String>
where
    T: ProjectApiTransport,
{
    let mut rate_limit_retries = 0;
    let mut transient_retries = 0;
    loop {
        ensure_step_not_cancelled(progress.control)?;
        match client.enable_link_sharing(project_id).await {
            Ok(tokens) => return Ok(tokens),
            Err(error) if is_rate_limited(&error) => {
                let seconds = rate_limit_delay(&error, rate_limit_retries);
                let error_message = project_api_error_message(error);
                ensure_step_not_cancelled(progress.control)?;
                if let Ok(tokens) = client.get_project_tokens(project_id).await {
                    ensure_step_not_cancelled(progress.control)?;
                    if tokens.read_only.is_some() || tokens.read_and_write.is_some() {
                        return Ok(tokens);
                    }
                }
                if rate_limit_retries + 1 >= MIGRATION_MAX_RETRIES {
                    ensure_step_not_cancelled(progress.control)?;
                    return Err(error_message);
                }
                wait_after_rate_limit(progress, "启用链接共享", rate_limit_retries, seconds)
                    .await?;
                rate_limit_retries += 1;
            }
            Err(error) if is_transient_retryable_error(&error) => {
                let error_message = project_api_error_message(error);
                ensure_step_not_cancelled(progress.control)?;
                if let Ok(tokens) = client.get_project_tokens(project_id).await {
                    ensure_step_not_cancelled(progress.control)?;
                    if tokens.read_only.is_some() || tokens.read_and_write.is_some() {
                        return Ok(tokens);
                    }
                }
                if transient_retries >= TRANSIENT_STEP_MAX_RETRIES {
                    ensure_step_not_cancelled(progress.control)?;
                    return Err(error_message);
                }
                let seconds = transient_retry_delay(transient_retries);
                transient_retries += 1;
                wait_after_transient_error(
                    progress,
                    "启用链接共享",
                    transient_retries,
                    &error_message,
                    seconds,
                )
                .await?;
            }
            Err(error) => return Err(project_api_error_message(error)),
        }
    }
}

async fn get_project_tokens_with_retry<T>(
    client: &ProjectApiClient<T>,
    project_id: &str,
    progress: MigrationProgressContext<'_>,
) -> Result<ProjectTokenSet, String>
where
    T: ProjectApiTransport,
{
    let mut rate_limit_retries = 0;
    let mut transient_retries = 0;
    loop {
        ensure_step_not_cancelled(progress.control)?;
        match client.get_project_tokens(project_id).await {
            Ok(tokens) => return Ok(tokens),
            Err(error) if is_rate_limited(&error) => {
                let seconds = rate_limit_delay(&error, rate_limit_retries);
                let error_message = project_api_error_message(error);
                if rate_limit_retries + 1 >= MIGRATION_MAX_RETRIES {
                    ensure_step_not_cancelled(progress.control)?;
                    return Err(error_message);
                }
                wait_after_rate_limit(progress, "读取链接共享令牌", rate_limit_retries, seconds)
                    .await?;
                rate_limit_retries += 1;
            }
            Err(error) if is_transient_retryable_error(&error) => {
                let error_message = project_api_error_message(error);
                if transient_retries >= TRANSIENT_STEP_MAX_RETRIES {
                    ensure_step_not_cancelled(progress.control)?;
                    return Err(error_message);
                }
                let seconds = transient_retry_delay(transient_retries);
                transient_retries += 1;
                wait_after_transient_error(
                    progress,
                    "读取链接共享令牌",
                    transient_retries,
                    &error_message,
                    seconds,
                )
                .await?;
            }
            Err(error) => return Err(project_api_error_message(error)),
        }
    }
}

async fn join_project_with_retry<T>(
    client: &ProjectApiClient<T>,
    token: &str,
    is_read_only: bool,
    expected_project_id: &str,
    progress: MigrationProgressContext<'_>,
) -> Result<Option<String>, String>
where
    T: ProjectApiTransport,
{
    let mut rate_limit_retries = 0;
    let mut transient_retries = 0;
    loop {
        ensure_step_not_cancelled(progress.control)?;
        wait_before_project_join(progress, is_read_only).await?;
        let result = client.join_project_via_token(token, is_read_only).await;
        progress
            .join_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[usize::from(is_read_only)]
            .push_back(Instant::now());
        match result {
            Ok(project_id) => {
                return Ok(project_id.or_else(|| Some(expected_project_id.to_string())))
            }
            Err(error) if is_rate_limited(&error) => {
                let seconds = rate_limit_delay(&error, rate_limit_retries);
                let error_message = project_api_error_message(error);
                ensure_step_not_cancelled(progress.control)?;
                let target_projects = client.list_projects().await.unwrap_or_default();
                ensure_step_not_cancelled(progress.control)?;
                match resolve_join_attempt(
                    expected_project_id,
                    ApiAttemptResult::RateLimited,
                    &target_projects,
                    rate_limit_retries,
                ) {
                    JoinAttemptDecision::Joined => {
                        return Ok(Some(expected_project_id.to_string()));
                    }
                    JoinAttemptDecision::RetryAfterSeconds(_) => {
                        if rate_limit_retries + 1 >= MIGRATION_MAX_RETRIES {
                            ensure_step_not_cancelled(progress.control)?;
                            return Err(error_message);
                        }
                        wait_after_rate_limit(
                            progress,
                            "加入共享项目",
                            rate_limit_retries,
                            seconds,
                        )
                        .await?;
                        rate_limit_retries += 1;
                    }
                    JoinAttemptDecision::Failed => return Err(error_message),
                }
            }
            Err(error) if is_transient_retryable_error(&error) => {
                let error_message = project_api_error_message(error);
                ensure_step_not_cancelled(progress.control)?;
                let projects = client.list_projects().await.unwrap_or_default();
                ensure_step_not_cancelled(progress.control)?;
                if active_project_id_exists(&projects, expected_project_id) {
                    return Ok(Some(expected_project_id.to_string()));
                }
                if transient_retries >= TRANSIENT_STEP_MAX_RETRIES {
                    ensure_step_not_cancelled(progress.control)?;
                    return Err(error_message);
                }
                let seconds = transient_retry_delay(transient_retries);
                transient_retries += 1;
                wait_after_transient_error(
                    progress,
                    "加入共享项目",
                    transient_retries,
                    &error_message,
                    seconds,
                )
                .await?;
                let projects = client.list_projects().await.unwrap_or_default();
                ensure_step_not_cancelled(progress.control)?;
                if active_project_id_exists(&projects, expected_project_id) {
                    return Ok(Some(expected_project_id.to_string()));
                }
            }
            Err(error) => return Err(project_api_error_message(error)),
        }
    }
}

fn join_request_delay(requests: &mut VecDeque<Instant>, now: Instant) -> Option<Duration> {
    while requests
        .front()
        .is_some_and(|sent| now.duration_since(*sent) >= JOIN_REQUEST_WINDOW)
    {
        requests.pop_front();
    }
    (requests.len() >= JOIN_REQUEST_LIMIT)
        .then(|| JOIN_REQUEST_WINDOW.saturating_sub(now.duration_since(requests[0])))
}

async fn wait_before_project_join(
    progress: MigrationProgressContext<'_>,
    is_read_only: bool,
) -> Result<(), String> {
    // 读写与只读加入接口各限每账号 10 次/分钟，按响应完成时间计窗。
    let delay = {
        let mut requests = progress
            .join_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        join_request_delay(&mut requests[usize::from(is_read_only)], Instant::now())
    };
    if let Some(delay) = delay {
        let initial_secs = ceil_seconds(delay);
        if initial_secs > 0 {
            notify_migration_progress(
                progress.control,
                progress.current,
                progress.total,
                ProjectMigrationProgressLevel::Info,
                format!(
                    "项目 {} 的共享加入请求已达速率上限，等待 {} 秒后继续",
                    progress.project_name, initial_secs
                ),
            );
            wait_with_cancellation_countdown(
                progress.control,
                progress.current,
                progress.total,
                delay,
                |remaining_secs| {
                    format!(
                        "项目 {} 的共享加入请求已达速率上限，等待 {} 秒后继续",
                        progress.project_name, remaining_secs
                    )
                },
            )
            .await?;
        }
    }
    Ok(())
}

enum CloneRecoveryState {
    AlreadyCloned(String),
    ConfirmedAbsent,
}

async fn confirm_clone_state_before_retry<T>(
    client: &ProjectApiClient<T>,
    project_name: &str,
    exclude_project_id: Option<&str>,
    progress: MigrationProgressContext<'_>,
    clone_error: &str,
    rate_limit_retries: &mut u32,
    transient_retries: &mut u32,
) -> Result<CloneRecoveryState, String>
where
    T: ProjectApiTransport,
{
    // 克隆是非幂等写操作：再次发起 POST /clone 前必须拿到一次成功的项目清单确认副本不存在。
    loop {
        ensure_step_not_cancelled(progress.control)?;
        match client.list_projects().await {
            Ok(projects) => {
                ensure_step_not_cancelled(progress.control)?;
                if let Some(existing_id) =
                    owned_project_id_by_name(&projects, project_name, exclude_project_id)
                {
                    return Ok(CloneRecoveryState::AlreadyCloned(existing_id));
                }
                return Ok(CloneRecoveryState::ConfirmedAbsent);
            }
            Err(error) if is_rate_limited(&error) => {
                let seconds = rate_limit_delay(&error, *rate_limit_retries);
                let probe_error = project_api_error_message(error);
                if *rate_limit_retries + 1 >= MIGRATION_MAX_RETRIES {
                    ensure_step_not_cancelled(progress.control)?;
                    return Err(format!("{clone_error}（核对克隆结果失败: {probe_error}）"));
                }
                wait_after_rate_limit(progress, "核对克隆结果", *rate_limit_retries, seconds)
                    .await?;
                *rate_limit_retries += 1;
            }
            Err(error) if is_transient_retryable_error(&error) => {
                let probe_error = project_api_error_message(error);
                if *transient_retries >= TRANSIENT_STEP_MAX_RETRIES {
                    ensure_step_not_cancelled(progress.control)?;
                    return Err(format!("{clone_error}（核对克隆结果失败: {probe_error}）"));
                }
                let seconds = transient_retry_delay(*transient_retries);
                *transient_retries += 1;
                wait_after_transient_error(
                    progress,
                    "核对克隆结果",
                    *transient_retries,
                    &probe_error,
                    seconds,
                )
                .await?;
            }
            Err(error) => {
                let probe_error = project_api_error_message(error);
                return Err(format!("{clone_error}（核对克隆结果失败: {probe_error}）"));
            }
        }
    }
}

async fn clone_project_with_retry<T>(
    client: &ProjectApiClient<T>,
    project_id: &str,
    project_name: &str,
    exclude_project_id: Option<&str>,
    progress: MigrationProgressContext<'_>,
) -> Result<String, String>
where
    T: ProjectApiTransport,
{
    // 结果不明的写请求只允许核对；本轮最终审计也不能再次创建副本。
    if progress
        .uncertain_clones
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(progress.project_id)
    {
        return Err("克隆结果尚未确认，请核对远端项目后再重试".to_string());
    }
    let mut rate_limit_retries = 0;
    let mut transient_retries = 0;
    loop {
        ensure_step_not_cancelled(progress.control)?;
        match client.clone_project(project_id, Some(project_name)).await {
            Ok(cloned_id) => return Ok(cloned_id),
            Err(error) if is_rate_limited(&error) => {
                let seconds = rate_limit_delay(&error, rate_limit_retries);
                let error_message = project_api_error_message(error);
                ensure_step_not_cancelled(progress.control)?;
                if let Ok(projects) = client.list_projects().await {
                    ensure_step_not_cancelled(progress.control)?;
                    if let Some(existing_id) =
                        owned_project_id_by_name(&projects, project_name, exclude_project_id)
                    {
                        return Ok(existing_id);
                    }
                }
                if rate_limit_retries + 1 >= MIGRATION_MAX_RETRIES {
                    ensure_step_not_cancelled(progress.control)?;
                    return Err(error_message);
                }
                wait_after_rate_limit(progress, "克隆项目", rate_limit_retries, seconds).await?;
                rate_limit_retries += 1;
                if let CloneRecoveryState::AlreadyCloned(existing_id) =
                    confirm_clone_state_before_retry(
                        client,
                        project_name,
                        exclude_project_id,
                        progress,
                        &error_message,
                        &mut rate_limit_retries,
                        &mut transient_retries,
                    )
                    .await?
                {
                    return Ok(existing_id);
                }
            }
            Err(error) if is_transient_retryable_error(&error) => {
                let error_message = project_api_error_message(error);
                progress
                    .uncertain_clones
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(progress.project_id.to_string());
                loop {
                    if let CloneRecoveryState::AlreadyCloned(existing_id) =
                        confirm_clone_state_before_retry(
                            client,
                            project_name,
                            exclude_project_id,
                            progress,
                            &error_message,
                            &mut rate_limit_retries,
                            &mut transient_retries,
                        )
                        .await?
                    {
                        progress
                            .uncertain_clones
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .remove(progress.project_id);
                        return Ok(existing_id);
                    }
                    if transient_retries >= TRANSIENT_STEP_MAX_RETRIES {
                        return Err(format!(
                            "{error_message}（克隆结果尚未确认，请核对远端项目后再重试）"
                        ));
                    }
                    let seconds = transient_retry_delay(transient_retries);
                    transient_retries += 1;
                    wait_after_transient_error(
                        progress,
                        "核对克隆结果",
                        transient_retries,
                        &error_message,
                        seconds,
                    )
                    .await?;
                }
            }
            Err(error) => return Err(project_api_error_message(error)),
        }
    }
}

async fn invite_collaborator_with_retry<T>(
    client: &ProjectApiClient<T>,
    project_id: &str,
    email: &str,
    privileges: &str,
    progress: MigrationProgressContext<'_>,
) -> Result<(), String>
where
    T: ProjectApiTransport,
{
    let mut rate_limit_retries = 0;
    let mut transient_retries = 0;
    loop {
        ensure_step_not_cancelled(progress.control)?;
        match client
            .invite_collaborator(project_id, email, privileges)
            .await
        {
            Ok(()) => return Ok(()),
            Err(error) if is_rate_limited(&error) => {
                let seconds = rate_limit_delay(&error, rate_limit_retries);
                let error_message = project_api_error_message(error);
                if rate_limit_retries + 1 >= MIGRATION_MAX_RETRIES {
                    ensure_step_not_cancelled(progress.control)?;
                    return Err(error_message);
                }
                wait_after_rate_limit(progress, "恢复项目协作者", rate_limit_retries, seconds)
                    .await?;
                rate_limit_retries += 1;
            }
            Err(error) if is_transient_retryable_error(&error) => {
                let error_message = project_api_error_message(error);
                if transient_retries >= TRANSIENT_STEP_MAX_RETRIES {
                    ensure_step_not_cancelled(progress.control)?;
                    return Err(error_message);
                }
                let seconds = transient_retry_delay(transient_retries);
                transient_retries += 1;
                wait_after_transient_error(
                    progress,
                    "恢复项目协作者",
                    transient_retries,
                    &error_message,
                    seconds,
                )
                .await?;
            }
            Err(error) => return Err(project_api_error_message(error)),
        }
    }
}

fn rate_limit_delay(error: &ProjectApiError, retry_index: u32) -> u64 {
    if let ProjectApiError::Http(error) = error {
        if let Some(seconds) = error.retry_after_seconds {
            return seconds.max(1);
        }
    }
    overleaf_workflows::rate_limit_wait_seconds(retry_index)
}

fn transient_retry_delay(retry_index: u32) -> u64 {
    // 短间隔原位退避（2s -> 4s -> 6s），在不明显拖慢批次的前提下吸收临时 5xx/网络抖动。
    2 * (u64::from(retry_index) + 1)
}

fn ceil_seconds(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() > 0))
}

#[cfg(test)]
mod retry_after_tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use overleaf_api::project_api::{
        ProjectApiHttpError, ProjectApiHttpResponse, ProjectApiRequest, ProjectApiTransportError,
    };
    use serde_json::json;

    use super::*;

    struct MockProjectTransport {
        projects: Mutex<Vec<serde_json::Value>>,
        clone_calls: Arc<Mutex<Vec<String>>>,
        clone_failures: Mutex<Vec<(String, u16, bool)>>,
        enable_failures: Mutex<Vec<(String, u16)>>,
        list_failures_after_clone: Mutex<VecDeque<u16>>,
    }

    impl MockProjectTransport {
        fn new(projects: Vec<serde_json::Value>) -> Self {
            Self {
                projects: Mutex::new(projects),
                clone_calls: Arc::new(Mutex::new(Vec::new())),
                clone_failures: Mutex::new(Vec::new()),
                enable_failures: Mutex::new(Vec::new()),
                list_failures_after_clone: Mutex::new(VecDeque::new()),
            }
        }
    }

    #[async_trait]
    impl ProjectApiTransport for MockProjectTransport {
        async fn send(
            &self,
            request: &ProjectApiRequest,
        ) -> Result<ProjectApiHttpResponse, ProjectApiTransportError> {
            if request.path == "/project" {
                if !self.clone_calls.lock().unwrap().is_empty() {
                    if let Some(status) = self.list_failures_after_clone.lock().unwrap().pop_front()
                    {
                        return Ok(ProjectApiHttpResponse::new(status, "list failed"));
                    }
                }
                let projects = self.projects.lock().unwrap().clone();
                return Ok(ProjectApiHttpResponse::new(
                    200,
                    json!({ "projects": projects }).to_string(),
                ));
            }
            if let Some(rest) = request.path.strip_prefix("/project/") {
                if let Some(project_id) = rest.strip_suffix("/settings/admin") {
                    let mut failures = self.enable_failures.lock().unwrap();
                    if let Some(pos) = failures.iter().position(|(id, _)| id == project_id) {
                        let (_, status) = failures.remove(pos);
                        return Ok(ProjectApiHttpResponse::new(status, "error"));
                    }
                    return Ok(ProjectApiHttpResponse::new(200, "{}"));
                }
                if let Some(project_id) = rest.strip_suffix("/tokens") {
                    return Ok(ProjectApiHttpResponse::new(
                        200,
                        json!({ "readOnly": format!("ro-{project_id}") }).to_string(),
                    ));
                }
                if let Some(project_id) = rest.strip_suffix("/clone") {
                    self.clone_calls
                        .lock()
                        .unwrap()
                        .push(project_id.to_string());
                    let name = request
                        .json_body
                        .as_ref()
                        .and_then(|body| body.get("projectName"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("cloned")
                        .to_string();
                    let cloned_id = format!("cloned-{project_id}");
                    let mut failures = self.clone_failures.lock().unwrap();
                    if let Some(pos) = failures.iter().position(|(id, _, _)| id == project_id) {
                        let (_, status, commit_before_error) = failures.remove(pos);
                        if commit_before_error {
                            self.projects.lock().unwrap().push(json!({
                                "id": cloned_id,
                                "name": name,
                                "accessLevel": "owner",
                                "trashed": false
                            }));
                        }
                        return Ok(ProjectApiHttpResponse::new(status, "server error"));
                    }
                    self.projects.lock().unwrap().push(json!({
                        "id": cloned_id,
                        "name": name,
                        "accessLevel": "owner",
                        "trashed": false
                    }));
                    return Ok(ProjectApiHttpResponse::new(
                        200,
                        json!({ "project_id": cloned_id }).to_string(),
                    ));
                }
                if let Some(project_id) = rest.strip_suffix("/leave") {
                    self.projects.lock().unwrap().retain(|project| {
                        project.get("id").and_then(|v| v.as_str()) != Some(project_id)
                    });
                    return Ok(ProjectApiHttpResponse::new(200, "{}"));
                }
            }
            if let Some(token) = request
                .path
                .strip_prefix("/read/")
                .and_then(|rest| rest.strip_suffix("/grant"))
            {
                let project_id = token.strip_prefix("ro-").unwrap_or(token).to_string();
                let mut projects = self.projects.lock().unwrap();
                if !projects
                    .iter()
                    .any(|project| project.get("id").and_then(|v| v.as_str()) == Some(&project_id))
                {
                    projects.push(json!({
                        "id": project_id,
                        "name": format!("shared-{project_id}"),
                        "accessLevel": "readOnly",
                        "trashed": false
                    }));
                }
                return Ok(ProjectApiHttpResponse::new(
                    200,
                    json!({ "projectId": project_id }).to_string(),
                ));
            }
            if let Some(token) = request.path.strip_prefix("/read/") {
                let project_id = token.strip_prefix("ro-").unwrap_or(token);
                return Ok(ProjectApiHttpResponse::new(
                    200,
                    format!(
                        r#"<meta name="ol-project" content='{{"_id":"{project_id}"}}'>"#,
                    ),
                ));
            }
            Ok(ProjectApiHttpResponse::new(404, "not found"))
        }
    }

    #[derive(Default)]
    struct RecordingMigrationControl {
        cancelled: AtomicBool,
        progress_messages: Mutex<Vec<String>>,
        countdown_messages: Mutex<Vec<String>>,
    }

    impl ProjectMigrationControl for RecordingMigrationControl {
        fn is_cancel_requested(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }

        fn on_progress(&self, event: ProjectMigrationProgressEvent) {
            self.progress_messages.lock().unwrap().push(event.message);
        }

        fn on_wait_countdown(&self, _current: u32, _total: u32, message: String) {
            self.countdown_messages.lock().unwrap().push(message);
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime")
    }

    #[test]
    fn join_request_window_preserves_initial_burst_and_releases_expired_slots() {
        let start = Instant::now();
        let mut requests = VecDeque::new();
        for index in 0..JOIN_REQUEST_LIMIT {
            let now = start + Duration::from_secs(index as u64 * 2);
            assert_eq!(join_request_delay(&mut requests, now), None);
            requests.push_back(now);
        }
        assert_eq!(
            join_request_delay(&mut requests, start + Duration::from_secs(30)),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            join_request_delay(&mut requests, start + JOIN_REQUEST_WINDOW),
            None
        );
        requests.push_back(start + JOIN_REQUEST_WINDOW);
        assert_eq!(
            join_request_delay(&mut requests, start + JOIN_REQUEST_WINDOW),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            join_request_delay(&mut requests, start + Duration::from_secs(120)),
            None
        );
        assert!(requests.is_empty());
    }

    #[test]
    fn retry_after_preserves_server_delay_and_existing_fallback() {
        for (delay, expected) in [(Some(3), 3), (Some(60), 60), (None, 30)] {
            let error = ProjectApiError::Http(ProjectApiHttpError {
                status: 429,
                kind: ProjectApiStatusKind::RateLimited,
                body_preview: None,
                retry_after_seconds: delay,
            });
            assert_eq!(rate_limit_delay(&error, 1), expected);
        }
    }

    #[test]
    fn preview_matches_migration_audit_for_same_names_and_shared_access() {
        let plans = vec![
            ProjectMigrationPlan {
                project_id: "source".into(),
                project_name: "Paper".into(),
                strategy: ProjectMigrationStrategy::OwnedProject,
            },
            ProjectMigrationPlan {
                project_id: "shared".into(),
                project_name: "Paper".into(),
                strategy: ProjectMigrationStrategy::LinkSharingCollaboration,
            },
        ];
        let mut owned = Project::new("copy", "Paper");
        owned.access_level = overleaf_core::ProjectAccessLevel::Owner;
        let preview = project_migration_preview("src", "dst", &plans, &[owned.clone()]);
        assert_eq!(
            preview.items[0].status,
            ProjectMigrationPreviewStatus::AlreadyExists
        );
        assert_eq!(
            preview.items[1].status,
            ProjectMigrationPreviewStatus::WillMigrate
        );
        let preview = project_migration_preview(
            "src",
            "dst",
            &plans,
            &[
                owned,
                Project::new("source", "Paper"),
                Project::new("shared", "Renamed"),
            ],
        );
        assert_eq!(
            preview.items[0].status,
            ProjectMigrationPreviewStatus::WillMigrate
        );
        assert_eq!(
            preview.items[1].target_project_id.as_deref(),
            Some("shared")
        );
    }

    #[test]
    fn uncertain_clone_is_not_repeated_by_final_audit() {
        let source = ProjectApiClient::new(MockProjectTransport::new(vec![
            json!({"id": "p1", "name": "Oldest", "accessLevel": "owner", "trashed": false, "lastUpdated": "2024-01-01T00:00:00Z"}),
            json!({"id": "p2", "name": "Middle", "accessLevel": "owner", "trashed": false, "lastUpdated": "2024-01-02T00:00:00Z"}),
            json!({"id": "p3", "name": "Newest", "accessLevel": "owner", "trashed": false, "lastUpdated": "2024-01-03T00:00:00Z"}),
        ]));
        let target_transport = MockProjectTransport::new(Vec::new());
        let clone_calls = Arc::clone(&target_transport.clone_calls);
        target_transport
            .clone_failures
            .lock()
            .unwrap()
            .push(("p2".to_string(), 500, false));
        let target = ProjectApiClient::new(target_transport);
        let control = RecordingMigrationControl::default();

        let error = runtime()
            .block_on(migrate_projects_between_clients_controlled(
                "src", "dst", &source, &target, &control,
            ))
            .expect_err("an ambiguous clone must remain unconfirmed");

        assert!(matches!(
            error,
            ProjectMigrationError::AuditIncomplete {
                failed_count: 1,
                missing_count: 1
            }
        ));
        assert_eq!(*clone_calls.lock().unwrap(), vec!["p1", "p2", "p3"]);
        let countdowns = control.countdown_messages.lock().unwrap();
        assert!(
            countdowns
                .iter()
                .any(|msg| msg.contains("Middle") && msg.contains("1 秒后原位重试")),
            "expected live countdown tick during 2s in-place retry, got: {countdowns:?}"
        );
    }

    #[test]
    fn transient_500_after_clone_committed_reuses_existing_clone_without_duplicate() {
        let source = ProjectApiClient::new(MockProjectTransport::new(vec![
            json!({"id": "p1", "name": "Report", "accessLevel": "owner", "trashed": false, "lastUpdated": "2024-01-01T00:00:00Z"}),
        ]));
        let target_transport = MockProjectTransport::new(Vec::new());
        let clone_calls = Arc::clone(&target_transport.clone_calls);
        target_transport
            .clone_failures
            .lock()
            .unwrap()
            .push(("p1".to_string(), 500, true));
        let target = ProjectApiClient::new(target_transport);
        let control = RecordingMigrationControl::default();

        let report = runtime()
            .block_on(migrate_projects_between_clients_controlled(
                "src", "dst", &source, &target, &control,
            ))
            .expect("idempotent clone recovery should succeed without retry sleep");

        assert_eq!(report.migrated_count, 1);
        assert_eq!(report.failed_count, 0);
        assert_eq!(report.remigration_attempted_count, 0);
        assert_eq!(*clone_calls.lock().unwrap(), vec!["p1"]);
    }

    #[test]
    fn client_4xx_skips_in_place_transient_retry_and_uses_audit_fallback() {
        let source_transport = MockProjectTransport::new(vec![
            json!({"id": "p1", "name": "First", "accessLevel": "owner", "trashed": false, "lastUpdated": "2024-01-01T00:00:00Z"}),
            json!({"id": "p2", "name": "Second", "accessLevel": "owner", "trashed": false, "lastUpdated": "2024-01-02T00:00:00Z"}),
        ]);
        source_transport
            .enable_failures
            .lock()
            .unwrap()
            .push(("p1".to_string(), 403));
        let source = ProjectApiClient::new(source_transport);
        let target_transport = MockProjectTransport::new(Vec::new());
        let clone_calls = Arc::clone(&target_transport.clone_calls);
        let target = ProjectApiClient::new(target_transport);
        let control = RecordingMigrationControl::default();

        let report = runtime()
            .block_on(migrate_projects_between_clients_controlled(
                "src", "dst", &source, &target, &control,
            ))
            .expect("final audit should act as last-resort recovery");

        assert_eq!(report.migrated_count, 2);
        assert_eq!(report.failed_count, 0);
        assert_eq!(report.remigration_attempted_count, 1);
        assert_eq!(*clone_calls.lock().unwrap(), vec!["p2", "p1"]);
        assert!(control.countdown_messages.lock().unwrap().is_empty());
    }

    #[test]
    fn wait_countdown_ticks_strictly_downward_and_respects_cancellation() {
        let control = RecordingMigrationControl::default();
        runtime()
            .block_on(wait_with_cancellation_countdown(
                &control,
                1,
                3,
                Duration::from_millis(2_150),
                |remaining| format!("等待 {remaining} 秒后继续"),
            ))
            .expect("countdown wait completes");
        assert_eq!(
            *control.countdown_messages.lock().unwrap(),
            vec!["等待 2 秒后继续", "等待 1 秒后继续"]
        );

        let cancelled = RecordingMigrationControl::default();
        cancelled.cancelled.store(true, Ordering::SeqCst);
        let err = runtime()
            .block_on(wait_with_cancellation_countdown(
                &cancelled,
                1,
                3,
                Duration::from_secs(30),
                |remaining| format!("等待 {remaining} 秒后继续"),
            ))
            .expect_err("cancelled wait should abort immediately");
        assert_eq!(err, "project migration cancelled");
    }

    #[test]
    fn transient_500_with_failing_recovery_list_recovers_without_duplicate_clone() {
        let source = ProjectApiClient::new(MockProjectTransport::new(vec![
            json!({"id": "p1", "name": "Report", "accessLevel": "owner", "trashed": false, "lastUpdated": "2024-01-01T00:00:00Z"}),
        ]));
        let target_transport = MockProjectTransport::new(Vec::new());
        let clone_calls = Arc::clone(&target_transport.clone_calls);
        target_transport
            .clone_failures
            .lock()
            .unwrap()
            .push(("p1".to_string(), 500, true));
        // 克隆已落库但紧接着两次恢复清单都返回 500；必须继续只重试读取清单，绝不能盲目二次 clone。
        target_transport
            .list_failures_after_clone
            .lock()
            .unwrap()
            .extend([500, 500]);
        let target = ProjectApiClient::new(target_transport);
        let control = RecordingMigrationControl::default();

        let report = runtime()
            .block_on(migrate_projects_between_clients_controlled(
                "src", "dst", &source, &target, &control,
            ))
            .expect("recovery list retry should detect committed clone without duplicate POST");

        assert_eq!(report.migrated_count, 1);
        assert_eq!(report.failed_count, 0);
        assert_eq!(report.remigration_attempted_count, 0);
        assert_eq!(*clone_calls.lock().unwrap(), vec!["p1"]);
    }

    #[test]
    fn transient_500_with_inconclusive_recovery_list_never_reclones() {
        let source = ProjectApiClient::new(MockProjectTransport::new(vec![
            json!({"id": "p1", "name": "Report", "accessLevel": "owner", "trashed": false, "lastUpdated": "2024-01-01T00:00:00Z"}),
        ]));
        let target_transport = MockProjectTransport::new(Vec::new());
        let clone_calls = Arc::clone(&target_transport.clone_calls);
        target_transport
            .clone_failures
            .lock()
            .unwrap()
            .push(("p1".to_string(), 500, true));
        // 预检 500、退避后核对 403（不可重试），必须中止二次 clone，交由最终审计仅离开原分享项目。
        target_transport
            .list_failures_after_clone
            .lock()
            .unwrap()
            .extend([500, 403]);
        let target = ProjectApiClient::new(target_transport);
        let control = RecordingMigrationControl::default();

        let report = runtime()
            .block_on(migrate_projects_between_clients_controlled(
                "src", "dst", &source, &target, &control,
            ))
            .expect("final audit should finish leave_project without duplicating clone");

        assert_eq!(report.migrated_count, 1);
        assert_eq!(report.failed_count, 0);
        assert_eq!(report.remigration_attempted_count, 1);
        assert_eq!(*clone_calls.lock().unwrap(), vec!["p1"]);
    }
}

async fn wait_after_rate_limit(
    progress: MigrationProgressContext<'_>,
    operation: &str,
    retry_index: u32,
    seconds: u64,
) -> Result<(), String> {
    let attempt = retry_index + 1;
    notify_migration_progress(
        progress.control,
        progress.current,
        progress.total,
        ProjectMigrationProgressLevel::Warning,
        format!(
            "项目 {} 的{}触发 API 限流，{} 秒后重试（第 {}/{} 次）",
            progress.project_name, operation, seconds, attempt, MIGRATION_MAX_RETRIES
        ),
    );
    wait_with_cancellation_countdown(
        progress.control,
        progress.current,
        progress.total,
        Duration::from_secs(seconds),
        |remaining_secs| {
            format!(
                "项目 {} 的{}触发 API 限流，{} 秒后重试（第 {}/{} 次）",
                progress.project_name, operation, remaining_secs, attempt, MIGRATION_MAX_RETRIES
            )
        },
    )
    .await
}

async fn wait_after_transient_error(
    progress: MigrationProgressContext<'_>,
    operation: &str,
    retry_attempt: u32,
    error_message: &str,
    seconds: u64,
) -> Result<(), String> {
    notify_migration_progress(
        progress.control,
        progress.current,
        progress.total,
        ProjectMigrationProgressLevel::Warning,
        format!(
            "项目 {} 的{}遇到临时错误（{}），{} 秒后原位重试（第 {}/{} 次）",
            progress.project_name,
            operation,
            error_message,
            seconds,
            retry_attempt,
            TRANSIENT_STEP_MAX_RETRIES
        ),
    );
    wait_with_cancellation_countdown(
        progress.control,
        progress.current,
        progress.total,
        Duration::from_secs(seconds),
        |remaining_secs| {
            format!(
                "项目 {} 的{}遇到临时错误（{}），{} 秒后原位重试（第 {}/{} 次）",
                progress.project_name,
                operation,
                error_message,
                remaining_secs,
                retry_attempt,
                TRANSIENT_STEP_MAX_RETRIES
            )
        },
    )
    .await
}

async fn wait_with_cancellation_countdown<F>(
    control: &(dyn ProjectMigrationControl + Send + Sync),
    current: u32,
    total: u32,
    duration: Duration,
    mut format_message: F,
) -> Result<(), String>
where
    F: FnMut(u64) -> String,
{
    let deadline = Instant::now() + duration;
    let mut last_reported_secs = ceil_seconds(duration);
    let interval = Duration::from_millis(250);
    loop {
        ensure_step_not_cancelled(control)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let remaining_secs = ceil_seconds(remaining);
        if remaining_secs > 0 && remaining_secs < last_reported_secs {
            last_reported_secs = remaining_secs;
            notify_migration_wait_countdown(
                control,
                current,
                total,
                format_message(remaining_secs),
            );
        }
        tokio::time::sleep(remaining.min(interval)).await;
    }
    ensure_step_not_cancelled(control)
}

fn ensure_step_not_cancelled(
    control: &(dyn ProjectMigrationControl + Send + Sync),
) -> Result<(), String> {
    if control.is_cancel_requested() {
        Err("project migration cancelled".to_string())
    } else {
        Ok(())
    }
}

async fn leave_project_with_retry<T>(
    client: &ProjectApiClient<T>,
    project_id: &str,
    progress: MigrationProgressContext<'_>,
) -> Result<(), String>
where
    T: ProjectApiTransport,
{
    let mut rate_limit_retries = 0;
    let mut transient_retries = 0;
    loop {
        ensure_step_not_cancelled(progress.control)?;
        match client.leave_project(project_id).await {
            Ok(()) => return Ok(()),
            Err(error) if is_not_found(&error) => return Ok(()),
            Err(error) if is_rate_limited(&error) => {
                let seconds = rate_limit_delay(&error, rate_limit_retries);
                let error_message = project_api_error_message(error);
                if rate_limit_retries + 1 >= MIGRATION_MAX_RETRIES {
                    ensure_step_not_cancelled(progress.control)?;
                    return Err(format!("离开原分享项目失败: {error_message}"));
                }
                wait_after_rate_limit(progress, "离开原分享项目", rate_limit_retries, seconds)
                    .await?;
                rate_limit_retries += 1;
            }
            Err(error) if is_transient_retryable_error(&error) => {
                let error_message = project_api_error_message(error);
                if transient_retries >= TRANSIENT_STEP_MAX_RETRIES {
                    ensure_step_not_cancelled(progress.control)?;
                    return Err(format!("离开原分享项目失败: {error_message}"));
                }
                let seconds = transient_retry_delay(transient_retries);
                transient_retries += 1;
                wait_after_transient_error(
                    progress,
                    "离开原分享项目",
                    transient_retries,
                    &error_message,
                    seconds,
                )
                .await?;
            }
            Err(error) => {
                return Err(format!(
                    "离开原分享项目失败: {}",
                    project_api_error_message(error)
                ));
            }
        }
    }
}

fn owned_project_id_by_name(
    projects: &[Project],
    project_name: &str,
    exclude_project_id: Option<&str>,
) -> Option<String> {
    projects
        .iter()
        .find(|project| {
            !project.trashed
                && project.name == project_name
                && project.access_level == overleaf_core::ProjectAccessLevel::Owner
                && Some(project.id.as_str()) != exclude_project_id
        })
        .map(|project| project.id.clone())
}

fn is_rate_limited(error: &ProjectApiError) -> bool {
    matches!(
        error,
        ProjectApiError::Http(http_error)
            if http_error.kind == ProjectApiStatusKind::RateLimited
    )
}

fn is_transient_retryable_error(error: &ProjectApiError) -> bool {
    matches!(
        error,
        ProjectApiError::Http(http_error)
            if http_error.kind == ProjectApiStatusKind::ServerError
    ) || matches!(
        error,
        ProjectApiError::Transport(transport_error)
            if transport_error.kind == ProjectApiTransportErrorKind::RequestFailed
    )
}

fn is_not_found(error: &ProjectApiError) -> bool {
    matches!(
        error,
        ProjectApiError::Http(http_error)
            if http_error.kind == ProjectApiStatusKind::NotFound
    )
}

async fn list_projects_with_retry<T>(
    client: &ProjectApiClient<T>,
    alias: &str,
    control: &(dyn ProjectMigrationControl + Send + Sync),
    current: u32,
    total: u32,
) -> Result<Vec<Project>, ProjectMigrationError>
where
    T: ProjectApiTransport,
{
    let mut rate_limit_retries = 0;
    let mut transient_retries = 0;
    loop {
        ensure_migration_not_cancelled(control)?;
        match client.list_projects().await {
            Ok(projects) => return Ok(projects),
            Err(error) if is_rate_limited(&error) => {
                let seconds = rate_limit_delay(&error, rate_limit_retries);
                let error_message = project_api_error_message(error);
                if rate_limit_retries + 1 >= MIGRATION_MAX_RETRIES {
                    ensure_migration_not_cancelled(control)?;
                    return Err(ProjectMigrationError::ProjectListFailed {
                        alias: alias.to_string(),
                        message: error_message,
                    });
                }
                let attempt = rate_limit_retries + 1;
                notify_migration_progress(
                    control,
                    current,
                    total,
                    ProjectMigrationProgressLevel::Warning,
                    format!(
                        "账号 {alias} 的项目清单触发 API 限流，{seconds} 秒后重试（第 {attempt}/{MIGRATION_MAX_RETRIES} 次）"
                    ),
                );
                wait_with_cancellation_countdown(
                    control,
                    current,
                    total,
                    Duration::from_secs(seconds),
                    |remaining_secs| {
                        format!(
                            "账号 {alias} 的项目清单触发 API 限流，{remaining_secs} 秒后重试（第 {attempt}/{MIGRATION_MAX_RETRIES} 次）"
                        )
                    },
                )
                .await
                .map_err(|_| ProjectMigrationError::Cancelled)?;
                rate_limit_retries += 1;
            }
            Err(error) if is_transient_retryable_error(&error) => {
                let error_message = project_api_error_message(error);
                if transient_retries >= TRANSIENT_STEP_MAX_RETRIES {
                    ensure_migration_not_cancelled(control)?;
                    return Err(ProjectMigrationError::ProjectListFailed {
                        alias: alias.to_string(),
                        message: error_message,
                    });
                }
                let seconds = transient_retry_delay(transient_retries);
                transient_retries += 1;
                notify_migration_progress(
                    control,
                    current,
                    total,
                    ProjectMigrationProgressLevel::Warning,
                    format!(
                        "账号 {alias} 的项目清单遇到临时错误（{error_message}），{seconds} 秒后原位重试（第 {transient_retries}/{TRANSIENT_STEP_MAX_RETRIES} 次）"
                    ),
                );
                wait_with_cancellation_countdown(
                    control,
                    current,
                    total,
                    Duration::from_secs(seconds),
                    |remaining_secs| {
                        format!(
                            "账号 {alias} 的项目清单遇到临时错误（{error_message}），{remaining_secs} 秒后原位重试（第 {transient_retries}/{TRANSIENT_STEP_MAX_RETRIES} 次）"
                        )
                    },
                )
                .await
                .map_err(|_| ProjectMigrationError::Cancelled)?;
            }
            Err(error) => {
                return Err(ProjectMigrationError::ProjectListFailed {
                    alias: alias.to_string(),
                    message: project_api_error_message(error),
                });
            }
        }
    }
}

async fn refresh_audit<T>(
    target_client: &ProjectApiClient<T>,
    target_alias: &str,
    plans: &[ProjectMigrationPlan],
    evidence: &mut ProjectMigrationEvidence,
    report: &mut ProjectMigrationReport,
    control: &(dyn ProjectMigrationControl + Send + Sync),
    total: u32,
) -> Result<(), ProjectMigrationError>
where
    T: ProjectApiTransport,
{
    evidence.target_projects =
        list_projects_with_retry(target_client, target_alias, control, total, total).await?;
    let audit = audit_project_migrations(plans, evidence);
    apply_audit(audit, report);
    Ok(())
}

fn apply_audit(audit: ProjectMigrationAudit, report: &mut ProjectMigrationReport) {
    for confirmed in &audit.confirmed {
        if let Some(item) = report.items.iter_mut().find(|item| {
            item.project_id == confirmed.project_id
                && item.status == ProjectMigrationItemStatus::Failed
        }) {
            item.status = ProjectMigrationItemStatus::Migrated;
            item.error_message = None;
        }
    }
    report.audit_confirmed_count = audit.confirmed.len();
    report.audit_missing_count = audit.missing.len();
    report.missing_projects = audit.missing.iter().map(summary).collect();
}

pub(crate) async fn reqwest_project_client(
    document: &AccountsDocument,
    alias: &str,
    secret_backend: &(dyn SecretBackend + Send + Sync),
) -> Result<ProjectApiClient<ReqwestProjectApiTransport>, ProjectMigrationError> {
    let record = document.accounts.get(alias).ok_or_else(|| {
        ProjectMigrationError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    let cookies = resolve_account_cookies(record, alias, secret_backend)
        .map_err(ProjectMigrationError::from_secret_store)?;
    if cookies
        .get(OVERLEAF_SESSION_COOKIE)
        .map(String::as_str)
        .map(str::trim)
        .is_none_or(|value| value.is_empty())
    {
        return Err(ProjectMigrationError::MissingCookies {
            alias: alias.to_string(),
        });
    }

    let session_client = OverleafSessionClient::new(ReqwestSessionTransport::new(cookies.clone()));
    let csrf_token = session_client.fetch_csrf_token().await.map_err(|error| {
        ProjectMigrationError::CsrfTokenFetchFailed {
            alias: alias.to_string(),
            message: error.to_string(),
        }
    })?;
    Ok(ProjectApiClient::new(
        ReqwestProjectApiTransport::new(cookies).with_csrf_token(csrf_token),
    ))
}

fn select_token(token_set: ProjectTokenSet) -> Result<(String, bool), String> {
    token_set
        .read_only
        .map(|token| (token, true))
        .or_else(|| token_set.read_and_write.map(|token| (token, false)))
        .ok_or_else(|| "missing project sharing token".to_string())
}

fn active_owned_project_names(projects: &[Project]) -> BTreeSet<String> {
    projects
        .iter()
        .filter(|project| !project.trashed && project.is_owner())
        .map(|project| project.name.clone())
        .collect()
}

fn active_project_id_exists(projects: &[Project], project_id: &str) -> bool {
    projects
        .iter()
        .any(|project| !project.trashed && project.id == project_id)
}

fn item(
    plan: &ProjectMigrationPlan,
    status: ProjectMigrationItemStatus,
    joined_project_id: Option<String>,
    cloned_project_id: Option<String>,
    error_message: Option<String>,
    warnings: Vec<String>,
) -> ProjectMigrationItem {
    ProjectMigrationItem {
        project_id: plan.project_id.clone(),
        project_name: plan.project_name.clone(),
        strategy: strategy_summary(plan.strategy),
        status,
        joined_project_id,
        cloned_project_id,
        error_message,
        warnings,
    }
}

fn summary(plan: &ProjectMigrationPlan) -> ProjectMigrationSummary {
    ProjectMigrationSummary {
        project_id: plan.project_id.clone(),
        project_name: plan.project_name.clone(),
        strategy: strategy_summary(plan.strategy),
    }
}

fn strategy_summary(strategy: ProjectMigrationStrategy) -> ProjectMigrationStrategySummary {
    match strategy {
        ProjectMigrationStrategy::OwnedProject => ProjectMigrationStrategySummary::OwnedProject,
        ProjectMigrationStrategy::LinkSharingCollaboration => {
            ProjectMigrationStrategySummary::LinkSharingCollaboration
        }
        ProjectMigrationStrategy::CopyCollaboration => {
            ProjectMigrationStrategySummary::CopyCollaboration
        }
    }
}

fn project_api_error_message(error: ProjectApiError) -> String {
    error.to_string()
}

impl ProjectMigrationError {
    fn from_io(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }

    fn from_secret_store(error: AccountSecretStoreError) -> Self {
        match error {
            AccountSecretStoreError::Missing { alias, .. } => Self::MissingCookies { alias },
            AccountSecretStoreError::ReadFailed { alias, category }
            | AccountSecretStoreError::WriteFailed { alias, category } => {
                Self::SecretReadFailed { alias, category }
            }
        }
    }
}
