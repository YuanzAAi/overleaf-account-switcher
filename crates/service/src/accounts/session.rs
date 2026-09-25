use std::collections::BTreeMap;
use std::sync::Mutex;

use overleaf_api::session::{
    OverleafSessionClient, OverleafUserInfo, ReqwestSessionTransport, SessionError,
    SessionTransport,
};
use overleaf_api::{
    SubscriptionState, SubscriptionStatus, TrialEligibility, TrialPlanAvailability,
};
use overleaf_browser::{CookieCapture, LoginResult};
use overleaf_core::normalize_email;
use overleaf_storage::{AccountRecord, AccountStore, AccountsDocument, SecretBackend};
use serde::Serialize;

use crate::account_secrets::{resolve_account_cookies, AccountSecretStoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionRefreshStatus {
    Detected,
    Unknown,
    FailedButAccountUpdated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountSessionRefreshReport {
    pub alias: String,
    pub email: Option<String>,
    pub user_id: Option<String>,
    pub subscription_status: Option<String>,
    pub subscription_label: Option<String>,
    pub trial_expiry: Option<i64>,
    pub subscription_refresh_status: SubscriptionRefreshStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountSessionRefreshBatchReport {
    pub items: Vec<AccountSessionRefreshBatchItem>,
    pub refreshed_count: usize,
    pub failed_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountSessionRefreshBatchItem {
    pub alias: String,
    pub refreshed: bool,
    pub report: Option<AccountSessionRefreshReport>,
    pub error: Option<AccountSessionError>,
}

impl AccountSessionRefreshBatchItem {
    pub(crate) fn from_result(
        alias: String,
        result: Result<AccountSessionRefreshReport, AccountSessionError>,
    ) -> Self {
        let (report, error) = match result {
            Ok(report) => {
                let error = (report.subscription_refresh_status
                    != SubscriptionRefreshStatus::Detected)
                    .then(|| AccountSessionError::Session {
                        message: "订阅状态暂未识别，请稍后重试".into(),
                    });
                (Some(report), error)
            }
            Err(error) => (None, Some(error)),
        };
        Self {
            alias,
            refreshed: error.is_none(),
            report,
            error,
        }
    }
}

impl AccountSessionRefreshBatchReport {
    pub(crate) fn from_items(items: Vec<AccountSessionRefreshBatchItem>) -> Self {
        let refreshed_count = items.iter().filter(|item| item.refreshed).count();
        Self {
            failed_count: items.len() - refreshed_count,
            refreshed_count,
            items,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountTrialEligibilityReport {
    pub alias: String,
    pub email: Option<String>,
    pub eligibility: TrialEligibility,
    pub plan_availability: TrialPlanAvailability,
    pub subscription_status: String,
    pub subscription_label: Option<String>,
    pub trial_expiry: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountTrialEligibilityBatchReport {
    pub items: Vec<AccountTrialEligibilityBatchItem>,
    pub eligible_count: usize,
    pub active_trial_count: usize,
    pub ineligible_count: usize,
    pub unknown_count: usize,
    pub failed_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountTrialEligibilityBatchItem {
    pub alias: String,
    pub report: Option<AccountTrialEligibilityReport>,
    pub error: Option<AccountSessionError>,
}

impl AccountTrialEligibilityBatchReport {
    fn from_items(items: Vec<AccountTrialEligibilityBatchItem>) -> Self {
        let reports = items.iter().filter_map(|item| item.report.as_ref());
        let eligible_count = reports
            .clone()
            .filter(|report| report.eligibility == TrialEligibility::Eligible)
            .count();
        let active_trial_count = reports
            .clone()
            .filter(|report| report.eligibility == TrialEligibility::ActiveTrial)
            .count();
        let ineligible_count = reports
            .clone()
            .filter(|report| report.eligibility == TrialEligibility::Ineligible)
            .count();
        let unknown_count = reports
            .filter(|report| report.eligibility == TrialEligibility::Unknown)
            .count();
        let failed_count = items.iter().filter(|item| item.error.is_some()).count();
        Self {
            items,
            eligible_count,
            active_trial_count,
            ineligible_count,
            unknown_count,
            failed_count,
        }
    }
}

#[async_trait::async_trait]
pub trait AccountSessionIdentityValidator {
    async fn validate(
        &self,
        alias: &str,
        expected_email: Option<&str>,
        cookies: &BTreeMap<String, String>,
    ) -> Result<OverleafUserInfo, AccountSessionError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReqwestAccountSessionIdentityValidator;

#[async_trait::async_trait]
impl AccountSessionIdentityValidator for ReqwestAccountSessionIdentityValidator {
    async fn validate(
        &self,
        alias: &str,
        expected_email: Option<&str>,
        cookies: &BTreeMap<String, String>,
    ) -> Result<OverleafUserInfo, AccountSessionError> {
        let client = OverleafSessionClient::new(ReqwestSessionTransport::new(cookies.clone()));
        validate_session_identity(alias, expected_email, &client).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountSessionError {
    Io {
        message: String,
    },
    AccountAliasNotFound {
        alias: String,
    },
    MissingCookies {
        alias: String,
    },
    InvalidSessionCookie {
        alias: String,
    },
    EmailMismatch {
        alias: String,
        stored_email: String,
        fetched_email: String,
    },
    EmailBelongsToAnotherAlias {
        alias: String,
        email: String,
        existing_alias: String,
    },
    Session {
        message: String,
    },
}

impl AccountSessionError {
    pub(crate) fn requires_cookie_recovery(&self) -> bool {
        matches!(
            self,
            Self::MissingCookies { .. }
                | Self::InvalidSessionCookie { .. }
                | Self::EmailMismatch { .. }
                | Self::EmailBelongsToAnotherAlias { .. }
        )
    }
}

pub async fn refresh_account_session_metadata<T: SessionTransport>(
    document: &mut AccountsDocument,
    alias: &str,
    client: &OverleafSessionClient<T>,
    now_unix: i64,
) -> Result<AccountSessionRefreshReport, AccountSessionError> {
    refresh_account_session_metadata_internal(document, alias, client, now_unix, true).await
}

pub(crate) async fn refresh_account_session_metadata_with_explicit_cookies<T: SessionTransport>(
    document: &mut AccountsDocument,
    alias: &str,
    client: &OverleafSessionClient<T>,
    now_unix: i64,
) -> Result<AccountSessionRefreshReport, AccountSessionError> {
    refresh_account_session_metadata_internal(document, alias, client, now_unix, false).await
}

async fn refresh_account_session_metadata_internal<T: SessionTransport>(
    document: &mut AccountsDocument,
    alias: &str,
    client: &OverleafSessionClient<T>,
    now_unix: i64,
    require_saved_cookies: bool,
) -> Result<AccountSessionRefreshReport, AccountSessionError> {
    let alias = alias.trim();
    let existing =
        document
            .accounts
            .get(alias)
            .ok_or_else(|| AccountSessionError::AccountAliasNotFound {
                alias: alias.to_string(),
            })?;
    if require_saved_cookies && existing.cookie_refs.is_empty() {
        return Err(AccountSessionError::MissingCookies {
            alias: alias.to_string(),
        });
    }

    let user_info = validate_session_identity(alias, existing.email.as_deref(), client).await?;
    if let Some(fetched_email) = user_info.email.as_deref() {
        validate_fetched_email(document, alias, fetched_email)?;
    }

    let subscription = client.fetch_subscription_status().await;

    let record = document.accounts.get_mut(alias).ok_or_else(|| {
        AccountSessionError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    if let Some(email) = user_info.email {
        record.email = Some(email);
    }
    if let Some(user_id) = user_info.user_id {
        record.user_id = Some(user_id);
    }
    record.last_login_at = Some(now_unix as f64);

    let refresh_status = match subscription {
        Ok(status) => {
            apply_subscription_status(record, status);
            record.subscription_checked_at = Some(now_unix as f64);
            subscription_refresh_status(record)
        }
        Err(_) => {
            record.subscription_checked_at = None;
            SubscriptionRefreshStatus::FailedButAccountUpdated
        }
    };

    Ok(AccountSessionRefreshReport {
        alias: alias.to_string(),
        email: record.email.clone(),
        user_id: record.user_id.clone(),
        subscription_status: record.subscription_status.clone(),
        subscription_label: record.subscription_label.clone(),
        trial_expiry: record.trial_expiry.map(|value| value as i64),
        subscription_refresh_status: refresh_status,
    })
}

pub async fn inspect_account_trial_eligibility<T: SessionTransport>(
    document: &AccountsDocument,
    alias: &str,
    client: &OverleafSessionClient<T>,
    trial_days: u32,
    now_unix: i64,
) -> Result<AccountTrialEligibilityReport, AccountSessionError> {
    let alias = alias.trim();
    let record =
        document
            .accounts
            .get(alias)
            .ok_or_else(|| AccountSessionError::AccountAliasNotFound {
                alias: alias.to_string(),
            })?;
    if record.cookie_refs.is_empty() {
        return Err(AccountSessionError::MissingCookies {
            alias: alias.to_string(),
        });
    }

    let user_info = validate_session_identity(alias, record.email.as_deref(), client).await?;
    if let Some(fetched_email) = user_info.email.as_deref() {
        validate_fetched_email(document, alias, fetched_email)?;
    }
    let status = client
        .fetch_trial_eligibility(now_unix, trial_days)
        .await
        .map_err(AccountSessionError::from_session)?;
    if status.eligibility == TrialEligibility::Unknown {
        return Err(AccountSessionError::Session {
            message: "试用资格检测失败，请重试".into(),
        });
    }

    Ok(AccountTrialEligibilityReport {
        alias: alias.to_string(),
        email: user_info.email.or_else(|| record.email.clone()),
        eligibility: status.eligibility,
        plan_availability: status.plan_availability,
        subscription_status: subscription_state_key(&status.subscription.state).to_string(),
        subscription_label: status.subscription.label,
        trial_expiry: status.subscription.trial_expiry,
    })
}

pub async fn inspect_saved_account_trial_eligibility_with_backend(
    store: &AccountStore,
    aliases: &[String],
    trial_days: u32,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<AccountTrialEligibilityBatchReport, AccountSessionError> {
    inspect_saved_account_trial_eligibility_with_progress(
        store,
        aliases,
        trial_days,
        now_unix,
        backend,
        |_, _| true,
    )
    .await
}

pub(crate) async fn inspect_saved_account_trial_eligibility_with_progress(
    store: &AccountStore,
    aliases: &[String],
    trial_days: u32,
    now_unix: i64,
    backend: &dyn SecretBackend,
    mut on_progress: impl FnMut(usize, usize) -> bool,
) -> Result<AccountTrialEligibilityBatchReport, AccountSessionError> {
    let document = store.load().map_err(AccountSessionError::from_io)?;
    let mut items = Vec::with_capacity(aliases.len());

    for alias in aliases {
        if !on_progress(items.len(), aliases.len()) {
            break;
        }
        let result = match document.accounts.get(alias) {
            Some(record) => match cookies_for_saved_session(record, alias, backend) {
                Ok(cookies) => {
                    let client = OverleafSessionClient::new(ReqwestSessionTransport::new(cookies));
                    inspect_account_trial_eligibility(
                        &document, alias, &client, trial_days, now_unix,
                    )
                    .await
                }
                Err(error) => Err(error),
            },
            None => Err(AccountSessionError::AccountAliasNotFound {
                alias: alias.to_string(),
            }),
        };
        items.push(match result {
            Ok(report) => AccountTrialEligibilityBatchItem {
                alias: alias.to_string(),
                report: Some(report),
                error: None,
            },
            Err(error) => AccountTrialEligibilityBatchItem {
                alias: alias.to_string(),
                report: None,
                error: Some(error),
            },
        });
    }

    on_progress(items.len(), aliases.len());

    Ok(AccountTrialEligibilityBatchReport::from_items(items))
}

pub async fn validate_session_identity<T: SessionTransport>(
    alias: &str,
    expected_email: Option<&str>,
    client: &OverleafSessionClient<T>,
) -> Result<OverleafUserInfo, AccountSessionError> {
    let alias = alias.trim();
    let user_info = client
        .fetch_user_info()
        .await
        .map_err(|error| AccountSessionError::from_identity_session(alias, error))?;
    if user_info.email.is_none() && user_info.user_id.is_none() {
        return Err(AccountSessionError::InvalidSessionCookie {
            alias: alias.to_string(),
        });
    }
    if let (Some(expected), Some(actual)) = (expected_email, user_info.email.as_deref()) {
        if normalize_email(expected) != normalize_email(actual) {
            return Err(AccountSessionError::EmailMismatch {
                alias: alias.to_string(),
                stored_email: expected.to_string(),
                fetched_email: actual.to_string(),
            });
        }
    } else if expected_email.is_some() {
        return Err(AccountSessionError::InvalidSessionCookie {
            alias: alias.to_string(),
        });
    }
    Ok(user_info)
}

pub async fn refresh_saved_account_sessions_in_store_with_backend(
    store: &AccountStore,
    aliases: &[String],
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<AccountSessionRefreshBatchReport, AccountSessionError> {
    let mut items = Vec::with_capacity(aliases.len());

    for alias in aliases {
        let result = refresh_saved_account_session_with_commit_lock(
            store,
            alias,
            now_unix,
            backend,
            &crate::ReqwestAccountSessionMetadataRefresher,
            None,
        )
        .await;
        items.push(AccountSessionRefreshBatchItem::from_result(
            alias.clone(),
            result,
        ));
    }

    Ok(AccountSessionRefreshBatchReport::from_items(items))
}

pub(crate) async fn refresh_saved_account_session_with_commit_lock(
    store: &AccountStore,
    alias: &str,
    now_unix: i64,
    backend: &dyn SecretBackend,
    refresher: &(dyn crate::AccountSessionMetadataRefresher + Send + Sync),
    commit_lock: Option<&Mutex<()>>,
) -> Result<AccountSessionRefreshReport, AccountSessionError> {
    let mut snapshot = store.load().map_err(AccountSessionError::from_io)?;
    let original = snapshot.accounts.get(alias).cloned().ok_or_else(|| {
        AccountSessionError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    let cookies = cookies_for_saved_session(&original, alias, backend)?;
    let result = refresher
        .refresh_with_cookies(&mut snapshot, alias, &cookies, now_unix)
        .await;
    if result
        .as_ref()
        .is_err_and(|error| !error.requires_cookie_recovery())
    {
        return result;
    }

    // 请求期间不持锁，只合并会话元数据，避免覆盖并发保存的凭据和当前账号。
    let _guard = commit_lock
        .map(Mutex::lock)
        .transpose()
        .map_err(|_| AccountSessionError::Io {
            message: "account commit lock poisoned".into(),
        })?;
    let mut document = store.load().map_err(AccountSessionError::from_io)?;
    if result.is_ok() {
        if let Some(email) = snapshot.accounts[alias].email.as_deref() {
            validate_fetched_email(&document, alias, email)?;
        }
    }
    let record = document.accounts.get_mut(alias).ok_or_else(|| {
        AccountSessionError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    if record.email != original.email
        || cookies_for_saved_session(record, alias, backend)? != cookies
        || record
            .subscription_checked_at
            .is_some_and(|checked| checked > now_unix as f64)
    {
        return Err(AccountSessionError::Session {
            message: "账号会话已更新，请重新刷新订阅状态".into(),
        });
    }
    if result.is_ok() {
        let updated = &snapshot.accounts[alias];
        if updated.subscription_status.as_deref() == Some("free") {
            record.trial_started_at = None;
        }
        record.set_trial_expiry(updated.trial_expiry);
        record.subscription_status = updated.subscription_status.clone();
        record.subscription_label = updated.subscription_label.clone();
        record.subscription_checked_at = updated.subscription_checked_at;
        record.email = updated.email.clone();
        record.user_id = updated.user_id.clone();
        if updated.last_login_at > record.last_login_at {
            record.last_login_at = updated.last_login_at;
        }
    } else {
        record.subscription_status = Some("unknown".into());
        record.subscription_label = None;
        record.subscription_checked_at = None;
    }
    store
        .save(&document)
        .map_err(AccountSessionError::from_io)?;
    result
}

fn cookies_for_saved_session(
    record: &overleaf_storage::AccountRecord,
    alias: &str,
    backend: &dyn SecretBackend,
) -> Result<std::collections::BTreeMap<String, String>, AccountSessionError> {
    resolve_account_cookies(record, alias, backend).map_err(AccountSessionError::from)
}

fn validate_fetched_email(
    document: &AccountsDocument,
    alias: &str,
    fetched_email: &str,
) -> Result<(), AccountSessionError> {
    if let Some(record) = document.accounts.get(alias) {
        if let Some(stored_email) = record.email.as_deref() {
            if normalize_email(stored_email) != normalize_email(fetched_email) {
                return Err(AccountSessionError::EmailMismatch {
                    alias: alias.to_string(),
                    stored_email: stored_email.to_string(),
                    fetched_email: fetched_email.to_string(),
                });
            }
        }
    }

    if let Some(existing_alias) = document.duplicate_alias_by_email(fetched_email) {
        if existing_alias != alias {
            return Err(AccountSessionError::EmailBelongsToAnotherAlias {
                alias: alias.to_string(),
                email: fetched_email.to_string(),
                existing_alias: existing_alias.to_string(),
            });
        }
    }

    Ok(())
}

pub(crate) fn overleaf_session_expiry(cookies: &[CookieCapture]) -> Option<i64> {
    cookies
        .iter()
        .find(|cookie| cookie.name == overleaf_api::OVERLEAF_SESSION_COOKIE)
        .and_then(|cookie| cookie.expiration_date)
}

pub(crate) fn apply_login_metadata(record: &mut AccountRecord, login: &LoginResult, now_unix: i64) {
    record.cookie_expiry = overleaf_session_expiry(&login.cookies).map(|value| value as f64);
    record.cookie_updated_at = Some(now_unix as f64);
    record.last_login_at = Some(now_unix as f64);
    if let Some(email) = &login.email {
        record.email = Some(email.clone());
    }
    if let Some(user_id) = &login.user_id {
        record.user_id = Some(user_id.clone());
    }
    if let Some(trial_expiry) = login.trial_expiry {
        record.trial_expiry = Some(trial_expiry as f64);
    }
    if let Some(status) = &login.subscription_status {
        record.subscription_status = Some(status.clone());
    }
    if let Some(label) = &login.subscription_label {
        record.subscription_label = Some(label.clone());
    }
    if login.trial_expiry.is_some()
        || login.subscription_status.is_some()
        || login.subscription_label.is_some()
    {
        record.subscription_checked_at = Some(now_unix as f64);
    }
}

fn apply_subscription_status(
    record: &mut overleaf_storage::AccountRecord,
    status: SubscriptionStatus,
) {
    if status.state == SubscriptionState::Free {
        record.trial_days = None;
        record.trial_started_at = None;
    }
    record.set_trial_expiry(status.trial_expiry.map(|value| value as f64));
    record.subscription_status = Some(subscription_state_key(&status.state).to_string());
    record.subscription_label = status.label;
}

fn subscription_refresh_status(
    record: &overleaf_storage::AccountRecord,
) -> SubscriptionRefreshStatus {
    match record
        .subscription_status
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "trial" | "pro" | "subscription" | "free" => SubscriptionRefreshStatus::Detected,
        _ => SubscriptionRefreshStatus::Unknown,
    }
}

fn subscription_state_key(state: &SubscriptionState) -> &'static str {
    match state {
        SubscriptionState::Trial => "trial",
        SubscriptionState::Pro => "pro",
        SubscriptionState::Subscription => "subscription",
        SubscriptionState::Free => "free",
        SubscriptionState::Unknown => "unknown",
    }
}

impl AccountSessionError {
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

    fn from_identity_session(alias: &str, error: SessionError) -> Self {
        match error {
            SessionError::MissingUserInfo
            | SessionError::Http(overleaf_api::session::SessionHttpError {
                status: 401 | 403,
                ..
            }) => Self::InvalidSessionCookie {
                alias: alias.to_string(),
            },
            error => Self::from_session(error),
        }
    }
}

impl From<AccountSecretStoreError> for AccountSessionError {
    fn from(error: AccountSecretStoreError) -> Self {
        match error {
            AccountSecretStoreError::Missing { alias, .. } => Self::MissingCookies { alias },
            AccountSecretStoreError::ReadFailed { alias, category }
            | AccountSecretStoreError::WriteFailed { alias, category } => Self::Session {
                message: format!(
                    "failed to read stored {category} secret for account alias: {alias}"
                ),
            },
        }
    }
}
