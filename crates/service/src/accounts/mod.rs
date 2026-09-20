use serde::Serialize;

pub mod actions;
pub mod browser;
pub mod credentials;
pub mod git_token;
pub mod io;
pub mod password_change;
pub mod password_policy;
pub mod registration;
pub mod secrets;
pub mod session;
pub mod switch;

use overleaf_browser::OVERLEAF_SESSION_COOKIE_NAME;
use overleaf_storage::{AccountRecord, AccountsDocument, SecretBackend};

use crate::account_secrets::{
    read_account_cookie_secret, read_account_cookie_secret_for_recovery,
    read_account_git_token_secret, read_account_git_token_secret_for_recovery,
    read_account_password_secret, read_account_password_secret_for_recovery,
    AccountSecretStoreError,
};

pub const EXPIRING_SOON_SECONDS: i64 = 3 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeStatus {
    Missing,
    Unknown,
    Expired,
    ExpiringSoon,
    Valid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExpiringSecretStatus {
    pub present: bool,
    pub expiry: Option<i64>,
    pub status: TimeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub masked_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretPresence {
    pub present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub masked_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountSummary {
    pub alias: String,
    pub email: Option<String>,
    pub is_current: bool,
    pub subscription_status: Option<String>,
    pub subscription_label: Option<String>,
    pub trial_days: Option<u32>,
    pub trial_started_at: Option<i64>,
    pub trial_expiry: Option<i64>,
    pub cookie: ExpiringSecretStatus,
    pub git_token: ExpiringSecretStatus,
    pub password: SecretPresence,
    pub last_login_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountSecretError {
    AliasNotFound {
        alias: String,
    },
    MissingSecret {
        alias: String,
        category: &'static str,
    },
    SecretReadFailed {
        alias: String,
        category: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountSecretField {
    Cookie,
    GitToken,
    Password,
}

impl AccountSecretField {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "cookie" => Some(Self::Cookie),
            "git_token" => Some(Self::GitToken),
            "password" => Some(Self::Password),
            _ => None,
        }
    }

    pub const fn category(self) -> &'static str {
        match self {
            Self::Cookie => "cookie",
            Self::GitToken => "git_token",
            Self::Password => "password",
        }
    }
}

pub fn list_account_summaries_with_backend(
    document: &AccountsDocument,
    now_unix: i64,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Vec<AccountSummary> {
    document
        .accounts
        .iter()
        .map(|(alias, record)| account_summary(document, alias, record, now_unix, backend))
        .collect()
}

pub fn account_secret_for_copy_with_backend(
    document: &AccountsDocument,
    alias: &str,
    field: AccountSecretField,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<String, AccountSecretError> {
    let record = document
        .accounts
        .get(alias)
        .ok_or_else(|| AccountSecretError::AliasNotFound {
            alias: alias.to_string(),
        })?;

    match field {
        AccountSecretField::Cookie => {
            read_account_cookie_secret(record, alias, OVERLEAF_SESSION_COOKIE_NAME, backend)
        }
        AccountSecretField::GitToken => read_account_git_token_secret(record, alias, backend),
        AccountSecretField::Password => read_account_password_secret(record, alias, backend),
    }
    .map_err(AccountSecretError::from)
}

struct SecretReadSummary {
    present: bool,
    read_failed: bool,
    masked_hint: Option<String>,
}

fn summarize_secret_read(
    read_result: Result<String, AccountSecretStoreError>,
) -> SecretReadSummary {
    match read_result {
        Ok(value) => SecretReadSummary {
            present: true,
            read_failed: false,
            masked_hint: Some(mask_secret_hint(&value)),
        },
        Err(AccountSecretStoreError::Missing { .. }) => SecretReadSummary {
            present: false,
            read_failed: false,
            masked_hint: None,
        },
        Err(
            AccountSecretStoreError::ReadFailed { .. }
            | AccountSecretStoreError::WriteFailed { .. },
        ) => SecretReadSummary {
            present: false,
            read_failed: true,
            masked_hint: None,
        },
    }
}

fn account_summary(
    document: &AccountsDocument,
    alias: &str,
    record: &AccountRecord,
    now_unix: i64,
    backend: &(dyn SecretBackend + Send + Sync),
) -> AccountSummary {
    let cookie = summarize_secret_read(read_account_cookie_secret_for_recovery(
        record,
        alias,
        OVERLEAF_SESSION_COOKIE_NAME,
        backend,
    ));
    let git_token = summarize_secret_read(read_account_git_token_secret_for_recovery(
        record, alias, backend,
    ));
    let password = summarize_secret_read(read_account_password_secret_for_recovery(
        record, alias, backend,
    ));

    AccountSummary {
        alias: alias.to_string(),
        email: record.email.clone(),
        is_current: document.current.as_deref() == Some(alias),
        subscription_status: record.subscription_status.clone(),
        subscription_label: record.subscription_label.clone(),
        trial_days: record.trial_duration_days(),
        trial_started_at: timestamp(record.trial_started_at),
        trial_expiry: timestamp(record.trial_expiry),
        cookie: ExpiringSecretStatus {
            present: cookie.present,
            expiry: timestamp(record.cookie_expiry),
            status: if cookie.read_failed {
                TimeStatus::Unknown
            } else {
                expiring_status(cookie.present, record.cookie_expiry, now_unix)
            },
            masked_hint: cookie.masked_hint,
        },
        git_token: ExpiringSecretStatus {
            present: git_token.present,
            expiry: timestamp(record.git_token_expiry),
            status: if git_token.read_failed {
                TimeStatus::Unknown
            } else {
                expiring_status(git_token.present, record.git_token_expiry, now_unix)
            },
            masked_hint: git_token.masked_hint,
        },
        password: SecretPresence {
            present: password.present,
            masked_hint: password.masked_hint,
        },
        last_login_at: timestamp(record.last_login_at),
    }
}

fn mask_secret_hint(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 6 {
        return "****".to_string();
    }
    let prefix: String = chars.iter().take(3).collect();
    let suffix: String = chars
        .iter()
        .rev()
        .take(3)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}****{suffix}")
}

impl From<AccountSecretStoreError> for AccountSecretError {
    fn from(error: AccountSecretStoreError) -> Self {
        match error {
            AccountSecretStoreError::Missing { alias, category } => {
                Self::MissingSecret { alias, category }
            }
            AccountSecretStoreError::ReadFailed { alias, category }
            | AccountSecretStoreError::WriteFailed { alias, category } => {
                Self::SecretReadFailed { alias, category }
            }
        }
    }
}

fn expiring_status(present: bool, expiry: Option<f64>, now_unix: i64) -> TimeStatus {
    if !present {
        return TimeStatus::Missing;
    }

    let Some(expiry) = timestamp(expiry) else {
        return TimeStatus::Unknown;
    };

    if expiry <= now_unix {
        TimeStatus::Expired
    } else if expiry - now_unix <= EXPIRING_SOON_SECONDS {
        TimeStatus::ExpiringSoon
    } else {
        TimeStatus::Valid
    }
}

fn timestamp(value: Option<f64>) -> Option<i64> {
    value.map(|timestamp| timestamp as i64)
}
