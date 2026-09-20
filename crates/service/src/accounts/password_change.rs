use std::collections::BTreeMap;

use overleaf_browser::{
    cookies_to_map, BrowserAutomation, BrowserAutomationError, CredentialsLoginInput, LoginResult,
    PasswordChangeInput, SecretText,
};
use overleaf_core::normalize_email;
use overleaf_storage::{AccountRecord, AccountStore, AccountsDocument, SecretBackend};
use serde::Serialize;

use crate::account_secrets::{
    read_account_password_secret, write_account_cookie_secrets_tracked,
    write_account_password_secret_tracked, AccountSecretStoreError, AccountSecretWriteJournal,
};
use crate::account_session::{apply_login_metadata, overleaf_session_expiry};
use crate::password_policy::validate_new_password;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverleafPasswordChangeStatus {
    Changed,
    FailedButLoginUpdated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverleafPasswordChangeReport {
    pub alias: String,
    pub email: String,
    pub status: OverleafPasswordChangeStatus,
    pub password_updated: bool,
    pub login_updated: bool,
    pub cookie_expiry: Option<i64>,
    pub failure_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OverleafPasswordChangeError {
    Io {
        message: String,
    },
    AccountAliasNotFound {
        alias: String,
    },
    MissingEmail {
        alias: String,
    },
    MissingCurrentPassword {
        alias: String,
    },
    MissingNewPassword,
    PasswordTooShort {
        minimum: usize,
    },
    MissingSessionCookie,
    InvalidCredentials,
    ChallengeRequired,
    RobotVerificationBlocked,
    LoginEmailMismatch {
        expected: String,
        actual: String,
    },
    SecretReadFailed {
        alias: String,
        category: &'static str,
    },
    SecretWriteFailed {
        alias: String,
        category: &'static str,
    },
    Browser {
        message: String,
    },
}

pub async fn change_account_overleaf_password_with_backend<B>(
    document: &mut AccountsDocument,
    alias: &str,
    new_password: SecretText,
    browser: &B,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<OverleafPasswordChangeReport, OverleafPasswordChangeError>
where
    B: BrowserAutomation + Sync + ?Sized,
{
    let (report, _) = change_account_overleaf_password_internal(
        document,
        alias,
        new_password,
        browser,
        now_unix,
        backend,
    )
    .await?;
    Ok(report)
}

async fn change_account_overleaf_password_internal<B>(
    document: &mut AccountsDocument,
    alias: &str,
    new_password: SecretText,
    browser: &B,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<(OverleafPasswordChangeReport, AccountSecretWriteJournal), OverleafPasswordChangeError>
where
    B: BrowserAutomation + Sync + ?Sized,
{
    let alias = alias.trim();
    if new_password.is_empty() {
        return Err(OverleafPasswordChangeError::MissingNewPassword);
    }
    if let Err(error) = validate_new_password(new_password.expose()) {
        return Err(OverleafPasswordChangeError::PasswordTooShort {
            minimum: error.minimum,
        });
    }

    let original = document.accounts.get(alias).cloned();
    let record =
        original
            .as_ref()
            .ok_or_else(|| OverleafPasswordChangeError::AccountAliasNotFound {
                alias: alias.to_string(),
            })?;
    let email = non_empty(record.email.as_deref())
        .ok_or_else(|| OverleafPasswordChangeError::MissingEmail {
            alias: alias.to_string(),
        })?
        .to_string();
    let current_password = SecretText::new(current_password_for_change(record, alias, backend)?);
    let login = browser
        .login_with_credentials(CredentialsLoginInput {
            email: email.to_string(),
            password: current_password.clone(),
        })
        .await
        .map_err(OverleafPasswordChangeError::from_browser)?;

    change_account_overleaf_password_from_login_result_internal(
        document,
        alias,
        current_password,
        new_password,
        login,
        browser,
        now_unix,
        backend,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn change_account_overleaf_password_from_login_result_with_backend<B>(
    document: &mut AccountsDocument,
    alias: &str,
    current_password: SecretText,
    new_password: SecretText,
    login: LoginResult,
    browser: &B,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<OverleafPasswordChangeReport, OverleafPasswordChangeError>
where
    B: BrowserAutomation + Sync + ?Sized,
{
    let (report, _) = change_account_overleaf_password_from_login_result_internal(
        document,
        alias,
        current_password,
        new_password,
        login,
        browser,
        now_unix,
        backend,
    )
    .await?;
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
async fn change_account_overleaf_password_from_login_result_internal<B>(
    document: &mut AccountsDocument,
    alias: &str,
    current_password: SecretText,
    new_password: SecretText,
    login: LoginResult,
    browser: &B,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<(OverleafPasswordChangeReport, AccountSecretWriteJournal), OverleafPasswordChangeError>
where
    B: BrowserAutomation + Sync + ?Sized,
{
    let alias = alias.trim();
    if new_password.is_empty() {
        return Err(OverleafPasswordChangeError::MissingNewPassword);
    }
    if let Err(error) = validate_new_password(new_password.expose()) {
        return Err(OverleafPasswordChangeError::PasswordTooShort {
            minimum: error.minimum,
        });
    }
    if current_password.is_empty() {
        return Err(OverleafPasswordChangeError::MissingCurrentPassword {
            alias: alias.to_string(),
        });
    }
    let record = document.accounts.get(alias).ok_or_else(|| {
        OverleafPasswordChangeError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    let email = non_empty(record.email.as_deref())
        .ok_or_else(|| OverleafPasswordChangeError::MissingEmail {
            alias: alias.to_string(),
        })?
        .to_string();

    if let Some(actual) = login
        .email
        .as_deref()
        .and_then(|value| non_empty(Some(value)))
    {
        if normalize_email(actual) != normalize_email(&email) {
            return Err(OverleafPasswordChangeError::LoginEmailMismatch {
                expected: email.clone(),
                actual: actual.to_string(),
            });
        }
    }

    let cookies = cookies_to_map(&login.cookies);
    let overleaf_session = session_cookie(&cookies)?;
    let cookie_expiry = overleaf_session_expiry(&login.cookies);
    let mut staged_document = document.clone();
    let mut journal = AccountSecretWriteJournal::default();
    if let Err(error) = apply_login_update(
        &mut staged_document,
        alias,
        &login,
        &cookies,
        now_unix,
        backend,
        &mut journal,
    ) {
        return Err(rollback_password_change_error(journal, backend, error));
    }

    let change_result = browser
        .change_password_with_cookie(PasswordChangeInput {
            email: email.clone(),
            overleaf_session: SecretText::new(overleaf_session),
            current_password,
            new_password: new_password.clone(),
        })
        .await;

    match change_result {
        Ok(result) if result.changed => {
            let record = staged_document.accounts.get_mut(alias).ok_or_else(|| {
                OverleafPasswordChangeError::AccountAliasNotFound {
                    alias: alias.to_string(),
                }
            })?;
            if let Err(error) = write_changed_password(
                record,
                alias,
                new_password.into_inner(),
                backend,
                &mut journal,
            ) {
                return Err(rollback_password_change_error(journal, backend, error));
            }
            *document = staged_document;
            Ok((
                OverleafPasswordChangeReport {
                    alias: alias.to_string(),
                    email: email.clone(),
                    status: OverleafPasswordChangeStatus::Changed,
                    password_updated: true,
                    login_updated: true,
                    cookie_expiry,
                    failure_message: None,
                },
                journal,
            ))
        }
        Ok(_) => {
            *document = staged_document;
            Ok((
                OverleafPasswordChangeReport {
                    alias: alias.to_string(),
                    email: email.clone(),
                    status: OverleafPasswordChangeStatus::FailedButLoginUpdated,
                    password_updated: false,
                    login_updated: true,
                    cookie_expiry,
                    failure_message: Some("browser reported password was not changed".to_string()),
                },
                journal,
            ))
        }
        Err(error) => {
            *document = staged_document;
            Ok((
                OverleafPasswordChangeReport {
                    alias: alias.to_string(),
                    email,
                    status: OverleafPasswordChangeStatus::FailedButLoginUpdated,
                    password_updated: false,
                    login_updated: true,
                    cookie_expiry,
                    failure_message: Some(error.to_string()),
                },
                journal,
            ))
        }
    }
}

pub async fn change_account_overleaf_password_in_store_with_backend<B>(
    store: &AccountStore,
    alias: &str,
    new_password: SecretText,
    browser: &B,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<OverleafPasswordChangeReport, OverleafPasswordChangeError>
where
    B: BrowserAutomation + Sync + ?Sized,
{
    let mut document = store.load().map_err(OverleafPasswordChangeError::from_io)?;
    let (report, journal) = change_account_overleaf_password_internal(
        &mut document,
        alias,
        new_password,
        browser,
        now_unix,
        backend,
    )
    .await?;
    if report.login_updated || report.password_updated {
        journal.commit(
            store,
            &document,
            backend,
            OverleafPasswordChangeError::from_io,
        )?;
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
pub async fn change_account_overleaf_password_from_login_result_in_store_with_backend<B>(
    store: &AccountStore,
    alias: &str,
    current_password: SecretText,
    new_password: SecretText,
    login: LoginResult,
    browser: &B,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<OverleafPasswordChangeReport, OverleafPasswordChangeError>
where
    B: BrowserAutomation + Sync + ?Sized,
{
    let mut document = store.load().map_err(OverleafPasswordChangeError::from_io)?;
    let (report, journal) = change_account_overleaf_password_from_login_result_internal(
        &mut document,
        alias,
        current_password,
        new_password,
        login,
        browser,
        now_unix,
        backend,
    )
    .await?;
    if report.login_updated || report.password_updated {
        journal.commit(
            store,
            &document,
            backend,
            OverleafPasswordChangeError::from_io,
        )?;
    }
    Ok(report)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn session_cookie(
    cookies: &BTreeMap<String, String>,
) -> Result<String, OverleafPasswordChangeError> {
    cookies
        .get(overleaf_api::OVERLEAF_SESSION_COOKIE)
        .map(String::as_str)
        .and_then(|value| non_empty(Some(value)))
        .map(ToOwned::to_owned)
        .ok_or(OverleafPasswordChangeError::MissingSessionCookie)
}

fn current_password_for_change(
    record: &AccountRecord,
    alias: &str,
    backend: &dyn SecretBackend,
) -> Result<String, OverleafPasswordChangeError> {
    read_account_password_secret(record, alias, backend).map_err(OverleafPasswordChangeError::from)
}

fn write_changed_password(
    record: &mut AccountRecord,
    alias: &str,
    new_password: String,
    backend: &dyn SecretBackend,
    journal: &mut AccountSecretWriteJournal,
) -> Result<(), OverleafPasswordChangeError> {
    write_account_password_secret_tracked(record, alias, &new_password, backend, journal)
        .map_err(OverleafPasswordChangeError::from)?;
    Ok(())
}

fn apply_login_update(
    document: &mut AccountsDocument,
    alias: &str,
    login: &LoginResult,
    cookies: &BTreeMap<String, String>,
    now_unix: i64,
    backend: &dyn SecretBackend,
    journal: &mut AccountSecretWriteJournal,
) -> Result<(), OverleafPasswordChangeError> {
    let record = document.accounts.get_mut(alias).ok_or_else(|| {
        OverleafPasswordChangeError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    write_account_cookie_secrets_tracked(record, alias, cookies, backend, journal)
        .map_err(OverleafPasswordChangeError::from)?;
    apply_login_metadata(record, login, now_unix);
    Ok(())
}

fn rollback_password_change_error(
    journal: AccountSecretWriteJournal,
    backend: &dyn SecretBackend,
    error: OverleafPasswordChangeError,
) -> OverleafPasswordChangeError {
    match journal.rollback(backend) {
        Ok(()) => error,
        Err(rollback_error) => OverleafPasswordChangeError::from(rollback_error),
    }
}

impl OverleafPasswordChangeError {
    fn from_io(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }

    fn from_browser(error: BrowserAutomationError) -> Self {
        match error {
            BrowserAutomationError::InvalidCredentials => Self::InvalidCredentials,
            BrowserAutomationError::ChallengeRequired { .. } => Self::ChallengeRequired,
            BrowserAutomationError::RobotVerificationBlocked => Self::RobotVerificationBlocked,
            error => Self::Browser {
                message: error.to_string(),
            },
        }
    }
}

impl From<AccountSecretStoreError> for OverleafPasswordChangeError {
    fn from(error: AccountSecretStoreError) -> Self {
        match error {
            AccountSecretStoreError::Missing { alias, category } => {
                if category == "password" {
                    Self::MissingCurrentPassword { alias }
                } else {
                    Self::SecretReadFailed { alias, category }
                }
            }
            AccountSecretStoreError::ReadFailed { alias, category } => {
                Self::SecretReadFailed { alias, category }
            }
            AccountSecretStoreError::WriteFailed { alias, category } => {
                Self::SecretWriteFailed { alias, category }
            }
        }
    }
}
