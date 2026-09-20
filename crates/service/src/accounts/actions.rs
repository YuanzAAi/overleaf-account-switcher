use std::collections::BTreeSet;

use overleaf_storage::{save_accounts_document, AccountStore, AccountsDocument, SecretBackend};
use overleaf_workflows::{plan_alias_passwords, BatchInputError};
use serde::Serialize;

use crate::account_secrets::{
    write_account_password_secret_tracked, AccountSecretStoreError, AccountSecretWriteJournal,
};
use crate::password_policy::validate_new_password;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalPasswordUpdateItem {
    pub alias: String,
    pub email: Option<String>,
    pub password_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalPasswordUpdateReport {
    pub updated_count: usize,
    pub updated: Vec<LocalPasswordUpdateItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountRemovalAction {
    RemoveLocalOnly,
    CleanupRemoteProjectsFirst,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountRemovalItem {
    pub alias: String,
    pub email: Option<String>,
    pub action: AccountRemovalAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountRemovalReport {
    pub items: Vec<AccountRemovalItem>,
    pub local_removal_ready_count: usize,
    pub remote_cleanup_required_count: usize,
    pub removed_count: usize,
    pub current_cleared: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountActionError {
    Io {
        message: String,
    },
    MissingEmail,
    MissingPassword,
    EmptyValue {
        field: String,
    },
    AliasCountMismatch {
        aliases: usize,
        emails: usize,
    },
    PasswordCountMismatch {
        passwords: usize,
        accounts: usize,
    },
    PasswordTooShort {
        minimum: usize,
    },
    MultiplePasswordsForSingleAccount {
        passwords: usize,
    },
    AliasConflict {
        alias: String,
    },
    AliasRepeatedInInput {
        alias: String,
    },
    AccountAliasNotFound {
        alias: String,
    },
    SecretWriteFailed {
        alias: String,
        category: &'static str,
    },
}

pub fn update_local_passwords_with_backend(
    document: &mut AccountsDocument,
    aliases_input: &str,
    passwords_input: &str,
    backend: &dyn SecretBackend,
) -> Result<LocalPasswordUpdateReport, AccountActionError> {
    let (report, _) =
        update_local_passwords_internal(document, aliases_input, passwords_input, backend)?;
    Ok(report)
}

fn update_local_passwords_internal(
    document: &mut AccountsDocument,
    aliases_input: &str,
    passwords_input: &str,
    backend: &dyn SecretBackend,
) -> Result<(LocalPasswordUpdateReport, AccountSecretWriteJournal), AccountActionError> {
    let plans = plan_alias_passwords(document, aliases_input, passwords_input)
        .map_err(AccountActionError::from)?;
    for plan in &plans {
        if let Err(error) = validate_new_password(&plan.password) {
            return Err(AccountActionError::PasswordTooShort {
                minimum: error.minimum,
            });
        }
    }
    let mut staged_document = document.clone();
    let mut journal = AccountSecretWriteJournal::default();
    let mut updated = Vec::with_capacity(plans.len());

    for plan in plans {
        let record = staged_document
            .accounts
            .get_mut(&plan.alias)
            .ok_or_else(|| AccountActionError::AccountAliasNotFound {
                alias: plan.alias.clone(),
            })?;
        if let Err(error) = write_account_password_secret_tracked(
            record,
            &plan.alias,
            &plan.password,
            backend,
            &mut journal,
        ) {
            return Err(rollback_password_update_error(journal, backend, error));
        }
        updated.push(LocalPasswordUpdateItem {
            alias: plan.alias,
            email: plan.email,
            password_present: true,
        });
    }

    *document = staged_document;
    Ok((
        LocalPasswordUpdateReport {
            updated_count: updated.len(),
            updated,
        },
        journal,
    ))
}

pub fn update_local_passwords_in_store_with_backend(
    store: &AccountStore,
    aliases_input: &str,
    passwords_input: &str,
    backend: &dyn SecretBackend,
) -> Result<LocalPasswordUpdateReport, AccountActionError> {
    let mut document = store.load().map_err(AccountActionError::from_io)?;
    let (report, journal) =
        update_local_passwords_internal(&mut document, aliases_input, passwords_input, backend)?;
    if report.updated_count > 0 {
        match save_accounts_document(store.path(), &document) {
            Ok(()) => {}
            Err(error) => {
                return Err(match journal.rollback(backend) {
                    Ok(()) => AccountActionError::from_io(error),
                    Err(rollback_error) => AccountActionError::from_secret_store(rollback_error),
                });
            }
        }
    }
    Ok(report)
}

fn rollback_password_update_error(
    journal: AccountSecretWriteJournal,
    backend: &dyn SecretBackend,
    error: AccountSecretStoreError,
) -> AccountActionError {
    match journal.rollback(backend) {
        Ok(()) => AccountActionError::from_secret_store(error),
        Err(rollback_error) => AccountActionError::from_secret_store(rollback_error),
    }
}

pub fn plan_account_removals(
    document: &AccountsDocument,
    aliases_input: &str,
    cleanup_remote_projects: bool,
) -> Result<AccountRemovalReport, AccountActionError> {
    let aliases = parse_alias_selection(aliases_input)?;
    let action = if cleanup_remote_projects {
        AccountRemovalAction::CleanupRemoteProjectsFirst
    } else {
        AccountRemovalAction::RemoveLocalOnly
    };
    let mut items = Vec::with_capacity(aliases.len());

    for alias in aliases {
        let record = document.accounts.get(&alias).ok_or_else(|| {
            AccountActionError::AccountAliasNotFound {
                alias: alias.clone(),
            }
        })?;
        items.push(AccountRemovalItem {
            alias,
            email: record.email.clone(),
            action,
        });
    }

    Ok(removal_report(items, 0, false))
}

pub fn remove_accounts_locally(
    document: &mut AccountsDocument,
    aliases_input: &str,
) -> Result<AccountRemovalReport, AccountActionError> {
    let planned = plan_account_removals(document, aliases_input, false)?;
    let mut removed_count = 0;
    let mut current_cleared = false;

    for item in &planned.items {
        if document.accounts.remove(&item.alias).is_some() {
            removed_count += 1;
        }
        if document.current.as_deref() == Some(item.alias.as_str()) {
            document.current = None;
            current_cleared = true;
        }
    }

    Ok(removal_report(
        planned.items,
        removed_count,
        current_cleared,
    ))
}

pub fn remove_accounts_locally_in_store(
    store: &AccountStore,
    aliases_input: &str,
) -> Result<AccountRemovalReport, AccountActionError> {
    let mut document = store.load().map_err(AccountActionError::from_io)?;
    let report = remove_accounts_locally(&mut document, aliases_input)?;
    if report.removed_count > 0 || report.current_cleared {
        save_accounts_document(store.path(), &document).map_err(AccountActionError::from_io)?;
    }
    Ok(report)
}

fn removal_report(
    items: Vec<AccountRemovalItem>,
    removed_count: usize,
    current_cleared: bool,
) -> AccountRemovalReport {
    AccountRemovalReport {
        local_removal_ready_count: items
            .iter()
            .filter(|item| item.action == AccountRemovalAction::RemoveLocalOnly)
            .count(),
        remote_cleanup_required_count: items
            .iter()
            .filter(|item| item.action == AccountRemovalAction::CleanupRemoteProjectsFirst)
            .count(),
        items,
        removed_count,
        current_cleared,
    }
}

fn parse_alias_selection(input: &str) -> Result<Vec<String>, AccountActionError> {
    if input.trim().is_empty() {
        return Err(AccountActionError::AccountAliasNotFound {
            alias: String::new(),
        });
    }

    let mut aliases = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in input.split([',', '，']) {
        let alias = raw.trim();
        if alias.is_empty() {
            return Err(AccountActionError::EmptyValue {
                field: "账号别名".to_string(),
            });
        }
        if !seen.insert(alias.to_string()) {
            return Err(AccountActionError::AliasRepeatedInInput {
                alias: alias.to_string(),
            });
        }
        aliases.push(alias.to_string());
    }

    Ok(aliases)
}

impl From<BatchInputError> for AccountActionError {
    fn from(error: BatchInputError) -> Self {
        match error {
            BatchInputError::MissingEmail => Self::MissingEmail,
            BatchInputError::MissingPassword => Self::MissingPassword,
            BatchInputError::EmptyValue { field } => Self::EmptyValue {
                field: field.to_string(),
            },
            BatchInputError::AliasCountMismatch { aliases, emails } => {
                Self::AliasCountMismatch { aliases, emails }
            }
            BatchInputError::PasswordCountMismatch {
                passwords,
                accounts,
            } => Self::PasswordCountMismatch {
                passwords,
                accounts,
            },
            BatchInputError::MultiplePasswordsForSingleAccount { passwords } => {
                Self::MultiplePasswordsForSingleAccount { passwords }
            }
            BatchInputError::AliasConflict { alias } => Self::AliasConflict { alias },
            BatchInputError::AliasRepeatedInInput { alias } => Self::AliasRepeatedInInput { alias },
            BatchInputError::AccountAliasNotFound { alias } => Self::AccountAliasNotFound { alias },
        }
    }
}

impl AccountActionError {
    fn from_io(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }

    fn from_secret_store(error: AccountSecretStoreError) -> Self {
        match error {
            AccountSecretStoreError::WriteFailed { alias, category }
            | AccountSecretStoreError::ReadFailed { alias, category } => {
                Self::SecretWriteFailed { alias, category }
            }
            AccountSecretStoreError::Missing { alias, category } => {
                Self::SecretWriteFailed { alias, category }
            }
        }
    }
}
