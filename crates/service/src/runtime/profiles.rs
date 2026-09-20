use std::collections::BTreeMap;

use overleaf_api::session::{
    OverleafSessionClient, OverleafUserInfo, ReqwestSessionTransport, SessionError,
};
use overleaf_browser::{ExtensionBridgeSession, OVERLEAF_SESSION_COOKIE_NAME};
use overleaf_storage::{AccountStore, AccountsDocument};
use serde::Serialize;

use crate::{AccountSwitchCommandExecutor, AccountSwitchExecutionError};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrowserProfileAccountReport {
    pub cookie_present: bool,
    pub email: Option<String>,
    pub user_id: Option<String>,
    pub saved_alias: Option<String>,
    pub saved_account_known: bool,
    pub cookie_expiry: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserProfileAccountError {
    Io { message: String },
    ExtensionBridgeUnavailable,
    ExtensionBridge { message: String },
    ExtensionCommandFailed { request_id: String, message: String },
    MissingSessionCookie,
    Session { message: String },
}

#[async_trait::async_trait]
pub trait BrowserProfileSessionInspector {
    async fn fetch_user_info(
        &self,
        cookies: &BTreeMap<String, String>,
    ) -> Result<OverleafUserInfo, BrowserProfileAccountError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReqwestBrowserProfileSessionInspector;

#[async_trait::async_trait]
impl BrowserProfileSessionInspector for ReqwestBrowserProfileSessionInspector {
    async fn fetch_user_info(
        &self,
        cookies: &BTreeMap<String, String>,
    ) -> Result<OverleafUserInfo, BrowserProfileAccountError> {
        let client = OverleafSessionClient::new(ReqwestSessionTransport::new(cookies.clone()));
        client
            .fetch_user_info()
            .await
            .map_err(browser_profile_session_error)
    }
}

pub async fn detect_browser_profile_account_in_store(
    store: &AccountStore,
    executor: &(dyn AccountSwitchCommandExecutor + Send + Sync),
    inspector: &(dyn BrowserProfileSessionInspector + Send + Sync),
) -> Result<BrowserProfileAccountReport, BrowserProfileAccountError> {
    let document = store
        .load()
        .map_err(|error| BrowserProfileAccountError::Io {
            message: error.to_string(),
        })?;
    detect_browser_profile_account(&document, executor, inspector).await
}

pub async fn detect_browser_profile_account(
    document: &AccountsDocument,
    executor: &(dyn AccountSwitchCommandExecutor + Send + Sync),
    inspector: &(dyn BrowserProfileSessionInspector + Send + Sync),
) -> Result<BrowserProfileAccountReport, BrowserProfileAccountError> {
    let mut bridge = ExtensionBridgeSession::new();
    let command = bridge.issue_get_cookie(OVERLEAF_SESSION_COOKIE_NAME);
    let response = executor
        .execute_extension_command(command)
        .await
        .map_err(browser_profile_extension_error)?;

    if !response.success {
        return Err(BrowserProfileAccountError::ExtensionCommandFailed {
            request_id: response.request_id,
            message: response
                .message
                .unwrap_or_else(|| "extension get_cookie command failed".to_string()),
        });
    }

    let cookie = response
        .cookie
        .filter(|cookie| {
            cookie.name == OVERLEAF_SESSION_COOKIE_NAME && !cookie.value.trim().is_empty()
        })
        .ok_or(BrowserProfileAccountError::MissingSessionCookie)?;

    let cookies = BTreeMap::from([(OVERLEAF_SESSION_COOKIE_NAME.to_string(), cookie.value)]);
    let user_info = inspector.fetch_user_info(&cookies).await?;
    let saved_alias = user_info
        .email
        .as_deref()
        .and_then(|email| saved_alias_by_email(document, email));

    Ok(BrowserProfileAccountReport {
        cookie_present: true,
        email: user_info.email,
        user_id: user_info.user_id,
        saved_account_known: saved_alias.is_some(),
        saved_alias,
        cookie_expiry: cookie.expiration_date,
    })
}

fn saved_alias_by_email(document: &AccountsDocument, email: &str) -> Option<String> {
    let normalized = email.trim().to_ascii_lowercase();
    document.accounts.iter().find_map(|(alias, record)| {
        let record_email = record.email.as_deref()?.trim().to_ascii_lowercase();
        (record_email == normalized).then(|| alias.clone())
    })
}

fn browser_profile_extension_error(
    error: AccountSwitchExecutionError,
) -> BrowserProfileAccountError {
    match error {
        AccountSwitchExecutionError::Bridge { message } => {
            BrowserProfileAccountError::ExtensionBridge { message }
        }
        AccountSwitchExecutionError::CommandFailed {
            request_id,
            message,
            ..
        } => BrowserProfileAccountError::ExtensionCommandFailed {
            request_id,
            message,
        },
        other => BrowserProfileAccountError::ExtensionBridge {
            message: format!("{other:?}"),
        },
    }
}

fn browser_profile_session_error(error: SessionError) -> BrowserProfileAccountError {
    BrowserProfileAccountError::Session {
        message: error.to_string(),
    }
}
