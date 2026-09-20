use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::fs;
use std::future::Future;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::ServiceTaskPhase;
use futures_util::{stream, StreamExt};
use overleaf_api::session::{OverleafSessionClient, ReqwestSessionTransport};
use overleaf_api::{
    classify_trial_eligibility, parse_subscription_status_with_now, SubscriptionState,
    TrialEligibility, TrialEligibilityStatus, TrialPlanAvailability,
};
use overleaf_browser::{
    cleanup_owned_chrome_profile, cookies_to_map, detect_chrome_executable,
    discover_chrome_profiles_from_environment, focus_browser_process_window,
    is_owned_temp_profile_name, BrowserAutoLoginInput, BrowserAutomation, BrowserAutomationError,
    BrowserAutomationResult, CdpLoginState, CdpRegistrationState, CdpTrialPurchaseAvailability,
    ChromeProxyAttempt, ChromeProxyMode, ChromeProxyPolicy, CredentialsLoginInput,
    ExtensionBridgeWebSocketServer, ExtensionCommand, ExtensionResponse,
    LocalChromeAutomationConfig, LocalChromeBrowserAutomation, LocalChromeCdpSession, LoginResult,
    RegistrationInput, RegistrationResult, SecretText, StripeAddressInput, StripePaymentInput,
    OVERLEAF_SESSION_COOKIE_NAME, PROFILE_REMOVE_RETRY_ATTEMPTS, PROFILE_REMOVE_RETRY_DELAY_MS,
    USER_WINDOW_PROFILE_PREFIX,
};
use overleaf_core::{make_alias, registration_trial_plan_code};
use overleaf_storage::{
    AccountRecord, AccountStore, AccountsDocument, AddressStore, CardStore, SecretBackend,
    SystemKeyringSecretBackend,
};
use overleaf_workflows::{
    plan_alias_passwords, plan_credential_accounts, split_comma_values, AliasPasswordPlan,
    CredentialAccountPlan, CredentialBatchInput, RegistrationState, RegistrationTaskEvent,
    RegistrationWorkflow, SkippedDuplicateEmail, RECAPTCHA_WAIT_SECONDS,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

mod registration;
use registration::{
    cancel_task_with_registration_release, continue_existing_trial_response,
    continue_registration_after_captcha_response, continue_registration_from_state_response,
    continue_registration_with_email_code_response,
    continue_registration_with_new_credentials_response,
    existing_account_trial_not_eligible_response, existing_trial_preflight,
    inspect_rendered_existing_trial_continuation, register_account_response,
    register_existing_account_trial_response_inner, release_registration_slot,
    validate_trial_cookie_eligibility,
};
mod runtime;
use runtime::{
    cleanup_runtime_artifacts_response, cleanup_temp_profiles_response,
    preview_temp_profiles_response, runtime_artifact_status,
};
pub use runtime::{
    RuntimeArtifactCleanupItem, RuntimeArtifactCleanupReport, RuntimeArtifactItem,
    RuntimeArtifactStatus, RuntimeTempProfileCleanupItem, RuntimeTempProfileCleanupPreview,
    RuntimeTempProfileCleanupReport,
};
mod errors;
use errors::*;
mod credential_jobs;
use credential_jobs::*;
mod account_io;
use account_io::*;
mod resources;
use resources::*;
mod account_switch;
mod config;
mod skills;
use account_switch::{
    account_cookie_recovery_plan, execute_account_switch_response,
    execute_account_switch_task_response, plan_account_switch_response,
    preview_account_switch_projects_response,
};
use config::config_body;
pub use config::*;
pub use skills::SkillInstallation;

use crate::account_browser::{
    browser_auto_login_batch_item_from_report, open_account_browser_login_with_login_result,
    open_account_browser_login_with_session, open_account_browser_login_with_store_backend,
    open_account_browser_logins_in_store_with_recovery_backend,
};
use crate::account_credentials::credential_report_failure_message;
use crate::account_registration::validate_registration_input;
use crate::{
    account_secret_for_copy_with_backend, add_account_from_login_result_in_store_with_backend,
    add_card_from_line_in_store_with_backend, apply_account_import_with_backend,
    change_account_overleaf_password_from_login_result_in_store_with_backend,
    change_account_overleaf_password_in_store_with_backend, dashboard_summary_with_backend,
    detect_browser_profile_account_in_store, ensure_registration_result_artifacts,
    export_accounts_to_directory_with_backend, export_accounts_to_json_with_backend,
    fetch_address_into_store, generate_account_git_token_with_browser_in_store_with_backend,
    has_reusable_git_token, health, inspect_account_trial_eligibility,
    list_account_summaries_with_backend, list_address_summaries, list_card_summaries,
    manual_cookie_import_candidate, mark_card_failed_in_store_with_backend,
    mark_card_used_in_store_with_backend, parse_account_import_json, plan_account_switch_in_store,
    read_account_password_secret_for_recovery,
    refresh_account_cookie_from_login_result_in_store_with_backend,
    refresh_account_cookie_with_credentials_in_store_with_backend,
    refresh_account_git_token_metadata_with_browser_batch_in_store_with_backend,
    refresh_account_git_token_metadata_with_browser_in_store_with_backend,
    refresh_saved_account_sessions_in_store_with_backend,
    register_account_with_subscription_in_store_with_backend, remove_accounts_locally_in_store,
    remove_card_in_store_with_backend, resolve_account_cookies_for_recovery,
    save_existing_trial_result_in_store_with_backend,
    save_registration_result_in_store_with_backend,
    save_registration_result_without_password_in_store_with_backend, select_address_by_index,
    select_fallback_address_in_store, select_payment_card_with_strategy_and_bin_with_backend_at,
    select_registration_address, update_local_passwords_in_store_with_backend,
    validate_manual_cookie_import_candidates, validate_new_password, AccountActionError,
    AccountCredentialError, AccountExportMode, AccountGitTokenError,
    AccountGitTokenRefreshBatchItem, AccountGitTokenRefreshBatchReport,
    AccountGitTokenRefreshReport, AccountImportCandidate, AccountImportDecisionStatus,
    AccountImportReport, AccountRegistrationError, AccountRegistrationInput,
    AccountRegistrationReport, AccountRegistrationStatus, AccountSecretField,
    AccountSecretStoreError, AccountSessionError, AccountSessionIdentityValidator,
    AccountSessionMetadataRefresher, AccountSessionRefreshBatchItem,
    AccountSessionRefreshBatchReport, AccountSessionRefreshReport, AccountSwitchCommandExecutor,
    AccountSwitchError, AccountSwitchExecutionError, AddressFetchSource, BrowserAutoLoginBatchItem,
    BrowserAutoLoginBatchReport, BrowserAutoLoginError, BrowserAutoLoginReport,
    BrowserBatchCoordinator, BrowserProfileAccountError, BrowserProfileSessionInspector,
    CardSelectionStrategy, CleanupThenRemoveAccountError, CleanupThenRemoveStatus,
    CredentialLoginReport, DashboardRuntimeState, GitTokenRefreshStatus,
    ManualCookieValidationReport, OverleafPasswordChangeReport, ProjectMigrationControl,
    ProjectMigrationError, ProjectMigrationExecutor, ProjectMigrationProgressEvent,
    ProjectMigrationProgressLevel, RegistrationAddressProvider, RegistrationAddressSelection,
    RegistrationAddressSource, RemoteProjectCleanupError, RemoteProjectCleanupExecutor,
    RemoteProjectCleanupPreviewReport, ReqwestAccountSessionIdentityValidator,
    ReqwestAccountSessionMetadataRefresher, ReqwestBrowserProfileSessionInspector,
    ReqwestMeiguodizhiAddressProvider, ReqwestProjectMigrationExecutor,
    ReqwestRemoteProjectCleanupExecutor, ServiceConfig, SubscriptionRefreshStatus,
    TaskAvailableAction, TaskFailureKind, TaskLogLevel, TaskOperationKind, TaskReplaySafety,
    TaskRetryPayload, TaskSnapshot, TaskStateError, TaskStateStore, TaskUserInputKind,
    DEFAULT_BROWSER_BATCH_CONCURRENCY,
};

pub const HEALTH_PATH: &str = "/health";
const ACCOUNT_BROWSER_SESSION_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const SERVICE_API_ROUTES: &[ServiceApiRouteSpec] = &[
    service_route(HEALTH_PATH, &["GET"]),
    service_route("/config", &["GET"]),
    service_route("/config/default-export-dir", &["POST"]),
    service_route("/config/browser-proxy", &["POST"]),
    service_route("/dashboard", &["GET"]),
    service_route("/accounts", &["GET"]),
    service_route("/accounts/import", &["POST"]),
    service_route("/accounts/import/cookie", &["POST"]),
    service_route("/accounts/export", &["POST"]),
    service_route("/accounts/export/json", &["POST"]),
    service_route("/accounts/credentials", &["POST"]),
    service_route("/accounts/register", &["POST"]),
    service_route("/accounts/credentials/refresh", &["POST"]),
    service_route("/accounts/session/refresh", &["POST"]),
    service_route("/accounts/password", &["POST"]),
    service_route("/accounts/overleaf-password", &["POST"]),
    service_route("/accounts/trial-eligibility", &["POST"]),
    service_route("/accounts/git-token/refresh", &["POST"]),
    service_route("/accounts/git-token/generate", &["POST"]),
    service_route("/accounts/browser-login", &["POST"]),
    service_route("/accounts/switch/plan", &["POST"]),
    service_route("/accounts/switch/projects/preview", &["POST"]),
    service_route("/accounts/switch/execute", &["POST"]),
    service_route("/accounts/remove/projects/preview", &["POST"]),
    service_route("/accounts/remove", &["POST"]),
    service_route("/browser/profiles", &["GET"]),
    service_route("/browser/current-account", &["POST"]),
    service_route("/cards", &["GET", "POST"]),
    service_route("/cards/status", &["POST"]),
    service_route("/cards/remove", &["POST"]),
    service_route("/cards/export", &["POST"]),
    service_route("/addresses", &["GET"]),
    service_route("/addresses/current", &["GET"]),
    service_route("/addresses/fetch", &["POST"]),
    service_route("/addresses/select", &["POST"]),
    service_route("/addresses/fallback", &["POST"]),
    service_route("/tasks", &["GET"]),
    service_route("/tasks/summary", &["GET"]),
    service_route("/tasks/cancel", &["POST"]),
    service_route("/tasks/retry", &["POST"]),
    service_route("/tasks/input", &["POST"]),
    service_route("/tasks/clear-terminal", &["POST"]),
    service_route("/runtime/artifacts", &["GET"]),
    service_route("/runtime/updates", &["GET"]),
    service_route("/runtime/updates/prepare", &["POST"]),
    service_route("/runtime/updates/cancel", &["POST"]),
    service_route("/runtime/skills", &["POST"]),
    service_route("/runtime/artifacts/cleanup", &["POST"]),
    service_route("/runtime/temp-profiles/preview", &["GET"]),
    service_route("/runtime/temp-profiles/cleanup", &["POST"]),
    service_route("/secrets/account", &["GET"]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ServiceApiRouteSpec {
    pub path: &'static str,
    pub methods: &'static [&'static str],
}

const fn service_route(
    path: &'static str,
    methods: &'static [&'static str],
) -> ServiceApiRouteSpec {
    ServiceApiRouteSpec { path, methods }
}

#[derive(Clone)]
pub struct ApiState {
    pub config: ServiceConfig,
    pub runtime: DashboardRuntimeState,
    pub tasks: TaskStateStore,
    update_prepared: bool,
    pub browser_automation: Option<Arc<dyn BrowserAutomation + Send + Sync>>,
    pub local_chrome_browser: Option<Arc<LocalChromeBrowserAutomation>>,
    registration_sessions: Arc<StdMutex<BTreeMap<String, RegistrationSessionEntry>>>,
    registration_slot_owner: Arc<StdMutex<Option<String>>>,
    account_browser_sessions: Arc<StdMutex<BTreeMap<String, AccountBrowserSessionEntry>>>,
    browser_batch_coordinator: BrowserBatchCoordinator,
    background_jobs: Arc<StdMutex<VecDeque<ApiBackgroundJob>>>,
    account_commit_lock: Arc<StdMutex<()>>,
    prefetched_registration_address: Arc<StdMutex<Option<RegistrationAddressSelection>>>,
    pub extension_bridge_executor: Option<Arc<dyn AccountSwitchCommandExecutor + Send + Sync>>,
    pub metadata_refresher: Arc<dyn AccountSessionMetadataRefresher + Send + Sync>,
    pub session_identity_validator: Arc<dyn AccountSessionIdentityValidator + Send + Sync>,
    pub browser_profile_inspector: Arc<dyn BrowserProfileSessionInspector + Send + Sync>,
    pub address_provider: Arc<dyn RegistrationAddressProvider + Send + Sync>,
    pub remote_project_cleanup_executor: Arc<dyn RemoteProjectCleanupExecutor + Send + Sync>,
    pub project_migration_executor: Arc<dyn ProjectMigrationExecutor + Send + Sync>,
    pub secret_backend: Arc<dyn SecretBackend + Send + Sync>,
}

type ApiBackgroundRunner = Box<dyn FnOnce(Arc<StdMutex<ApiState>>) + Send + 'static>;

pub struct ApiBackgroundJob {
    name: String,
    runner: Option<ApiBackgroundRunner>,
}

impl ApiBackgroundJob {
    pub fn new(
        name: impl Into<String>,
        runner: impl FnOnce(Arc<StdMutex<ApiState>>) + Send + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            runner: Some(Box::new(runner)),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn run(mut self, state: Arc<StdMutex<ApiState>>) {
        if let Some(runner) = self.runner.take() {
            runner(state);
        }
    }
}

struct RegistrationSessionEntry {
    runtime: tokio::runtime::Runtime,
    session: LocalChromeCdpSession,
    context: RegistrationSessionContext,
    last_activity: Instant,
}

struct AccountBrowserSessionEntry {
    runtime: tokio::runtime::Runtime,
    session: LocalChromeCdpSession,
    context: AccountBrowserSessionContext,
    last_activity: Instant,
}

#[derive(Clone, Copy)]
enum CancelledBrowserSessionKind {
    Registration,
    AccountBrowser,
}

impl CancelledBrowserSessionKind {
    fn job_name(self) -> &'static str {
        match self {
            Self::Registration => "registration-session-cancel-cleanup",
            Self::AccountBrowser => "account-browser-session-cancel-cleanup",
        }
    }

    fn queued_message(self) -> &'static str {
        match self {
            Self::Registration => "注册浏览器会话正在清理",
            Self::AccountBrowser => "账号浏览器会话正在清理",
        }
    }

    fn cleaned_log_message(self) -> &'static str {
        match self {
            Self::Registration => "Registration browser session cleaned after cancel request",
            Self::AccountBrowser => "账号浏览器会话已按用户取消请求清理",
        }
    }

    fn cancelled_message(self) -> &'static str {
        match self {
            Self::Registration => "Registration cancelled and browser session cleaned",
            Self::AccountBrowser => "任务已取消，浏览器会话已清理",
        }
    }
}

enum AccountBrowserSessionContext {
    ExistingTrialCredentials {
        alias_hint: Option<String>,
        email: String,
        password: SecretText,
        trial_days: u32,
        auto_fetch_git_token: bool,
        card_selection_strategy: CardSelectionStrategy,
        card_bin: Option<String>,
        session_restart_count: usize,
        now_unix: i64,
    },
}

enum CredentialRefreshContinuation {
    ExecuteAccountSwitch {
        alias: String,
        migrate_projects: bool,
        now_unix: i64,
    },
    StartExistingTrial {
        alias: String,
        trial_days: u32,
        auto_fetch_git_token: bool,
        card_selection_strategy: CardSelectionStrategy,
        card_bin: Option<String>,
        now_unix: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingTrialContinuation {
    Purchase,
    PurchasePageReady,
    ResumeActiveTrial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExistingTrialEligibilityEvidence {
    eligibility: TrialEligibility,
    plan_availability: TrialPlanAvailability,
    subscription_state: SubscriptionState,
    trial_expiry: Option<i64>,
}

#[derive(Debug)]
enum ExistingTrialResolutionError {
    NotEligible(TrialEligibility),
    Browser(BrowserAutomationError),
}

struct RegistrationSessionContext {
    alias_hint: Option<String>,
    existing_alias: Option<String>,
    email: String,
    password: Option<SecretText>,
    trial_days: u32,
    auto_fetch_git_token: bool,
    card_selection_strategy: CardSelectionStrategy,
    card_bin: Option<String>,
}

impl ApiState {
    pub fn from_environment() -> Self {
        let config = ServiceConfig::from_environment();
        let local_chrome_browser = default_local_chrome_automation(&config);
        let browser_automation = local_chrome_browser
            .as_ref()
            .map(|automation| Arc::clone(automation) as Arc<dyn BrowserAutomation + Send + Sync>);
        Self {
            config,
            runtime: DashboardRuntimeState::default(),
            tasks: TaskStateStore::default(),
            update_prepared: false,
            browser_automation,
            local_chrome_browser,
            registration_sessions: Arc::new(StdMutex::new(BTreeMap::new())),
            registration_slot_owner: Arc::new(StdMutex::new(None)),
            account_browser_sessions: Arc::new(StdMutex::new(BTreeMap::new())),
            browser_batch_coordinator: BrowserBatchCoordinator::default(),
            background_jobs: Arc::new(StdMutex::new(VecDeque::new())),
            account_commit_lock: Arc::new(StdMutex::new(())),
            prefetched_registration_address: Arc::new(StdMutex::new(None)),
            extension_bridge_executor: default_extension_bridge_executor(),
            metadata_refresher: Arc::new(ReqwestAccountSessionMetadataRefresher),
            session_identity_validator: Arc::new(ReqwestAccountSessionIdentityValidator),
            browser_profile_inspector: Arc::new(ReqwestBrowserProfileSessionInspector),
            address_provider: Arc::new(ReqwestMeiguodizhiAddressProvider),
            remote_project_cleanup_executor: Arc::new(ReqwestRemoteProjectCleanupExecutor),
            project_migration_executor: Arc::new(ReqwestProjectMigrationExecutor),
            secret_backend: Arc::new(SystemKeyringSecretBackend::default()),
        }
        .with_startup_address_prefetch()
    }

    fn with_startup_address_prefetch(self) -> Self {
        let provider = Arc::clone(&self.address_provider);
        let store = AddressStore::new(self.config.addresses_path());
        let cache = Arc::clone(&self.prefetched_registration_address);
        let selector = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as usize)
            .unwrap_or_default();

        let _ = std::thread::Builder::new()
            .name("overleaf-address-prefetch".to_string())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };

                if let Ok(report) = runtime.block_on(fetch_address_into_store(
                    provider.as_ref(),
                    &store,
                    selector,
                )) {
                    let source = match report.source {
                        AddressFetchSource::Api => RegistrationAddressSource::Api,
                        AddressFetchSource::Fallback => RegistrationAddressSource::Fallback,
                    };
                    let selection = RegistrationAddressSelection {
                        source,
                        fallback_index: (source == RegistrationAddressSource::Fallback)
                            .then_some(report.index),
                        address: report.address,
                        api_error: report.api_error,
                    };
                    if let Ok(mut cached) = cache.lock() {
                        if cached.is_none() {
                            *cached = Some(selection);
                        }
                    }
                }
            });

