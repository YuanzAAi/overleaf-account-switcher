use std::collections::BTreeSet;
use std::time::Duration;

use overleaf_api::project_api::{
    ProjectApiClient, ProjectApiError, ProjectApiStatusKind, ProjectApiTransport, ProjectTokenSet,
    ReqwestProjectApiTransport,
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
                current,
                total,
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
                    current: total,
                    total,
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
    let items = plans
        .iter()
        .map(|plan| {
            let target_project_id = active_project_id_by_name(target_projects, &plan.project_name);
            let status = if target_project_id.is_some() {
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
    if existing_owned_names.contains(&plan.project_name) {
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
    current: u32,
    total: u32,
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
            let token = source_client
                .get_project_tokens(&plan.project_id)
                .await
                .map_err(project_api_error_message)?;
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
    let mut last_error = None;
    for retry_index in 0..MIGRATION_MAX_RETRIES {
        ensure_step_not_cancelled(progress.control)?;
        match client.enable_link_sharing(project_id).await {
            Ok(tokens) => return Ok(tokens),
            Err(error) if is_rate_limited(&error) => {
                last_error = Some(project_api_error_message(error));
                if let Ok(tokens) = client.get_project_tokens(project_id).await {
                    return Ok(tokens);
                }
                sleep_after_retry(progress, "启用链接共享", retry_index).await?;
            }
            Err(error) => return Err(project_api_error_message(error)),
        }
    }

    Err(last_error.unwrap_or_else(|| "rate limited while enabling link sharing".to_string()))
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
    let mut last_error = None;
    for retry_index in 0..MIGRATION_MAX_RETRIES {
        ensure_step_not_cancelled(progress.control)?;
        match client.join_project_via_token(token, is_read_only).await {
            Ok(project_id) => {
                return Ok(project_id.or_else(|| Some(expected_project_id.to_string())))
            }
            Err(error) if is_rate_limited(&error) => {
                last_error = Some(project_api_error_message(error));
                let target_projects = client.list_projects().await.unwrap_or_default();
                match resolve_join_attempt(
                    expected_project_id,
                    ApiAttemptResult::RateLimited,
                    &target_projects,
                    retry_index,
                ) {
                    JoinAttemptDecision::Joined => {
                        return Ok(Some(expected_project_id.to_string()));
                    }
                    JoinAttemptDecision::RetryAfterSeconds(seconds) => {
                        wait_after_rate_limit(progress, "加入共享项目", retry_index, seconds)
                            .await?;
                    }
                    JoinAttemptDecision::Failed => {
                        return Err(last_error.unwrap_or_else(|| "join failed".to_string()));
                    }
                }
            }
            Err(error) => return Err(project_api_error_message(error)),
        }
    }

    if client
        .list_projects()
        .await
        .unwrap_or_default()
        .iter()
        .any(|project| !project.trashed && project.id == expected_project_id)
    {
        return Ok(Some(expected_project_id.to_string()));
    }

    Err(last_error.unwrap_or_else(|| "rate limited while joining project".to_string()))
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
    let mut last_error = None;
    for retry_index in 0..MIGRATION_MAX_RETRIES {
        ensure_step_not_cancelled(progress.control)?;
        match client.clone_project(project_id, Some(project_name)).await {
            Ok(cloned_id) => return Ok(cloned_id),
            Err(error) if is_rate_limited(&error) => {
                last_error = Some(project_api_error_message(error));
                if let Some(existing_id) = owned_project_id_by_name(
                    &client.list_projects().await.unwrap_or_default(),
                    project_name,
                    exclude_project_id,
                ) {
                    return Ok(existing_id);
                }
                sleep_after_retry(progress, "克隆项目", retry_index).await?;
            }
            Err(error) => return Err(project_api_error_message(error)),
        }
    }

    Err(last_error.unwrap_or_else(|| "rate limited while cloning project".to_string()))
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
    let mut last_error = None;
    for retry_index in 0..MIGRATION_MAX_RETRIES {
        ensure_step_not_cancelled(progress.control)?;
        match client
            .invite_collaborator(project_id, email, privileges)
            .await
        {
            Ok(()) => return Ok(()),
            Err(error) if is_rate_limited(&error) => {
                last_error = Some(project_api_error_message(error));
                sleep_after_retry(progress, "恢复项目协作者", retry_index).await?;
            }
            Err(error) => return Err(project_api_error_message(error)),
        }
    }

    Err(last_error.unwrap_or_else(|| "rate limited while inviting collaborator".to_string()))
}

async fn sleep_after_retry(
    progress: MigrationProgressContext<'_>,
    operation: &str,
    retry_index: u32,
) -> Result<(), String> {
    let seconds = overleaf_workflows::rate_limit_wait_seconds(retry_index);
    wait_after_rate_limit(progress, operation, retry_index, seconds).await
}

async fn wait_after_rate_limit(
    progress: MigrationProgressContext<'_>,
    operation: &str,
    retry_index: u32,
    seconds: u64,
) -> Result<(), String> {
    notify_migration_progress(
        progress.control,
        progress.current,
        progress.total,
        ProjectMigrationProgressLevel::Warning,
        format!(
            "项目 {} 的{}触发 API 限流，{} 秒后重试（第 {}/{} 次）",
            progress.project_name,
            operation,
            seconds,
            retry_index + 1,
            MIGRATION_MAX_RETRIES
        ),
    );
    wait_with_cancellation(progress.control, Duration::from_secs(seconds)).await
}

async fn wait_with_cancellation(
    control: &(dyn ProjectMigrationControl + Send + Sync),
    duration: Duration,
) -> Result<(), String> {
    let mut remaining = duration;
    let interval = Duration::from_millis(250);
    while !remaining.is_zero() {
        ensure_step_not_cancelled(control)?;
        let slice = remaining.min(interval);
        tokio::time::sleep(slice).await;
        remaining = remaining.saturating_sub(slice);
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
    let mut last_error = None;
    for retry_index in 0..MIGRATION_MAX_RETRIES {
        ensure_step_not_cancelled(progress.control)?;
        match client.leave_project(project_id).await {
            Ok(()) => return Ok(()),
            Err(error) if is_not_found(&error) => return Ok(()),
            Err(error) if is_rate_limited(&error) => {
                last_error = Some(project_api_error_message(error));
                if retry_index + 1 < MIGRATION_MAX_RETRIES {
                    sleep_after_retry(progress, "离开原分享项目", retry_index).await?;
                }
            }
            Err(error) => {
                return Err(format!(
                    "离开原分享项目失败: {}",
                    project_api_error_message(error)
                ));
            }
        }
    }

    Err(format!(
        "离开原分享项目失败: {}",
        last_error.unwrap_or_else(|| format!("failed to leave project {project_id}"))
    ))
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
    let mut last_error = None;
    for retry_index in 0..MIGRATION_MAX_RETRIES {
        ensure_migration_not_cancelled(control)?;
        match client.list_projects().await {
            Ok(projects) => return Ok(projects),
            Err(error) if is_rate_limited(&error) => {
                last_error = Some(project_api_error_message(error));
                if retry_index + 1 < MIGRATION_MAX_RETRIES {
                    let seconds = overleaf_workflows::rate_limit_wait_seconds(retry_index);
                    notify_migration_progress(
                        control,
                        current,
                        total,
                        ProjectMigrationProgressLevel::Warning,
                        format!(
                            "账号 {alias} 的项目清单触发 API 限流，{seconds} 秒后重试（第 {}/{} 次）",
                            retry_index + 1,
                            MIGRATION_MAX_RETRIES
                        ),
                    );
                    wait_with_cancellation(control, Duration::from_secs(seconds))
                        .await
                        .map_err(|_| ProjectMigrationError::Cancelled)?;
                }
            }
            Err(error) => {
                return Err(ProjectMigrationError::ProjectListFailed {
                    alias: alias.to_string(),
                    message: project_api_error_message(error),
                });
            }
        }
    }

    Err(ProjectMigrationError::ProjectListFailed {
        alias: alias.to_string(),
        message: last_error
            .unwrap_or_else(|| "project list rate limit retry exhausted".to_string()),
    })
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

fn active_project_id_by_name(projects: &[Project], project_name: &str) -> Option<String> {
    projects
        .iter()
        .find(|project| !project.trashed && project.name == project_name)
        .map(|project| project.id.clone())
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
