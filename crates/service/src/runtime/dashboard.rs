use serde::Serialize;

use overleaf_storage::{AccountsDocument, SecretBackend};

use crate::accounts::{
    list_account_summaries_with_backend, AccountSummary, TimeStatus, EXPIRING_SOON_SECONDS,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DashboardRuntimeState {
    pub chrome_profile: Option<String>,
    pub extension_bridge_configured: bool,
    pub extension_connected: bool,
    pub extension_client_count: usize,
    pub browser_account_email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DashboardSummary {
    pub chrome_profile: Option<String>,
    pub extension_bridge_configured: bool,
    pub extension_connected: bool,
    pub extension_client_count: usize,
    pub browser_account_email: Option<String>,
    pub current_alias: Option<String>,
    pub current_email: Option<String>,
    pub account_count: usize,
    pub cookie_missing_count: usize,
    pub cookie_unknown_count: usize,
    pub cookie_expiring_soon_count: usize,
    pub cookie_expired_count: usize,
    pub git_token_missing_count: usize,
    pub git_token_unknown_count: usize,
    pub git_token_expiring_soon_count: usize,
    pub git_token_expired_count: usize,
    pub trial_account_count: usize,
    pub trial_active_count: usize,
    pub trial_expiring_soon_count: usize,
    pub pro_account_count: usize,
    pub free_account_count: usize,
    pub unknown_subscription_count: usize,
}

pub fn dashboard_summary_with_backend(
    document: &AccountsDocument,
    runtime: DashboardRuntimeState,
    now_unix: i64,
    backend: &(dyn SecretBackend + Send + Sync),
) -> DashboardSummary {
    dashboard_summary_from_accounts(
        list_account_summaries_with_backend(document, now_unix, backend),
        runtime,
        now_unix,
    )
}

fn dashboard_summary_from_accounts(
    accounts: Vec<AccountSummary>,
    runtime: DashboardRuntimeState,
    now_unix: i64,
) -> DashboardSummary {
    let current = accounts.iter().find(|account| account.is_current);
    let mut summary = DashboardSummary {
        chrome_profile: runtime.chrome_profile,
        extension_bridge_configured: runtime.extension_bridge_configured,
        extension_connected: runtime.extension_connected,
        extension_client_count: runtime.extension_client_count,
        browser_account_email: runtime.browser_account_email,
        current_alias: current.map(|account| account.alias.clone()),
        current_email: current.and_then(|account| account.email.clone()),
        account_count: accounts.len(),
        cookie_missing_count: 0,
        cookie_unknown_count: 0,
        cookie_expiring_soon_count: 0,
        cookie_expired_count: 0,
        git_token_missing_count: 0,
        git_token_unknown_count: 0,
        git_token_expiring_soon_count: 0,
        git_token_expired_count: 0,
        trial_account_count: 0,
        trial_active_count: 0,
        trial_expiring_soon_count: 0,
        pro_account_count: 0,
        free_account_count: 0,
        unknown_subscription_count: 0,
    };

    for account in &accounts {
        count_time_status(
            &account.cookie.status,
            &mut summary.cookie_missing_count,
            &mut summary.cookie_unknown_count,
            &mut summary.cookie_expiring_soon_count,
            &mut summary.cookie_expired_count,
        );
        count_time_status(
            &account.git_token.status,
            &mut summary.git_token_missing_count,
            &mut summary.git_token_unknown_count,
            &mut summary.git_token_expiring_soon_count,
            &mut summary.git_token_expired_count,
        );

        match normalized_status(account.subscription_status.as_deref()).as_deref() {
            Some("trial") => {
                summary.trial_account_count += 1;
                if is_future(account.trial_expiry, now_unix) {
                    summary.trial_active_count += 1;
                }
                if is_expiring_soon(account.trial_expiry, now_unix) {
                    summary.trial_expiring_soon_count += 1;
                }
            }
            Some("pro") | Some("subscription") => {
                summary.pro_account_count += 1;
            }
            Some("free") => {
                summary.free_account_count += 1;
            }
            _ => {
                summary.unknown_subscription_count += 1;
            }
        }
    }

    summary
}

fn count_time_status(
    status: &TimeStatus,
    missing_count: &mut usize,
    unknown_count: &mut usize,
    expiring_soon_count: &mut usize,
    expired_count: &mut usize,
) {
    match status {
        TimeStatus::Missing => *missing_count += 1,
        TimeStatus::Unknown => *unknown_count += 1,
        TimeStatus::ExpiringSoon => *expiring_soon_count += 1,
        TimeStatus::Expired => *expired_count += 1,
        TimeStatus::Valid => {}
    }
}

fn normalized_status(status: Option<&str>) -> Option<String> {
    status
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn is_future(expiry: Option<i64>, now_unix: i64) -> bool {
    expiry.map(|expiry| expiry > now_unix).unwrap_or(false)
}

fn is_expiring_soon(expiry: Option<i64>, now_unix: i64) -> bool {
    expiry
        .map(|expiry| expiry > now_unix && expiry - now_unix <= EXPIRING_SOON_SECONDS)
        .unwrap_or(false)
}