        self
    }

    pub fn browser_batch_coordinator(&self) -> BrowserBatchCoordinator {
        self.browser_batch_coordinator.clone()
    }

    pub fn enqueue_background_job(&self, job: ApiBackgroundJob) -> Result<(), ApiResponse> {
        self.background_jobs
            .lock()
            .map_err(|_| {
                json_response(
                    500,
                    &ApiErrorBody {
                        error: "background job queue lock poisoned".to_string(),
                    },
                )
            })?
            .push_back(job);
        Ok(())
    }

    pub fn take_background_jobs(&self) -> Vec<ApiBackgroundJob> {
        self.background_jobs
            .lock()
            .map(|mut jobs| jobs.drain(..).collect())
            .unwrap_or_default()
    }

    pub fn account_commit_lock(&self) -> Arc<StdMutex<()>> {
        Arc::clone(&self.account_commit_lock)
    }

    fn take_prefetched_registration_address(&self) -> Option<RegistrationAddressSelection> {
        self.prefetched_registration_address
            .lock()
            .ok()
            .and_then(|mut cached| cached.take())
    }

    fn current_registration_address(&self) -> Option<RegistrationAddressSelection> {
        self.prefetched_registration_address
            .lock()
            .ok()
            .and_then(|cached| cached.clone())
    }

    fn set_prefetched_registration_address(&self, selection: RegistrationAddressSelection) {
        if let Ok(mut cached) = self.prefetched_registration_address.lock() {
            *cached = Some(selection);
        }
    }

    fn registration_session_count(&self) -> usize {
        self.registration_sessions
            .lock()
            .map(|sessions| sessions.len())
            .unwrap_or_default()
    }
}

fn default_local_chrome_automation(
    config: &ServiceConfig,
) -> Option<Arc<LocalChromeBrowserAutomation>> {
    let chrome_executable = detect_chrome_executable()?;
    Some(Arc::new(LocalChromeBrowserAutomation::new(
        LocalChromeAutomationConfig::new(chrome_executable, config.tmp_dir.clone())
            .with_proxy_policy(config.browser_proxy_policy.clone()),
    )))
}

fn default_extension_bridge_executor() -> Option<Arc<dyn AccountSwitchCommandExecutor + Send + Sync>>
{
    ExtensionBridgeWebSocketServer::from_environment()
        .ok()
        .map(|executor| Arc::new(executor) as Arc<dyn AccountSwitchCommandExecutor + Send + Sync>)
}

#[async_trait::async_trait]
impl AccountSwitchCommandExecutor for ExtensionBridgeWebSocketServer {
    async fn execute_extension_command(
        &self,
        command: ExtensionCommand,
    ) -> Result<ExtensionResponse, AccountSwitchExecutionError> {
        self.execute_command(command)
            .await
            .map_err(|error| AccountSwitchExecutionError::Bridge {
                message: error.to_string(),
            })
    }

    async fn extension_client_count(&self) -> Option<usize> {
        Some(self.connected_clients().await)
    }
}

