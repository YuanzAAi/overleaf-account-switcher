use std::path::{Path, PathBuf};

pub mod automation;
pub use automation::{
    cookies_to_map, overleaf_session_cookie, BrowserAutoLoginInput, BrowserAutoLoginResult,
    BrowserAutomation, BrowserAutomationError, BrowserAutomationResult, CookieCapture,
    CredentialsLoginInput, GitTokenInput, GitTokenPageState, GitTokenResult, LoginResult,
    PasswordChangeInput, PasswordChangeResult, RegistrationInput, RegistrationResult, SecretText,
};
pub use overleaf_core::registration_trial_plan_code;
pub mod cdp;
pub use cdp::login as cdp_login;
pub use cdp::session as cdp_session;
pub use cdp::ws as cdp_ws;
pub use cdp::{
    browser_close_command, cookies_from_response, dom_text_command,
    execution_context_id_from_response, frames_from_tree_response, input_insert_text_command,
    network_get_cookies_command, network_get_overleaf_cookies_command, network_set_cookie_command,
    network_set_overleaf_session_cookie_command, overleaf_session_cookie_from_response,
    page_create_isolated_world_command, page_get_frame_tree_command, page_navigate_command,
    parse_browser_websocket_url, parse_cdp_response, parse_page_websocket_url,
    runtime_evaluate_command, runtime_evaluate_in_context_command, runtime_evaluate_string,
    CdpCommand, CdpCookie, CdpError, CdpFrame, CdpParseError, CdpResponse, CDP_LIST_ENDPOINT_PATH,
    CDP_VERSION_ENDPOINT_PATH, DEFAULT_OVERLEAF_COOKIE_URL, OVERLEAF_SESSION_COOKIE_DOMAIN,
    OVERLEAF_SESSION_COOKIE_NAME, OVERLEAF_SESSION_COOKIE_PATH,
};
pub use cdp_login::{
    accept_extra_trial_offer_expression, cancel_subscription_final_expression,
    change_to_pro_annual_plan_expression, classify_login_page_state,
    classify_password_change_page_state, classify_registration_page_state,
    classify_registration_trial_purchase_page, detect_registration_payment_error,
    fill_login_form_expression, fill_password_change_form_expression,
    fill_registration_email_code_expression, fill_registration_form_expression,
    git_token_action_expression, git_token_page_state_expression, is_pro_annual_subscription_page,
    is_subscription_cancellation_confirmed, is_subscription_management_page,
    login_page_state_expression, open_subscription_management_expression, parse_action_result,
    parse_login_page_state, registration_subscription_url, submit_login_form_submit_expression,
    submit_password_change_form_expression, submit_registration_email_code_expression,
    submit_registration_form_expression, submit_registration_payment_expression,
    CdpLoginActionResult, CdpLoginAutomation, CdpLoginOptions, CdpLoginPageState, CdpLoginState,
    CdpPasswordChangeState, CdpRegistrationState, CdpTrialPurchaseAvailability,
    DEFAULT_LOGIN_POLL_DELAY_MS, DEFAULT_LOGIN_STATUS_CHECKS, OVERLEAF_LOGIN_URL,
    OVERLEAF_PROJECT_URL, OVERLEAF_REGISTER_URL, OVERLEAF_SETTINGS_URL,
    OVERLEAF_SUBSCRIPTION_DASHBOARD_URL, OVERLEAF_SUBSCRIPTION_NEW_URL,
};
pub use cdp_session::{CdpSession, CdpSessionError, CdpTransport, CdpTransportError};
pub use cdp_ws::{CdpWebSocketTransport, DEFAULT_CDP_RESPONSE_SCAN_LIMIT};
pub mod extension_bridge;
pub use extension_bridge::ws as extension_bridge_ws;
pub use extension_bridge::{
    get_cookie_command, get_cookie_expiry_command, parse_extension_event, refresh_tabs_command,
    set_cookie_command, CookiePayload, ExtensionBridgeError, ExtensionBridgeEvent,
    ExtensionBridgeSession, ExtensionCommand, ExtensionCommandKind, ExtensionEvent,
    ExtensionResponse, PendingExtensionRequest, OVERLEAF_BRIDGE_WS_URL,
};
pub use extension_bridge_ws::{
    bind_addr_from_environment, ExtensionBridgeWebSocketError, ExtensionBridgeWebSocketServer,
    DEFAULT_EXTENSION_BRIDGE_BIND_HOST, DEFAULT_EXTENSION_BRIDGE_PORT,
    DEFAULT_EXTENSION_BRIDGE_RESPONSE_TIMEOUT, ENV_EXTENSION_BRIDGE_HOST,
    ENV_EXTENSION_BRIDGE_PORT,
};
pub mod chrome;
pub use chrome as local_chrome;
pub use chrome::profiles as chrome_profile;
pub use chrome_profile::{
    chrome_user_data_dir_candidates, default_chrome_user_data_dir, discover_chrome_profiles,
    discover_chrome_profiles_from_environment, ChromeProfileDiscovery, ChromeProfileSummary,
    ENV_CHROME_USER_DATA_DIR,
};
pub use local_chrome::{
    chrome_executable_candidates, cleanup_owned_chrome_profile, detect_chrome_executable,
    find_available_debug_port, page_websocket_url_from_debugger_http_response,
    LocalChromeAutomationConfig, LocalChromeAutomationError, LocalChromeBrowserAutomation,
    LocalChromeCdpSession, DEFAULT_CDP_PORT_ATTEMPTS, DEFAULT_CDP_PORT_START,
    DEFAULT_CHROME_EXIT_WAIT_MS, ENV_CDP_PORT_START, ENV_CHROME_PATH,
};
pub mod runtime;
pub use cdp::stripe as stripe_frame;
pub use chrome::window_focus;
pub use runtime::{
    BrowserCleanupPlan, BrowserCleanupStep, BrowserSessionDescriptor, ChromeLaunchOptions,
    ChromeLaunchPlan, ChromeProxyAttempt, ChromeProxyMode, ChromeProxyPolicy, ChromeProxyRoute,
    CleanupTrigger, DEFAULT_OVERLEAF_URL, PROFILE_REMOVE_RETRY_ATTEMPTS,
    PROFILE_REMOVE_RETRY_DELAY_MS,
};
pub use stripe_frame::{
    fill_stripe_oopif_frames, split_stripe_name, stripe_payment_expiry_value, StripeAddressInput,
    StripeFrameActionResult, StripeOopifError, StripePaymentFramesResult, StripePaymentInput,
};
pub use window_focus::focus_browser_process_window;

