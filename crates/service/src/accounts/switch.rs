use overleaf_browser::{
    CookiePayload, ExtensionBridgeSession, ExtensionCommand, ExtensionResponse,
};
use overleaf_storage::{AccountStore, AccountsDocument, SecretBackend};
use serde::Serialize;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::account_secrets::{resolve_account_cookies_for_recovery, AccountSecretStoreError};
use crate::{
    AccountSessionError, AccountSessionIdentityValidator, ProjectMigrationControl,
    ProjectMigrationError, ProjectMigrationExecutor, ProjectMigrationReport,
};

const OVERLEAF_SESSION_COOKIE_NAME: &str = "overleaf_session2";

#[derive(Debug, Clone, PartialEq)]
pub struct AccountSwitchExtensionPlan {
    pub alias: String,
    pub email: Option<String>,
    pub current_before: Option<String>,
    pub migrate_projects: bool,
    pub commands: Vec<ExtensionCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountSwitchPlanReport {
    pub alias: String,
    pub email: Option<String>,
    pub current_before: Option<String>,
    pub migrate_projects: bool,
    pub command_count: usize,
    pub commands: Vec<AccountSwitchCommandSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccountSwitchExecutionReport {
    pub plan: AccountSwitchPlanReport,
    pub project_migration: Option<ProjectMigrationReport>,
    pub executed_count: usize,
    pub current_before: Option<String>,
    pub current_after: Option<String>,
    pub responses: Vec<AccountSwitchResponseSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountSwitchCommandSummary {
    pub action: &'static str,
    pub request_id: Option<String>,
    pub cookie_name: Option<String>,
    pub cookie_present: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccountSwitchResponseSummary {
    pub action: &'static str,
    pub request_id: String,
    pub success: bool,
    pub message: Option<String>,
    pub refreshed: Option<u32>,
    pub expiration_date: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountSwitchError {
    Io {
        message: String,
    },
    AccountAliasNotFound {
        alias: String,
    },
    MissingSessionCookie {
        alias: String,
    },
    SecretReadFailed {
        alias: String,
        category: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AccountSwitchExecutionError {
    Plan(AccountSwitchError),
    Session(AccountSessionError),
    Bridge {
        message: String,
    },
    ProjectMigrationNotConfigured,
    ProjectMigration(ProjectMigrationError),
    CommandFailed {
        action: &'static str,
        request_id: String,
        message: String,
    },
}

impl AccountSwitchExecutionError {
    pub(crate) fn requires_cookie_recovery(&self) -> bool {
        match self {
            Self::Plan(AccountSwitchError::MissingSessionCookie { .. }) => true,
            Self::Session(error) => error.requires_cookie_recovery(),
            Self::CommandFailed {
                action, message, ..
            } => {
                matches!(*action, "get_cookie" | "get_cookie_expiry")
                    && message.trim() == "Cookie不存在"
            }
            _ => false,
        }
    }
}

#[async_trait::async_trait]
pub trait AccountSwitchCommandExecutor {
    async fn execute_extension_command(
        &self,
        command: ExtensionCommand,
    ) -> Result<ExtensionResponse, AccountSwitchExecutionError>;

    async fn extension_client_count(&self) -> Option<usize> {
        None
    }
}

pub fn plan_account_switch(
    document: &AccountsDocument,
    alias: &str,
    migrate_projects: bool,
    secret_backend: &(dyn SecretBackend + Send + Sync),
) -> Result<AccountSwitchExtensionPlan, AccountSwitchError> {
    let alias = alias.trim();
    let record =
        document
            .accounts
            .get(alias)
            .ok_or_else(|| AccountSwitchError::AccountAliasNotFound {
                alias: alias.to_string(),
            })?;
    let cookies = resolve_account_cookies_for_recovery(record, alias, secret_backend)
        .map_err(AccountSwitchError::from_secret_store)?;
    let session_cookie = cookies
        .get(OVERLEAF_SESSION_COOKIE_NAME)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AccountSwitchError::MissingSessionCookie {
            alias: alias.to_string(),
        })?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let mut bridge = ExtensionBridgeSession::new();
    let commands = vec![
        bridge.issue_set_cookie(CookiePayload::overleaf_session(
            session_cookie,
            record
                .cookie_expiry
                .filter(|expiry| expiry.is_finite() && *expiry > now),
        )),
        bridge.issue_refresh_tabs(),
        bridge.issue_get_cookie_expiry(),
    ];

    Ok(AccountSwitchExtensionPlan {
        alias: alias.to_string(),
        email: record.email.clone(),
        current_before: document.current.clone(),
        migrate_projects,
        commands,
    })
}

pub fn plan_account_switch_in_store(
    store: &AccountStore,
    alias: &str,
    migrate_projects: bool,
    secret_backend: &(dyn SecretBackend + Send + Sync),
) -> Result<AccountSwitchExtensionPlan, AccountSwitchError> {
    let document = store.load().map_err(|error| AccountSwitchError::Io {
        message: error.to_string(),
    })?;
    plan_account_switch(&document, alias, migrate_projects, secret_backend)
}

pub async fn execute_account_switch_in_store(
    store: &AccountStore,
    alias: &str,
    migrate_projects: bool,
    executor: &(dyn AccountSwitchCommandExecutor + Send + Sync),
    secret_backend: &(dyn SecretBackend + Send + Sync),
    session_identity_validator: &(dyn AccountSessionIdentityValidator + Send + Sync),
) -> Result<AccountSwitchExecutionReport, AccountSwitchExecutionError> {
    execute_account_switch_with_project_migration_in_store(
        store,
        alias,
        migrate_projects,
        executor,
        secret_backend,
        session_identity_validator,
        None,
    )
    .await
}

pub async fn execute_account_switch_with_project_migration_in_store(
    store: &AccountStore,
    alias: &str,
    migrate_projects: bool,
    executor: &(dyn AccountSwitchCommandExecutor + Send + Sync),
    secret_backend: &(dyn SecretBackend + Send + Sync),
    session_identity_validator: &(dyn AccountSessionIdentityValidator + Send + Sync),
    project_migration_executor: Option<&(dyn ProjectMigrationExecutor + Send + Sync)>,
) -> Result<AccountSwitchExecutionReport, AccountSwitchExecutionError> {
    execute_account_switch_with_project_migration_controlled_in_store(
        store,
        alias,
        migrate_projects,
        executor,
        secret_backend,
        session_identity_validator,
        project_migration_executor,
        None,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "Preserve the public switch API; internal calls use AccountSwitchContext"
)]
pub async fn execute_account_switch_with_project_migration_controlled_in_store(
    store: &AccountStore,
    alias: &str,
    migrate_projects: bool,
    executor: &(dyn AccountSwitchCommandExecutor + Send + Sync),
    secret_backend: &(dyn SecretBackend + Send + Sync),
    session_identity_validator: &(dyn AccountSessionIdentityValidator + Send + Sync),
    project_migration_executor: Option<&(dyn ProjectMigrationExecutor + Send + Sync)>,
    project_migration_control: Option<&(dyn ProjectMigrationControl + Send + Sync)>,
) -> Result<AccountSwitchExecutionReport, AccountSwitchExecutionError> {
    execute_account_switch_with_commit_lock(
        store,
        alias,
        migrate_projects,
        AccountSwitchContext {
            executor,
            secret_backend,
            session_identity_validator,
            project_migration_executor,
            project_migration_control,
            commit_lock: None,
        },
    )
    .await
}

pub(crate) struct AccountSwitchContext<'a> {
    pub executor: &'a (dyn AccountSwitchCommandExecutor + Send + Sync),
    pub secret_backend: &'a (dyn SecretBackend + Send + Sync),
    pub session_identity_validator: &'a (dyn AccountSessionIdentityValidator + Send + Sync),
    pub project_migration_executor: Option<&'a (dyn ProjectMigrationExecutor + Send + Sync)>,
    pub project_migration_control: Option<&'a (dyn ProjectMigrationControl + Send + Sync)>,
    pub commit_lock: Option<&'a Mutex<()>>,
}

pub(crate) async fn execute_account_switch_with_commit_lock(
    store: &AccountStore,
    alias: &str,
    migrate_projects: bool,
    context: AccountSwitchContext<'_>,
) -> Result<AccountSwitchExecutionReport, AccountSwitchExecutionError> {
    let AccountSwitchContext {
        executor,
        secret_backend,
        session_identity_validator,
        project_migration_executor,
        project_migration_control,
        commit_lock,
    } = context;
    let document = store.load().map_err(|error| {
        AccountSwitchExecutionError::Plan(AccountSwitchError::Io {
            message: error.to_string(),
        })
    })?;
    validate_account_switch_session(&document, alias, secret_backend, session_identity_validator)
        .await?;
    let plan = plan_account_switch(&document, alias, migrate_projects, secret_backend)
        .map_err(AccountSwitchExecutionError::Plan)?;

    let project_migration = if migrate_projects {
        let Some(project_migration_executor) = project_migration_executor else {
            return Err(AccountSwitchExecutionError::ProjectMigrationNotConfigured);
        };
        let migration = match project_migration_control {
            Some(control) => {
                project_migration_executor
                    .migrate_projects_with_control(store, &plan.alias, secret_backend, control)
                    .await
            }
            None => {
                project_migration_executor
                    .migrate_projects(store, &plan.alias, secret_backend)
                    .await
            }
        }
        .map_err(AccountSwitchExecutionError::ProjectMigration)?;
        Some(migration)
    } else {
        None
    };

    let mut responses = Vec::with_capacity(plan.commands.len());
    for command in plan.commands.iter().cloned() {
        if project_migration_control.is_some_and(|control| control.is_cancel_requested()) {
            return Err(AccountSwitchExecutionError::ProjectMigration(
                ProjectMigrationError::Cancelled,
            ));
        }
        let action = command_action(&command);
        let response = executor.execute_extension_command(command).await?;
        let summary = response_summary(action, &response);
        if !response.success {
            return Err(AccountSwitchExecutionError::CommandFailed {
                action,
                request_id: response.request_id,
                message: response
                    .message
                    .unwrap_or_else(|| "extension command failed".to_string()),
            });
        }
        responses.push(summary);
    }

    // 网络操作结束后再持锁读取，避免覆盖其他账号在此期间保存的结果。
    let _guard = commit_lock.map(Mutex::lock).transpose().map_err(|_| {
        AccountSwitchExecutionError::Plan(AccountSwitchError::Io {
            message: "account commit lock poisoned".to_string(),
        })
    })?;
    let mut document = store.load().map_err(|error| {
        AccountSwitchExecutionError::Plan(AccountSwitchError::Io {
            message: error.to_string(),
        })
    })?;
    if !document.accounts.contains_key(&plan.alias) {
        return Err(AccountSwitchExecutionError::Plan(
            AccountSwitchError::AccountAliasNotFound { alias: plan.alias },
        ));
    }
    let current_before = document.current.clone();
    document.current = Some(plan.alias.clone());
    store.save(&document).map_err(|error| {
        AccountSwitchExecutionError::Plan(AccountSwitchError::Io {
            message: error.to_string(),
        })
    })?;

    Ok(AccountSwitchExecutionReport {
        plan: plan.redacted_report(),
        project_migration,
        executed_count: responses.len(),
        current_before,
        current_after: Some(plan.alias),
        responses,
    })
}

async fn validate_account_switch_session(
    document: &AccountsDocument,
    alias: &str,
    secret_backend: &(dyn SecretBackend + Send + Sync),
    session_identity_validator: &(dyn AccountSessionIdentityValidator + Send + Sync),
) -> Result<(), AccountSwitchExecutionError> {
    let alias = alias.trim();
    let record = document.accounts.get(alias).ok_or_else(|| {
        AccountSwitchExecutionError::Plan(AccountSwitchError::AccountAliasNotFound {
            alias: alias.to_string(),
        })
    })?;
    let cookies = resolve_account_cookies_for_recovery(record, alias, secret_backend)
        .map_err(AccountSwitchError::from_secret_store)
        .map_err(AccountSwitchExecutionError::Plan)?;
    session_identity_validator
        .validate(alias, record.email.as_deref(), &cookies)
        .await
        .map_err(AccountSwitchExecutionError::Session)?;
    Ok(())
}

impl AccountSwitchError {
    fn from_secret_store(error: AccountSecretStoreError) -> Self {
        match error {
            AccountSecretStoreError::Missing { alias, .. } => Self::MissingSessionCookie { alias },
            AccountSecretStoreError::ReadFailed { alias, category }
            | AccountSecretStoreError::WriteFailed { alias, category } => {
                Self::SecretReadFailed { alias, category }
            }
        }
    }
}

impl AccountSwitchExtensionPlan {
    pub fn redacted_report(&self) -> AccountSwitchPlanReport {
        AccountSwitchPlanReport {
            alias: self.alias.clone(),
            email: self.email.clone(),
            current_before: self.current_before.clone(),
            migrate_projects: self.migrate_projects,
            command_count: self.commands.len(),
            commands: self.commands.iter().map(command_summary).collect(),
        }
    }
}

fn command_summary(command: &ExtensionCommand) -> AccountSwitchCommandSummary {
    let action = command_action(command);
    match command {
        ExtensionCommand::SetCookie { cookie, request_id } => AccountSwitchCommandSummary {
            action,
            request_id: Some(request_id.clone()),
            cookie_name: Some(cookie.name.clone()),
            cookie_present: !cookie.value.trim().is_empty(),
        },
        ExtensionCommand::RefreshTabs { request_id } => AccountSwitchCommandSummary {
            action,
            request_id: Some(request_id.clone()),
            cookie_name: None,
            cookie_present: false,
        },
        ExtensionCommand::GetCookieExpiry { request_id } => AccountSwitchCommandSummary {
            action,
            request_id: Some(request_id.clone()),
            cookie_name: None,
            cookie_present: false,
        },
        ExtensionCommand::GetCookie { name, request_id } => AccountSwitchCommandSummary {
            action,
            request_id: Some(request_id.clone()),
            cookie_name: Some(name.clone()),
            cookie_present: false,
        },
        ExtensionCommand::Ping => AccountSwitchCommandSummary {
            action,
            request_id: None,
            cookie_name: None,
            cookie_present: false,
        },
    }
}

fn command_action(command: &ExtensionCommand) -> &'static str {
    match command {
        ExtensionCommand::GetCookie { .. } => "get_cookie",
        ExtensionCommand::SetCookie { .. } => "set_cookie",
        ExtensionCommand::RefreshTabs { .. } => "refresh_tabs",
        ExtensionCommand::GetCookieExpiry { .. } => "get_cookie_expiry",
        ExtensionCommand::Ping => "ping",
    }
}

fn response_summary(
    action: &'static str,
    response: &ExtensionResponse,
) -> AccountSwitchResponseSummary {
    AccountSwitchResponseSummary {
        action,
        request_id: response.request_id.clone(),
        success: response.success,
        message: response.message.clone(),
        refreshed: response.refreshed,
        expiration_date: response.expiration_date,
    }
}
