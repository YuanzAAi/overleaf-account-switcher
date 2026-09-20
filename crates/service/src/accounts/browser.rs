use std::sync::Arc;

use futures_util::{stream, StreamExt};
use overleaf_browser::{
    cookies_to_map, BrowserAutoLoginInput, BrowserAutomation, BrowserAutomationError,
    CredentialsLoginInput, LoginResult, SecretText,
};
use overleaf_storage::{AccountRecord, AccountStore, AccountsDocument, SecretBackend};
use serde::Serialize;
use tokio::sync::Mutex as AsyncMutex;

use crate::account_credentials::{
    credential_report_failure_message,
    refresh_account_cookie_from_login_result_in_store_with_backend, AccountCredentialError,
    AccountSessionMetadataRefresher,
};
use crate::account_secrets::{
    read_account_password_secret_for_recovery, resolve_account_cookies_for_recovery,
    AccountSecretStoreError,
};
use crate::browser_batch::DEFAULT_BROWSER_BATCH_CONCURRENCY;

const OVERLEAF_SESSION_COOKIE_NAME: &str = "overleaf_session2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserAutoLoginReport {
    pub alias: String,
    pub email: Option<String>,
    pub opened: bool,
    pub url: String,
    pub profile_dir: String,
    pub debug_port: u16,
    pub process_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserAutoLoginBatchReport {
    pub items: Vec<BrowserAutoLoginBatchItem>,
    pub opened_count: usize,
    pub failed_count: usize,
    pub cancelled_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserAutoLoginBatchItem {
    pub alias: String,
    pub opened: bool,
    pub cancelled: bool,
    pub report: Option<BrowserAutoLoginReport>,
    pub error: Option<BrowserAutoLoginError>,
}

pub(crate) fn browser_auto_login_batch_item_from_report(
    alias: &str,
    report: BrowserAutoLoginReport,
) -> BrowserAutoLoginBatchItem {
    let opened = report.opened;
    BrowserAutoLoginBatchItem {
        alias: alias.to_string(),
        opened,
        cancelled: false,
        report: Some(report),
        error: (!opened).then(|| BrowserAutoLoginError::Browser {
            message: "browser login result did not confirm window opened".to_string(),
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserAutoLoginError {
    Io {
        message: String,
    },
    AccountAliasNotFound {
        alias: String,
    },
    MissingCookie {
        alias: String,
    },
    InvalidCookie {
        alias: String,
    },
    MissingPassword {
        alias: String,
    },
    MissingEmail {
        alias: String,
    },
    SecretReadFailed {
        alias: String,
        category: &'static str,
    },
    Browser {
        message: String,
    },
}

pub async fn open_account_browser_login_with_backend(
    document: &AccountsDocument,
    alias: &str,
    browser: &(dyn BrowserAutomation + Send + Sync),
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<BrowserAutoLoginReport, BrowserAutoLoginError> {
    let alias = alias.trim();
    let record = document.accounts.get(alias).ok_or_else(|| {
        BrowserAutoLoginError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    match open_account_browser_login_with_saved_cookie(document, alias, browser, backend).await {
        Ok(report) => return Ok(report),
        Err(BrowserAutoLoginError::MissingCookie { .. })
        | Err(BrowserAutoLoginError::InvalidCookie { .. }) => {}
        Err(error) => return Err(error),
    }

    let email = record
        .email
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| BrowserAutoLoginError::MissingEmail {
            alias: alias.to_string(),
        })?;
    let password = match read_account_password_secret_for_recovery(record, alias, backend) {
        Ok(password) => password,
        Err(AccountSecretStoreError::Missing { .. }) => {
            return Err(BrowserAutoLoginError::MissingPassword {
                alias: alias.to_string(),
            });
        }
        Err(error) => return Err(BrowserAutoLoginError::from(error)),
    };
    let login = browser
        .login_with_credentials(CredentialsLoginInput {
            email,
            password: SecretText::new(password),
        })
        .await
        .map_err(|error| BrowserAutoLoginError::Browser {
            message: error.to_string(),
        })?;

    open_account_browser_login_with_login_result(alias, record.email.clone(), &login, browser).await
}

pub(crate) async fn open_account_browser_login_with_store_backend(
    store: &AccountStore,
    alias: &str,
    browser: &(dyn BrowserAutomation + Send + Sync),
    metadata_refresher: &(dyn AccountSessionMetadataRefresher + Send + Sync),
    now_unix: i64,
    backend: &(dyn SecretBackend + Send + Sync),
    recovery_lock: Option<Arc<AsyncMutex<()>>>,
) -> Result<BrowserAutoLoginReport, BrowserAutoLoginError> {
    let document = store.load().map_err(BrowserAutoLoginError::from_io)?;
    let alias = alias.trim();
    let record = document.accounts.get(alias).ok_or_else(|| {
        BrowserAutoLoginError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;

    match open_account_browser_login_with_saved_cookie(&document, alias, browser, backend).await {
        Ok(report) => return Ok(report),
        Err(BrowserAutoLoginError::MissingCookie { .. })
        | Err(BrowserAutoLoginError::InvalidCookie { .. }) => {}
        Err(error) => return Err(error),
    }

    let email = record
        .email
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| BrowserAutoLoginError::MissingEmail {
            alias: alias.to_string(),
        })?;
    let password = match read_account_password_secret_for_recovery(record, alias, backend) {
        Ok(password) => password,
        Err(AccountSecretStoreError::Missing { .. }) => {
            return Err(BrowserAutoLoginError::MissingPassword {
                alias: alias.to_string(),
            });
        }
        Err(error) => return Err(BrowserAutoLoginError::from(error)),
    };
    let login = browser
        .login_with_credentials(CredentialsLoginInput {
            email: email.clone(),
            password: SecretText::new(password.clone()),
        })
        .await
        .map_err(BrowserAutoLoginError::from_credential_browser)?;

    if let Some(lock) = recovery_lock {
        let _guard = lock.lock().await;
        persist_recovered_browser_login(
            store,
            alias,
            password,
            login.clone(),
            metadata_refresher,
            now_unix,
            backend,
        )
        .await?;
    } else {
        persist_recovered_browser_login(
            store,
            alias,
            password,
            login.clone(),
            metadata_refresher,
            now_unix,
            backend,
        )
        .await?;
    }

    open_account_browser_login_with_login_result(alias, Some(email), &login, browser).await
}

async fn persist_recovered_browser_login(
    store: &AccountStore,
    alias: &str,
    password: String,
    login: LoginResult,
    metadata_refresher: &(dyn AccountSessionMetadataRefresher + Send + Sync),
    now_unix: i64,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<(), BrowserAutoLoginError> {
    let report = refresh_account_cookie_from_login_result_in_store_with_backend(
        store,
        alias,
        SecretText::new(password),
        login,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await
    .map_err(|error| BrowserAutoLoginError::from_credential_refresh(alias, error))?;
    if let Some(message) = credential_report_failure_message(&report) {
        return Err(BrowserAutoLoginError::Browser { message });
    }
    Ok(())
}

pub(crate) async fn open_account_browser_login_with_saved_cookie(
    document: &AccountsDocument,
    alias: &str,
    browser: &(dyn BrowserAutomation + Send + Sync),
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<BrowserAutoLoginReport, BrowserAutoLoginError> {
    let alias = alias.trim();
    let record = document.accounts.get(alias).ok_or_else(|| {
        BrowserAutoLoginError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    let session = resolve_session_cookie(record, alias, backend)?;
    open_account_browser_login_with_session(
        alias,
        record.email.clone(),
        session,
        record.cookie_expiry,
        browser,
    )
    .await
}

pub(crate) async fn open_account_browser_login_with_login_result(
    alias: &str,
    email: Option<String>,
    login: &LoginResult,
    browser: &(dyn BrowserAutomation + Send + Sync),
) -> Result<BrowserAutoLoginReport, BrowserAutoLoginError> {
    let cookies = cookies_to_map(&login.cookies);
    let session = cookies
        .get(OVERLEAF_SESSION_COOKIE_NAME)
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| BrowserAutoLoginError::MissingCookie {
            alias: alias.to_string(),
        })?;
    let cookie_expiry =
        crate::account_session::overleaf_session_expiry(&login.cookies).map(|value| value as f64);

    open_account_browser_login_with_session(alias, email, session, cookie_expiry, browser).await
}

pub(crate) async fn open_account_browser_login_with_session(
    alias: &str,
    email: Option<String>,
    session: String,
    cookie_expiry: Option<f64>,
    browser: &(dyn BrowserAutomation + Send + Sync),
) -> Result<BrowserAutoLoginReport, BrowserAutoLoginError> {
    let result = browser
        .open_user_window_with_cookie(BrowserAutoLoginInput {
            overleaf_session: SecretText::new(session),
            cookie_expiry,
        })
        .await
        .map_err(|error| match error {
            BrowserAutomationError::InvalidCredentials => BrowserAutoLoginError::InvalidCookie {
                alias: alias.to_string(),
            },
            error => BrowserAutoLoginError::Browser {
                message: error.to_string(),
            },
        })?;

    Ok(BrowserAutoLoginReport {
        alias: alias.to_string(),
        email,
        opened: true,
        url: result.url,
        profile_dir: result.profile_dir.to_string_lossy().into_owned(),
        debug_port: result.debug_port,
        process_id: result.process_id,
    })
}

fn resolve_session_cookie(
    record: &AccountRecord,
    alias: &str,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<String, BrowserAutoLoginError> {
    let cookies = resolve_account_cookies_for_recovery(record, alias, backend)
        .map_err(BrowserAutoLoginError::from)?;
    cookies
        .get(OVERLEAF_SESSION_COOKIE_NAME)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| BrowserAutoLoginError::MissingCookie {
            alias: alias.to_string(),
        })
}

pub async fn open_account_browser_logins_in_store_with_backend(
    store: &AccountStore,
    aliases: &[String],
    browser: &(dyn BrowserAutomation + Send + Sync),
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<BrowserAutoLoginBatchReport, BrowserAutoLoginError> {
    let document = store.load().map_err(BrowserAutoLoginError::from_io)?;
    let mut indexed_items = stream::iter(aliases.iter().enumerate().map(|(index, alias)| {
        let document = &document;
        async move {
            let item =
                match open_account_browser_login_with_backend(document, alias, browser, backend)
                    .await
                {
                    Ok(report) => browser_auto_login_batch_item_from_report(alias, report),
                    Err(error) => BrowserAutoLoginBatchItem {
                        alias: alias.to_string(),
                        opened: false,
                        cancelled: false,
                        report: None,
                        error: Some(error),
                    },
                };
            (index, item)
        }
    }))
    .buffer_unordered(DEFAULT_BROWSER_BATCH_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;
    indexed_items.sort_by_key(|(index, _)| *index);
    let items = indexed_items
        .into_iter()
        .map(|(_, item)| item)
        .collect::<Vec<_>>();
    let opened_count = items.iter().filter(|item| item.opened).count();

    Ok(BrowserAutoLoginBatchReport {
        failed_count: items.len().saturating_sub(opened_count),
        items,
        opened_count,
        cancelled_count: 0,
    })
}

pub(crate) async fn open_account_browser_logins_in_store_with_recovery_backend(
    store: &AccountStore,
    aliases: &[String],
    browser: &(dyn BrowserAutomation + Send + Sync),
    metadata_refresher: &(dyn AccountSessionMetadataRefresher + Send + Sync),
    now_unix: i64,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<BrowserAutoLoginBatchReport, BrowserAutoLoginError> {
    store.load().map_err(BrowserAutoLoginError::from_io)?;
    let recovery_lock = Arc::new(AsyncMutex::new(()));
    let mut indexed_items = stream::iter(aliases.iter().enumerate().map(|(index, alias)| {
        let recovery_lock = Arc::clone(&recovery_lock);
        async move {
            let item = match open_account_browser_login_with_store_backend(
                store,
                alias,
                browser,
                metadata_refresher,
                now_unix,
                backend,
                Some(recovery_lock),
            )
            .await
            {
                Ok(report) => browser_auto_login_batch_item_from_report(alias, report),
                Err(error) => BrowserAutoLoginBatchItem {
                    alias: alias.to_string(),
                    opened: false,
                    cancelled: false,
                    report: None,
                    error: Some(error),
                },
            };
            (index, item)
        }
    }))
    .buffer_unordered(DEFAULT_BROWSER_BATCH_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;
    indexed_items.sort_by_key(|(index, _)| *index);
    let items = indexed_items
        .into_iter()
        .map(|(_, item)| item)
        .collect::<Vec<_>>();
    let opened_count = items.iter().filter(|item| item.opened).count();

    Ok(BrowserAutoLoginBatchReport {
        failed_count: items.len().saturating_sub(opened_count),
        items,
        opened_count,
        cancelled_count: 0,
    })
}

impl BrowserAutoLoginError {
    fn from_io(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }

    fn from_credential_browser(error: BrowserAutomationError) -> Self {
        let message = match error {
            BrowserAutomationError::InvalidCredentials => {
                "saved password was rejected during Cookie recovery".to_string()
            }
            BrowserAutomationError::ChallengeRequired { .. } => {
                "Cookie recovery requires browser verification".to_string()
            }
            BrowserAutomationError::RobotVerificationBlocked => {
                "Cookie recovery was blocked by robot verification".to_string()
            }
            error => error.to_string(),
        };
        Self::Browser { message }
    }

    fn from_credential_refresh(alias: &str, error: AccountCredentialError) -> Self {
        match error {
            AccountCredentialError::Io { message } => Self::Io { message },
            AccountCredentialError::AccountAliasNotFound { alias } => {
                Self::AccountAliasNotFound { alias }
            }
            AccountCredentialError::MissingEmail { alias } => Self::MissingEmail { alias },
            AccountCredentialError::MissingSessionCookie => Self::MissingCookie {
                alias: alias.to_string(),
            },
            AccountCredentialError::InvalidCredentials => Self::Browser {
                message: "saved password was rejected during Cookie recovery".to_string(),
            },
            AccountCredentialError::ChallengeRequired => Self::Browser {
                message: "Cookie recovery requires browser verification".to_string(),
            },
            AccountCredentialError::RobotVerificationBlocked => Self::Browser {
                message: "Cookie recovery was blocked by robot verification".to_string(),
            },
            AccountCredentialError::SecretWriteFailed { category, .. } => Self::Browser {
                message: format!("failed to persist recovered {category}"),
            },
            AccountCredentialError::AliasConflict { .. }
            | AccountCredentialError::Browser { .. }
            | AccountCredentialError::SessionIdentity { .. } => Self::Browser {
                message: "saved-password Cookie recovery did not complete".to_string(),
            },
        }
    }
}

impl From<AccountSecretStoreError> for BrowserAutoLoginError {
    fn from(error: AccountSecretStoreError) -> Self {
        match error {
            AccountSecretStoreError::Missing { alias, .. } => Self::MissingCookie { alias },
            AccountSecretStoreError::ReadFailed { alias, category }
            | AccountSecretStoreError::WriteFailed { alias, category } => {
                Self::SecretReadFailed { alias, category }
            }
        }
    }
}