pub const ADDRESS_FETCH_PROFILE_PREFIX: &str = "address_fetch_";
pub const REGISTRATION_PROFILE_PREFIX: &str = "overleaf_auto_";
pub const LOGIN_PROFILE_PREFIX: &str = "overleaf_login_";
pub const GIT_TOKEN_PROFILE_PREFIX: &str = "overleaf_token_";
pub const PASSWORD_PROFILE_PREFIX: &str = "overleaf_password_";
pub const USER_WINDOW_PROFILE_PREFIX: &str = "overleaf_user_";

pub const OWNED_TEMP_PROFILE_PREFIXES: &[&str] = &[
    ADDRESS_FETCH_PROFILE_PREFIX,
    REGISTRATION_PROFILE_PREFIX,
    LOGIN_PROFILE_PREFIX,
    GIT_TOKEN_PROFILE_PREFIX,
    PASSWORD_PROFILE_PREFIX,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserTaskKind {
    AddressFetch,
    AccountPasswordLogin,
    CookieRefresh,
    GitToken,
    PasswordChange,
    Registration,
    ProjectMigration,
    BrowserAutoLogin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosePolicy {
    AutoCloseTemp,
    KeepUserWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserSessionPolicy {
    pub task_kind: BrowserTaskKind,
    pub close_policy: ClosePolicy,
    profile_prefix: &'static str,
}

impl BrowserSessionPolicy {
    pub fn for_task(task_kind: BrowserTaskKind) -> Self {
        let (close_policy, profile_prefix) = match task_kind {
            BrowserTaskKind::AddressFetch => {
                (ClosePolicy::AutoCloseTemp, ADDRESS_FETCH_PROFILE_PREFIX)
            }
            BrowserTaskKind::AccountPasswordLogin | BrowserTaskKind::CookieRefresh => {
                (ClosePolicy::AutoCloseTemp, LOGIN_PROFILE_PREFIX)
            }
            BrowserTaskKind::GitToken => (ClosePolicy::AutoCloseTemp, GIT_TOKEN_PROFILE_PREFIX),
            BrowserTaskKind::PasswordChange => {
                (ClosePolicy::AutoCloseTemp, PASSWORD_PROFILE_PREFIX)
            }
            BrowserTaskKind::Registration | BrowserTaskKind::ProjectMigration => {
                (ClosePolicy::AutoCloseTemp, REGISTRATION_PROFILE_PREFIX)
            }
            BrowserTaskKind::BrowserAutoLogin => {
                (ClosePolicy::KeepUserWindow, USER_WINDOW_PROFILE_PREFIX)
            }
        };

        Self {
            task_kind,
            close_policy,
            profile_prefix,
        }
    }

    pub fn should_close_browser(self) -> bool {
        self.close_policy == ClosePolicy::AutoCloseTemp
    }

    pub fn should_cleanup_profile(self) -> bool {
        self.close_policy == ClosePolicy::AutoCloseTemp
    }

    pub fn is_user_controlled(self) -> bool {
        self.close_policy == ClosePolicy::KeepUserWindow
    }

    pub fn profile_prefix(self) -> &'static str {
        self.profile_prefix
    }

    pub fn profile_dir(self, nonce: impl AsRef<str>) -> PathBuf {
        self.profile_dir_in(overleaf_storage::tmp_dir(), nonce)
    }

    pub fn profile_dir_in(self, base_dir: impl AsRef<Path>, nonce: impl AsRef<str>) -> PathBuf {
        temporary_profile_dir(base_dir, self.profile_prefix, nonce)
    }
}

pub fn temporary_profile_dir(
    base_dir: impl AsRef<Path>,
    prefix: &str,
    nonce: impl AsRef<str>,
) -> PathBuf {
    base_dir.as_ref().join(format!(
        "{}{}",
        prefix,
        sanitize_profile_nonce(nonce.as_ref())
    ))
}

pub fn is_owned_temp_profile_path(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .map(is_owned_temp_profile_name)
        .unwrap_or(false)
    {
        return true;
    }

    path.to_string_lossy()
        .rsplit(['/', '\\'])
        .next()
        .map(is_owned_temp_profile_name)
        .unwrap_or(false)
}

pub fn is_owned_temp_profile_name(name: &str) -> bool {
    OWNED_TEMP_PROFILE_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

fn sanitize_profile_nonce(input: &str) -> String {
    let sanitized: String = input
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect();

    if sanitized.is_empty() {
        "session".to_string()
    } else {
        sanitized
    }
}
