use futures_util::{stream, StreamExt};
use overleaf_api::session::{
    OverleafSessionClient, ReqwestSessionTransport, SessionError, SessionTransport,
};
use overleaf_api::GitTokenPageState;
use overleaf_browser::{
    BrowserAutomation, BrowserAutomationError, GitTokenInput, GitTokenResult, SecretText,
};
use overleaf_storage::{
    save_accounts_document, AccountRecord, AccountStore, AccountsDocument, SecretBackend,
};
use serde::Serialize;

use crate::account_secrets::{
    read_account_cookie_secret, read_account_git_token_secret_for_recovery,
    resolve_account_cookies, write_account_git_token_secret_tracked, AccountSecretStoreError,
    AccountSecretWriteJournal,
};
use crate::browser_batch::DEFAULT_BROWSER_BATCH_CONCURRENCY;

const OVERLEAF_SESSION_COOKIE_NAME: &str = "overleaf_session2";
pub const GIT_TOKEN_REUSE_WINDOW_SECONDS: f64 = 7.0 * 24.0 * 60.0 * 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitTokenRefreshStatus {
    VisibleTokenSaved,
    ExistingMaskedTokenOnly,
    TokenActionAvailable,
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountGitTokenRefreshReport {
    pub alias: String,
    pub email: Option<String>,
    pub status: GitTokenRefreshStatus,
    pub token_present: bool,
    pub token_saved: bool,
    pub git_token_expiry: Option<i64>,
    pub has_existing_masked_token: bool,
    pub can_generate_token: bool,
    pub can_add_another_token: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountGitTokenRefreshBatchReport {
    pub items: Vec<AccountGitTokenRefreshBatchItem>,
    pub refreshed_count: usize,
    pub skipped_count: usize,
    pub failed_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountGitTokenRefreshBatchItem {
    pub alias: String,
    pub refreshed: bool,
    pub skipped: bool,
    pub report: Option<AccountGitTokenRefreshReport>,
    pub error: Option<AccountGitTokenError>,
}

pub fn has_reusable_git_token(
    record: &AccountRecord,
    alias: &str,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> bool {
    if record.git_token_ref.is_none() {
        return false;
    }
    // 页面只展示 olp_ 加四位前缀时，不能当作可复用令牌。
    let has_full_secret = read_account_git_token_secret_for_recovery(record, alias, backend)
        .ok()
        .is_some_and(|token| token.trim().len() > 8);
    has_full_secret
        && record
            .git_token_expiry
            .is_some_and(|expiry| expiry > now_unix as f64 + GIT_TOKEN_REUSE_WINDOW_SECONDS)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountGitTokenError {
    Io {
        message: String,
    },
    Session {
        message: String,
    },
    Browser {
        message: String,
    },
    AccountAliasNotFound {
        alias: String,
    },
    MissingCookies {
        alias: String,
    },
    TokenNotAvailable {
        alias: String,
    },
    SecretReadFailed {
        alias: String,
        category: &'static str,
    },
    SecretWriteFailed {
        alias: String,
        category: &'static str,
    },
}

pub fn apply_git_token_page_state_with_backend(
    document: &mut AccountsDocument,
    alias: &str,
    state: &GitTokenPageState,
    backend: &dyn SecretBackend,
) -> Result<AccountGitTokenRefreshReport, AccountGitTokenError> {
    let (report, _) = apply_git_token_page_state_internal(document, alias, state, backend)?;
    Ok(report)
}

fn apply_git_token_page_state_internal(
    document: &mut AccountsDocument,
    alias: &str,
    state: &GitTokenPageState,
    backend: &dyn SecretBackend,
) -> Result<(AccountGitTokenRefreshReport, AccountSecretWriteJournal), AccountGitTokenError> {
    let alias = alias.trim();
    let record = document.accounts.get_mut(alias).ok_or_else(|| {
        AccountGitTokenError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;

    let mut journal = AccountSecretWriteJournal::default();
    let mut token_saved = false;
    if let Some(token) = state
        .visible_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        if let Err(error) = write_git_token(record, alias, token.to_string(), backend, &mut journal)
        {
            return Err(rollback_git_secret_error(journal, backend, error));
        }
        token_saved = true;
    }

    if let Some(expiry) = state.expiry {
        record.git_token_expiry = Some(expiry as f64);
    }

    let token_present = if token_saved {
        true
    } else {
        git_token_present(record, alias, backend)?
    };

    let status = if token_saved {
        GitTokenRefreshStatus::VisibleTokenSaved
    } else if state.has_existing_masked_token {
        GitTokenRefreshStatus::ExistingMaskedTokenOnly
    } else if state.can_generate_token || state.can_add_another_token {
        GitTokenRefreshStatus::TokenActionAvailable
    } else {
        GitTokenRefreshStatus::NotFound
    };

    Ok((
        AccountGitTokenRefreshReport {
            alias: alias.to_string(),
            email: record.email.clone(),
            status,
            token_present,
            token_saved,
            git_token_expiry: record.git_token_expiry.map(|expiry| expiry as i64),
            has_existing_masked_token: state.has_existing_masked_token,
            can_generate_token: state.can_generate_token,
            can_add_another_token: state.can_add_another_token,
        },
        journal,
    ))
}

pub async fn refresh_account_git_token_metadata_with_backend<T: SessionTransport>(
    document: &mut AccountsDocument,
    alias: &str,
    client: &OverleafSessionClient<T>,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<AccountGitTokenRefreshReport, AccountGitTokenError> {
    let (report, _) =
        refresh_account_git_token_metadata_internal(document, alias, client, now_unix, backend)
            .await?;
    Ok(report)
}

async fn refresh_account_git_token_metadata_internal<T: SessionTransport>(
    document: &mut AccountsDocument,
    alias: &str,
    client: &OverleafSessionClient<T>,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<(AccountGitTokenRefreshReport, AccountSecretWriteJournal), AccountGitTokenError> {
    let alias = alias.trim();
    let existing =
        document
            .accounts
            .get(alias)
            .ok_or_else(|| AccountGitTokenError::AccountAliasNotFound {
                alias: alias.to_string(),
            })?;
    if existing.cookie_refs.is_empty() {
        return Err(AccountGitTokenError::MissingCookies {
            alias: alias.to_string(),
        });
    }

    let state = client
        .fetch_git_token_page_state(now_unix)
        .await
        .map_err(AccountGitTokenError::from_session)?;
    apply_git_token_page_state_internal(document, alias, &state, backend)
}

pub async fn refresh_account_git_token_metadata_in_store_with_backend<T: SessionTransport>(
    store: &AccountStore,
    alias: &str,
    client: &OverleafSessionClient<T>,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<AccountGitTokenRefreshReport, AccountGitTokenError> {
    let mut document = store.load().map_err(AccountGitTokenError::from_io)?;
    let (report, journal) = refresh_account_git_token_metadata_internal(
        &mut document,
        alias,
        client,
        now_unix,
        backend,
    )
    .await?;
    save_git_document_with_rollback(store, &document, vec![journal], backend)?;
    Ok(report)
}

pub async fn refresh_account_git_token_metadata_with_browser_with_backend(
    document: &mut AccountsDocument,
    alias: &str,
    browser: &(dyn BrowserAutomation + Send + Sync),
    backend: &dyn SecretBackend,
) -> Result<AccountGitTokenRefreshReport, AccountGitTokenError> {
    let (report, _) =
        refresh_account_git_token_metadata_with_browser_internal(document, alias, browser, backend)
            .await?;
    Ok(report)
}

async fn refresh_account_git_token_metadata_with_browser_internal(
    document: &mut AccountsDocument,
    alias: &str,
    browser: &(dyn BrowserAutomation + Send + Sync),
    backend: &dyn SecretBackend,
) -> Result<(AccountGitTokenRefreshReport, AccountSecretWriteJournal), AccountGitTokenError> {
    let alias = alias.trim();
    let record =
        document
            .accounts
            .get(alias)
            .ok_or_else(|| AccountGitTokenError::AccountAliasNotFound {
                alias: alias.to_string(),
            })?;
    let session = session_cookie_for_git_token(record, alias, backend)?;
    let browser_state = browser
        .read_git_token_state_with_cookie(GitTokenInput {
            overleaf_session: SecretText::new(session),
        })
        .await
        .map_err(|error| AccountGitTokenError::from_browser(alias, error))?;
    let state = GitTokenPageState {
        visible_token: browser_state.visible_token.map(SecretText::into_inner),
        expiry: browser_state.expires_at,
        has_existing_masked_token: browser_state.has_existing_masked_token,
        can_generate_token: browser_state.can_generate_token,
        can_add_another_token: browser_state.can_add_another_token,
    };
    apply_git_token_page_state_internal(document, alias, &state, backend)
}

pub async fn refresh_account_git_token_metadata_with_browser_in_store_with_backend(
    store: &AccountStore,
    alias: &str,
    browser: &(dyn BrowserAutomation + Send + Sync),
    backend: &dyn SecretBackend,
) -> Result<AccountGitTokenRefreshReport, AccountGitTokenError> {
    let mut document = store.load().map_err(AccountGitTokenError::from_io)?;
    let (report, journal) = refresh_account_git_token_metadata_with_browser_internal(
        &mut document,
        alias,
        browser,
        backend,
    )
    .await?;
    save_git_document_with_rollback(store, &document, vec![journal], backend)?;
    Ok(report)
}

pub async fn refresh_account_git_token_metadata_with_browser_batch_in_store_with_backend(
    store: &AccountStore,
    aliases: &[String],
    browser: &(dyn BrowserAutomation + Send + Sync),
    backend: &dyn SecretBackend,
) -> Result<AccountGitTokenRefreshBatchReport, AccountGitTokenError> {
    let base_document = store.load().map_err(AccountGitTokenError::from_io)?;
    let jobs = aliases.iter().cloned().enumerate().map(|(index, alias)| {
        let mut isolated_document = base_document.clone();
        async move {
            let result = refresh_account_git_token_metadata_with_browser_internal(
                &mut isolated_document,
                &alias,
                browser,
                backend,
            )
            .await;
            let updated_record = result
                .as_ref()
                .ok()
                .and_then(|_| isolated_document.accounts.get(&alias).cloned());
            (index, alias, result, updated_record)
        }
    });
    let mut results = stream::iter(jobs)
        .buffer_unordered(DEFAULT_BROWSER_BATCH_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    results.sort_by_key(|(index, _, _, _)| *index);

    let mut document = base_document;
    let mut items = Vec::with_capacity(results.len());
    let mut refreshed_count = 0;
    let mut journals = Vec::new();
    for (_, alias, result, updated_record) in results {
        match result {
            Ok((report, journal)) => {
                if let Some(record) = updated_record {
                    document.accounts.insert(alias.clone(), record);
                }
                journals.push(journal);
                refreshed_count += 1;
                items.push(AccountGitTokenRefreshBatchItem {
                    alias,
                    refreshed: true,
                    skipped: false,
                    report: Some(report),
                    error: None,
                });
            }
            Err(error) => items.push(AccountGitTokenRefreshBatchItem {
                alias,
                refreshed: false,
                skipped: false,
                report: None,
                error: Some(error),
            }),
        }
    }

    if refreshed_count > 0 {
        save_git_document_with_rollback(store, &document, journals, backend)?;
    }

    Ok(AccountGitTokenRefreshBatchReport {
        items,
        refreshed_count,
        skipped_count: 0,
        failed_count: aliases.len().saturating_sub(refreshed_count),
    })
}

pub async fn generate_account_git_token_with_browser_with_backend(
    document: &mut AccountsDocument,
    alias: &str,
    browser: &(dyn BrowserAutomation + Send + Sync),
    backend: &dyn SecretBackend,
) -> Result<AccountGitTokenRefreshReport, AccountGitTokenError> {
    let (report, _) =
        generate_account_git_token_with_browser_internal(document, alias, browser, backend).await?;
    Ok(report)
}

async fn generate_account_git_token_with_browser_internal(
    document: &mut AccountsDocument,
    alias: &str,
    browser: &(dyn BrowserAutomation + Send + Sync),
    backend: &dyn SecretBackend,
) -> Result<(AccountGitTokenRefreshReport, AccountSecretWriteJournal), AccountGitTokenError> {
    let alias = alias.trim();
    let record =
        document
            .accounts
            .get(alias)
            .ok_or_else(|| AccountGitTokenError::AccountAliasNotFound {
                alias: alias.to_string(),
            })?;
    let session = session_cookie_for_git_token(record, alias, backend)?;

    let result = browser
        .extract_git_token_with_cookie(GitTokenInput {
            overleaf_session: SecretText::new(session),
        })
        .await
        .map_err(|error| AccountGitTokenError::from_browser(alias, error))?;

    apply_browser_git_token_result_internal(document, alias, result, backend)
}

fn session_cookie_for_git_token(
    record: &AccountRecord,
    alias: &str,
    backend: &dyn SecretBackend,
) -> Result<String, AccountGitTokenError> {
    read_account_cookie_secret(record, alias, OVERLEAF_SESSION_COOKIE_NAME, backend)
        .map_err(AccountGitTokenError::from)
}

pub async fn generate_account_git_token_with_browser_in_store_with_backend(
    store: &AccountStore,
    aliases: &[String],
    browser: &(dyn BrowserAutomation + Send + Sync),
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<AccountGitTokenRefreshBatchReport, AccountGitTokenError> {
    let base_document = store.load().map_err(AccountGitTokenError::from_io)?;
    let jobs = aliases.iter().cloned().enumerate().map(|(index, alias)| {
        let mut isolated_document = base_document.clone();
        let skip_existing = base_document
            .accounts
            .get(&alias)
            .is_some_and(|record| has_reusable_git_token(record, &alias, now_unix, backend));
        async move {
            if skip_existing {
                return (index, alias, None, None, true);
            }
            let result = generate_account_git_token_with_browser_internal(
                &mut isolated_document,
                &alias,
                browser,
                backend,
            )
            .await;
            let updated_record = result
                .as_ref()
                .ok()
                .and_then(|_| isolated_document.accounts.get(&alias).cloned());
            (index, alias, Some(result), updated_record, false)
        }
    });
    let mut results = stream::iter(jobs)
        .buffer_unordered(DEFAULT_BROWSER_BATCH_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    results.sort_by_key(|(index, _, _, _, _)| *index);

    let mut document = base_document;
    let mut items = Vec::with_capacity(results.len());
    let mut refreshed_count = 0;
    let mut skipped_count = 0;
    let mut journals = Vec::new();

    for (_, alias, result, updated_record, skipped) in results {
        if skipped {
            skipped_count += 1;
            items.push(AccountGitTokenRefreshBatchItem {
                alias,
                refreshed: false,
                skipped: true,
                report: None,
                error: None,
            });
            continue;
        }
        let Some(result) = result else {
            items.push(AccountGitTokenRefreshBatchItem {
                alias: alias.clone(),
                refreshed: false,
                skipped: false,
                report: None,
                error: Some(AccountGitTokenError::Browser {
                    message: "Git token generation returned no result".to_string(),
                }),
            });
            continue;
        };
        match result {
            Ok((report, journal)) if report.token_present => {
                if let Some(record) = updated_record {
                    document.accounts.insert(alias.clone(), record);
                }
                journals.push(journal);
                refreshed_count += 1;
                items.push(AccountGitTokenRefreshBatchItem {
                    alias,
                    refreshed: true,
                    skipped: false,
                    report: Some(report),
                    error: None,
                });
            }
            Ok((_, journal)) => {
                let error = match journal.rollback(backend) {
                    Ok(()) => AccountGitTokenError::TokenNotAvailable {
                        alias: alias.clone(),
                    },
                    Err(rollback_error) => AccountGitTokenError::from(rollback_error),
                };
                items.push(AccountGitTokenRefreshBatchItem {
                    alias,
                    refreshed: false,
                    skipped: false,
                    report: None,
                    error: Some(error),
                });
            }
            Err(error) => items.push(AccountGitTokenRefreshBatchItem {
                alias,
                refreshed: false,
                skipped: false,
                report: None,
                error: Some(error),
            }),
        }
    }

    if refreshed_count > 0 {
        save_git_document_with_rollback(store, &document, journals, backend)?;
    }

    Ok(AccountGitTokenRefreshBatchReport {
        failed_count: items
            .iter()
            .filter(|item| !item.refreshed && !item.skipped)
            .count(),
        items,
        refreshed_count,
        skipped_count,
    })
}

pub async fn refresh_saved_git_token_metadata_in_store_with_backend(
    store: &AccountStore,
    aliases: &[String],
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<AccountGitTokenRefreshBatchReport, AccountGitTokenError> {
    let mut document = store.load().map_err(AccountGitTokenError::from_io)?;
    let mut items = Vec::with_capacity(aliases.len());
    let mut refreshed_count = 0;
    let mut journals = Vec::new();

    for alias in aliases {
        let Some(record) = document.accounts.get(alias) else {
            items.push(AccountGitTokenRefreshBatchItem {
                alias: alias.to_string(),
                refreshed: false,
                skipped: false,
                report: None,
                error: Some(AccountGitTokenError::AccountAliasNotFound {
                    alias: alias.to_string(),
                }),
            });
            continue;
        };
        let cookies = match cookies_for_saved_git_token_refresh(record, alias, backend) {
            Ok(cookies) => cookies,
            Err(error) => {
                items.push(AccountGitTokenRefreshBatchItem {
                    alias: alias.to_string(),
                    refreshed: false,
                    skipped: false,
                    report: None,
                    error: Some(error),
                });
                continue;
            }
        };

        let client = OverleafSessionClient::new(ReqwestSessionTransport::new(cookies));
        match refresh_account_git_token_metadata_internal(
            &mut document,
            alias,
            &client,
            now_unix,
            backend,
        )
        .await
        {
            Ok((report, journal)) => {
                journals.push(journal);
                refreshed_count += 1;
                items.push(AccountGitTokenRefreshBatchItem {
                    alias: alias.to_string(),
                    refreshed: true,
                    skipped: false,
                    report: Some(report),
                    error: None,
                });
            }
            Err(error) => items.push(AccountGitTokenRefreshBatchItem {
                alias: alias.to_string(),
                refreshed: false,
                skipped: false,
                report: None,
                error: Some(error),
            }),
        }
    }

    if refreshed_count > 0 {
        save_git_document_with_rollback(store, &document, journals, backend)?;
    }

    Ok(AccountGitTokenRefreshBatchReport {
        failed_count: items
            .iter()
            .filter(|item| !item.refreshed && !item.skipped)
            .count(),
        items,
        refreshed_count,
        skipped_count: 0,
    })
}

fn cookies_for_saved_git_token_refresh(
    record: &AccountRecord,
    alias: &str,
    backend: &dyn SecretBackend,
) -> Result<std::collections::BTreeMap<String, String>, AccountGitTokenError> {
    resolve_account_cookies(record, alias, backend).map_err(AccountGitTokenError::from)
}

fn apply_browser_git_token_result_internal(
    document: &mut AccountsDocument,
    alias: &str,
    result: GitTokenResult,
    backend: &dyn SecretBackend,
) -> Result<(AccountGitTokenRefreshReport, AccountSecretWriteJournal), AccountGitTokenError> {
    let record = document.accounts.get_mut(alias).ok_or_else(|| {
        AccountGitTokenError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;

    let mut journal = AccountSecretWriteJournal::default();
    let mut token_saved = false;
    if let Some(token) = result
        .token
        .map(|token| token.into_inner())
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
    {
        if let Err(error) = write_git_token(record, alias, token, backend, &mut journal) {
            return Err(rollback_git_secret_error(journal, backend, error));
        }
        token_saved = true;
    }
    if let Some(expiry) = result.expires_at {
        record.git_token_expiry = Some(expiry as f64);
    }

    let token_present = if token_saved {
        true
    } else {
        git_token_present(record, alias, backend)?
    };

    Ok((
        AccountGitTokenRefreshReport {
            alias: alias.to_string(),
            email: record.email.clone(),
            status: if token_saved {
                GitTokenRefreshStatus::VisibleTokenSaved
            } else {
                GitTokenRefreshStatus::NotFound
            },
            token_present,
            token_saved,
            git_token_expiry: record.git_token_expiry.map(|expiry| expiry as i64),
            has_existing_masked_token: false,
            can_generate_token: false,
            can_add_another_token: false,
        },
        journal,
    ))
}

fn write_git_token(
    record: &mut AccountRecord,
    alias: &str,
    token: String,
    backend: &dyn SecretBackend,
    journal: &mut AccountSecretWriteJournal,
) -> Result<(), AccountGitTokenError> {
    write_account_git_token_secret_tracked(record, alias, &token, backend, journal)?;
    Ok(())
}

fn rollback_git_secret_error(
    journal: AccountSecretWriteJournal,
    backend: &dyn SecretBackend,
    error: AccountGitTokenError,
) -> AccountGitTokenError {
    match journal.rollback(backend) {
        Ok(()) => error,
        Err(rollback_error) => AccountGitTokenError::from(rollback_error),
    }
}

fn save_git_document_with_rollback(
    store: &AccountStore,
    document: &AccountsDocument,
    journals: Vec<AccountSecretWriteJournal>,
    backend: &dyn SecretBackend,
) -> Result<(), AccountGitTokenError> {
    match save_accounts_document(store.path(), document) {
        Ok(()) => Ok(()),
        Err(error) => match rollback_git_journals(journals, backend) {
            Ok(()) => Err(AccountGitTokenError::from_io(error)),
            Err(rollback_error) => Err(rollback_error),
        },
    }
}

fn rollback_git_journals(
    journals: Vec<AccountSecretWriteJournal>,
    backend: &dyn SecretBackend,
) -> Result<(), AccountGitTokenError> {
    let mut first_error = None;
    for journal in journals.into_iter().rev() {
        if let Err(error) = journal.rollback(backend) {
            if first_error.is_none() {
                first_error = Some(AccountGitTokenError::from(error));
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn git_token_present(
    record: &AccountRecord,
    alias: &str,
    backend: &dyn SecretBackend,
) -> Result<bool, AccountGitTokenError> {
    match read_account_git_token_secret_for_recovery(record, alias, backend) {
        Ok(token) => Ok(!token.trim().is_empty()),
        Err(AccountSecretStoreError::Missing { .. }) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

impl AccountGitTokenError {
    fn from_browser(alias: &str, error: BrowserAutomationError) -> Self {
        match error {
            BrowserAutomationError::InvalidCredentials => Self::MissingCookies {
                alias: alias.to_string(),
            },
            error => Self::Browser {
                message: error.to_string(),
            },
        }
    }

    fn from_io(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }

    fn from_session(error: SessionError) -> Self {
        Self::Session {
            message: error.to_string(),
        }
    }
}

impl From<AccountSecretStoreError> for AccountGitTokenError {
    fn from(error: AccountSecretStoreError) -> Self {
        match error {
            AccountSecretStoreError::Missing { alias, .. } => Self::MissingCookies { alias },
            AccountSecretStoreError::ReadFailed { alias, category } => {
                Self::SecretReadFailed { alias, category }
            }
            AccountSecretStoreError::WriteFailed { alias, category } => {
                Self::SecretWriteFailed { alias, category }
            }
        }
    }
}