impl fmt::Debug for ApiState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiState")
            .field("config", &self.config)
            .field("runtime", &self.runtime)
            .field("tasks", &self.tasks)
            .field(
                "browser_automation",
                &self.browser_automation.as_ref().map(|_| "<configured>"),
            )
            .field(
                "local_chrome_browser",
                &self.local_chrome_browser.as_ref().map(|_| "<configured>"),
            )
            .field("registration_sessions", &self.registration_session_count())
            .field(
                "registration_slot_active",
                &self
                    .registration_slot_owner
                    .lock()
                    .map(|owner| owner.is_some())
                    .unwrap_or(false),
            )
            .field(
                "extension_bridge_executor",
                &self
                    .extension_bridge_executor
                    .as_ref()
                    .map(|_| "<configured>"),
            )
            .field("metadata_refresher", &"<configured>")
            .field("browser_profile_inspector", &"<configured>")
            .field("address_provider", &"<configured>")
            .field("remote_project_cleanup_executor", &"<configured>")
            .field("project_migration_executor", &"<configured>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    pub status_code: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl ApiResponse {
    pub fn status_text(&self) -> &'static str {
        match self.status_code {
            200 => "OK",
            202 => "Accepted",
            204 => "No Content",
            400 => "Bad Request",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "OK",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiErrorBody {
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ApiTypedErrorBody {
    error: String,
    kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountSecretBody {
    pub alias: String,
    pub field: AccountSecretField,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct CredentialAddRequest {
    aliases: String,
    emails: String,
    passwords: String,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct CredentialRefreshRequest {
    aliases: String,
    passwords: String,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct RegistrationStartRequest {
    #[serde(default)]
    alias: Option<String>,
    #[serde(default)]
    existing_alias: Option<String>,
    #[serde(default)]
    existing_login_source: Option<ExistingTrialLoginSource>,
    #[serde(default)]
    session_cookie: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    password: String,
    trial_days: u32,
    #[serde(default = "default_auto_fetch_git_token")]
    auto_fetch_git_token: bool,
    #[serde(default)]
    card_selection_strategy: CardSelectionStrategy,
    #[serde(default)]
    card_bin: String,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExistingTrialLoginSource {
    Alias,
    Credentials,
    Cookie,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct RegistrationCredentialsTaskInput {
    #[serde(default)]
    alias: Option<String>,
    email: String,
    password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct BrowserCredentialsTaskInput {
    #[serde(default)]
    email: Option<String>,
    password: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
struct CredentialBatchReport {
    imported_count: usize,
    updated_count: usize,
    skipped_duplicate_email_count: usize,
    failed_count: usize,
    items: Vec<CredentialBatchItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CredentialBatchItem {
    alias: String,
    email: String,
    status: CredentialBatchItemStatus,
    existing_alias: Option<String>,
    report: Option<CredentialLoginReport>,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CredentialBatchItemStatus {
    Imported,
    Updated,
    SkippedDuplicateEmail,
    Failed,
}

struct CredentialTaskProgress<'a> {
    tasks: &'a mut TaskStateStore,
    task_id: &'a str,
    total: usize,
}

impl<'a> CredentialTaskProgress<'a> {
    fn new(tasks: &'a mut TaskStateStore, task_id: &'a str, total: usize) -> Self {
        Self {
            tasks,
            task_id,
            total: total.max(1),
        }
    }

    fn started(&mut self, index: usize, alias: &str, action: &str) {
        self.record(
            index.saturating_sub(1),
            TaskLogLevel::Info,
            format!("账号 {index}/{}（{alias}）：{action}", self.total),
        );
    }

    fn finished(&mut self, index: usize, alias: &str, level: TaskLogLevel, detail: &str) {
        self.record(
            index,
            level,
            format!("账号 {index}/{}（{alias}）：{detail}", self.total),
        );
    }

    fn record(&mut self, current: usize, level: TaskLogLevel, message: String) {
        let total = task_progress_total(self.total);
        let current = (current.min(total as usize)) as u32;
        let _ = self
            .tasks
            .set_progress(self.task_id, current, total, message.clone());
        let _ = self.tasks.append_log(self.task_id, level, message);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct LocalPasswordUpdateRequest {
    aliases: String,
    passwords: String,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct OverleafPasswordChangeRequest {
    aliases: String,
    passwords: String,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct OverleafPasswordChangeBatchReport {
    changed_count: usize,
    failed_count: usize,
    items: Vec<OverleafPasswordChangeBatchItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct OverleafPasswordChangeBatchItem {
    alias: String,
    email: Option<String>,
    password_updated: bool,
    report: Option<OverleafPasswordChangeReport>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AccountRemovalRequest {
    aliases: String,
    #[serde(default)]
    cleanup_remote_projects: bool,
    #[serde(default)]
    confirm_local_only: bool,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RemoteCleanupPreviewBatchReport {
    previewed_count: usize,
    failed_count: usize,
    total_projects: usize,
    delete_count: usize,
    leave_count: usize,
    items: Vec<RemoteCleanupPreviewBatchItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RemoteCleanupPreviewBatchItem {
    alias: String,
    report: Option<RemoteProjectCleanupPreviewReport>,
    error: Option<RemoteProjectCleanupError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RemoteCleanupRemovalBatchReport {
    removed_count: usize,
    incomplete_count: usize,
    failed_count: usize,
    items: Vec<RemoteCleanupRemovalBatchItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RemoteCleanupRemovalBatchItem {
    alias: String,
    status: RemoteCleanupRemovalBatchStatus,
    report: Option<crate::CleanupThenRemoveAccountReport>,
    error: Option<CleanupThenRemoveAccountError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RemoteCleanupRemovalBatchStatus {
    RemovedLocally,
    RemoteCleanupIncomplete,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct BrowserCurrentAccountRequest {
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountImportRequest {
    path: Option<String>,
    json: Option<String>,
    #[serde(default)]
    refresh_session_metadata: bool,
    #[serde(default)]
    fetch_git_token: bool,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AccountManualCookieImportRequest {
    #[serde(default)]
    entries: Vec<AccountManualCookieImportEntryRequest>,
    #[serde(default)]
    refresh_session_metadata: bool,
    #[serde(default)]
    fetch_git_token: bool,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AccountManualCookieImportEntryRequest {
    #[serde(default)]
    alias: Option<String>,
    email: String,
    cookie: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AccountImportWithPostActionsReport {
    import: AccountImportReport,
    session_refresh: Option<AccountSessionRefreshBatchReport>,
    git_token_generation: Option<AccountGitTokenRefreshBatchReport>,
    post_action_errors: Vec<AccountImportPostActionError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ManualCookieImportBatchReport {
    import: AccountImportReport,
    validation: ManualCookieValidationReport,
    session_refresh: Option<AccountSessionRefreshBatchReport>,
    git_token_generation: Option<AccountGitTokenRefreshBatchReport>,
    post_action_errors: Vec<AccountImportPostActionError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AccountImportPostActionError {
    action: AccountImportPostAction,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AccountImportPostAction {
    RefreshSessionMetadata,
    FetchGitToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AccountImportPostActionOptions {
    refresh_session_metadata: bool,
    fetch_git_token: bool,
}

#[derive(Clone)]
enum AccountImportPostActionSeed {
    Accounts(AccountImportReport),
    ManualCookie {
        import: AccountImportReport,
        validation: ManualCookieValidationReport,
    },
}

impl AccountImportPostActionSeed {
    fn import(&self) -> &AccountImportReport {
        match self {
            Self::Accounts(import) | Self::ManualCookie { import, .. } => import,
        }
    }
}

struct AccountImportPostActionBatchContext {
    seed: AccountImportPostActionSeed,
    options: AccountImportPostActionOptions,
    imported_aliases: Vec<String>,
    retry_session_aliases: BTreeSet<String>,
    initial_session_refresh: Option<AccountSessionRefreshBatchReport>,
    session_retry_report: Option<AccountSessionRefreshBatchReport>,
    git_token_generation: Option<AccountGitTokenRefreshBatchReport>,
    post_action_errors: Vec<AccountImportPostActionError>,
    now_unix: i64,
}

const IMPORT_SESSION_METADATA_FRESH_SECONDS: f64 = 24.0 * 60.0 * 60.0;

impl AccountImportPostActionOptions {
    const fn any(self) -> bool {
        self.refresh_session_metadata || self.fetch_git_token
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AccountExportRequest {
    aliases: String,
    mode: AccountExportMode,
    output_dir: String,
    #[serde(default)]
    confirm_overwrite: bool,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AccountExportJsonRequest {
    aliases: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct DefaultExportDirRequest {
    default_export_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct BrowserProxyRequest {
    mode: ChromeProxyMode,
    #[serde(default)]
    server: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AliasBatchRequest {
    aliases: String,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedAliasBatchRequest {
    aliases: Vec<String>,
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct TrialEligibilityRequest {
    aliases: String,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default = "default_trial_eligibility_days")]
    trial_days: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedTrialEligibilityRequest {
    aliases: Vec<String>,
    task_id: Option<String>,
    trial_days: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct TaskCancelRequest {
    task_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct TaskRetryRequest {
    task_id: String,
    #[serde(default)]
    new_task_id: Option<String>,
    #[serde(default)]
    confirm_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct TaskInputRequest {
    task_id: String,
    #[serde(default)]
    item_id: Option<String>,
    kind: TaskUserInputKind,
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TaskClearTerminalReport {
    removed_count: usize,
    removed_task_ids: Vec<String>,
    remaining_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TaskSummary {
    total_count: usize,
    pending_count: usize,
    running_count: usize,
    waiting_for_user_count: usize,
    completed_count: usize,
    failed_count: usize,
    cancelled_count: usize,
    active_count: usize,
    terminal_count: usize,
    clearable_count: usize,
    cancel_requested_count: usize,
    waiting_for_input_count: usize,
    unsupported_waiting_input_count: usize,
    recoverable_failed_count: usize,
    waiting_for_input_by_kind: BTreeMap<&'static str, usize>,
    failed_by_kind: BTreeMap<&'static str, usize>,
    available_actions_count: BTreeMap<&'static str, usize>,
    locked_alias_count: usize,
    latest_sequence: u64,
}

fn default_auto_fetch_git_token() -> bool {
    true
}

pub fn handle_api_request_with_body_mut(
    state: &mut ApiState,
    method: &str,
    target: &str,
    body: &str,
    now_unix: i64,
) -> ApiResponse {
    let path = normalize_path(target);
    if method == "OPTIONS" {
        return ApiResponse {
            status_code: 204,
            content_type: "text/plain; charset=utf-8",
            body: String::new(),
        };
    }

    reconcile_browser_sessions(state, now_unix);

    if method == "POST" && path == "/runtime/updates/cancel" {
        state.update_prepared = false;
        return json_response(200, &serde_json::json!({ "ready": false }));
    }
    if method == "POST" && state.update_prepared {
        return json_response(
            503,
            &ApiErrorBody {
                error: "应用正在更新，请稍后重试".to_string(),
            },
        );
    }
    if method == "POST" && path == "/runtime/updates/prepare" {
        let busy = state.tasks.list_snapshots().iter().any(|task| {
            matches!(
                task.phase,
                ServiceTaskPhase::Pending
                    | ServiceTaskPhase::Running
                    | ServiceTaskPhase::WaitingForUser
            )
        });
        if busy {
            return json_response(
                409,
                &ApiErrorBody {
                    error: "请先完成或取消运行中及等待验证的任务".to_string(),
                },
            );
        }
        state.update_prepared = true;
        return json_response(200, &serde_json::json!({ "ready": true }));
    }

    let response = match (method, path.as_str()) {
        ("POST", "/accounts/import") => import_accounts_response(state, body, now_unix),
        ("POST", "/accounts/import/cookie") => {
            import_manual_cookie_accounts_response(state, body, now_unix)
        }
        ("POST", "/accounts/export") => export_accounts_response(state, body),
        ("POST", "/accounts/export/json") => export_accounts_json_response(state, body),
        ("POST", "/config/default-export-dir") => update_default_export_dir_response(state, body),
        ("POST", "/config/browser-proxy") => update_browser_proxy_response(state, body),
        ("POST", "/accounts/credentials") => add_credentials_response(state, body, now_unix),
        ("POST", "/accounts/register") => register_account_response(state, body, now_unix),
        ("POST", "/accounts/credentials/refresh") => {
            refresh_credentials_response(state, body, now_unix)
        }
        ("POST", "/accounts/session/refresh") => {
            refresh_account_subscriptions_response(state, body, now_unix)
        }
        ("POST", "/accounts/password") => update_local_password_response(state, body),
        ("POST", "/accounts/overleaf-password") => {
            change_overleaf_password_response(state, body, now_unix)
        }
        ("POST", "/accounts/trial-eligibility") => {
            inspect_account_trial_eligibility_response(state, body, now_unix)
        }
        ("POST", "/accounts/git-token/refresh") => {
            refresh_account_git_token_response(state, body, now_unix)
        }
        ("POST", "/accounts/git-token/generate") => {
            generate_account_git_token_response(state, body, now_unix)
        }
        ("POST", "/browser/current-account") => {
            detect_browser_current_account_response(state, body)
        }
        ("POST", "/accounts/browser-login") => browser_auto_login_response(state, body, now_unix),
        ("POST", "/accounts/switch/plan") => plan_account_switch_response(state, body),
        ("POST", "/accounts/switch/projects/preview") => {
            preview_account_switch_projects_response(state, body)
        }
        ("POST", "/accounts/switch/execute") => {
            execute_account_switch_response(state, body, now_unix)
        }
        ("POST", "/accounts/remove/projects/preview") => {
            preview_remote_cleanup_response(state, body)
        }
        ("POST", "/accounts/remove") => remove_accounts_response(state, body),
        ("POST", "/cards") => add_card_response(state, body),
        ("POST", "/cards/status") => update_card_status_response(state, body),
        ("POST", "/cards/remove") => remove_card_response(state, body),
        ("POST", "/cards/export") => export_cards_response(state, body),
        ("POST", "/runtime/skills") => skills::manage_response(state, body),
        ("POST", "/addresses/fetch") => fetch_address_response(state, now_unix),
        ("POST", "/addresses/select") => select_address_response(state, body),
        ("POST", "/addresses/fallback") => select_fallback_address_response(state, body),
        ("POST", "/tasks/cancel") => cancel_task_response(state, body),
        ("POST", "/tasks/retry") => retry_task_response(state, body, now_unix),
        ("POST", "/tasks/input") => task_input_response(state, body, now_unix),
        ("POST", "/tasks/clear-terminal") => clear_terminal_tasks_response(state),
        ("POST", "/runtime/artifacts/cleanup") => cleanup_runtime_artifacts_response(state, body),
        ("POST", "/runtime/temp-profiles/cleanup") => cleanup_temp_profiles_response(state, body),
        _ if method != "GET" && method != "HEAD" => method_not_allowed_response(),
        (_, path)
            if SERVICE_API_ROUTES
                .iter()
                .any(|route| route.path == path && !route.methods.contains(&"GET")) =>
        {
            method_not_allowed_response()
        }
        (_, "/" | HEALTH_PATH) => json_response(200, &json!({ "status": health() })),
        (_, "/config") => json_response(200, &config_body(state)),
        (_, "/browser/profiles") => {
            json_response(200, &discover_chrome_profiles_from_environment())
        }
        (_, "/dashboard") => dashboard_response(state, now_unix),
        (_, "/accounts") => accounts_response(state, now_unix),
        (_, "/cards") => cards_response(state),
        (_, "/addresses") => addresses_response(state),
        (_, "/addresses/current") => json_response(200, &state.current_registration_address()),
        (_, "/tasks/summary") => json_response(200, &task_summary(state)),
        (_, "/tasks") => tasks_response(state, target),
        (_, "/runtime/artifacts") => json_response(200, &runtime_artifact_status(state)),
        (_, "/runtime/temp-profiles/preview") => preview_temp_profiles_response(state),
        (_, "/secrets/account") => account_secret_response(state, target),
        _ => json_response(
            404,
            &ApiErrorBody {
                error: "not found".to_string(),
            },
        ),
    };

    if method == "HEAD" {
        ApiResponse {
            body: String::new(),
            ..response
        }
    } else {
        response
    }
}

fn dashboard_response(state: &ApiState, now_unix: i64) -> ApiResponse {
    match load_display_accounts(state) {
        Ok(document) => json_response(
            200,
            &dashboard_summary_with_backend(
                &document,
                runtime_state(state),
                now_unix,
                state.secret_backend.as_ref(),
            ),
        ),
        Err(error) => io_error_response(error),
    }
}

fn accounts_response(state: &ApiState, now_unix: i64) -> ApiResponse {
    match load_display_accounts(state) {
        Ok(document) => json_response(
            200,
            &list_account_summaries_with_backend(
                &document,
                now_unix,
                state.secret_backend.as_ref(),
            ),
        ),
        Err(error) => io_error_response(error),
    }
}

fn tasks_response(state: &ApiState, target: &str) -> ApiResponse {
    let Some(since) = query_param(target, "since") else {
        return json_response(200, &state.tasks.list_snapshots());
    };
    let since = since.trim();
    if since.is_empty() {
        return json_response(
            400,
            &ApiErrorBody {
                error: "invalid since".to_string(),
            },
        );
    }
    match since.parse::<u64>() {
        Ok(sequence) => json_response(200, &state.tasks.list_snapshots_since(sequence)),
        Err(_) => json_response(
            400,
            &ApiErrorBody {
                error: "invalid since".to_string(),
            },
        ),
    }
}

fn task_summary(state: &ApiState) -> TaskSummary {
    let snapshots = state.tasks.list_snapshots();
    let mut summary = TaskSummary {
        total_count: snapshots.len(),
        pending_count: 0,
        running_count: 0,
        waiting_for_user_count: 0,
        completed_count: 0,
        failed_count: 0,
        cancelled_count: 0,
        active_count: 0,
        terminal_count: 0,
        clearable_count: 0,
        cancel_requested_count: 0,
        waiting_for_input_count: 0,
        unsupported_waiting_input_count: 0,
        recoverable_failed_count: 0,
        waiting_for_input_by_kind: BTreeMap::new(),
        failed_by_kind: BTreeMap::new(),
        available_actions_count: BTreeMap::new(),
        locked_alias_count: 0,
        latest_sequence: 0,
    };
    let mut locked_aliases = BTreeSet::new();

    for task in snapshots {
        summary.latest_sequence = summary.latest_sequence.max(task.last_sequence);
        if task.cancel_requested {
            summary.cancel_requested_count += 1;
        }
        for waiting_item in &task.waiting_items {
            summary.waiting_for_input_count += 1;
            *summary
                .waiting_for_input_by_kind
                .entry(task_user_input_kind_name(waiting_item.kind))
                .or_default() += 1;
        }
        for action in &task.available_actions {
            *summary
                .available_actions_count
                .entry(task_available_action_name(*action))
                .or_default() += 1;
        }
        match task.phase {
            crate::ServiceTaskPhase::Pending => {
                summary.pending_count += 1;
                summary.active_count += 1;
                locked_aliases.extend(task.locked_aliases);
            }
            crate::ServiceTaskPhase::Running => {
                summary.running_count += 1;
                summary.active_count += 1;
                locked_aliases.extend(task.locked_aliases);
            }
            crate::ServiceTaskPhase::WaitingForUser => {
                summary.waiting_for_user_count += 1;
                summary.active_count += 1;
                if let Some(kind) = task.waiting_for_input {
                    summary.waiting_for_input_count += 1;
                    *summary
                        .waiting_for_input_by_kind
                        .entry(task_user_input_kind_name(kind))
                        .or_default() += 1;
                } else if task.waiting_items.is_empty() {
                    summary.unsupported_waiting_input_count += 1;
                }
                locked_aliases.extend(task.locked_aliases);
            }
            crate::ServiceTaskPhase::Completed => {
                summary.completed_count += 1;
                summary.terminal_count += 1;
            }
            crate::ServiceTaskPhase::Failed => {
                summary.terminal_count += 1;
                if task.resolved_by.is_some() {
                    continue;
                }
                summary.failed_count += 1;
                if let Some(kind) = task.failure_kind {
                    *summary
                        .failed_by_kind
                        .entry(task_failure_kind_name(kind))
                        .or_default() += 1;
                }
                if task.available_actions.contains(&TaskAvailableAction::Retry) {
                    summary.recoverable_failed_count += 1;
                }
            }
            crate::ServiceTaskPhase::Cancelled => {
                summary.cancelled_count += 1;
                summary.terminal_count += 1;
            }
        }
    }

    summary.clearable_count = summary.terminal_count;
    summary.locked_alias_count = locked_aliases.len();
    summary
}

fn task_failure_kind_name(kind: TaskFailureKind) -> &'static str {
    match kind {
        TaskFailureKind::Retryable => "retryable",
        TaskFailureKind::NeedsUserInput => "needs_user_input",
        TaskFailureKind::NeedsLogin => "needs_login",
        TaskFailureKind::PageChanged => "page_changed",
        TaskFailureKind::Fatal => "fatal",
    }
}

fn task_available_action_name(action: TaskAvailableAction) -> &'static str {
    action.as_str()
}

fn task_user_input_kind_name(kind: TaskUserInputKind) -> &'static str {
    kind.as_str()
}

fn clear_terminal_tasks_response(state: &mut ApiState) -> ApiResponse {
    let removed_task_ids = state.tasks.clear_terminal_tasks();
    let report = TaskClearTerminalReport {
        removed_count: removed_task_ids.len(),
        removed_task_ids,
        remaining_count: state.tasks.list_snapshots().len(),
    };
    json_response(200, &report)
}

fn inspect_account_trial_eligibility_response(
    state: &mut ApiState,
    body: &str,
    now_unix: i64,
) -> ApiResponse {
    let request = match parse_trial_eligibility_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if let Err(response) = start_alias_batch_task(
        state,
        request.task_id.as_deref(),
        "检查账号试用资格",
        &request.aliases,
    ) {
        return response;
    }

    let store = AccountStore::new(state.config.accounts_path());
    let backend = state.secret_backend.clone();
    if let Some(task_id) = request.task_id {
        let owned_task_id = task_id.clone();
        let job = ApiBackgroundJob::new("trial-eligibility", move |shared_state| {
            let result = block_on_api(
                crate::account_session::inspect_saved_account_trial_eligibility_with_progress(
                    &store,
                    &request.aliases,
                    request.trial_days,
                    now_unix,
                    backend.as_ref(),
                    |current, total| {
                        let Ok(mut state) = shared_state.lock() else {
                            return false;
                        };
                        let _ = state.tasks.set_progress_value(
                            &owned_task_id,
                            current as u32,
                            total as u32,
                        );
                        state
                            .tasks
                            .snapshot(&owned_task_id)
                            .is_ok_and(|task| !task.cancel_requested)
                    },
                ),
            )
            .and_then(|result| result.map_err(account_session_error_response));
            if let Ok(mut state) = shared_state.lock() {
                if state
                    .tasks
                    .snapshot(&owned_task_id)
                    .is_ok_and(|task| task.cancel_requested)
                {
                    let _ = state
                        .tasks
                        .cancel_task(&owned_task_id, "试用资格检查已取消");
                } else {
                    finish_trial_eligibility_response(&mut state, Some(&owned_task_id), result);
                }
            }
        });
        if let Err(response) = state.enqueue_background_job(job) {
            fail_tracked_task(state, Some(&task_id), response.body.clone());
            return response;
        }
        return match state.tasks.snapshot(&task_id) {
            Ok(snapshot) => json_response(202, &snapshot),
            Err(error) => task_state_error_response(error),
        };
    }

    let result = block_on_api(
        crate::account_session::inspect_saved_account_trial_eligibility_with_backend(
            &store,
            &request.aliases,
            request.trial_days,
            now_unix,
            backend.as_ref(),
        ),
    )
    .and_then(|result| result.map_err(account_session_error_response));
    finish_trial_eligibility_response(state, None, result)
}

fn finish_trial_eligibility_response(
    state: &mut ApiState,
    task_id: Option<&str>,
    result: Result<crate::AccountTrialEligibilityBatchReport, ApiResponse>,
) -> ApiResponse {
    match result {
        Ok(report) => {
            if let Some(task_id) = task_id {
                for item in &report.items {
                    if let Some(error) = &item.error {
                        let _ = state.tasks.append_log(
                            task_id,
                            TaskLogLevel::Error,
                            format!("{}：{}", item.alias, account_session_error_message(error)),
                        );
                    }
                }
            }
            if report.failed_count > 0 {
                let message = format!(
                    "试用资格检查完成：{} 个成功，{} 个失败",
                    report.items.len().saturating_sub(report.failed_count),
                    report.failed_count
                );
                fail_tracked_task_with_result(state, task_id, &message, &report);
            } else {
                complete_tracked_task(state, task_id, "账号试用资格检查完成", &report);
            }
            json_response(200, &report)
        }
        Err(response) => {
            fail_tracked_task(state, task_id, response.body.clone());
            response
        }
    }
}

fn refresh_account_git_token_response(
    state: &mut ApiState,
    body: &str,
    now_unix: i64,
) -> ApiResponse {
    let request = match parse_alias_batch_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if let Err(response) = start_alias_batch_task(
        state,
        request.task_id.as_deref(),
        "刷新 Git Integration token 状态",
        &request.aliases,
    ) {
        return response;
    }

    if let (Some(task_id), Some(local_chrome)) = (
        request.task_id.as_deref(),
        state.local_chrome_browser.clone(),
    ) {
        return queue_git_token_refresh_browser_session_response(
            state,
            task_id,
            request.aliases,
            local_chrome,
            now_unix,
        );
    }

    let Some(browser) = state.browser_automation.clone() else {
        return browser_not_configured_response(state, request.task_id.as_deref());
    };
    let store = AccountStore::new(state.config.accounts_path());
    match block_on_api(
        refresh_account_git_token_metadata_with_browser_batch_in_store_with_backend(
            &store,
            &request.aliases,
            browser.as_ref(),
            state.secret_backend.as_ref(),
        ),
    ) {
        Ok(Ok(report)) => {
            append_git_token_batch_logs(
                state,
                request.task_id.as_deref(),
                &report,
                "Git 令牌状态刷新",
            );
            finish_git_token_batch_task(
                state,
                request.task_id.as_deref(),
                &report,
                CredentialBrowserOperation::GitTokenRefresh,
            );
            json_response(200, &report)
        }
        Ok(Err(error)) => {
            fail_tracked_task(
                state,
                request.task_id.as_deref(),
                account_git_token_error_message(&error),
            );
            account_git_token_error_response(error)
        }
        Err(response) => {
            fail_tracked_task(state, request.task_id.as_deref(), response.body.clone());
            response
        }
    }
}

fn generate_account_git_token_response(
    state: &mut ApiState,
    body: &str,
    now_unix: i64,
) -> ApiResponse {
    let request = match parse_alias_batch_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if let Err(response) = start_alias_batch_task(
        state,
        request.task_id.as_deref(),
        "获取 Git Integration token",
        &request.aliases,
    ) {
        return response;
    }

    if let (Some(task_id), Some(local_chrome)) = (
        request.task_id.as_deref(),
        state.local_chrome_browser.clone(),
    ) {
        return queue_git_token_browser_session_response(
            state,
            task_id,
            request.aliases,
            local_chrome,
            now_unix,
        );
    }

    let Some(browser) = state.browser_automation.clone() else {
        return browser_not_configured_response(state, request.task_id.as_deref());
    };

    let store = AccountStore::new(state.config.accounts_path());
    match block_on_api(
        generate_account_git_token_with_browser_in_store_with_backend(
            &store,
            &request.aliases,
            browser.as_ref(),
            now_unix,
            state.secret_backend.as_ref(),
        ),
    ) {
        Ok(Ok(report)) => {
            append_git_token_batch_logs(state, request.task_id.as_deref(), &report, "Git 令牌获取");
            finish_git_token_batch_task(
                state,
                request.task_id.as_deref(),
                &report,
                CredentialBrowserOperation::GitToken,
            );
            json_response(200, &report)
        }
        Ok(Err(error)) => {
            fail_tracked_task(
                state,
                request.task_id.as_deref(),
                account_git_token_error_message(&error),
            );
            account_git_token_error_response(error)
        }
        Err(response) => {
            fail_tracked_task(state, request.task_id.as_deref(), response.body.clone());
            response
        }
    }
}

fn queue_browser_window_batch_response(
    state: &mut ApiState,
    task_id: &str,
    aliases: Vec<String>,
    browser: Arc<dyn BrowserAutomation + Send + Sync>,
    now_unix: i64,
) -> ApiResponse {
    let store = AccountStore::new(state.config.accounts_path());
    if let Err(error) = store.load() {
        let error = BrowserAutoLoginError::Io {
            message: error.to_string(),
        };
        fail_tracked_task(
            state,
            Some(task_id),
            browser_auto_login_error_message(&error),
        );
        return browser_auto_login_error_response(error);
    }
    let coordinator = state.browser_batch_coordinator();
    let registered = match coordinator.register_batch(task_id, aliases) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let message = format!("failed to register browser window batch: {error:?}");
            fail_tracked_task(state, Some(task_id), message.clone());
            return json_response(500, &ApiErrorBody { error: message });
        }
    };
    let ordered_items = registered
        .items
        .iter()
        .map(|item| (item.item_id.clone(), item.alias.clone()))
        .collect::<Vec<_>>();
    let accumulator = Arc::new(StdMutex::new(BrowserWindowBatchAccumulator {
        ordered_items,
        items: BTreeMap::new(),
        finalized: false,
    }));
    let job = BrowserWindowBatchJob {
        task_id: task_id.to_string(),
        store,
        browser,
        metadata_refresher: Arc::clone(&state.metadata_refresher),
        secret_backend: Arc::clone(&state.secret_backend),
        now_unix,
        recovery_lock: Arc::new(tokio::sync::Mutex::new(())),
        coordinator: coordinator.clone(),
        accumulator,
    };
    let background_job = ApiBackgroundJob::new("browser-window-batch", move |shared_state| {
        job.run(shared_state);
    });
    if let Err(response) = state.enqueue_background_job(background_job) {
        let _ = coordinator.remove_batch(task_id);
        fail_tracked_task(state, Some(task_id), response.body.clone());
        return response;
    }
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Info,
        format!(
            "浏览器窗口批次已进入有界调度，最多同时启动 {} 个窗口",
            registered.max_concurrency
        ),
    );
    registration_waiting_response(state, task_id)
}

struct BrowserWindowBatchAccumulator {
    ordered_items: Vec<(String, String)>,
    items: BTreeMap<String, BrowserAutoLoginBatchItem>,
    finalized: bool,
}

struct BrowserWindowBatchJob {
    task_id: String,
    store: AccountStore,
    browser: Arc<dyn BrowserAutomation + Send + Sync>,
    metadata_refresher: Arc<dyn AccountSessionMetadataRefresher + Send + Sync>,
    secret_backend: Arc<dyn SecretBackend + Send + Sync>,
    now_unix: i64,
    recovery_lock: Arc<tokio::sync::Mutex<()>>,
    coordinator: BrowserBatchCoordinator,
    accumulator: Arc<StdMutex<BrowserWindowBatchAccumulator>>,
}

#[derive(Clone)]
struct BrowserWindowItemJob {
    task_id: String,
    item_id: String,
    alias: String,
    store: AccountStore,
    browser: Arc<dyn BrowserAutomation + Send + Sync>,
    metadata_refresher: Arc<dyn AccountSessionMetadataRefresher + Send + Sync>,
    secret_backend: Arc<dyn SecretBackend + Send + Sync>,
    now_unix: i64,
    recovery_lock: Arc<tokio::sync::Mutex<()>>,
    coordinator: BrowserBatchCoordinator,
    accumulator: Arc<StdMutex<BrowserWindowBatchAccumulator>>,
}

impl BrowserWindowBatchJob {
    fn run(self, shared_state: Arc<StdMutex<ApiState>>) {
        let (completed_tx, completed_rx) = std::sync::mpsc::channel::<String>();
        let mut handles = BTreeMap::new();

        loop {
            if browser_task_cancel_requested(&shared_state, &self.task_id) {
                if let Ok(snapshot) = self.coordinator.snapshot(&self.task_id) {
                    for item in snapshot.items {
                        if !item.phase.is_terminal() {
                            let _ = self
                                .coordinator
                                .request_item_cancel(&self.task_id, &item.item_id);
                        }
                    }
                }
            }

            let leases = match self.coordinator.claim_available_for(&self.task_id) {
                Ok(leases) => leases,
                Err(_) => break,
            };
            for lease in leases {
                let item_id = lease.item_id.clone();
                let alias = lease.alias.clone();
                let worker = BrowserWindowItemJob {
                    task_id: self.task_id.clone(),
                    item_id: item_id.clone(),
                    alias: alias.clone(),
                    store: self.store.clone(),
                    browser: Arc::clone(&self.browser),
                    metadata_refresher: Arc::clone(&self.metadata_refresher),
                    secret_backend: Arc::clone(&self.secret_backend),
                    now_unix: self.now_unix,
                    recovery_lock: Arc::clone(&self.recovery_lock),
                    coordinator: self.coordinator.clone(),
                    accumulator: Arc::clone(&self.accumulator),
                };
                let completed_tx = completed_tx.clone();
                let worker_state = Arc::clone(&shared_state);
                let handle = std::thread::Builder::new()
                    .name("browser-window-worker".to_string())
                    .spawn(move || {
                        worker.run(&worker_state);
                        let _ = completed_tx.send(item_id.clone());
                    });
                match handle {
                    Ok(handle) => {
                        handles.insert(lease.item_id, handle);
                    }
                    Err(error) => {
                        let message = format!("failed to start browser window worker: {error}");
                        let item = browser_window_failed_item(&alias, message.clone());
                        if let Ok(mut accumulator) = self.accumulator.lock() {
                            accumulator.items.insert(lease.item_id.clone(), item);
                        }
                        let _ = self.coordinator.mark_failed(
                            &self.task_id,
                            &lease.item_id,
                            message.clone(),
                        );
                        if let Ok(mut state) = shared_state.lock() {
                            let _ = state.tasks.append_log(
                                &self.task_id,
                                TaskLogLevel::Warning,
                                format!("{} 的浏览器窗口启动线程创建失败", alias),
                            );
                        }
                    }
                }
            }

            while let Ok(item_id) = completed_rx.try_recv() {
                if let Some(handle) = handles.remove(&item_id) {
                    let _ = handle.join();
                }
            }
            finish_browser_window_batch_if_terminal(
                &shared_state,
                &self.coordinator,
                &self.accumulator,
                &self.task_id,
            );
            let terminal = self
                .coordinator
                .snapshot(&self.task_id)
                .map(|snapshot| snapshot.is_terminal())
                .unwrap_or(true);
            if terminal && handles.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        for (_, handle) in handles {
            let _ = handle.join();
        }
        finish_browser_window_batch_if_terminal(
            &shared_state,
            &self.coordinator,
            &self.accumulator,
            &self.task_id,
        );
    }
}

impl BrowserWindowItemJob {
    fn run(&self, shared_state: &Arc<StdMutex<ApiState>>) {
        if let Ok(mut state) = shared_state.lock() {
            let _ = state.tasks.append_log(
                &self.task_id,
                TaskLogLevel::Info,
                format!("正在为 {} 打开浏览器窗口", self.alias),
            );
        }
        let result = match build_api_runtime() {
            Ok(runtime) => runtime.block_on(open_account_browser_login_with_store_backend(
                &self.store,
                &self.alias,
                self.browser.as_ref(),
                self.metadata_refresher.as_ref(),
                self.now_unix,
                self.secret_backend.as_ref(),
                Some(Arc::clone(&self.recovery_lock)),
            )),
            Err(response) => Err(BrowserAutoLoginError::Browser {
                message: response.body,
            }),
        };
        let (item, completed, message) = match result {
            Ok(report) => {
                let item = browser_auto_login_batch_item_from_report(&self.alias, report);
                let completed = item.opened;
                let message = if completed {
                    format!("{} 的浏览器窗口已打开", self.alias)
                } else {
                    item.error
                        .as_ref()
                        .map(browser_auto_login_error_message)
                        .unwrap_or_else(|| "浏览器登录结果未确认窗口已打开".to_string())
                };
                (item, completed, message)
            }
            Err(error) => {
                let message = browser_auto_login_error_message(&error);
                (
                    BrowserAutoLoginBatchItem {
                        alias: self.alias.clone(),
                        opened: false,
                        cancelled: false,
                        report: None,
                        error: Some(error),
                    },
                    false,
                    message,
                )
            }
        };
        if let Ok(mut accumulator) = self.accumulator.lock() {
            accumulator.items.insert(self.item_id.clone(), item);
        }
        let snapshot = if completed {
            self.coordinator
                .mark_completed(&self.task_id, &self.item_id, message.clone())
        } else {
            self.coordinator
                .mark_failed(&self.task_id, &self.item_id, message.clone())
        };
        if let Ok(mut state) = shared_state.lock() {
            let level = if completed {
                TaskLogLevel::Info
            } else {
                TaskLogLevel::Warning
            };
            let log_message = if completed {
                message
            } else {
                format!("{} 的浏览器窗口打开失败: {}", self.alias, message)
            };
            let _ = state.tasks.append_log(&self.task_id, level, log_message);
            if let Ok(snapshot) = snapshot {
                let completed_items = snapshot
                    .items
                    .iter()
                    .filter(|item| item.phase.is_terminal())
                    .count();
                let _ = state.tasks.set_progress(
                    &self.task_id,
                    task_progress_total(completed_items),
                    task_progress_total(snapshot.items.len()),
                    format!(
                        "浏览器窗口启动进度 {completed_items}/{}",
                        snapshot.items.len()
                    ),
                );
            }
        }
        finish_browser_window_batch_if_terminal(
            shared_state,
            &self.coordinator,
            &self.accumulator,
            &self.task_id,
        );
    }
}

fn browser_window_failed_item(alias: &str, message: String) -> BrowserAutoLoginBatchItem {
    BrowserAutoLoginBatchItem {
        alias: alias.to_string(),
        opened: false,
        cancelled: false,
        report: None,
        error: Some(BrowserAutoLoginError::Browser { message }),
    }
}

fn finish_browser_window_batch_if_terminal(
    shared_state: &Arc<StdMutex<ApiState>>,
    coordinator: &BrowserBatchCoordinator,
    accumulator: &Arc<StdMutex<BrowserWindowBatchAccumulator>>,
    task_id: &str,
) {
    let Ok(snapshot) = coordinator.snapshot(task_id) else {
        return;
    };
    if !snapshot.is_terminal() {
        return;
    }

    let items = {
        let Ok(mut accumulator) = accumulator.lock() else {
            return;
        };
        if accumulator.finalized {
            return;
        }
        accumulator.finalized = true;
        accumulator
            .ordered_items
            .iter()
            .map(|(item_id, alias)| {
                accumulator.items.get(item_id).cloned().unwrap_or_else(|| {
                    let cancelled = snapshot.items.iter().any(|item| {
                        item.item_id == *item_id
                            && item.phase == crate::BrowserBatchItemPhase::Cancelled
                    });
                    BrowserAutoLoginBatchItem {
                        alias: alias.clone(),
                        opened: false,
                        cancelled,
                        report: None,
                        error: (!cancelled).then(|| BrowserAutoLoginError::Browser {
                            message: "browser window item finished without a result".to_string(),
                        }),
                    }
                })
            })
            .collect::<Vec<_>>()
    };
    let report = BrowserAutoLoginBatchReport {
        opened_count: items.iter().filter(|item| item.opened).count(),
        failed_count: items
            .iter()
            .filter(|item| !item.opened && !item.cancelled)
            .count(),
        cancelled_count: items.iter().filter(|item| item.cancelled).count(),
        items,
    };
    let _ = coordinator.remove_terminal_batch(task_id);

    if let Ok(mut state) = shared_state.lock() {
        let task = state.tasks.snapshot(task_id).ok();
        if task.as_ref().is_some_and(|task| task.cancel_requested) {
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Warning,
                format!(
                    "浏览器窗口批次已停止：已打开 {} 个，取消 {} 个，失败 {} 个",
                    report.opened_count, report.cancelled_count, report.failed_count
                ),
            );
            let _ =
                cancel_task_with_registration_release(&mut state, task_id, "浏览器窗口批次已取消");
        } else if report.failed_count > 0 {
            let message = format!(
                "浏览器窗口未完全打开：{} 个成功，{} 个失败",
                report.opened_count, report.failed_count
            );
            let total = task_completion_progress_total(&state, task_id);
            let _ = state.tasks.set_progress(task_id, total, total, &message);
            let result = serde_json::to_value(&report).ok();
            let _ = state.tasks.fail_task_with_result(task_id, message, result);
        } else {
            complete_tracked_task(
                &mut state,
                Some(task_id),
                "浏览器自动登录窗口已打开",
                &report,
            );
        }
    }
}

fn browser_auto_login_response(state: &mut ApiState, body: &str, now_unix: i64) -> ApiResponse {
    let request = match parse_alias_batch_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if let Err(response) = start_alias_batch_task(
        state,
        request.task_id.as_deref(),
        "浏览器自动登录",
        &request.aliases,
    ) {
        return response;
    }

    if let (Some(task_id), Some(local_chrome)) = (
        request.task_id.as_deref(),
        state.local_chrome_browser.clone(),
    ) {
        return queue_browser_login_credential_batch_response(
            state,
            task_id,
            request.aliases,
            local_chrome,
            now_unix,
        );
    }

    let Some(browser) = state.browser_automation.clone() else {
        return browser_not_configured_response(state, request.task_id.as_deref());
    };

    if let Some(task_id) = request.task_id.as_deref() {
        return queue_browser_window_batch_response(
            state,
            task_id,
            request.aliases,
            browser,
            now_unix,
        );
    }

    let store = AccountStore::new(state.config.accounts_path());
    match block_on_api(open_account_browser_logins_in_store_with_recovery_backend(
        &store,
        &request.aliases,
        browser.as_ref(),
        state.metadata_refresher.as_ref(),
        now_unix,
        state.secret_backend.as_ref(),
    )) {
        Ok(Ok(report)) => {
            complete_tracked_task(
                state,
                request.task_id.as_deref(),
                "浏览器自动登录窗口已打开",
                &report,
            );
            json_response(200, &report)
        }
        Ok(Err(error)) => {
            fail_tracked_task(
                state,
                request.task_id.as_deref(),
                browser_auto_login_error_message(&error),
            );
            browser_auto_login_error_response(error)
        }
        Err(response) => {
            fail_tracked_task(state, request.task_id.as_deref(), response.body.clone());
            response
        }
    }
}

fn detect_browser_current_account_response(state: &mut ApiState, body: &str) -> ApiResponse {
    let request: BrowserCurrentAccountRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let task_id = normalized_optional_text(request.task_id);
    state.runtime.browser_account_email = None;
    if let Err(response) = start_task_if_requested(state, task_id.as_deref(), "读取当前浏览器账号")
    {
        return response;
    }

    let Some(executor) = state.extension_bridge_executor.clone() else {
        let error = BrowserProfileAccountError::ExtensionBridgeUnavailable;
        fail_tracked_task(
            state,
            task_id.as_deref(),
            browser_profile_account_error_message(&error),
        );
        return browser_profile_account_error_response(error);
    };

    let store = AccountStore::new(state.config.accounts_path());
    let inspector = state.browser_profile_inspector.clone();
    match block_on_api(detect_browser_profile_account_in_store(
        &store,
        executor.as_ref(),
        inspector.as_ref(),
    )) {
        Ok(Ok(report)) => {
            state.runtime.browser_account_email = report.email.clone();
            complete_tracked_task(state, task_id.as_deref(), "当前浏览器账号已读取", &report);
            json_response(200, &report)
        }
        Ok(Err(error)) => {
            fail_tracked_task(
                state,
                task_id.as_deref(),
                browser_profile_account_error_message(&error),
            );
            browser_profile_account_error_response(error)
        }
        Err(response) => {
            fail_tracked_task(state, task_id.as_deref(), response.body.clone());
            response
        }
    }
}

fn cancel_task_response(state: &mut ApiState, body: &str) -> ApiResponse {
    let request: TaskCancelRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let task_id = request.task_id.trim();
    if task_id.is_empty() {
        return json_response(
            400,
            &ApiErrorBody {
                error: "missing task_id".to_string(),
            },
        );
    }

    if let Ok(snapshot) = state.tasks.snapshot(task_id) {
        let is_terminal = matches!(
            snapshot.phase,
            crate::ServiceTaskPhase::Completed
                | crate::ServiceTaskPhase::Failed
                | crate::ServiceTaskPhase::Cancelled
        );
        if snapshot.cancel_requested && !is_terminal {
            return json_response(202, &snapshot);
        }
    }

    match state.tasks.request_cancel(task_id) {
        Ok(snapshot) => {
            if let Some(entry) = take_registration_session(state, task_id) {
                return enqueue_cancelled_browser_session_cleanup(
                    state,
                    task_id,
                    snapshot,
                    CancelledBrowserSessionKind::Registration,
                    move || cleanup_registration_session(entry),
                );
            }
            if let Some(entry) = take_account_browser_session(state, task_id) {
                return enqueue_cancelled_browser_session_cleanup(
                    state,
                    task_id,
                    snapshot,
                    CancelledBrowserSessionKind::AccountBrowser,
                    move || cleanup_account_browser_session(entry),
                );
            }
            // CDP 会话归工作线程所有，清理结束前保留任务和账号锁。
            let active_browser_batch = state
                .browser_batch_coordinator()
                .snapshot(task_id)
                .map(|batch| !batch.is_terminal())
                .unwrap_or(false);
            if active_browser_batch {
                return match state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Warning,
                    "浏览器批次已收到取消请求，等待各账号会话完成清理",
                ) {
                    Ok(snapshot) => json_response(200, &snapshot),
                    Err(error) => task_state_error_response(error),
                };
            }
            if snapshot.phase == crate::ServiceTaskPhase::WaitingForUser {
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Warning,
                    "Waiting task cancelled after user request",
                );
                return match cancel_task_with_registration_release(
                    state,
                    task_id,
                    "Task cancelled while waiting for user input",
                ) {
                    Ok(snapshot) => json_response(200, &snapshot),
                    Err(error) => task_state_error_response(error),
                };
            }
            match state
                .tasks
                .append_log(task_id, TaskLogLevel::Warning, "Cancel requested")
            {
                Ok(snapshot) => json_response(200, &snapshot),
                Err(error) => task_state_error_response(error),
            }
        }
        Err(error) => task_state_error_response(error),
    }
}

fn enqueue_cancelled_browser_session_cleanup<F>(
    state: &mut ApiState,
    task_id: &str,
    snapshot: TaskSnapshot,
    kind: CancelledBrowserSessionKind,
    cleanup: F,
) -> ApiResponse
where
    F: FnOnce() + Send + 'static,
{
    let owned_task_id = task_id.to_string();
    let job = ApiBackgroundJob::new(kind.job_name(), move |shared_state| {
        cleanup();
        let Ok(mut state) = shared_state.lock() else {
            return;
        };
        let _ = state.tasks.append_log(
            &owned_task_id,
            TaskLogLevel::Warning,
            kind.cleaned_log_message(),
        );
        let _ = cancel_task_with_registration_release(
            &mut state,
            &owned_task_id,
            kind.cancelled_message(),
        );
    });

    if let Err(response) = state.enqueue_background_job(job) {
        fail_tracked_task(state, Some(task_id), response.body.clone());
        return response;
    }

    let snapshot = state
        .tasks
        .append_log(task_id, TaskLogLevel::Warning, kind.queued_message())
        .unwrap_or(snapshot);
    json_response(202, &snapshot)
}

fn retry_task_response(state: &mut ApiState, body: &str, now_unix: i64) -> ApiResponse {
    let request: TaskRetryRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let source_task_id = request.task_id.trim();
    if source_task_id.is_empty() {
        return json_response(
            400,
            &ApiErrorBody {
                error: "missing task_id".to_string(),
            },
        );
    }

    let snapshot = match state.tasks.snapshot(source_task_id) {
        Ok(snapshot) => snapshot,
        Err(error) => return task_state_error_response(error),
    };
    if snapshot.phase != crate::ServiceTaskPhase::Failed || snapshot.resolved_by.is_some() {
        return json_response(
            400,
            &ApiErrorBody {
                error: "only failed tasks can be retried".to_string(),
            },
        );
    }
    let Some(descriptor) = snapshot.retry_descriptor else {
        return json_response(
            400,
            &ApiErrorBody {
                error: "task has no retry descriptor".to_string(),
            },
        );
    };
    if descriptor.replay_safety == TaskReplaySafety::RequiresConfirmation && !request.confirm_replay
    {
        return json_response(
            400,
            &ApiErrorBody {
                error: "retry requires confirm_replay=true".to_string(),
            },
        );
    }

    let retry_task_id = next_retry_task_id(state, source_task_id, request.new_task_id, now_unix);
    if state.tasks.snapshot(&retry_task_id).is_ok() {
        return json_response(
            409,
            &ApiErrorBody {
                error: "重试任务编号已存在".into(),
            },
        );
    }
    let response = retry_operation_from_descriptor(
        state,
        descriptor.operation_kind,
        &descriptor.aliases,
        &descriptor.payload,
        &retry_task_id,
        now_unix,
    );
    if state.tasks.snapshot(&retry_task_id).is_ok() {
        let _ = state.tasks.link_retry(source_task_id, &retry_task_id);
    }
    response
}

fn next_retry_task_id(
    state: &ApiState,
    source_task_id: &str,
    requested_task_id: Option<String>,
    now_unix: i64,
) -> String {
    if let Some(task_id) = normalized_optional_text(requested_task_id) {
        return task_id;
    }

    let mut attempt = 0usize;
    loop {
        let candidate = if attempt == 0 {
            format!("{source_task_id}-retry-{now_unix}")
        } else {
            format!("{source_task_id}-retry-{now_unix}-{attempt}")
        };
        if state.tasks.snapshot(&candidate).is_err() {
            return candidate;
        }
        attempt += 1;
    }
}

fn retry_operation_from_descriptor(
    state: &mut ApiState,
    operation_kind: TaskOperationKind,
    aliases: &[String],
    payload: &TaskRetryPayload,
    retry_task_id: &str,
    now_unix: i64,
) -> ApiResponse {
    let aliases_text = aliases.join(",");
    let body = match operation_kind {
        TaskOperationKind::BrowserCurrentAccount => json!({ "task_id": retry_task_id }).to_string(),
        TaskOperationKind::AccountGitTokenRefresh => {
            json!({ "aliases": aliases_text, "task_id": retry_task_id }).to_string()
        }
        TaskOperationKind::AccountGitTokenGenerate => {
            json!({ "aliases": aliases_text, "task_id": retry_task_id }).to_string()
        }
        TaskOperationKind::BrowserLogin => {
            json!({ "aliases": aliases_text, "task_id": retry_task_id }).to_string()
        }
        TaskOperationKind::AccountSwitchProjectPreview => {
            let Some(alias) = single_retry_alias(aliases) else {
                return invalid_retry_descriptor_response(operation_kind);
            };
            json!({ "alias": alias, "task_id": retry_task_id }).to_string()
        }
        TaskOperationKind::AccountSwitchPlan => {
            let Some(alias) = single_retry_alias(aliases) else {
                return invalid_retry_descriptor_response(operation_kind);
            };
            json!({
                "alias": alias,
                "migrate_projects": payload.migrate_projects.unwrap_or(false),
                "task_id": retry_task_id
            })
            .to_string()
        }
        TaskOperationKind::AccountSwitchExecute => {
            let Some(alias) = single_retry_alias(aliases) else {
                return invalid_retry_descriptor_response(operation_kind);
            };
            json!({
                "alias": alias,
                "migrate_projects": payload.migrate_projects.unwrap_or(false),
                "sync_skills": payload.sync_skills.unwrap_or(false),
                "task_id": retry_task_id
            })
            .to_string()
        }
        TaskOperationKind::RemoteProjectCleanupPreview => {
            json!({ "aliases": aliases_text, "task_id": retry_task_id }).to_string()
        }
        _ => return invalid_retry_descriptor_response(operation_kind),
    };

    match operation_kind {
        TaskOperationKind::BrowserCurrentAccount => {
            detect_browser_current_account_response(state, &body)
        }
        TaskOperationKind::AccountGitTokenRefresh => {
            refresh_account_git_token_response(state, &body, now_unix)
        }
        TaskOperationKind::AccountGitTokenGenerate => {
            generate_account_git_token_response(state, &body, now_unix)
        }
        TaskOperationKind::BrowserLogin => browser_auto_login_response(state, &body, now_unix),
        TaskOperationKind::AccountSwitchPlan => plan_account_switch_response(state, &body),
        TaskOperationKind::AccountSwitchExecute => {
            execute_account_switch_response(state, &body, now_unix)
        }
        TaskOperationKind::AccountSwitchProjectPreview => {
            preview_account_switch_projects_response(state, &body)
        }
        TaskOperationKind::RemoteProjectCleanupPreview => {
            preview_remote_cleanup_response(state, &body)
        }
        _ => invalid_retry_descriptor_response(operation_kind),
    }
}

fn single_retry_alias(aliases: &[String]) -> Option<&str> {
    match aliases {
        [alias] if !alias.trim().is_empty() => Some(alias.as_str()),
        _ => None,
    }
}

fn invalid_retry_descriptor_response(operation_kind: TaskOperationKind) -> ApiResponse {
    json_response(
        400,
        &ApiErrorBody {
            error: format!("unsupported retry operation: {operation_kind:?}"),
        },
    )
}

fn task_input_response(state: &mut ApiState, body: &str, now_unix: i64) -> ApiResponse {
    let request: TaskInputRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let task_id = request.task_id.trim();
    if task_id.is_empty() {
        return json_response(
            400,
            &ApiErrorBody {
                error: "missing task_id".to_string(),
            },
        );
    }
    let kind = request.kind;
    let value = request.value.clone();
    let item_id = request
        .item_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(item_id) = item_id {
        if !matches!(
            kind,
            TaskUserInputKind::NewBrowserCredentials | TaskUserInputKind::CaptchaCompleted
        ) {
            return json_response(
                400,
                &ApiErrorBody {
                    error: "unsupported batch item input kind".to_string(),
                },
            );
        }
        return match state
            .tasks
            .submit_item_user_input(task_id, item_id, kind, request.value)
        {
            Ok(snapshot) => json_response(202, &snapshot),
            Err(error) => task_state_error_response(error),
        };
    }

    if registration_session_required(kind)
        && !has_registration_session(state, task_id)
        && !(kind == TaskUserInputKind::CaptchaCompleted
            && has_account_browser_session(state, task_id))
    {
        if let Err(error) = state.tasks.snapshot(task_id) {
            return task_state_error_response(error);
        }
        return registration_session_missing_response(task_id);
    }
    if kind == TaskUserInputKind::NewBrowserCredentials
        && !has_account_browser_session(state, task_id)
    {
        if let Err(error) = state.tasks.snapshot(task_id) {
            return task_state_error_response(error);
        }
        return account_browser_session_missing_response(task_id);
    }

    match state.tasks.submit_user_input(task_id, kind, request.value) {
        Ok(snapshot) => match kind {
            TaskUserInputKind::EmailCode => {
                continue_registration_with_email_code_response(state, task_id, &value, now_unix)
                    .unwrap_or_else(|| json_response(200, &snapshot))
            }
            TaskUserInputKind::NewRegistrationCredentials => {
                continue_registration_with_new_credentials_response(
                    state, task_id, &value, now_unix,
                )
                .unwrap_or_else(|| registration_session_missing_response(task_id))
            }
            TaskUserInputKind::NewBrowserCredentials => {
                continue_account_browser_with_new_credentials_response(state, task_id, &value)
                    .unwrap_or_else(|| account_browser_session_missing_response(task_id))
            }
            TaskUserInputKind::CaptchaCompleted => {
                continue_registration_after_captcha_response(state, task_id, now_unix)
                    .or_else(|| continue_account_browser_after_captcha_response(state, task_id))
                    .unwrap_or_else(|| registration_session_missing_response(task_id))
            }
        },
        Err(error) => task_state_error_response(error),
    }
}

fn registration_session_required(kind: TaskUserInputKind) -> bool {
    matches!(
        kind,
        TaskUserInputKind::NewRegistrationCredentials | TaskUserInputKind::CaptchaCompleted
    )
}

fn registration_session_missing_response(task_id: &str) -> ApiResponse {
    json_response(
        409,
        &ApiErrorBody {
            error: format!("no active registration browser session for task: {task_id}"),
        },
    )
}

fn account_browser_session_missing_response(task_id: &str) -> ApiResponse {
    json_response(
        409,
        &ApiErrorBody {
            error: format!("no active account browser session for task: {task_id}"),
        },
    )
}

fn continue_account_browser_with_new_credentials_response(
    state: &mut ApiState,
    task_id: &str,
    value: &str,
) -> Option<ApiResponse> {
    let mut entry = take_account_browser_session(state, task_id)?;
    let _ = state
        .tasks
        .discard_user_inputs(task_id, TaskUserInputKind::NewBrowserCredentials);
    let input: BrowserCredentialsTaskInput = match serde_json::from_str(value) {
        Ok(input) => input,
        Err(error) => {
            let response = wait_for_account_browser_credentials_response(
                state,
                task_id,
                entry,
                "重新输入格式无效，请检查邮箱和密码",
            );
            return Some(if response.status_code == 202 {
                json_response(
                    400,
                    &ApiErrorBody {
                        error: format!("invalid browser credentials input: {error}"),
                    },
                )
            } else {
                response
            });
        }
    };
    let password = input.password.trim();
    if password.is_empty() {
        return Some(wait_for_account_browser_credentials_response(
            state,
            task_id,
            entry,
            "密码不能为空，请重新输入",
        ));
    }

    let AccountBrowserSessionContext::ExistingTrialCredentials {
        email,
        password: current_password,
        ..
    } = &mut entry.context;
    if let Some(replacement_email) = input
        .email
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        *email = replacement_email.to_string();
    }
    *current_password = SecretText::new(password);
    entry.last_activity = Instant::now();
    Some(continue_account_browser_session_response(
        state, task_id, entry,
    ))
}

fn continue_account_browser_after_captcha_response(
    state: &mut ApiState,
    task_id: &str,
) -> Option<ApiResponse> {
    let mut entry = take_account_browser_session(state, task_id)?;
    let _ = state
        .tasks
        .discard_user_inputs(task_id, TaskUserInputKind::CaptchaCompleted);
    entry.last_activity = Instant::now();
    Some(continue_account_browser_session_response(
        state, task_id, entry,
    ))
}

fn account_secret_response(state: &ApiState, target: &str) -> ApiResponse {
    let Some(alias) = query_param(target, "alias").filter(|value| !value.trim().is_empty()) else {
        return json_response(
            400,
            &ApiErrorBody {
                error: "missing alias".to_string(),
            },
        );
    };
    let Some(raw_field) = query_param(target, "field").filter(|value| !value.trim().is_empty())
    else {
        return json_response(
            400,
            &ApiErrorBody {
                error: "missing account secret field".to_string(),
            },
        );
    };
    let Some(field) = AccountSecretField::parse(&raw_field) else {
        return json_response(
            400,
            &ApiErrorBody {
                error: "unsupported account secret field".to_string(),
            },
        );
    };

    match load_accounts(state) {
        Ok(document) => match account_secret_for_copy_with_backend(
            &document,
            &alias,
            field,
            state.secret_backend.as_ref(),
        ) {
            Ok(value) => json_response(
                200,
                &AccountSecretBody {
                    alias,
                    field,
                    value,
                },
            ),
            Err(error) => account_secret_error_response(error),
        },
        Err(error) => io_error_response(error),
    }
}

fn add_credentials_response(state: &mut ApiState, body: &str, now_unix: i64) -> ApiResponse {
    let request: CredentialAddRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let task_id = normalized_optional_text(request.task_id);
    let store = AccountStore::new(state.config.accounts_path());
    let document = match store.load() {
        Ok(document) => document,
        Err(error) => return io_error_response(error),
    };
    let plan = match plan_credential_accounts(
        &document,
        CredentialBatchInput {
            aliases: request.aliases,
            emails: request.emails,
            passwords: request.passwords,
        },
    ) {
        Ok(plan) => plan,
        Err(error) => return account_action_error_response(AccountActionError::from(error)),
    };
    let aliases = credential_add_lock_aliases(&plan);
    if let Err(response) =
        start_alias_batch_task(state, task_id.as_deref(), "账号密码添加账号", &aliases)
    {
        return response;
    }

    if let (Some(task_id), Some(local_chrome)) =
        (task_id.as_deref(), state.local_chrome_browser.clone())
    {
        return queue_credential_add_browser_session_response(
            state,
            task_id,
            plan.login_accounts,
            plan.skipped_duplicate_emails,
            local_chrome,
            now_unix,
        );
    }

    let Some(browser) = state.browser_automation.clone() else {
        if plan.login_accounts.is_empty() {
            let report = credential_report_from_skipped_duplicates(&plan.skipped_duplicate_emails);
            complete_tracked_task(state, task_id.as_deref(), "账号密码添加账号完成", &report);
            return json_response(200, &report);
        }
        return browser_not_configured_response(state, task_id.as_deref());
    };
    let metadata_refresher = state.metadata_refresher.clone();
    let secret_backend = state.secret_backend.clone();

    let operation_count = plan.login_accounts.len() + plan.skipped_duplicate_emails.len();
    let progress = task_id
        .as_deref()
        .map(|task_id| CredentialTaskProgress::new(&mut state.tasks, task_id, operation_count));
    match block_on_api(add_credential_accounts_in_store(
        &store,
        &plan,
        browser.as_ref(),
        metadata_refresher.as_ref(),
        now_unix,
        secret_backend.as_ref(),
        progress,
    )) {
        Ok(report) => {
            finish_credential_batch_task(state, task_id.as_deref(), "账号密码添加账号", &report);
            json_response(200, &report)
        }
        Err(response) => {
            fail_tracked_task(state, task_id.as_deref(), response.body.clone());
            response
        }
    }
}

fn refresh_credentials_response(state: &mut ApiState, body: &str, now_unix: i64) -> ApiResponse {
    let request: CredentialRefreshRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let task_id = normalized_optional_text(request.task_id);
    let store = AccountStore::new(state.config.accounts_path());
    let document = match store.load() {
        Ok(document) => document,
        Err(error) => return io_error_response(error),
    };
    let plans = if request.passwords.trim().is_empty() {
        match plan_saved_password_refreshes(
            &document,
            &request.aliases,
            state.secret_backend.as_ref(),
        ) {
            Ok(plans) => plans,
            Err(response) => return response,
        }
    } else {
        match plan_alias_passwords(&document, &request.aliases, &request.passwords) {
            Ok(plans) => plans,
            Err(error) => return account_action_error_response(AccountActionError::from(error)),
        }
    };
    let aliases = plans
        .iter()
        .map(|plan| plan.alias.clone())
        .collect::<Vec<_>>();
    if let Err(response) =
        start_alias_batch_task(state, task_id.as_deref(), "账号密码刷新 Cookie", &aliases)
    {
        return response;
    }
    if request.passwords.trim().is_empty() {
        let saved_count = plans
            .iter()
            .filter(|plan| !plan.password.is_empty())
            .count();
        let waiting_count = plans.len().saturating_sub(saved_count);
        append_optional_task_log(
            state,
            task_id.as_deref(),
            TaskLogLevel::Info,
            format!(
                "已读取 {} 个本地密码，{} 个账号需要用户补充密码",
                saved_count, waiting_count
            ),
        );
    }

    if let (Some(task_id), Some(local_chrome)) =
        (task_id.as_deref(), state.local_chrome_browser.clone())
    {
        return queue_credential_refresh_browser_session_response(
            state,
            task_id,
            plans,
            CredentialBrowserBatchCompletion::Standard,
            local_chrome,
            now_unix,
        );
    }

    let Some(browser) = state.browser_automation.clone() else {
        return browser_not_configured_response(state, task_id.as_deref());
    };
    let metadata_refresher = state.metadata_refresher.clone();
    let secret_backend = state.secret_backend.clone();

    let progress = task_id
        .as_deref()
        .map(|task_id| CredentialTaskProgress::new(&mut state.tasks, task_id, plans.len()));
    match block_on_api(refresh_credential_accounts_in_store(
        &store,
        &plans,
        browser.as_ref(),
        metadata_refresher.as_ref(),
        now_unix,
        secret_backend.as_ref(),
        progress,
    )) {
        Ok(report) => {
            finish_credential_batch_task(state, task_id.as_deref(), "账号密码刷新 Cookie", &report);
            json_response(200, &report)
        }
        Err(response) => {
            fail_tracked_task(state, task_id.as_deref(), response.body.clone());
            response
        }
    }
}

fn plan_saved_password_refreshes(
    document: &AccountsDocument,
    aliases_input: &str,
    backend: &dyn SecretBackend,
) -> Result<Vec<AliasPasswordPlan>, ApiResponse> {
    let aliases = split_comma_values(aliases_input);
    if aliases.is_empty() {
        return Err(account_action_error_response(
            AccountActionError::EmptyValue {
                field: "账号别名".to_string(),
            },
        ));
    }

    let mut seen = BTreeSet::new();
    let mut plans = Vec::with_capacity(aliases.len());
    for alias in aliases {
        if !seen.insert(alias.clone()) {
            return Err(account_action_error_response(
                AccountActionError::AliasRepeatedInInput { alias },
            ));
        }
        let Some(record) = document.accounts.get(&alias) else {
            return Err(account_action_error_response(
                AccountActionError::AccountAliasNotFound { alias },
            ));
        };
        let password = match read_account_password_secret_for_recovery(record, &alias, backend) {
            Ok(password) => password,
            Err(AccountSecretStoreError::Missing { .. }) => String::new(),
            Err(error) => {
                return Err(json_response(
                    500,
                    &ApiErrorBody {
                        error: account_secret_store_error_message(&error),
                    },
                ));
            }
        };
        plans.push(AliasPasswordPlan {
            alias,
            email: record.email.clone(),
            password,
        });
    }
    Ok(plans)
}

fn open_account_browser_session(
    local_chrome: Arc<LocalChromeBrowserAutomation>,
    proxy_attempt: ChromeProxyAttempt,
) -> Result<(tokio::runtime::Runtime, LocalChromeCdpSession), ApiResponse> {
    let runtime = build_api_runtime()?;
    let session = runtime
        .block_on(local_chrome.start_account_login_cdp_session_for_attempt(proxy_attempt))
        .map_err(|error| {
            json_response(
                503,
                &ApiErrorBody {
                    error: error.to_string(),
                },
            )
        })?;
    Ok((runtime, session))
}

fn continue_account_browser_session_response(
    state: &mut ApiState,
    task_id: &str,
    entry: AccountBrowserSessionEntry,
) -> ApiResponse {
    if !entry.session.is_active() {
        cleanup_account_browser_session(entry);
        return cancel_closed_account_browser_task(state, task_id);
    }
    if entry.last_activity.elapsed() >= ACCOUNT_BROWSER_SESSION_TIMEOUT {
        cleanup_account_browser_session(entry);
        let message = "浏览器会话等待超时".to_string();
        fail_tracked_task(state, Some(task_id), message.clone());
        return json_response(408, &ApiErrorBody { error: message });
    }

    let AccountBrowserSessionEntry {
        runtime,
        session,
        context,
        ..
    } = entry;
    let AccountBrowserSessionContext::ExistingTrialCredentials {
        alias_hint,
        email,
        password,
        trial_days,
        auto_fetch_git_token,
        card_selection_strategy,
        card_bin,
        session_restart_count,
        now_unix,
    } = context;
    continue_existing_trial_credentials_browser_session(
        state,
        task_id,
        runtime,
        session,
        alias_hint,
        email,
        password,
        trial_days,
        auto_fetch_git_token,
        card_selection_strategy,
        card_bin,
        session_restart_count,
        now_unix,
    )
}

#[allow(clippy::too_many_arguments)]
fn continue_existing_trial_credentials_browser_session(
    state: &mut ApiState,
    task_id: &str,
    runtime: tokio::runtime::Runtime,
    session: LocalChromeCdpSession,
    alias_hint: Option<String>,
    email: String,
    password: SecretText,
    trial_days: u32,
    auto_fetch_git_token: bool,
    card_selection_strategy: CardSelectionStrategy,
    card_bin: Option<String>,
    session_restart_count: usize,
    now_unix: i64,
) -> ApiResponse {
    let result = runtime.block_on(session.automation().login_with_credentials(
        CredentialsLoginInput {
            email: email.clone(),
            password: password.clone(),
        },
    ));
    match result {
        Ok(login) => continue_account_browser_session_from_login_result(
            state,
            task_id,
            AccountBrowserSessionEntry {
                runtime,
                session,
                context: AccountBrowserSessionContext::ExistingTrialCredentials {
                    alias_hint,
                    email,
                    password,
                    trial_days,
                    auto_fetch_git_token,
                    card_selection_strategy,
                    card_bin,
                    session_restart_count,
                    now_unix,
                },
                last_activity: Instant::now(),
            },
            login,
        ),
        Err(BrowserAutomationError::InvalidCredentials) => {
            wait_for_account_browser_credentials_response(
                state,
                task_id,
                AccountBrowserSessionEntry {
                    runtime,
                    session,
                    context: AccountBrowserSessionContext::ExistingTrialCredentials {
                        alias_hint,
                        email,
                        password,
                        trial_days,
                        auto_fetch_git_token,
                        card_selection_strategy,
                        card_bin,
                        session_restart_count,
                        now_unix,
                    },
                    last_activity: Instant::now(),
                },
                "邮箱或密码错误，请重新输入",
            )
        }
        Err(BrowserAutomationError::ChallengeRequired { .. }) => {
            wait_for_account_browser_captcha_response(
                state,
                task_id,
                AccountBrowserSessionEntry {
                    runtime,
                    session,
                    context: AccountBrowserSessionContext::ExistingTrialCredentials {
                        alias_hint,
                        email,
                        password,
                        trial_days,
                        auto_fetch_git_token,
                        card_selection_strategy,
                        card_bin,
                        session_restart_count,
                        now_unix,
                    },
                    last_activity: Instant::now(),
                },
            )
        }
        Err(BrowserAutomationError::RobotVerificationBlocked)
            if session_restart_count < 1 && state.config.browser_proxy_policy.has_fallback() =>
        {
            let _ = runtime.block_on(session.cleanup());
            let Some(local_chrome) = state.local_chrome_browser.clone() else {
                return browser_not_configured_response(state, Some(task_id));
            };
            match open_account_browser_session(local_chrome, ChromeProxyAttempt::Fallback) {
                Ok((new_runtime, new_session)) => {
                    let _ = state.tasks.append_log(
                        task_id,
                        TaskLogLevel::Warning,
                        "机器人验证未通过，自动切换系统代理重试一次",
                    );
                    continue_existing_trial_credentials_browser_session(
                        state,
                        task_id,
                        new_runtime,
                        new_session,
                        alias_hint,
                        email,
                        password,
                        trial_days,
                        auto_fetch_git_token,
                        card_selection_strategy,
                        card_bin,
                        session_restart_count + 1,
                        now_unix,
                    )
                }
                Err(response) => {
                    fail_tracked_task(state, Some(task_id), response.body.clone());
                    response
                }
            }
        }
        Err(error) => {
            let _ = runtime.block_on(session.cleanup());
            handle_registration_error(state, Some(task_id), AccountRegistrationError::from(error))
        }
    }
}

fn continue_account_browser_session_from_login_result(
    state: &mut ApiState,
    task_id: &str,
    entry: AccountBrowserSessionEntry,
    login: LoginResult,
) -> ApiResponse {
    let AccountBrowserSessionEntry {
        runtime,
        session,
        context,
        ..
    } = entry;

    let AccountBrowserSessionContext::ExistingTrialCredentials {
        alias_hint,
        email,
        password,
        trial_days,
        auto_fetch_git_token,
        card_selection_strategy,
        card_bin,
        now_unix,
        ..
    } = context;
    let alias = make_alias(alias_hint.as_deref(), &email);
    let cookies = cookies_to_map(&login.cookies);
    let evidence = match runtime.block_on(validate_trial_cookie_eligibility(
        state, task_id, &alias, &email, &cookies, trial_days, now_unix,
    )) {
        Ok(evidence) => evidence,
        Err(response) => {
            let _ = runtime.block_on(session.cleanup());
            return response;
        }
    };
    let continuation = match existing_trial_preflight(&evidence) {
        Ok(Some(continuation)) => continuation,
        Ok(None) => match inspect_rendered_existing_trial_continuation(
            state, task_id, &alias, &runtime, &session, trial_days, now_unix,
        ) {
            Ok(continuation) => continuation,
            Err(ExistingTrialResolutionError::NotEligible(eligibility)) => {
                let _ = runtime.block_on(session.cleanup());
                return existing_account_trial_not_eligible_response(
                    state,
                    task_id,
                    &alias,
                    eligibility,
                );
            }
            Err(ExistingTrialResolutionError::Browser(error)) => {
                let _ = runtime.block_on(session.cleanup());
                return handle_registration_error(
                    state,
                    Some(task_id),
                    AccountRegistrationError::from(error),
                );
            }
        },
        Err(eligibility) => {
            let _ = runtime.block_on(session.cleanup());
            return existing_account_trial_not_eligible_response(
                state,
                task_id,
                &alias,
                eligibility,
            );
        }
    };
    continue_existing_trial_response(
        state,
        task_id,
        RegistrationSessionEntry {
            runtime,
            session,
            context: RegistrationSessionContext {
                alias_hint,
                existing_alias: None,
                email,
                password: Some(password),
                trial_days,
                auto_fetch_git_token,
                card_selection_strategy,
                card_bin,
            },
            last_activity: Instant::now(),
        },
        now_unix,
        continuation,
    )
}

fn finish_credential_refresh_continuation(
    state: &mut ApiState,
    task_id: &str,
    report: CredentialBatchReport,
    continuation: CredentialRefreshContinuation,
) -> ApiResponse {
    match continuation {
        CredentialRefreshContinuation::ExecuteAccountSwitch {
            alias,
            migrate_projects,
            now_unix,
        } => {
            if report.failed_count > 0 || report.updated_count == 0 {
                fail_tracked_task_with_result(
                    state,
                    Some(task_id),
                    "Cookie 恢复失败，无感换号未执行",
                    &report,
                );
                return json_response(400, &report);
            }
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Info,
                "Cookie 已恢复并验证，正在继续原无感换号任务",
            );
            execute_account_switch_task_response(
                state,
                task_id,
                &alias,
                migrate_projects,
                now_unix,
                false,
            )
        }
        CredentialRefreshContinuation::StartExistingTrial {
            alias,
            trial_days,
            auto_fetch_git_token,
            card_selection_strategy,
            card_bin,
            now_unix,
        } => {
            if report.failed_count > 0 || report.updated_count == 0 {
                fail_tracked_task_with_result(
                    state,
                    Some(task_id),
                    "Cookie 恢复失败，已有账号试用未启动",
                    &report,
                );
                return json_response(400, &report);
            }
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Info,
                "Cookie 已恢复并验证，正在继续已有账号试用任务",
            );
            register_existing_account_trial_response_inner(
                state,
                &alias,
                trial_days,
                auto_fetch_git_token,
                card_selection_strategy,
                card_bin,
                Some(task_id),
                now_unix,
                false,
            )
        }
    }
}

async fn add_credential_accounts_in_store(
    store: &AccountStore,
    plan: &overleaf_workflows::CredentialBatchPlan,
    browser: &(dyn BrowserAutomation + Send + Sync),
    metadata_refresher: &(dyn AccountSessionMetadataRefresher + Send + Sync),
    now_unix: i64,
    secret_backend: &(dyn SecretBackend + Send + Sync),
    mut progress: Option<CredentialTaskProgress<'_>>,
) -> CredentialBatchReport {
    let overleaf_workflows::CredentialBatchPlan {
        login_accounts,
        skipped_duplicate_emails,
    } = plan;
    let mut report = credential_report_from_skipped_duplicates(skipped_duplicate_emails);
    let mut processed = 0_usize;
    for item in &report.items {
        processed += 1;
        if let Some(progress) = progress.as_mut() {
            progress.finished(
                processed,
                &item.alias,
                TaskLogLevel::Info,
                "邮箱已存在，跳过重复导入",
            );
        }
    }

    for (offset, account) in login_accounts.iter().enumerate() {
        if let Some(progress) = progress.as_mut() {
            progress.started(
                processed + offset + 1,
                &account.alias,
                "正在打开登录页并填写凭据（有界并发）",
            );
        }
    }

    let mut completions = stream::iter(login_accounts.iter().cloned().enumerate())
        .map(|(offset, account)| async move {
            let login = browser
                .login_with_credentials(overleaf_browser::CredentialsLoginInput {
                    email: account.email.clone(),
                    password: SecretText::new(account.password.clone()),
                })
                .await
                .map_err(AccountCredentialError::from_browser);
            (offset, account, login)
        })
        .buffer_unordered(DEFAULT_BROWSER_BATCH_CONCURRENCY);
    let mut ordered_items = vec![None; login_accounts.len()];

    while let Some((offset, account, login)) = completions.next().await {
        let index = processed + offset + 1;
        let result = match login {
            Ok(login) => {
                add_account_from_login_result_in_store_with_backend(
                    store,
                    account.alias_hint.as_deref(),
                    &account.email,
                    SecretText::new(account.password.clone()),
                    login,
                    metadata_refresher,
                    now_unix,
                    secret_backend,
                )
                .await
            }
            Err(error) => Err(error),
        };

        ordered_items[offset] = Some(match result {
            Ok(item_report) => {
                let (item, completed, failure_message) =
                    classify_credential_batch_item(&mut report, item_report);
                if let Some(progress) = progress.as_mut() {
                    if completed {
                        progress.finished(
                            index,
                            &account.alias,
                            TaskLogLevel::Info,
                            "登录并保存完成",
                        );
                    } else {
                        let detail = failure_message
                            .as_deref()
                            .unwrap_or("Cookie/订阅/使用状态未完整刷新");
                        progress.finished(
                            index,
                            &account.alias,
                            TaskLogLevel::Warning,
                            &format!("未完成：{detail}"),
                        );
                    }
                }
                item
            }
            Err(error) => {
                let error = account_credential_error_message(&error);
                report.failed_count += 1;
                let item = CredentialBatchItem {
                    alias: account.alias.clone(),
                    email: account.email.clone(),
                    status: CredentialBatchItemStatus::Failed,
                    existing_alias: None,
                    report: None,
                    error: Some(error.clone()),
                };
                if let Some(progress) = progress.as_mut() {
                    progress.finished(
                        index,
                        &account.alias,
                        TaskLogLevel::Warning,
                        &format!("未完成：{error}"),
                    );
                }
                item
            }
        });
    }
    report.items.extend(ordered_items.into_iter().flatten());
    report
}

async fn refresh_credential_accounts_in_store(
    store: &AccountStore,
    plans: &[AliasPasswordPlan],
    browser: &(dyn BrowserAutomation + Send + Sync),
    metadata_refresher: &(dyn AccountSessionMetadataRefresher + Send + Sync),
    now_unix: i64,
    secret_backend: &(dyn SecretBackend + Send + Sync),
    mut progress: Option<CredentialTaskProgress<'_>>,
) -> CredentialBatchReport {
    let mut report = CredentialBatchReport::default();
    for (offset, plan) in plans.iter().enumerate() {
        if let Some(progress) = progress.as_mut() {
            progress.started(
                offset + 1,
                &plan.alias,
                "正在打开登录页并刷新 Cookie（有界并发）",
            );
        }
    }

    let mut completions = stream::iter(plans.iter().cloned().enumerate())
        .map(|(offset, plan)| async move {
            let login = match plan.email.as_deref().map(str::trim) {
                Some(email) if !email.is_empty() => browser
                    .login_with_credentials(overleaf_browser::CredentialsLoginInput {
                        email: email.to_string(),
                        password: SecretText::new(plan.password.clone()),
                    })
                    .await
                    .map_err(AccountCredentialError::from_browser),
                _ => Err(AccountCredentialError::MissingEmail {
                    alias: plan.alias.clone(),
                }),
            };
            (offset, plan, login)
        })
        .buffer_unordered(DEFAULT_BROWSER_BATCH_CONCURRENCY);
    let mut ordered_items = vec![None; plans.len()];

    while let Some((offset, plan, login)) = completions.next().await {
        let index = offset + 1;
        let result = match login {
            Ok(login) => {
                refresh_account_cookie_from_login_result_in_store_with_backend(
                    store,
                    &plan.alias,
                    SecretText::new(plan.password.clone()),
                    login,
                    metadata_refresher,
                    now_unix,
                    secret_backend,
                )
                .await
            }
            Err(error) => Err(error),
        };

        ordered_items[offset] = Some(match result {
            Ok(item_report) => {
                let (item, completed, failure_message) =
                    classify_credential_batch_item(&mut report, item_report);
                if let Some(progress) = progress.as_mut() {
                    if completed {
                        progress.finished(
                            index,
                            &plan.alias,
                            TaskLogLevel::Info,
                            "Cookie 刷新完成",
                        );
                    } else {
                        let detail = failure_message
                            .as_deref()
                            .unwrap_or("Cookie/订阅/使用状态未完整刷新");
                        progress.finished(
                            index,
                            &plan.alias,
                            TaskLogLevel::Warning,
                            &format!("未完成：{detail}"),
                        );
                    }
                }
                item
            }
            Err(error) => {
                let error = account_credential_error_message(&error);
                report.failed_count += 1;
                let item = CredentialBatchItem {
                    alias: plan.alias.clone(),
                    email: plan.email.clone().unwrap_or_default(),
                    status: CredentialBatchItemStatus::Failed,
                    existing_alias: None,
                    report: None,
                    error: Some(error.clone()),
                };
                if let Some(progress) = progress.as_mut() {
                    progress.finished(
                        index,
                        &plan.alias,
                        TaskLogLevel::Warning,
                        &format!("未完成：{error}"),
                    );
                }
                item
            }
        });
    }
    report.items = ordered_items.into_iter().flatten().collect();
    report
}

fn credential_report_from_skipped_duplicates(
    skipped_duplicate_emails: &[SkippedDuplicateEmail],
) -> CredentialBatchReport {
    CredentialBatchReport {
        imported_count: 0,
        updated_count: 0,
        skipped_duplicate_email_count: skipped_duplicate_emails.len(),
        failed_count: 0,
        items: skipped_duplicate_emails
            .iter()
            .map(|skipped| CredentialBatchItem {
                alias: skipped.alias.clone(),
                email: skipped.email.clone(),
                status: CredentialBatchItemStatus::SkippedDuplicateEmail,
                existing_alias: Some(skipped.existing_alias.clone()),
                report: None,
                error: None,
            })
            .collect(),
    }
}

fn credential_add_lock_aliases(plan: &overleaf_workflows::CredentialBatchPlan) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut aliases = Vec::new();
    for account in &plan.login_accounts {
        if seen.insert(account.alias.clone()) {
            aliases.push(account.alias.clone());
        }
    }
    for skipped in &plan.skipped_duplicate_emails {
        for alias in [&skipped.alias, &skipped.existing_alias] {
            if seen.insert(alias.clone()) {
                aliases.push(alias.clone());
            }
        }
    }
    aliases
}

fn browser_not_configured_response(state: &mut ApiState, task_id: Option<&str>) -> ApiResponse {
    let message = "browser automation is not configured".to_string();
    fail_tracked_task(state, task_id, message.clone());
    json_response(503, &ApiErrorBody { error: message })
}

fn validated_alias_password_plans(
    document: &AccountsDocument,
    aliases_input: &str,
    passwords_input: &str,
) -> Result<Vec<AliasPasswordPlan>, AccountActionError> {
    let plans = plan_alias_passwords(document, aliases_input, passwords_input)
        .map_err(AccountActionError::from)?;
    if let Some(minimum) = short_password_minimum(&plans) {
        return Err(AccountActionError::PasswordTooShort { minimum });
    }
    Ok(plans)
}

fn short_password_minimum(plans: &[AliasPasswordPlan]) -> Option<usize> {
    plans
        .iter()
        .find_map(|plan| validate_new_password(&plan.password).err())
        .map(|error| error.minimum)
}

fn reject_locked_password_aliases(
    state: &ApiState,
    task_id: Option<&str>,
    aliases: &[String],
) -> Result<(), ApiResponse> {
    if task_id.is_none() {
        return Ok(());
    }
    for alias in aliases {
        if let Some(owner) = state.tasks.account_lock_owner(alias) {
            return Err(task_state_error_response(
                TaskStateError::AccountAlreadyLocked {
                    alias: alias.clone(),
                    task_id: owner.to_string(),
                },
            ));
        }
    }
    Ok(())
}

fn update_local_password_response(state: &mut ApiState, body: &str) -> ApiResponse {
    let request: LocalPasswordUpdateRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let task_id = normalized_optional_text(request.task_id);
    let aliases = split_comma_values(&request.aliases);
    if let Err(response) = reject_locked_password_aliases(state, task_id.as_deref(), &aliases) {
        return response;
    }
    let store = AccountStore::new(state.config.accounts_path());
    if let Ok(document) = store.load() {
        if let Ok(plans) = plan_alias_passwords(&document, &request.aliases, &request.passwords) {
            if let Some(minimum) = short_password_minimum(&plans) {
                return account_action_error_response(AccountActionError::PasswordTooShort {
                    minimum,
                });
            }
        }
    }
    if let Err(response) =
        start_alias_batch_task(state, task_id.as_deref(), "更新本地账号密码", &aliases)
    {
        return response;
    }

    match update_local_passwords_in_store_with_backend(
        &store,
        &request.aliases,
        &request.passwords,
        state.secret_backend.as_ref(),
    ) {
        Ok(report) => {
            complete_tracked_task(state, task_id.as_deref(), "本地账号密码更新完成", &report);
            json_response(200, &report)
        }
        Err(error) => {
            fail_tracked_task(
                state,
                task_id.as_deref(),
                account_action_error_message(&error),
            );
            account_action_error_response(error)
        }
    }
}

fn change_overleaf_password_response(
    state: &mut ApiState,
    body: &str,
    now_unix: i64,
) -> ApiResponse {
    let request: OverleafPasswordChangeRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let task_id = normalized_optional_text(request.task_id);
    let requested_aliases = split_comma_values(&request.aliases);
    if let Err(response) =
        reject_locked_password_aliases(state, task_id.as_deref(), &requested_aliases)
    {
        return response;
    }
    let store = AccountStore::new(state.config.accounts_path());
    let document = match store.load() {
        Ok(document) => document,
        Err(error) => return io_error_response(error),
    };
    let plans =
        match validated_alias_password_plans(&document, &request.aliases, &request.passwords) {
            Ok(plans) => plans,
            Err(error) => return account_action_error_response(error),
        };
    let aliases = plans
        .iter()
        .map(|plan| plan.alias.clone())
        .collect::<Vec<_>>();
    if let Err(response) =
        start_alias_batch_task(state, task_id.as_deref(), "修改 Overleaf 密码", &aliases)
    {
        return response;
    }

    if let (Some(task_id), Some(local_chrome)) =
        (task_id.as_deref(), state.local_chrome_browser.clone())
    {
        return queue_overleaf_password_change_browser_session_response(
            state,
            task_id,
            plans,
            local_chrome,
            now_unix,
        );
    }

    let Some(browser) = state.browser_automation.clone() else {
        let message = "browser automation is not configured".to_string();
        fail_tracked_task(state, task_id.as_deref(), message.clone());
        return json_response(503, &ApiErrorBody { error: message });
    };

    match block_on_api(change_overleaf_passwords_in_store(
        &store,
        &plans,
        browser.as_ref(),
        now_unix,
        state.secret_backend.clone(),
    )) {
        Ok(report) => {
            finish_overleaf_password_change_batch_task(state, task_id.as_deref(), &report);
            json_response(200, &report)
        }
        Err(response) => {
            fail_tracked_task(state, task_id.as_deref(), response.body.clone());
            response
        }
    }
}

async fn change_overleaf_passwords_in_store(
    store: &AccountStore,
    plans: &[overleaf_workflows::AliasPasswordPlan],
    browser: &(dyn BrowserAutomation + Send + Sync),
    now_unix: i64,
    secret_backend: Arc<dyn SecretBackend + Send + Sync>,
) -> OverleafPasswordChangeBatchReport {
    let mut items = Vec::with_capacity(plans.len());
    let mut changed_count = 0;
    let mut failed_count = 0;

    for plan in plans {
        match change_account_overleaf_password_in_store_with_backend(
            store,
            &plan.alias,
            SecretText::new(plan.password.clone()),
            browser,
            now_unix,
            secret_backend.as_ref(),
        )
        .await
        {
            Ok(report) => {
                if report.password_updated {
                    changed_count += 1;
                } else {
                    failed_count += 1;
                }
                items.push(OverleafPasswordChangeBatchItem {
                    alias: plan.alias.clone(),
                    email: Some(report.email.clone()),
                    password_updated: report.password_updated,
                    report: Some(report),
                    error: None,
                });
            }
            Err(error) => {
                failed_count += 1;
                items.push(OverleafPasswordChangeBatchItem {
                    alias: plan.alias.clone(),
                    email: plan.email.clone(),
                    password_updated: false,
                    report: None,
                    error: Some(overleaf_password_change_error_message(&error)),
                });
            }
        }
    }

    OverleafPasswordChangeBatchReport {
        changed_count,
        failed_count,
        items,
    }
}

fn spawn_follow_up_jobs(
    shared_state: &Arc<StdMutex<ApiState>>,
    task_id: &str,
    jobs: Vec<ApiBackgroundJob>,
) {
    for job in jobs {
        let job_name = job.name().to_string();
        let job_state = Arc::clone(shared_state);
        if let Err(error) = thread::Builder::new()
            .name(format!("overleaf-api-{job_name}"))
            .spawn(move || job.run(job_state))
        {
            if let Ok(mut state) = shared_state.lock() {
                fail_tracked_task(
                    &mut state,
                    Some(task_id),
                    format!("failed to start background job {job_name}: {error}"),
                );
            }
        }
    }
}

fn preview_remote_cleanup_response(state: &mut ApiState, body: &str) -> ApiResponse {
    let ParsedAliasBatchRequest { aliases, task_id } = match parse_alias_batch_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    if let Err(response) =
        start_alias_batch_task(state, task_id.as_deref(), "预览远端项目清理", &aliases)
    {
        return response;
    }

    remote_cleanup_response(state, task_id, aliases, false)
}

fn remove_accounts_response(state: &mut ApiState, body: &str) -> ApiResponse {
    let request: AccountRemovalRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let task_id = normalized_optional_text(request.task_id);
    let store = AccountStore::new(state.config.accounts_path());
    if request.cleanup_remote_projects {
        let ParsedAliasBatchRequest { aliases, task_id } =
            match parse_alias_batch_parts(&request.aliases, task_id) {
                Ok(request) => request,
                Err(response) => return response,
            };
        if !request.confirm_local_only {
            return json_response(
                400,
                &ApiErrorBody {
                    error: "remote cleanup removal requires confirm_local_only=true".to_string(),
                },
            );
        }
        if let Err(response) = start_alias_batch_task(
            state,
            task_id.as_deref(),
            "清理远端项目并移除账号",
            &aliases,
        ) {
            return response;
        }
        return remote_cleanup_response(state, task_id, aliases, true);
    }

    let aliases = split_comma_values(&request.aliases);
    if !request.confirm_local_only {
        return json_response(
            400,
            &ApiErrorBody {
                error: "local removal requires confirm_local_only=true".to_string(),
            },
        );
    }

    if let Err(response) =
        start_alias_batch_task(state, task_id.as_deref(), "移除本地账号", &aliases)
    {
        return response;
    }

    match remove_accounts_locally_in_store(&store, &request.aliases) {
        Ok(report) => {
            complete_tracked_task(state, task_id.as_deref(), "本地账号移除完成", &report);
            json_response(200, &report)
        }
        Err(error) => {
            fail_tracked_task(
                state,
                task_id.as_deref(),
                account_action_error_message(&error),
            );
            account_action_error_response(error)
        }
    }
}

fn remote_cleanup_response(
    state: &mut ApiState,
    task_id: Option<String>,
    aliases: Vec<String>,
    remove: bool,
) -> ApiResponse {
    let store = AccountStore::new(state.config.accounts_path());
    let executor = state.remote_project_cleanup_executor.clone();
    let backend = state.secret_backend.clone();
    let commit_lock = task_id.as_ref().map(|_| state.account_commit_lock());
    let work = move |progress: &mut dyn FnMut(usize, usize) -> bool| {
        if remove {
            block_on_api(cleanup_remote_then_remove_accounts(
                &store,
                &aliases,
                executor.as_ref(),
                backend.as_ref(),
                commit_lock.as_deref(),
                progress,
            ))
            .map(|report| json!(report))
        } else {
            block_on_api(preview_remote_cleanup_accounts(
                &store,
                &aliases,
                executor.as_ref(),
                backend.as_ref(),
                progress,
            ))
            .map(|report| json!(report))
        }
    };
    if let Some(task_id) = task_id {
        let owned_task_id = task_id.clone();
        let job = ApiBackgroundJob::new("project-cleanup", move |shared_state| {
            let result = work(&mut |current, total| {
                let Ok(mut state) = shared_state.lock() else {
                    return false;
                };
                let _ =
                    state
                        .tasks
                        .set_progress_value(&owned_task_id, current as u32, total as u32);
                state
                    .tasks
                    .snapshot(&owned_task_id)
                    .is_ok_and(|task| !task.cancel_requested)
            });
            if let Ok(mut state) = shared_state.lock() {
                finish_remote_cleanup(&mut state, Some(&owned_task_id), result);
            }
        });
        if let Err(response) = state.enqueue_background_job(job) {
            fail_tracked_task(state, Some(&task_id), response.body.clone());
            return response;
        }
        return registration_waiting_response(state, &task_id);
    }
    finish_remote_cleanup(state, None, work(&mut |_, _| true))
}

fn finish_remote_cleanup(
    state: &mut ApiState,
    task_id: Option<&str>,
    result: Result<serde_json::Value, ApiResponse>,
) -> ApiResponse {
    match result {
        Ok(report) => {
            if let Some(task_id) = task_id.filter(|id| {
                state
                    .tasks
                    .snapshot(id)
                    .is_ok_and(|task| task.cancel_requested)
            }) {
                let _ = state
                    .tasks
                    .cancel_task(task_id, "远端项目处理已取消，已完成的操作保留");
                return json_response(200, &report);
            }
            if report["failed_count"].as_u64().unwrap_or(0) > 0
                || report["incomplete_count"].as_u64().unwrap_or(0) > 0
            {
                fail_tracked_task_with_result(state, task_id, "远端项目处理未完全完成", &report);
            } else {
                complete_tracked_task(state, task_id, "远端项目处理完成", &report);
            }
            json_response(200, &report)
        }
        Err(response) => {
            fail_tracked_task(state, task_id, response.body.clone());
            response
        }
    }
}

async fn preview_remote_cleanup_accounts(
    store: &AccountStore,
    aliases: &[String],
    executor: &(dyn RemoteProjectCleanupExecutor + Send + Sync),
    secret_backend: &(dyn SecretBackend + Send + Sync),
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> RemoteCleanupPreviewBatchReport {
    let mut report = RemoteCleanupPreviewBatchReport {
        previewed_count: 0,
        failed_count: 0,
        total_projects: 0,
        delete_count: 0,
        leave_count: 0,
        items: Vec::with_capacity(aliases.len()),
    };

    for alias in aliases {
        if !progress(report.items.len(), aliases.len()) {
            break;
        }
        match executor.preview_cleanup(store, alias, secret_backend).await {
            Ok(item_report) => {
                report.previewed_count += 1;
                report.total_projects += item_report.total_projects;
                report.delete_count += item_report.delete_count;
                report.leave_count += item_report.leave_count;
                report.items.push(RemoteCleanupPreviewBatchItem {
                    alias: alias.clone(),
                    report: Some(item_report),
                    error: None,
                });
            }
            Err(error) => {
                report.failed_count += 1;
                report.items.push(RemoteCleanupPreviewBatchItem {
                    alias: alias.clone(),
                    report: None,
                    error: Some(error),
                });
            }
        }
    }

    progress(report.items.len(), aliases.len());
    report
}

async fn cleanup_remote_then_remove_accounts(
    store: &AccountStore,
    aliases: &[String],
    executor: &(dyn RemoteProjectCleanupExecutor + Send + Sync),
    secret_backend: &(dyn SecretBackend + Send + Sync),
    commit_lock: Option<&StdMutex<()>>,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> RemoteCleanupRemovalBatchReport {
    let mut report = RemoteCleanupRemovalBatchReport {
        removed_count: 0,
        incomplete_count: 0,
        failed_count: 0,
        items: Vec::with_capacity(aliases.len()),
    };

    for alias in aliases {
        if !progress(report.items.len(), aliases.len()) {
            break;
        }
        match executor
            .cleanup_then_remove_account(store, alias, secret_backend, commit_lock)
            .await
        {
            Ok(item_report) => {
                let status = match item_report.status {
                    CleanupThenRemoveStatus::RemovedLocally => {
                        report.removed_count += item_report
                            .local_removal
                            .as_ref()
                            .map(|removal| removal.removed_count)
                            .unwrap_or(1);
                        RemoteCleanupRemovalBatchStatus::RemovedLocally
                    }
                    CleanupThenRemoveStatus::RemoteCleanupIncomplete => {
                        report.incomplete_count += 1;
                        RemoteCleanupRemovalBatchStatus::RemoteCleanupIncomplete
                    }
                };
                report.items.push(RemoteCleanupRemovalBatchItem {
                    alias: alias.clone(),
                    status,
                    report: Some(item_report),
                    error: None,
                });
            }
            Err(error) => {
                report.failed_count += 1;
                report.items.push(RemoteCleanupRemovalBatchItem {
                    alias: alias.clone(),
                    status: RemoteCleanupRemovalBatchStatus::Failed,
                    report: None,
                    error: Some(error),
                });
            }
        }
    }

    progress(report.items.len(), aliases.len());
    report
}

fn update_browser_proxy_response(state: &mut ApiState, body: &str) -> ApiResponse {
    let request: BrowserProxyRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let policy = match normalize_browser_proxy_policy(request) {
        Ok(policy) => policy,
        Err(message) => return json_response(400, &ApiErrorBody { error: message }),
    };

    if let Err(error) = state.config.set_browser_proxy_policy(policy) {
        return io_error_response(error);
    }
    state.local_chrome_browser = default_local_chrome_automation(&state.config);
    state.browser_automation = state
        .local_chrome_browser
        .as_ref()
        .map(|automation| Arc::clone(automation) as Arc<dyn BrowserAutomation + Send + Sync>);
    json_response(200, &config_body(state))
}

fn normalize_browser_proxy_policy(
    request: BrowserProxyRequest,
) -> Result<ChromeProxyPolicy, String> {
    match request.mode {
        ChromeProxyMode::Auto | ChromeProxyMode::InheritSystem | ChromeProxyMode::Direct => {
            Ok(ChromeProxyPolicy {
                mode: request.mode,
                server: None,
            })
        }
        ChromeProxyMode::Custom => {
            let server = request
                .server
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "custom proxy server is required".to_string())?;
            let supported_scheme = ["http://", "https://", "socks4://", "socks5://"]
                .iter()
                .any(|scheme| server.starts_with(scheme));
            if !supported_scheme || server.chars().any(char::is_whitespace) || server.contains('@')
            {
                return Err(
                    "custom proxy server must be an unauthenticated http/https/socks4/socks5 URL"
                        .to_string(),
                );
            }
            Ok(ChromeProxyPolicy {
                mode: ChromeProxyMode::Custom,
                server: Some(server.to_string()),
            })
        }
    }
}

fn parse_alias_batch_request(body: &str) -> Result<ParsedAliasBatchRequest, ApiResponse> {
    let request: AliasBatchRequest = parse_json_request(body)?;
    parse_alias_batch_parts(&request.aliases, request.task_id)
}

fn parse_trial_eligibility_request(
    body: &str,
) -> Result<ParsedTrialEligibilityRequest, ApiResponse> {
    let request: TrialEligibilityRequest = parse_json_request(body)?;
    if registration_trial_plan_code(request.trial_days).is_none() {
        return Err(account_registration_error_response(
            AccountRegistrationError::InvalidTrialDays {
                trial_days: request.trial_days,
            },
        ));
    }
    let parsed = parse_alias_batch_parts(&request.aliases, request.task_id)?;
    Ok(ParsedTrialEligibilityRequest {
        aliases: parsed.aliases,
        task_id: parsed.task_id,
        trial_days: request.trial_days,
    })
}

fn default_trial_eligibility_days() -> u32 {
    21
}

fn parse_alias_batch_parts(
    aliases: &str,
    task_id: Option<String>,
) -> Result<ParsedAliasBatchRequest, ApiResponse> {
    let aliases = split_comma_values(aliases);
    if aliases.is_empty() {
        return Err(json_response(
            400,
            &ApiErrorBody {
                error: "missing aliases".to_string(),
            },
        ));
    }

    let mut seen = BTreeSet::new();
    for alias in &aliases {
        if !seen.insert(alias.clone()) {
            return Err(json_response(
                400,
                &ApiErrorBody {
                    error: format!("account alias repeated in input: {alias}"),
                },
            ));
        }
    }

    Ok(ParsedAliasBatchRequest {
        aliases,
        task_id: normalized_optional_text(task_id),
    })
}

fn normalized_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn task_operation_kind_for_task_name(name: &str) -> TaskOperationKind {
    match name {
        "读取当前浏览器账号" => TaskOperationKind::BrowserCurrentAccount,
        "账号导入" | "导入账号" => TaskOperationKind::AccountImport,
        "手动 Cookie 导入账号" | "手动导入 Cookie 账号" => {
            TaskOperationKind::AccountManualCookieImport
        }
        "导出账号" => TaskOperationKind::AccountExport,
        "账号密码添加账号" => TaskOperationKind::AccountCredentialsAdd,
        "账号密码刷新 Cookie" => TaskOperationKind::AccountCredentialsRefresh,
        "刷新 Git Integration token 状态" => TaskOperationKind::AccountGitTokenRefresh,
        "获取 Git Integration token" => TaskOperationKind::AccountGitTokenGenerate,
        "浏览器自动登录" => TaskOperationKind::BrowserLogin,
        "规划无感换号扩展命令" => TaskOperationKind::AccountSwitchPlan,
        "预览项目迁移" => TaskOperationKind::AccountSwitchProjectPreview,
        "执行无感换号扩展命令" => TaskOperationKind::AccountSwitchExecute,
        "预览远端项目清理" => TaskOperationKind::RemoteProjectCleanupPreview,
        "清理远端项目后移除账号" | "清理远端项目并移除账号" | "移除本地账号" => {
            TaskOperationKind::AccountRemove
        }
        "更新本地账号密码" => TaskOperationKind::LocalPasswordUpdate,
        "修改 Overleaf 密码" => TaskOperationKind::OverleafPasswordChange,
        _ => TaskOperationKind::Generic,
    }
}

fn start_alias_batch_task(
    state: &mut ApiState,
    task_id: Option<&str>,
    name: &str,
    aliases: &[String],
) -> Result<(), ApiResponse> {
    start_alias_batch_task_with_retry_payload(
        state,
        task_id,
        name,
        aliases,
        TaskRetryPayload::default(),
    )
}

fn start_alias_batch_task_with_retry_payload(
    state: &mut ApiState,
    task_id: Option<&str>,
    name: &str,
    aliases: &[String],
    retry_payload: TaskRetryPayload,
) -> Result<(), ApiResponse> {
    let Some(task_id) = task_id else {
        return Ok(());
    };

    state
        .tasks
        .start_task_with_locks_operation_and_retry_payload(
            task_id,
            name,
            aliases.iter().cloned(),
            Some(task_operation_kind_for_task_name(name)),
            retry_payload,
        )
        .map_err(task_state_error_response)?;
    let total = task_progress_total(aliases.len());
    let _ = state
        .tasks
        .set_progress(task_id, 0, total, format!("{name} 已开始"));
    Ok(())
}

fn start_task_if_requested(
    state: &mut ApiState,
    task_id: Option<&str>,
    name: &str,
) -> Result<(), ApiResponse> {
    let Some(task_id) = task_id else {
        return Ok(());
    };

    state
        .tasks
        .start_task_with_operation(task_id, name, Some(task_operation_kind_for_task_name(name)))
        .map_err(task_state_error_response)?;
    let _ = state
        .tasks
        .set_progress(task_id, 0, 1, format!("{name} 已开始"));
    Ok(())
}

fn start_registration_task(
    state: &mut ApiState,
    task_id: Option<&str>,
    alias: &str,
    trial_days: u32,
    name: &str,
) -> Result<(), ApiResponse> {
    let Some(task_id) = task_id else {
        return Ok(());
    };

    state
        .tasks
        .start_task_with_locks_and_operation(
            task_id,
            name,
            [alias.to_string()],
            Some(TaskOperationKind::AccountRegistration),
        )
        .map_err(task_state_error_response)?;
    set_registration_task_step(state, task_id, trial_days, RegistrationState::InitBrowser);
    Ok(())
}

fn set_registration_task_step(
    state: &mut ApiState,
    task_id: &str,
    trial_days: u32,
    step: RegistrationState,
) {
    let workflow = RegistrationWorkflow::new(trial_days);
    let Some(index) = workflow
        .steps()
        .iter()
        .position(|candidate| *candidate == step)
    else {
        return;
    };
    let current = (index + 1) as u32;
    let total = workflow.total_steps();
    let event = RegistrationTaskEvent::StepChanged {
        task_id: task_id.to_string(),
        state: step,
    };

    let _ = state.tasks.apply_core_event(task_id, event.to_core_event());
    let _ = state.tasks.set_progress_value(task_id, current, total);
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Info,
        format!("步骤 {current}/{total}: {}", step.message()),
    );
}

fn registration_waiting_response(state: &mut ApiState, task_id: &str) -> ApiResponse {
    match state.tasks.snapshot(task_id) {
        Ok(snapshot) => json_response(202, &snapshot),
        Err(error) => task_state_error_response(error),
    }
}

fn wait_for_registration_email_code(state: &mut ApiState, task_id: &str, email: &str) {
    let _ = state.tasks.wait_for_user_input(
        task_id,
        TaskUserInputKind::EmailCode,
        format!("等待邮箱验证码: {email}"),
    );
}

fn wait_for_registration_new_credentials(state: &mut ApiState, task_id: &str, old_email: &str) {
    let _ = state.tasks.wait_for_user_input(
        task_id,
        TaskUserInputKind::NewRegistrationCredentials,
        format!("邮箱已注册，请替换注册信息: {old_email}"),
    );
}

fn wait_for_registration_captcha(state: &mut ApiState, task_id: &str) {
    let _ = state.tasks.wait_for_user_input(
        task_id,
        TaskUserInputKind::CaptchaCompleted,
        format!("等待 reCAPTCHA，最长 {RECAPTCHA_WAIT_SECONDS} 秒"),
    );
}

fn insert_registration_session(
    state: &mut ApiState,
    task_id: &str,
    entry: RegistrationSessionEntry,
) -> Result<(), ApiResponse> {
    let old = match state.registration_sessions.lock() {
        Ok(mut sessions) => sessions.insert(task_id.to_string(), entry),
        Err(_) => {
            cleanup_registration_session(entry);
            return Err(json_response(
                500,
                &ApiErrorBody {
                    error: "registration session store lock poisoned".to_string(),
                },
            ));
        }
    };
    if let Some(old) = old {
        cleanup_registration_session(old);
    }
    Ok(())
}

fn has_registration_session(state: &ApiState, task_id: &str) -> bool {
    state
        .registration_sessions
        .lock()
        .map(|sessions| sessions.contains_key(task_id))
        .unwrap_or(false)
}

fn take_registration_session(
    state: &mut ApiState,
    task_id: &str,
) -> Option<RegistrationSessionEntry> {
    state
        .registration_sessions
        .lock()
        .ok()
        .and_then(|mut sessions| sessions.remove(task_id))
}

fn cleanup_registration_session(entry: RegistrationSessionEntry) {
    let _ = entry.runtime.block_on(entry.session.cleanup());
}

fn insert_account_browser_session(
    state: &mut ApiState,
    task_id: &str,
    entry: AccountBrowserSessionEntry,
) -> Result<(), ApiResponse> {
    let old = match state.account_browser_sessions.lock() {
        Ok(mut sessions) => sessions.insert(task_id.to_string(), entry),
        Err(_) => {
            cleanup_account_browser_session(entry);
            return Err(json_response(
                500,
                &ApiErrorBody {
                    error: "account browser session store lock poisoned".to_string(),
                },
            ));
        }
    };
    if let Some(old) = old {
        cleanup_account_browser_session(old);
    }
    Ok(())
}

fn has_account_browser_session(state: &ApiState, task_id: &str) -> bool {
    state
        .account_browser_sessions
        .lock()
        .map(|sessions| sessions.contains_key(task_id))
        .unwrap_or(false)
}

fn take_account_browser_session(
    state: &mut ApiState,
    task_id: &str,
) -> Option<AccountBrowserSessionEntry> {
    state
        .account_browser_sessions
        .lock()
        .ok()
        .and_then(|mut sessions| sessions.remove(task_id))
}

fn cleanup_account_browser_session(entry: AccountBrowserSessionEntry) {
    let _ = entry.runtime.block_on(entry.session.cleanup());
}

fn wait_for_account_browser_credentials_response(
    state: &mut ApiState,
    task_id: &str,
    entry: AccountBrowserSessionEntry,
    message: &str,
) -> ApiResponse {
    if let Err(response) = insert_account_browser_session(state, task_id, entry) {
        return response;
    }
    let _ =
        state
            .tasks
            .wait_for_user_input(task_id, TaskUserInputKind::NewBrowserCredentials, message);
    let _ = state
        .tasks
        .append_log(task_id, TaskLogLevel::Warning, message);
    registration_waiting_response(state, task_id)
}

fn wait_for_account_browser_captcha_response(
    state: &mut ApiState,
    task_id: &str,
    entry: AccountBrowserSessionEntry,
) -> ApiResponse {
    if let Err(response) = insert_account_browser_session(state, task_id, entry) {
        return response;
    }
    let _ = state.tasks.wait_for_user_input(
        task_id,
        TaskUserInputKind::CaptchaCompleted,
        "检测到可见 reCAPTCHA，请在保留的浏览器窗口中完成验证",
    );
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Warning,
        "检测到可见 reCAPTCHA，浏览器会话已保留",
    );
    registration_waiting_response(state, task_id)
}

fn cancel_closed_account_browser_task(state: &mut ApiState, task_id: &str) -> ApiResponse {
    let message = "用户已关闭浏览器，任务已取消";
    match cancel_task_with_registration_release(state, task_id, message) {
        Ok(snapshot) => json_response(200, &snapshot),
        Err(error) => task_state_error_response(error),
    }
}

fn reconcile_account_browser_sessions(state: &mut ApiState) {
    let task_ids = state
        .account_browser_sessions
        .lock()
        .map(|sessions| sessions.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    for task_id in task_ids {
        let Some(entry) = take_account_browser_session(state, &task_id) else {
            continue;
        };
        let task_is_terminal_or_missing = match state.tasks.snapshot(&task_id) {
            Ok(snapshot) => matches!(
                snapshot.phase,
                crate::ServiceTaskPhase::Completed
                    | crate::ServiceTaskPhase::Failed
                    | crate::ServiceTaskPhase::Cancelled
            ),
            Err(_) => true,
        };
        if task_is_terminal_or_missing {
            cleanup_account_browser_session(entry);
            release_registration_slot(state, &task_id);
            continue;
        }
        if !entry.session.is_active() {
            cleanup_account_browser_session(entry);
            let _ = cancel_task_with_registration_release(
                state,
                &task_id,
                "用户已关闭浏览器，任务已取消",
            );
            continue;
        }

        let waiting_for_captcha = state
            .tasks
            .snapshot(&task_id)
            .ok()
            .and_then(|snapshot| snapshot.waiting_for_input)
            == Some(TaskUserInputKind::CaptchaCompleted);
        let entry = if waiting_for_captcha {
            if reconcile_waiting_account_browser_captcha(state, &task_id, entry) {
                continue;
            }
            let Some(entry) = take_account_browser_session(state, &task_id) else {
                continue;
            };
            entry
        } else {
            entry
        };
        if entry.last_activity.elapsed() >= ACCOUNT_BROWSER_SESSION_TIMEOUT {
            cleanup_account_browser_session(entry);
            fail_tracked_task(state, Some(&task_id), "浏览器会话等待超时，已自动清理");
        } else {
            if let Err(response) = insert_account_browser_session(state, &task_id, entry) {
                fail_tracked_task(state, Some(&task_id), response.body);
            }
        }
    }
}

pub fn reconcile_browser_sessions(state: &mut ApiState, now_unix: i64) {
    reconcile_account_browser_sessions(state);
    reconcile_registration_sessions(state, now_unix);
}

fn reconcile_registration_sessions(state: &mut ApiState, now_unix: i64) {
    let task_ids = state
        .registration_sessions
        .lock()
        .map(|sessions| sessions.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    for task_id in task_ids {
        let task_snapshot = state.tasks.snapshot(&task_id).ok();
        let Some(task_snapshot) = task_snapshot else {
            if let Some(entry) = take_registration_session(state, &task_id) {
                cleanup_registration_session(entry);
            }
            release_registration_slot(state, &task_id);
            continue;
        };
        if matches!(
            task_snapshot.phase,
            crate::ServiceTaskPhase::Completed
                | crate::ServiceTaskPhase::Failed
                | crate::ServiceTaskPhase::Cancelled
        ) {
            if let Some(entry) = take_registration_session(state, &task_id) {
                cleanup_registration_session(entry);
            }
            release_registration_slot(state, &task_id);
            continue;
        }

        let waiting_for_input = task_snapshot.waiting_for_input;
        let should_poll_browser = matches!(
            waiting_for_input,
            Some(TaskUserInputKind::CaptchaCompleted) | Some(TaskUserInputKind::EmailCode)
        );

        let Some(entry) = take_registration_session(state, &task_id) else {
            continue;
        };
        if !entry.session.is_active() {
            cleanup_registration_session(entry);
            let _ = cancel_task_with_registration_release(
                state,
                &task_id,
                "用户已关闭浏览器，注册任务已取消",
            );
            continue;
        }
        let wait_timeout = if waiting_for_input == Some(TaskUserInputKind::CaptchaCompleted) {
            Duration::from_secs(RECAPTCHA_WAIT_SECONDS)
        } else {
            ACCOUNT_BROWSER_SESSION_TIMEOUT
        };
        if entry.last_activity.elapsed() >= wait_timeout {
            cleanup_registration_session(entry);
            fail_tracked_task(
                state,
                Some(&task_id),
                if waiting_for_input == Some(TaskUserInputKind::CaptchaCompleted) {
                    "reCAPTCHA 等待超时，浏览器会话已自动清理"
                } else {
                    "注册浏览器会话等待超时，已自动清理"
                },
            );
            continue;
        }

        if !should_poll_browser {
            if let Err(response) = insert_registration_session(state, &task_id, entry) {
                let message = response.body;
                fail_tracked_task(state, Some(&task_id), message);
            }
            continue;
        }

        match entry
            .runtime
            .block_on(entry.session.automation().current_registration_state())
        {
            Ok(registration_state)
                if registration_session_poll_action(waiting_for_input, &registration_state)
                    == RegistrationSessionPollAction::KeepWaiting =>
            {
                if let Err(response) = insert_registration_session(state, &task_id, entry) {
                    fail_tracked_task(state, Some(&task_id), response.body);
                }
            }
            Ok(registration_state) => {
                if let Some(kind) = waiting_for_input {
                    let _ = state.tasks.discard_user_inputs(&task_id, kind);
                }
                let message = match (waiting_for_input, &registration_state) {
                    (
                        Some(TaskUserInputKind::EmailCode),
                        CdpRegistrationState::AccountCreated
                        | CdpRegistrationState::SubscriptionSucceeded,
                    ) => "已自动检测到邮箱验证完成，继续注册流程",
                    (
                        Some(TaskUserInputKind::EmailCode),
                        CdpRegistrationState::ChallengeRequired,
                    ) => "邮箱验证页面出现 reCAPTCHA，转入验证等待",
                    (Some(TaskUserInputKind::EmailCode), CdpRegistrationState::RegisteredEmail) => {
                        "检测到邮箱已注册，返回注册信息输入"
                    }
                    (
                        Some(TaskUserInputKind::CaptchaCompleted),
                        CdpRegistrationState::EmailVerificationRequired
                        | CdpRegistrationState::EmailVerificationFailed,
                    ) => "已自动检测到 reCAPTCHA 完成，转入邮箱验证码阶段",
                    _ => "已自动检测到 reCAPTCHA 完成，继续注册流程",
                };
                let _ = state
                    .tasks
                    .append_log(&task_id, TaskLogLevel::Info, message);
                let _ = continue_registration_from_state_response(
                    state,
                    &task_id,
                    entry,
                    registration_state,
                    now_unix,
                );
            }
            Err(error) => {
                cleanup_registration_session(entry);
                fail_tracked_task(state, Some(&task_id), error.to_string());
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegistrationSessionPollAction {
    KeepWaiting,
    Advance,
}

fn registration_session_poll_action(
    waiting_for_input: Option<TaskUserInputKind>,
    state: &CdpRegistrationState,
) -> RegistrationSessionPollAction {
    match waiting_for_input {
        Some(TaskUserInputKind::CaptchaCompleted) => match state {
            CdpRegistrationState::ChallengeRequired | CdpRegistrationState::Pending => {
                RegistrationSessionPollAction::KeepWaiting
            }
            _ => RegistrationSessionPollAction::Advance,
        },
        Some(TaskUserInputKind::EmailCode) => match state {
            CdpRegistrationState::EmailVerificationRequired
            | CdpRegistrationState::EmailVerificationFailed
            | CdpRegistrationState::Pending => RegistrationSessionPollAction::KeepWaiting,
            _ => RegistrationSessionPollAction::Advance,
        },
        _ => RegistrationSessionPollAction::KeepWaiting,
    }
}

fn reconcile_waiting_account_browser_captcha(
    state: &mut ApiState,
    task_id: &str,
    entry: AccountBrowserSessionEntry,
) -> bool {
    let login_state = entry
        .runtime
        .block_on(entry.session.automation().current_login_state());
    match login_state.map(account_browser_captcha_action) {
        Ok(AccountBrowserCaptchaAction::PromptCredentials) => {
            let _ = state
                .tasks
                .discard_user_inputs(task_id, TaskUserInputKind::CaptchaCompleted);
            let _ = wait_for_account_browser_credentials_response(
                state,
                task_id,
                entry,
                "邮箱或密码错误，请重新输入",
            );
            true
        }
        Ok(AccountBrowserCaptchaAction::FinalizeLogin) => {
            let email = account_browser_pending_email(&entry.context);
            match entry
                .runtime
                .block_on(entry.session.automation().finalize_login_result(email))
            {
                Ok(login) => {
                    let _ = state
                        .tasks
                        .discard_user_inputs(task_id, TaskUserInputKind::CaptchaCompleted);
                    let _ = continue_account_browser_session_from_login_result(
                        state, task_id, entry, login,
                    );
                }
                Err(error) => {
                    cleanup_account_browser_session(entry);
                    fail_tracked_task(state, Some(task_id), error.to_string());
                }
            }
            true
        }
        Ok(AccountBrowserCaptchaAction::ResumeAutomation) => {
            let _ = state
                .tasks
                .discard_user_inputs(task_id, TaskUserInputKind::CaptchaCompleted);
            let _ = continue_account_browser_session_response(state, task_id, entry);
            true
        }
        Ok(AccountBrowserCaptchaAction::KeepWaiting) | Err(_) => {
            if let Err(response) = insert_account_browser_session(state, task_id, entry) {
                fail_tracked_task(state, Some(task_id), response.body);
                return true;
            }
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccountBrowserCaptchaAction {
    KeepWaiting,
    PromptCredentials,
    FinalizeLogin,
    ResumeAutomation,
}

fn account_browser_captcha_action(state: CdpLoginState) -> AccountBrowserCaptchaAction {
    match state {
        CdpLoginState::InvalidCredentials => AccountBrowserCaptchaAction::PromptCredentials,
        CdpLoginState::Success => AccountBrowserCaptchaAction::FinalizeLogin,
        CdpLoginState::RobotVerificationBlocked | CdpLoginState::RegisteredEmail => {
            AccountBrowserCaptchaAction::ResumeAutomation
        }
        CdpLoginState::ChallengeRequired
        | CdpLoginState::ChallengePossible
        | CdpLoginState::Pending => AccountBrowserCaptchaAction::KeepWaiting,
    }
}

fn account_browser_pending_email(context: &AccountBrowserSessionContext) -> Option<String> {
    let AccountBrowserSessionContext::ExistingTrialCredentials { email, .. } = context;
    Some(email.clone())
}

fn complete_tracked_task<T: Serialize>(
    state: &mut ApiState,
    task_id: Option<&str>,
    message: &str,
    result: &T,
) {
    if let Some(task_id) = task_id {
        let total = task_completion_progress_total(state, task_id);
        let _ = state.tasks.set_progress(task_id, total, total, message);
        let result = serde_json::to_value(result).ok();
        let _ = state.tasks.complete_task(task_id, message, result);
        release_registration_slot(state, task_id);
    }
}

fn finish_credential_batch_task(
    state: &mut ApiState,
    task_id: Option<&str>,
    operation: &str,
    report: &CredentialBatchReport,
) {
    let Some(task_id) = task_id else {
        return;
    };

    if report.failed_count == 0 {
        complete_tracked_task(state, Some(task_id), &format!("{operation}完成"), report);
        return;
    }

    let message = format!("{operation}未完全完成：{} 个账号失败", report.failed_count);
    let total = task_completion_progress_total(state, task_id);
    let _ = state.tasks.set_progress(task_id, total, total, &message);
    let result = serde_json::to_value(report).ok();
    let _ = state.tasks.fail_task_with_result(task_id, message, result);
}

fn finish_overleaf_password_change_batch_task(
    state: &mut ApiState,
    task_id: Option<&str>,
    report: &OverleafPasswordChangeBatchReport,
) {
    let Some(task_id) = task_id else {
        return;
    };
    if report.failed_count == 0 {
        complete_tracked_task(state, Some(task_id), "Overleaf 密码修改流程完成", report);
        return;
    }

    let message = format!(
        "Overleaf 密码修改未完全完成：{} 个账号失败",
        report.failed_count
    );
    let total = task_completion_progress_total(state, task_id);
    let _ = state.tasks.set_progress(task_id, total, total, &message);
    let result = serde_json::to_value(report).ok();
    let _ = state.tasks.fail_task_with_result(task_id, message, result);
}

fn finish_git_token_batch_task(
    state: &mut ApiState,
    task_id: Option<&str>,
    report: &AccountGitTokenRefreshBatchReport,
    operation: CredentialBrowserOperation,
) {
    let Some(task_id) = task_id else {
        return;
    };
    if report.failed_count == 0 {
        complete_tracked_task(
            state,
            Some(task_id),
            operation.git_token_batch_success_message(),
            report,
        );
        return;
    }

    let message = format!(
        "{}：{} 个账号失败",
        operation.git_token_batch_failure_prefix(),
        report.failed_count,
    );
    let total = task_completion_progress_total(state, task_id);
    let _ = state.tasks.set_progress(task_id, total, total, &message);
    let result = serde_json::to_value(report).ok();
    let _ = state.tasks.fail_task_with_result(task_id, message, result);
}

fn task_progress_total(unit_count: usize) -> u32 {
    (unit_count.min(u32::MAX as usize) as u32).max(1)
}

fn task_completion_progress_total(state: &ApiState, task_id: &str) -> u32 {
    state
        .tasks
        .snapshot(task_id)
        .ok()
        .and_then(|snapshot| snapshot.progress.map(|progress| progress.total))
        .unwrap_or(1)
        .max(1)
}

fn handle_registration_error(
    state: &mut ApiState,
    task_id: Option<&str>,
    error: AccountRegistrationError,
) -> ApiResponse {
    match &error {
        AccountRegistrationError::RegisteredEmail { email } => {
            if let Some(task_id) = task_id {
                wait_for_registration_new_credentials(state, task_id, email);
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Warning,
                    format!("邮箱已注册: {email}"),
                );
            }
        }
        AccountRegistrationError::EmailCodeRequired { email } => {
            if let Some(task_id) = task_id {
                wait_for_registration_email_code(state, task_id, email);
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Warning,
                    format!("等待邮箱验证码: {email}"),
                );
            }
        }
        AccountRegistrationError::ChallengeRequired { .. } => {
            if let Some(task_id) = task_id {
                wait_for_registration_captcha(state, task_id);
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Warning,
                    "等待用户完成 reCAPTCHA",
                );
            }
        }
        _ => fail_tracked_task(state, task_id, account_registration_error_message(&error)),
    }

    account_registration_error_response(error)
}

fn fail_tracked_task(state: &mut ApiState, task_id: Option<&str>, message: impl Into<String>) {
    if let Some(task_id) = task_id {
        let _ = state.tasks.fail_task(task_id, message);
        release_registration_slot(state, task_id);
    }
}

fn fail_tracked_task_with_result<T: Serialize>(
    state: &mut ApiState,
    task_id: Option<&str>,
    message: impl Into<String>,
    result: &T,
) {
    if let Some(task_id) = task_id {
        let result = serde_json::to_value(result).ok();
        let _ = state.tasks.fail_task_with_result(task_id, message, result);
        release_registration_slot(state, task_id);
    }
}

fn block_on_api<F, T>(future: F) -> Result<T, ApiResponse>
where
    F: Future<Output = T>,
{
    let runtime = build_api_runtime()?;
    Ok(runtime.block_on(future))
}

fn build_api_runtime() -> Result<tokio::runtime::Runtime, ApiResponse> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            json_response(
                500,
                &ApiErrorBody {
                    error: format!("failed to create async runtime: {error}"),
                },
            )
        })
}

fn load_accounts(state: &ApiState) -> io::Result<overleaf_storage::AccountsDocument> {
    AccountStore::new(state.config.accounts_path()).load()
}

fn load_display_accounts(state: &ApiState) -> io::Result<overleaf_storage::AccountsDocument> {
    let mut document = load_accounts(state)?;
    let runtime = runtime_state(state);
    // 历史换号记录仍用于迁移；运行中标记仅依据本次浏览器身份。
    document.current = runtime.browser_account_email.as_deref().and_then(|email| {
        document.accounts.iter().find_map(|(alias, account)| {
            account
                .email
                .as_deref()
                .filter(|saved| saved.eq_ignore_ascii_case(email))
                .map(|_| alias.clone())
        })
    });
    Ok(document)
}

fn runtime_state(state: &ApiState) -> DashboardRuntimeState {
    let mut runtime = state.runtime.clone();
    let bridge_client_count = extension_bridge_client_count(state);
    runtime.extension_bridge_configured = state.extension_bridge_executor.is_some();
    runtime.extension_client_count = bridge_client_count;
    runtime.extension_connected = bridge_client_count > 0;
    if !runtime.extension_connected {
        runtime.browser_account_email = None;
    }
    runtime
}

fn extension_bridge_client_count(state: &ApiState) -> usize {
    let Some(executor) = state.extension_bridge_executor.as_ref() else {
        return 0;
    };

    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return 0;
    };
    runtime
        .block_on(executor.extension_client_count())
        .unwrap_or(0)
}

fn normalize_path(target: &str) -> String {
    let without_query = target
        .split_once('?')
        .map(|(path, _)| path)
        .unwrap_or(target);
    if without_query.trim().is_empty() {
        "/".to_string()
    } else if without_query.starts_with('/') {
        without_query.to_string()
    } else {
        format!("/{without_query}")
    }
}

fn query_param(target: &str, name: &str) -> Option<String> {
    let (_, query) = target.split_once('?')?;
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        (percent_decode(key) == name).then(|| percent_decode(value))
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let first = hex_value(bytes[index + 1]);
                let second = hex_value(bytes[index + 2]);
                if let (Some(first), Some(second)) = (first, second) {
                    output.push(first * 16 + second);
                    index += 3;
                } else {
                    output.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&output).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
