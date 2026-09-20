use overleaf_api::project_api::{
    ProjectApiClient, ProjectApiError, ProjectApiStatusKind, ProjectApiTransport,
    ReqwestProjectApiTransport,
};
use overleaf_api::session::{OverleafSessionClient, ReqwestSessionTransport};
use overleaf_core::Project;
use overleaf_storage::{save_accounts_document, AccountStore, AccountsDocument, SecretBackend};
use serde::Serialize;
use std::sync::Mutex;

use crate::account_actions::{remove_accounts_locally, AccountActionError, AccountRemovalReport};
use crate::account_secrets::{resolve_account_cookies, AccountSecretStoreError};

const CLEANUP_MAX_RETRIES: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteProjectCleanupAction {
    DeleteOwnedProject,
    LeaveCollaboratorProject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteProjectCleanupStatus {
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteProjectCleanupItem {
    pub project_id: String,
    pub project_name: String,
    pub action: RemoteProjectCleanupAction,
    pub status: RemoteProjectCleanupStatus,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteProjectCleanupPreviewItem {
    pub project_id: String,
    pub project_name: String,
    pub action: RemoteProjectCleanupAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteProjectCleanupReport {
    pub alias: String,
    pub email: Option<String>,
    pub total_projects: usize,
    pub deleted_count: usize,
    pub left_count: usize,
    pub failed_count: usize,
    pub local_account_removal_allowed: bool,
    pub items: Vec<RemoteProjectCleanupItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteProjectCleanupPreviewReport {
    pub alias: String,
    pub email: Option<String>,
    pub total_projects: usize,
    pub delete_count: usize,
    pub leave_count: usize,
    pub items: Vec<RemoteProjectCleanupPreviewItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupThenRemoveStatus {
    RemovedLocally,
    RemoteCleanupIncomplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CleanupThenRemoveAccountReport {
    pub status: CleanupThenRemoveStatus,
    pub cleanup: RemoteProjectCleanupReport,
    pub local_removal: Option<AccountRemovalReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteProjectCleanupError {
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CleanupThenRemoveAccountError {
    Io { message: String },
    Cleanup { error: RemoteProjectCleanupError },
    LocalRemoval { error: AccountActionError },
}

pub async fn preview_remote_project_cleanup_for_account<T: ProjectApiTransport>(
    document: &AccountsDocument,
    alias: &str,
    client: &ProjectApiClient<T>,
) -> Result<RemoteProjectCleanupPreviewReport, RemoteProjectCleanupError> {
    let alias = alias.trim();
    let record = document.accounts.get(alias).ok_or_else(|| {
        RemoteProjectCleanupError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    let projects = list_active_projects_with_retry(alias, client).await?;

    Ok(remote_project_cleanup_preview(
        alias,
        record.email.clone(),
        projects,
    ))
}

pub async fn cleanup_remote_projects_for_account<T: ProjectApiTransport>(
    document: &AccountsDocument,
    alias: &str,
    client: &ProjectApiClient<T>,
) -> Result<RemoteProjectCleanupReport, RemoteProjectCleanupError> {
    let alias = alias.trim();
    let record = document.accounts.get(alias).ok_or_else(|| {
        RemoteProjectCleanupError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    let projects = list_active_projects_with_retry(alias, client).await?;

    let mut report = RemoteProjectCleanupReport {
        alias: alias.to_string(),
        email: record.email.clone(),
        total_projects: projects.len(),
        deleted_count: 0,
        left_count: 0,
        failed_count: 0,
        local_account_removal_allowed: true,
        items: Vec::with_capacity(projects.len()),
    };

    for project in projects {
        let action = cleanup_action_for_project(&project);
        let result = cleanup_project_with_retry(client, &project, action).await;
        record_cleanup_result(&mut report, project, action, result);
    }

    let mut remaining = list_active_projects_with_retry(alias, client).await?;
    if !remaining.is_empty() {
        for project in remaining {
            let action = cleanup_action_for_project(&project);
            let result = cleanup_project_with_retry(client, &project, action).await;
            record_cleanup_result(&mut report, project, action, result);
        }
        remaining = list_active_projects_with_retry(alias, client).await?;
    }

    reconcile_cleanup_audit(&mut report, &remaining);

    Ok(report)
}

async fn list_active_projects_with_retry<T: ProjectApiTransport>(
    alias: &str,
    client: &ProjectApiClient<T>,
) -> Result<Vec<Project>, RemoteProjectCleanupError> {
    let mut last_error = None;
    for retry_index in 0..CLEANUP_MAX_RETRIES {
        match client.list_projects().await {
            Ok(projects) => {
                return Ok(projects
                    .into_iter()
                    .filter(|project| !project.trashed)
                    .collect())
            }
            Err(error) if is_rate_limited(&error) => {
                last_error = Some(project_api_error_message(error));
                if retry_index + 1 < CLEANUP_MAX_RETRIES {
                    sleep_after_rate_limit(retry_index).await;
                }
            }
            Err(error) => {
                return Err(RemoteProjectCleanupError::ProjectListFailed {
                    alias: alias.to_string(),
                    message: project_api_error_message(error),
                });
            }
        }
    }

    Err(RemoteProjectCleanupError::ProjectListFailed {
        alias: alias.to_string(),
        message: last_error
            .unwrap_or_else(|| "project list rate limit retry exhausted".to_string()),
    })
}

async fn cleanup_project_with_retry<T: ProjectApiTransport>(
    client: &ProjectApiClient<T>,
    project: &Project,
    action: RemoteProjectCleanupAction,
) -> Result<(), String> {
    let mut last_error = None;
    for retry_index in 0..CLEANUP_MAX_RETRIES {
        let result = match action {
            RemoteProjectCleanupAction::DeleteOwnedProject => {
                client.delete_project(&project.id).await
            }
            RemoteProjectCleanupAction::LeaveCollaboratorProject => {
                client.leave_project(&project.id).await
            }
        };
        match result {
            Ok(()) => return Ok(()),
            Err(error) if is_not_found(&error) => return Ok(()),
            Err(error) if is_rate_limited(&error) => {
                last_error = Some(project_api_error_message(error));
                if retry_index + 1 < CLEANUP_MAX_RETRIES {
                    sleep_after_rate_limit(retry_index).await;
                }
            }
            Err(error) => return Err(project_api_error_message(error)),
        }
    }

    Err(last_error.unwrap_or_else(|| "project cleanup rate limit retry exhausted".to_string()))
}

async fn sleep_after_rate_limit(retry_index: u32) {
    tokio::time::sleep(std::time::Duration::from_secs(
        overleaf_workflows::rate_limit_wait_seconds(retry_index),
    ))
    .await;
}

fn record_cleanup_result(
    report: &mut RemoteProjectCleanupReport,
    project: Project,
    action: RemoteProjectCleanupAction,
    result: Result<(), String>,
) {
    let (status, error_message) = match result {
        Ok(()) => (RemoteProjectCleanupStatus::Completed, None),
        Err(error) => (RemoteProjectCleanupStatus::Failed, Some(error)),
    };
    let next = cleanup_item(project, action, status, error_message);
    if let Some(existing) = report
        .items
        .iter_mut()
        .find(|item| item.project_id == next.project_id)
    {
        *existing = next;
    } else {
        report.items.push(next);
    }
}

fn reconcile_cleanup_audit(report: &mut RemoteProjectCleanupReport, remaining: &[Project]) {
    for item in &mut report.items {
        if remaining
            .iter()
            .any(|project| project.id == item.project_id)
        {
            item.status = RemoteProjectCleanupStatus::Failed;
            item.error_message = Some("project is still active after cleanup".to_string());
        } else {
            item.status = RemoteProjectCleanupStatus::Completed;
            item.error_message = None;
        }
    }
    for project in remaining {
        if report
            .items
            .iter()
            .any(|item| item.project_id == project.id)
        {
            continue;
        }
        let action = cleanup_action_for_project(project);
        report.items.push(cleanup_item(
            project.clone(),
            action,
            RemoteProjectCleanupStatus::Failed,
            Some("project is still active after cleanup".to_string()),
        ));
    }

    report.deleted_count = report
        .items
        .iter()
        .filter(|item| {
            item.status == RemoteProjectCleanupStatus::Completed
                && item.action == RemoteProjectCleanupAction::DeleteOwnedProject
        })
        .count();
    report.left_count = report
        .items
        .iter()
        .filter(|item| {
            item.status == RemoteProjectCleanupStatus::Completed
                && item.action == RemoteProjectCleanupAction::LeaveCollaboratorProject
        })
        .count();
    report.failed_count = report
        .items
        .iter()
        .filter(|item| item.status == RemoteProjectCleanupStatus::Failed)
        .count();
    report.local_account_removal_allowed = report.failed_count == 0 && remaining.is_empty();
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

pub async fn cleanup_remote_projects_then_remove_account<T: ProjectApiTransport>(
    document: &mut AccountsDocument,
    alias: &str,
    client: &ProjectApiClient<T>,
) -> Result<CleanupThenRemoveAccountReport, CleanupThenRemoveAccountError> {
    let cleanup = cleanup_remote_projects_for_account(document, alias, client)
        .await
        .map_err(|error| CleanupThenRemoveAccountError::Cleanup { error })?;

    if !cleanup.local_account_removal_allowed {
        return Ok(CleanupThenRemoveAccountReport {
            status: CleanupThenRemoveStatus::RemoteCleanupIncomplete,
            cleanup,
            local_removal: None,
        });
    }

    let local_removal = remove_accounts_locally(document, alias)
        .map_err(|error| CleanupThenRemoveAccountError::LocalRemoval { error })?;
    Ok(CleanupThenRemoveAccountReport {
        status: CleanupThenRemoveStatus::RemovedLocally,
        cleanup,
        local_removal: Some(local_removal),
    })
}

pub async fn cleanup_remote_projects_then_remove_account_in_store<T: ProjectApiTransport>(
    store: &AccountStore,
    alias: &str,
    client: &ProjectApiClient<T>,
) -> Result<CleanupThenRemoveAccountReport, CleanupThenRemoveAccountError> {
    let mut document = store
        .load()
        .map_err(CleanupThenRemoveAccountError::from_io)?;
    let report = cleanup_remote_projects_then_remove_account(&mut document, alias, client).await?;
    if report.status == CleanupThenRemoveStatus::RemovedLocally {
        save_accounts_document(store.path(), &document)
            .map_err(CleanupThenRemoveAccountError::from_io)?;
    }
    Ok(report)
}

pub async fn cleanup_remote_projects_in_store<T: ProjectApiTransport>(
    store: &AccountStore,
    alias: &str,
    client: &ProjectApiClient<T>,
) -> Result<RemoteProjectCleanupReport, RemoteProjectCleanupError> {
    let document = store
        .load()
        .map_err(|error| RemoteProjectCleanupError::ProjectListFailed {
            alias: alias.trim().to_string(),
            message: error.to_string(),
        })?;
    cleanup_remote_projects_for_account(&document, alias, client).await
}

#[async_trait::async_trait]
pub trait RemoteProjectCleanupExecutor {
    async fn preview_cleanup(
        &self,
        store: &AccountStore,
        alias: &str,
        secret_backend: &(dyn SecretBackend + Send + Sync),
    ) -> Result<RemoteProjectCleanupPreviewReport, RemoteProjectCleanupError>;

    async fn cleanup_then_remove_account(
        &self,
        store: &AccountStore,
        alias: &str,
        secret_backend: &(dyn SecretBackend + Send + Sync),
        commit_lock: Option<&Mutex<()>>,
    ) -> Result<CleanupThenRemoveAccountReport, CleanupThenRemoveAccountError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReqwestRemoteProjectCleanupExecutor;

#[async_trait::async_trait]
impl RemoteProjectCleanupExecutor for ReqwestRemoteProjectCleanupExecutor {
    async fn preview_cleanup(
        &self,
        store: &AccountStore,
        alias: &str,
        secret_backend: &(dyn SecretBackend + Send + Sync),
    ) -> Result<RemoteProjectCleanupPreviewReport, RemoteProjectCleanupError> {
        preview_remote_project_cleanup_with_reqwest_in_store(store, alias, secret_backend).await
    }

    async fn cleanup_then_remove_account(
        &self,
        store: &AccountStore,
        alias: &str,
        secret_backend: &(dyn SecretBackend + Send + Sync),
        commit_lock: Option<&Mutex<()>>,
    ) -> Result<CleanupThenRemoveAccountReport, CleanupThenRemoveAccountError> {
        cleanup_then_remove_with_commit_lock(store, alias, secret_backend, commit_lock).await
    }
}

pub async fn preview_remote_project_cleanup_with_reqwest_in_store(
    store: &AccountStore,
    alias: &str,
    secret_backend: &(dyn SecretBackend + Send + Sync),
) -> Result<RemoteProjectCleanupPreviewReport, RemoteProjectCleanupError> {
    let document = store
        .load()
        .map_err(|error| RemoteProjectCleanupError::ProjectListFailed {
            alias: alias.trim().to_string(),
            message: error.to_string(),
        })?;
    let alias = alias.trim();
    let project_client =
        reqwest_project_client_for_account(&document, alias, secret_backend).await?;
    preview_remote_project_cleanup_for_account(&document, alias, &project_client).await
}

pub async fn cleanup_remote_projects_with_reqwest_in_store(
    store: &AccountStore,
    alias: &str,
    secret_backend: &(dyn SecretBackend + Send + Sync),
) -> Result<RemoteProjectCleanupReport, RemoteProjectCleanupError> {
    let document = store
        .load()
        .map_err(|error| RemoteProjectCleanupError::ProjectListFailed {
            alias: alias.trim().to_string(),
            message: error.to_string(),
        })?;
    let alias = alias.trim();
    let project_client =
        reqwest_project_client_for_account(&document, alias, secret_backend).await?;
    cleanup_remote_projects_for_account(&document, alias, &project_client).await
}

pub async fn cleanup_remote_projects_then_remove_account_with_reqwest_in_store(
    store: &AccountStore,
    alias: &str,
    secret_backend: &(dyn SecretBackend + Send + Sync),
) -> Result<CleanupThenRemoveAccountReport, CleanupThenRemoveAccountError> {
    cleanup_then_remove_with_commit_lock(store, alias, secret_backend, None).await
}

async fn cleanup_then_remove_with_commit_lock(
    store: &AccountStore,
    alias: &str,
    secret_backend: &(dyn SecretBackend + Send + Sync),
    commit_lock: Option<&Mutex<()>>,
) -> Result<CleanupThenRemoveAccountReport, CleanupThenRemoveAccountError> {
    let cleanup = cleanup_remote_projects_with_reqwest_in_store(store, alias, secret_backend)
        .await
        .map_err(|error| CleanupThenRemoveAccountError::Cleanup { error })?;
    if !cleanup.local_account_removal_allowed {
        return Ok(CleanupThenRemoveAccountReport {
            status: CleanupThenRemoveStatus::RemoteCleanupIncomplete,
            cleanup,
            local_removal: None,
        });
    }
    // 只在提交本地删除时持锁，并重新读取，保留并发任务写入的其他账号。
    let _guard = commit_lock.map(Mutex::lock).transpose().map_err(|_| {
        CleanupThenRemoveAccountError::Io {
            message: "account commit lock poisoned".to_string(),
        }
    })?;
    let local_removal = crate::remove_accounts_locally_in_store(store, alias)
        .map_err(|error| CleanupThenRemoveAccountError::LocalRemoval { error })?;
    Ok(CleanupThenRemoveAccountReport {
        status: CleanupThenRemoveStatus::RemovedLocally,
        cleanup,
        local_removal: Some(local_removal),
    })
}

async fn reqwest_project_client_for_account(
    document: &AccountsDocument,
    alias: &str,
    secret_backend: &(dyn SecretBackend + Send + Sync),
) -> Result<ProjectApiClient<ReqwestProjectApiTransport>, RemoteProjectCleanupError> {
    let record = document.accounts.get(alias).ok_or_else(|| {
        RemoteProjectCleanupError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    let cookies = resolve_account_cookies(record, alias, secret_backend)
        .map_err(RemoteProjectCleanupError::from_secret_store)?;
    let session_client = OverleafSessionClient::new(ReqwestSessionTransport::new(cookies.clone()));
    let csrf_token = session_client.fetch_csrf_token().await.map_err(|error| {
        RemoteProjectCleanupError::CsrfTokenFetchFailed {
            alias: alias.to_string(),
            message: error.to_string(),
        }
    })?;
    Ok(ProjectApiClient::new(
        ReqwestProjectApiTransport::new(cookies).with_csrf_token(csrf_token),
    ))
}

fn remote_project_cleanup_preview(
    alias: &str,
    email: Option<String>,
    projects: Vec<Project>,
) -> RemoteProjectCleanupPreviewReport {
    let mut delete_count = 0;
    let mut leave_count = 0;
    let items = projects
        .into_iter()
        .map(|project| {
            let action = cleanup_action_for_project(&project);
            match action {
                RemoteProjectCleanupAction::DeleteOwnedProject => delete_count += 1,
                RemoteProjectCleanupAction::LeaveCollaboratorProject => leave_count += 1,
            }
            cleanup_preview_item(project, action)
        })
        .collect::<Vec<_>>();

    RemoteProjectCleanupPreviewReport {
        alias: alias.to_string(),
        email,
        total_projects: items.len(),
        delete_count,
        leave_count,
        items,
    }
}

fn cleanup_action_for_project(project: &Project) -> RemoteProjectCleanupAction {
    if project.is_owner() {
        RemoteProjectCleanupAction::DeleteOwnedProject
    } else {
        RemoteProjectCleanupAction::LeaveCollaboratorProject
    }
}

fn cleanup_preview_item(
    project: Project,
    action: RemoteProjectCleanupAction,
) -> RemoteProjectCleanupPreviewItem {
    RemoteProjectCleanupPreviewItem {
        project_id: project.id,
        project_name: project.name,
        action,
    }
}

fn cleanup_item(
    project: Project,
    action: RemoteProjectCleanupAction,
    status: RemoteProjectCleanupStatus,
    error_message: Option<String>,
) -> RemoteProjectCleanupItem {
    RemoteProjectCleanupItem {
        project_id: project.id,
        project_name: project.name,
        action,
        status,
        error_message,
    }
}

fn project_api_error_message(error: ProjectApiError) -> String {
    error.to_string()
}

impl CleanupThenRemoveAccountError {
    fn from_io(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }
}

impl RemoteProjectCleanupError {
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
