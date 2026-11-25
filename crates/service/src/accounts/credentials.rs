use std::collections::BTreeMap;

use overleaf_api::session::{OverleafSessionClient, ReqwestSessionTransport, SessionTransport};
use overleaf_browser::{
    cookies_to_map, BrowserAutomation, BrowserAutomationError, CredentialsLoginInput, LoginResult,
    SecretText,
};
use overleaf_core::make_alias;
use overleaf_storage::{AccountRecord, AccountStore, AccountsDocument, SecretBackend};
use serde::Serialize;

use crate::account_secrets::{
    write_account_cookie_secrets_tracked, write_account_password_secret_tracked,
    AccountSecretStoreError, AccountSecretWriteJournal,
};
use crate::account_session::{
    apply_login_metadata, overleaf_session_expiry,
    refresh_account_session_metadata_with_explicit_cookies, AccountSessionError,
    AccountSessionRefreshReport, SubscriptionRefreshStatus,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialLoginStatus {
    Imported,
    UpdatedExisting,
    SkippedDuplicateEmail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMetadataStatus {
    Refreshed,
    FailedButAccountSaved,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CredentialLoginReport {
    pub alias: String,
    pub email: String,
    pub status: CredentialLoginStatus,
    pub existing_alias: Option<String>,
    pub cookie_expiry: Option<i64>,
    pub metadata_status: CredentialMetadataStatus,
    pub metadata: Option<AccountSessionRefreshReport>,
}

pub(crate) fn credential_report_failure_message(report: &CredentialLoginReport) -> Option<String> {
    if report.status == CredentialLoginStatus::SkippedDuplicateEmail {
        return None;
    }

    if report.metadata_status == CredentialMetadataStatus::Refreshed
        && report.metadata.as_ref().is_some_and(|metadata| {
            metadata.subscription_refresh_status
                != SubscriptionRefreshStatus::FailedButAccountUpdated
        })
    {
        return None;
    }

    Some("账号凭据已保存，但 Cookie/订阅/使用状态未完整刷新，请稍后重试".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountCredentialError {
    Io {
        message: String,
    },
    AccountAliasNotFound {
        alias: String,
    },
    MissingEmail {
        alias: String,
    },
    AliasConflict {
        alias: String,
    },
    MissingSessionCookie,
    InvalidCredentials,
    ChallengeRequired,
    RobotVerificationBlocked,
    Browser {
        message: String,
    },
    SessionIdentity {
        message: String,
    },
    SecretWriteFailed {
        alias: String,
        category: &'static str,
    },
}

#[async_trait::async_trait]
pub trait AccountSessionMetadataRefresher {
    async fn refresh_with_cookies(
        &self,
        document: &mut AccountsDocument,
        alias: &str,
        cookies: &BTreeMap<String, String>,
        now_unix: i64,
    ) -> Result<AccountSessionRefreshReport, AccountSessionError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReqwestAccountSessionMetadataRefresher;

#[async_trait::async_trait]
impl AccountSessionMetadataRefresher for ReqwestAccountSessionMetadataRefresher {
    async fn refresh_with_cookies(
        &self,
        document: &mut AccountsDocument,
        alias: &str,
        cookies: &BTreeMap<String, String>,
        now_unix: i64,
    ) -> Result<AccountSessionRefreshReport, AccountSessionError> {
        let client = OverleafSessionClient::new(ReqwestSessionTransport::new(cookies.clone()));
        refresh_account_session_metadata_with_explicit_cookies(document, alias, &client, now_unix)
            .await
    }
}

#[async_trait::async_trait]
impl<T> AccountSessionMetadataRefresher for OverleafSessionClient<T>
where
    T: SessionTransport + Send + Sync,
{
    async fn refresh_with_cookies(
        &self,
        document: &mut AccountsDocument,
        alias: &str,
        _cookies: &BTreeMap<String, String>,
        now_unix: i64,
    ) -> Result<AccountSessionRefreshReport, AccountSessionError> {
        refresh_account_session_metadata_with_explicit_cookies(document, alias, self, now_unix)
            .await
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Preserve the public credential API; internal calls use CredentialsLoginInput"
)]
pub async fn add_account_with_credentials_with_backend<B, R>(
    document: &mut AccountsDocument,
    alias_hint: Option<&str>,
    email: &str,
    password: SecretText,
    browser: &B,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<CredentialLoginReport, AccountCredentialError>
where
    B: BrowserAutomation + Sync + ?Sized,
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let (report, _) = add_account_with_credentials_internal(
        document,
        alias_hint,
        CredentialsLoginInput {
            email: email.trim().to_string(),
            password,
        },
        browser,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await?;
    Ok(report)
}

async fn add_account_with_credentials_internal<B, R>(
    document: &mut AccountsDocument,
    alias_hint: Option<&str>,
    input: CredentialsLoginInput,
    browser: &B,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<(CredentialLoginReport, AccountSecretWriteJournal), AccountCredentialError>
where
    B: BrowserAutomation + Sync + ?Sized,
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let alias = make_alias(alias_hint, &input.email);
    if let Some(report) = validate_account_import(document, &alias, &input.email)? {
        return Ok((report, AccountSecretWriteJournal::default()));
    }

    let login = browser
        .login_with_credentials(input.clone())
        .await
        .map_err(AccountCredentialError::from_browser)?;

    finish_account_login(
        document,
        &alias,
        input,
        login,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await
}

fn validate_account_import(
    document: &AccountsDocument,
    alias: &str,
    email: &str,
) -> Result<Option<CredentialLoginReport>, AccountCredentialError> {
    if let Some(existing_alias) = document.duplicate_alias_by_email(email) {
        return Ok(Some(CredentialLoginReport {
            alias: alias.to_string(),
            email: email.to_string(),
            status: CredentialLoginStatus::SkippedDuplicateEmail,
            existing_alias: Some(existing_alias.to_string()),
            cookie_expiry: None,
            metadata_status: CredentialMetadataStatus::Skipped,
            metadata: None,
        }));
    }
    if document.accounts.contains_key(alias) {
        return Err(AccountCredentialError::AliasConflict {
            alias: alias.to_string(),
        });
    }
    Ok(None)
}

// Callers validate the target before login: an import is new, a refresh already exists.
async fn finish_account_login<R>(
    document: &mut AccountsDocument,
    alias: &str,
    input: CredentialsLoginInput,
    login: LoginResult,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<(CredentialLoginReport, AccountSecretWriteJournal), AccountCredentialError>
where
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let (mut staged_document, report) = prepare_account_login(
        document,
        alias,
        &input,
        &login,
        metadata_refresher,
        now_unix,
    )
    .await?;
    let record = staged_document
        .accounts
        .get_mut(alias)
        .expect("prepared account");
    let journal = persist_login_secrets(
        record,
        alias,
        input.password,
        &cookies_to_map(&login.cookies),
        backend,
    )?;
    *document = staged_document;
    Ok((report, journal))
}

async fn prepare_account_login<R>(
    document: &AccountsDocument,
    alias: &str,
    input: &CredentialsLoginInput,
    login: &LoginResult,
    metadata_refresher: &R,
    now_unix: i64,
) -> Result<(AccountsDocument, CredentialLoginReport), AccountCredentialError>
where
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let cookies = cookies_to_map(&login.cookies);
    let cookie_expiry = overleaf_session_expiry(&login.cookies);
    ensure_session_cookie(&cookies)?;
    if login
        .email
        .as_deref()
        .is_some_and(|email| !email.trim().eq_ignore_ascii_case(input.email.trim()))
    {
        return Err(AccountCredentialError::SessionIdentity {
            message: "登录会话与目标账号邮箱不一致".to_string(),
        });
    }

    let mut staged_document = document.clone();
    let status = match staged_document.accounts.entry(alias.to_string()) {
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            apply_login_metadata(entry.get_mut(), login, now_unix);
            CredentialLoginStatus::UpdatedExisting
        }
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(account_record_from_login(&input.email, login, now_unix));
            CredentialLoginStatus::Imported
        }
    };
    let metadata_result = metadata_refresher
        .refresh_with_cookies(&mut staged_document, alias, &cookies, now_unix)
        .await;
    if let Err(error) = &metadata_result {
        if is_identity_error(error) {
            return Err(AccountCredentialError::from_session_identity(error.clone()));
        }
    }
    let metadata = metadata_result.ok();
    Ok((
        staged_document,
        CredentialLoginReport {
            alias: alias.to_string(),
            email: input.email.clone(),
            status,
            existing_alias: None,
            cookie_expiry,
            metadata_status: metadata.as_ref().map_or(
                CredentialMetadataStatus::FailedButAccountSaved,
                credential_metadata_status,
            ),
            metadata,
        },
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn save_account_login_with_commit_lock<R>(
    store: &AccountStore,
    alias: &str,
    input: CredentialsLoginInput,
    login: LoginResult,
    importing: bool,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
    commit_lock: &std::sync::Mutex<()>,
    expected_session: Option<&crate::account_session::AccountSessionSnapshot>,
) -> Result<CredentialLoginReport, AccountCredentialError>
where
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let (snapshot, original_session) = {
        let _guard = commit_lock.lock().map_err(|_| AccountCredentialError::Io {
            message: "account commit lock poisoned".to_string(),
        })?;
        let snapshot = store.load().map_err(AccountCredentialError::from_io)?;
        if expected_session.is_some_and(|expected| {
            !snapshot
                .accounts
                .get(alias)
                .is_some_and(|record| expected.matches(record, alias, backend))
        }) {
            return Err(AccountCredentialError::SessionIdentity {
                message: "目标账号在登录期间已变更，请重试".to_string(),
            });
        }
        let original = snapshot.accounts.get(alias).map(|record| {
            crate::account_session::AccountSessionSnapshot::capture(record, alias, backend)
        });
        (snapshot, original)
    };
    if importing {
        if let Some(report) = validate_account_import(&snapshot, alias, &input.email)? {
            return Ok(report);
        }
    } else {
        let original = snapshot.accounts.get(alias).ok_or_else(|| {
            AccountCredentialError::AccountAliasNotFound {
                alias: alias.to_string(),
            }
        })?;
        if !original
            .email
            .as_deref()
            .is_some_and(|email| email.trim().eq_ignore_ascii_case(input.email.trim()))
        {
            return Err(AccountCredentialError::SessionIdentity {
                message: "目标账号邮箱已变更，请重新登录".to_string(),
            });
        }
    }
    let (mut prepared, report) = prepare_account_login(
        &snapshot,
        alias,
        &input,
        &login,
        metadata_refresher,
        now_unix,
    )
    .await?;

    // 网络请求结束后才持锁；重新读取全量数据，只替换未被其他操作修改的目标账号。
    let _guard = commit_lock.lock().map_err(|_| AccountCredentialError::Io {
        message: "account commit lock poisoned".to_string(),
    })?;
    let mut document = store.load().map_err(AccountCredentialError::from_io)?;
    if importing {
        if let Some(report) = validate_account_import(&document, alias, &input.email)? {
            return Ok(report);
        }
    }
    if !match (document.accounts.get(alias), original_session.as_ref()) {
        (Some(current), Some(original)) => original.matches(current, alias, backend),
        (None, None) => true,
        _ => false,
    } {
        return Err(AccountCredentialError::SessionIdentity {
            message: "目标账号在登录期间已变更，请重试".to_string(),
        });
    }
    let prepared_record = prepared.accounts.remove(alias).expect("prepared account");
    let mut record = if let Some(current) = document.accounts.get(alias) {
        let mut record = current.clone();
        if current
            .subscription_checked_at
            .is_none_or(|checked| checked <= now_unix as f64)
        {
            crate::account_session::merge_account_session_metadata(&mut record, &prepared_record);
            record.trial_days = prepared_record.trial_days;
            record.trial_started_at = prepared_record.trial_started_at;
        }
        record.email = prepared_record.email;
        record.user_id = prepared_record.user_id.or(record.user_id);
        record.cookie_expiry = prepared_record.cookie_expiry;
        record.cookie_updated_at = prepared_record.cookie_updated_at;
        if prepared_record.last_login_at > record.last_login_at {
            record.last_login_at = prepared_record.last_login_at;
        }
        record
    } else {
        prepared_record
    };
    let journal = persist_login_secrets(
        &mut record,
        alias,
        input.password,
        &cookies_to_map(&login.cookies),
        backend,
    )?;
    document.accounts.insert(alias.to_string(), record);
    journal.commit(store, &document, backend, AccountCredentialError::from_io)?;
    Ok(report)
}

#[expect(
    clippy::too_many_arguments,
    reason = "Preserve the public credential API; internal calls use CredentialsLoginInput"
)]
pub async fn add_account_from_login_result_with_backend<R>(
    document: &mut AccountsDocument,
    alias_hint: Option<&str>,
    email: &str,
    password: SecretText,
    login: LoginResult,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<CredentialLoginReport, AccountCredentialError>
where
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let (report, _) = add_account_from_login_result_internal(
        document,
        alias_hint,
        CredentialsLoginInput {
            email: email.trim().to_string(),
            password,
        },
        login,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await?;
    Ok(report)
}

async fn add_account_from_login_result_internal<R>(
    document: &mut AccountsDocument,
    alias_hint: Option<&str>,
    input: CredentialsLoginInput,
    login: LoginResult,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<(CredentialLoginReport, AccountSecretWriteJournal), AccountCredentialError>
where
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let alias = make_alias(alias_hint, &input.email);
    if let Some(report) = validate_account_import(document, &alias, &input.email)? {
        return Ok((report, AccountSecretWriteJournal::default()));
    }

    finish_account_login(
        document,
        &alias,
        input,
        login,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await
}

pub async fn refresh_account_cookie_with_credentials_with_backend<B, R>(
    document: &mut AccountsDocument,
    alias: &str,
    password: SecretText,
    browser: &B,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<CredentialLoginReport, AccountCredentialError>
where
    B: BrowserAutomation + Sync + ?Sized,
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let (report, _) = refresh_account_cookie_with_credentials_internal(
        document,
        alias,
        password,
        browser,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await?;
    Ok(report)
}

async fn refresh_account_cookie_with_credentials_internal<B, R>(
    document: &mut AccountsDocument,
    alias: &str,
    password: SecretText,
    browser: &B,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<(CredentialLoginReport, AccountSecretWriteJournal), AccountCredentialError>
where
    B: BrowserAutomation + Sync + ?Sized,
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let alias = alias.trim();
    let email = document
        .accounts
        .get(alias)
        .ok_or_else(|| AccountCredentialError::AccountAliasNotFound {
            alias: alias.to_string(),
        })?
        .email
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AccountCredentialError::MissingEmail {
            alias: alias.to_string(),
        })?;

    let login = browser
        .login_with_credentials(CredentialsLoginInput {
            email: email.clone(),
            password: password.clone(),
        })
        .await
        .map_err(AccountCredentialError::from_browser)?;

    finish_account_login(
        document,
        alias,
        CredentialsLoginInput { email, password },
        login,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await
}

pub async fn refresh_account_cookie_from_login_result_with_backend<R>(
    document: &mut AccountsDocument,
    alias: &str,
    password: SecretText,
    login: LoginResult,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<CredentialLoginReport, AccountCredentialError>
where
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let (report, _) = refresh_account_cookie_from_login_result_internal(
        document,
        alias,
        password,
        login,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await?;
    Ok(report)
}

async fn refresh_account_cookie_from_login_result_internal<R>(
    document: &mut AccountsDocument,
    alias: &str,
    password: SecretText,
    login: LoginResult,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<(CredentialLoginReport, AccountSecretWriteJournal), AccountCredentialError>
where
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let alias = alias.trim();
    let email = document
        .accounts
        .get(alias)
        .ok_or_else(|| AccountCredentialError::AccountAliasNotFound {
            alias: alias.to_string(),
        })?
        .email
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AccountCredentialError::MissingEmail {
            alias: alias.to_string(),
        })?;
    finish_account_login(
        document,
        alias,
        CredentialsLoginInput { email, password },
        login,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "Preserve the public credential API; internal calls use CredentialsLoginInput"
)]
pub async fn add_account_with_credentials_in_store_with_backend<B, R>(
    store: &AccountStore,
    alias_hint: Option<&str>,
    email: &str,
    password: SecretText,
    browser: &B,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<CredentialLoginReport, AccountCredentialError>
where
    B: BrowserAutomation + Sync + ?Sized,
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let mut document = store.load().map_err(AccountCredentialError::from_io)?;
    let (report, journal) = add_account_with_credentials_internal(
        &mut document,
        alias_hint,
        CredentialsLoginInput {
            email: email.trim().to_string(),
            password,
        },
        browser,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await?;

    if report.status != CredentialLoginStatus::SkippedDuplicateEmail {
        journal.commit(store, &document, backend, AccountCredentialError::from_io)?;
    }

    Ok(report)
}

#[expect(
    clippy::too_many_arguments,
    reason = "Preserve the public credential API; internal calls use CredentialsLoginInput"
)]
pub async fn add_account_from_login_result_in_store_with_backend<R>(
    store: &AccountStore,
    alias_hint: Option<&str>,
    email: &str,
    password: SecretText,
    login: LoginResult,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<CredentialLoginReport, AccountCredentialError>
where
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let mut document = store.load().map_err(AccountCredentialError::from_io)?;
    let (report, journal) = add_account_from_login_result_internal(
        &mut document,
        alias_hint,
        CredentialsLoginInput {
            email: email.trim().to_string(),
            password,
        },
        login,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await?;

    if report.status != CredentialLoginStatus::SkippedDuplicateEmail {
        journal.commit(store, &document, backend, AccountCredentialError::from_io)?;
    }

    Ok(report)
}

pub async fn refresh_account_cookie_with_credentials_in_store_with_backend<B, R>(
    store: &AccountStore,
    alias: &str,
    password: SecretText,
    browser: &B,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<CredentialLoginReport, AccountCredentialError>
where
    B: BrowserAutomation + Sync + ?Sized,
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let mut document = store.load().map_err(AccountCredentialError::from_io)?;
    let (report, journal) = refresh_account_cookie_with_credentials_internal(
        &mut document,
        alias,
        password,
        browser,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await?;
    journal.commit(store, &document, backend, AccountCredentialError::from_io)?;
    Ok(report)
}

pub async fn refresh_account_cookie_from_login_result_in_store_with_backend<R>(
    store: &AccountStore,
    alias: &str,
    password: SecretText,
    login: LoginResult,
    metadata_refresher: &R,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<CredentialLoginReport, AccountCredentialError>
where
    R: AccountSessionMetadataRefresher + Sync + ?Sized,
{
    let mut document = store.load().map_err(AccountCredentialError::from_io)?;
    let (report, journal) = refresh_account_cookie_from_login_result_internal(
        &mut document,
        alias,
        password,
        login,
        metadata_refresher,
        now_unix,
        backend,
    )
    .await?;
    journal.commit(store, &document, backend, AccountCredentialError::from_io)?;
    Ok(report)
}

fn account_record_from_login(email: &str, login: &LoginResult, now_unix: i64) -> AccountRecord {
    let mut record = AccountRecord {
        email: Some(email.to_string()),
        created_at: Some(now_unix as f64),
        ..Default::default()
    };
    apply_login_metadata(&mut record, login, now_unix);
    record
}

fn persist_login_secrets(
    record: &mut AccountRecord,
    alias: &str,
    password: SecretText,
    cookies: &BTreeMap<String, String>,
    backend: &dyn SecretBackend,
) -> Result<AccountSecretWriteJournal, AccountCredentialError> {
    let mut journal = AccountSecretWriteJournal::default();
    let password = password.into_inner();
    if let Err(error) =
        write_account_password_secret_tracked(record, alias, &password, backend, &mut journal)
    {
        return Err(rollback_login_secret_error(journal, backend, error));
    }
    if let Err(error) =
        write_account_cookie_secrets_tracked(record, alias, cookies, backend, &mut journal)
    {
        return Err(rollback_login_secret_error(journal, backend, error));
    }
    Ok(journal)
}

fn rollback_login_secret_error(
    journal: AccountSecretWriteJournal,
    backend: &dyn SecretBackend,
    error: AccountSecretStoreError,
) -> AccountCredentialError {
    match journal.rollback(backend) {
        Ok(()) => AccountCredentialError::from(error),
        Err(rollback_error) => AccountCredentialError::from(rollback_error),
    }
}

fn credential_metadata_status(report: &AccountSessionRefreshReport) -> CredentialMetadataStatus {
    if report.subscription_refresh_status == SubscriptionRefreshStatus::FailedButAccountUpdated {
        CredentialMetadataStatus::FailedButAccountSaved
    } else {
        CredentialMetadataStatus::Refreshed
    }
}

fn ensure_session_cookie(cookies: &BTreeMap<String, String>) -> Result<(), AccountCredentialError> {
    cookies
        .get(overleaf_api::OVERLEAF_SESSION_COOKIE)
        .filter(|value| !value.trim().is_empty())
        .map(|_| ())
        .ok_or(AccountCredentialError::MissingSessionCookie)
}

fn is_identity_error(error: &AccountSessionError) -> bool {
    matches!(
        error,
        AccountSessionError::InvalidSessionCookie { .. }
            | AccountSessionError::EmailMismatch { .. }
            | AccountSessionError::EmailBelongsToAnotherAlias { .. }
    )
}

impl AccountCredentialError {
    fn from_io(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }

    pub(crate) fn from_browser(error: BrowserAutomationError) -> Self {
        match error {
            BrowserAutomationError::InvalidCredentials => Self::InvalidCredentials,
            BrowserAutomationError::ChallengeRequired { .. } => Self::ChallengeRequired,
            BrowserAutomationError::RobotVerificationBlocked => Self::RobotVerificationBlocked,
            error => Self::Browser {
                message: error.to_string(),
            },
        }
    }

    fn from_session_identity(error: AccountSessionError) -> Self {
        Self::SessionIdentity {
            message: format!("{error:?}"),
        }
    }
}

impl From<AccountSecretStoreError> for AccountCredentialError {
    fn from(error: AccountSecretStoreError) -> Self {
        match error {
            AccountSecretStoreError::WriteFailed { alias, category } => {
                Self::SecretWriteFailed { alias, category }
            }
            AccountSecretStoreError::ReadFailed { alias, category } => {
                Self::SecretWriteFailed { alias, category }
            }
            AccountSecretStoreError::Missing { alias, category } => {
                Self::SecretWriteFailed { alias, category }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account_secrets::{
        read_account_password_secret, resolve_account_cookies, tests::MemorySecrets,
    };
    use std::sync::Mutex;

    struct ConcurrentMetadata<'a> {
        store: &'a AccountStore,
        commit_lock: &'a Mutex<()>,
    }

    #[async_trait::async_trait]
    impl AccountSessionMetadataRefresher for ConcurrentMetadata<'_> {
        async fn refresh_with_cookies(
            &self,
            document: &mut AccountsDocument,
            alias: &str,
            cookies: &BTreeMap<String, String>,
            _now_unix: i64,
        ) -> Result<AccountSessionRefreshReport, AccountSessionError> {
            assert_eq!(cookies["overleaf_session2"], format!("session-{alias}"));
            {
                let _guard = self
                    .commit_lock
                    .try_lock()
                    .expect("network work must not hold the commit lock");
                let mut current = self.store.load().unwrap();
                current.current = Some("beta".into());
                let record = current.accounts.get_mut(alias).unwrap();
                record.subscription_checked_at = Some(200.0);
                record.subscription_status = Some("pro".into());
                current.accounts.entry("other".into()).or_default();
                self.store.save(&current).unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            let record = document.accounts.get_mut(alias).unwrap();
            record.subscription_status = Some("free".into());
            record.subscription_checked_at = Some(100.0);
            Ok(AccountSessionRefreshReport {
                alias: alias.into(),
                email: record.email.clone(),
                user_id: record.user_id.clone(),
                subscription_status: Some("free".into()),
                subscription_label: None,
                trial_expiry: None,
                subscription_refresh_status: SubscriptionRefreshStatus::Detected,
            })
        }
    }

    fn login(alias: &str) -> LoginResult {
        LoginResult {
            email: Some(format!("{alias}@example.com")),
            user_id: None,
            cookies: vec![overleaf_browser::CookieCapture {
                name: "overleaf_session2".into(),
                value: SecretText::new(format!("session-{alias}")),
                expiration_date: Some(300),
            }],
            trial_expiry: None,
            subscription_status: None,
            subscription_label: None,
        }
    }

    #[test]
    fn concurrent_login_saves_preserve_other_accounts_and_newer_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccountStore::new(directory.path().join("accounts.json"));
        let mut document = AccountsDocument::default();
        for alias in ["alpha", "beta"] {
            document.accounts.insert(
                alias.into(),
                AccountRecord {
                    email: Some(format!("{alias}@example.com")),
                    ..Default::default()
                },
            );
        }
        store.save(&document).unwrap();
        let backend = MemorySecrets::default();
        let lock = Mutex::new(());
        let metadata = ConcurrentMetadata {
            store: &store,
            commit_lock: &lock,
        };
        let save = |alias: &'static str| {
            save_account_login_with_commit_lock(
                &store,
                alias,
                CredentialsLoginInput {
                    email: format!("{alias}@example.com"),
                    password: SecretText::new(format!("password-{alias}")),
                },
                login(alias),
                false,
                &metadata,
                100,
                &backend,
                &lock,
                None,
            )
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (alpha, beta) =
            runtime.block_on(futures_util::future::join(save("alpha"), save("beta")));
        assert!(alpha.is_ok(), "{alpha:?}");
        assert!(beta.is_ok(), "{beta:?}");
        let saved = store.load().unwrap();
        assert_eq!(saved.current.as_deref(), Some("beta"));
        assert!(saved.accounts.contains_key("other"));
        for alias in ["alpha", "beta"] {
            let record = &saved.accounts[alias];
            assert_eq!(record.subscription_checked_at, Some(200.0));
            assert_eq!(record.subscription_status.as_deref(), Some("pro"));
            assert_eq!(
                resolve_account_cookies(record, alias, &backend).unwrap()["overleaf_session2"],
                format!("session-{alias}")
            );
            assert_eq!(
                read_account_password_secret(record, alias, &backend).unwrap(),
                format!("password-{alias}")
            );
        }
    }

    #[test]
    fn mismatched_login_cannot_overwrite_another_accounts_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccountStore::new(directory.path().join("accounts.json"));
        let mut document = AccountsDocument::default();
        document.accounts.insert(
            "alpha".into(),
            AccountRecord {
                email: Some("alpha@example.com".into()),
                ..Default::default()
            },
        );
        store.save(&document).unwrap();
        let backend = MemorySecrets::default();
        let lock = Mutex::new(());
        let metadata = ConcurrentMetadata {
            store: &store,
            commit_lock: &lock,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(save_account_login_with_commit_lock(
            &store,
            "alpha",
            CredentialsLoginInput {
                email: "alpha@example.com".into(),
                password: SecretText::new("password-alpha"),
            },
            login("beta"),
            false,
            &metadata,
            100,
            &backend,
            &lock,
            None,
        ));
        assert!(matches!(
            result,
            Err(AccountCredentialError::SessionIdentity { .. })
        ));
        assert_eq!(store.load().unwrap(), document);
        assert!(
            read_account_password_secret(&document.accounts["alpha"], "alpha", &backend).is_err()
        );

        let record = document.accounts.get_mut("alpha").unwrap();
        crate::account_secrets::write_account_cookie_secrets(
            record,
            "alpha",
            &BTreeMap::from([("overleaf_session2".into(), "before-browser-login".into())]),
            &backend,
        )
        .unwrap();
        store.save(&document).unwrap();
        let expected = crate::account_session::AccountSessionSnapshot::capture(
            &document.accounts["alpha"],
            "alpha",
            &backend,
        );
        crate::account_secrets::write_account_cookie_secrets(
            document.accounts.get_mut("alpha").unwrap(),
            "alpha",
            &BTreeMap::from([("overleaf_session2".into(), "replacement-session".into())]),
            &backend,
        )
        .unwrap();
        let stale = runtime.block_on(save_account_login_with_commit_lock(
            &store,
            "alpha",
            CredentialsLoginInput {
                email: "alpha@example.com".into(),
                password: SecretText::new("password-alpha"),
            },
            login("alpha"),
            false,
            &metadata,
            100,
            &backend,
            &lock,
            Some(&expected),
        ));
        assert!(matches!(
            stale,
            Err(AccountCredentialError::SessionIdentity { .. })
        ));
        assert_eq!(
            resolve_account_cookies(&store.load().unwrap().accounts["alpha"], "alpha", &backend)
                .unwrap()["overleaf_session2"],
            "replacement-session"
        );
    }
}
