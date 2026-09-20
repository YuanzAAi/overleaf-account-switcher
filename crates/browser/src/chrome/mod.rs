pub mod profiles;
pub mod window_focus;

use std::collections::BTreeSet;
use std::env;
use std::fmt;
use std::fs;
use std::future::Future;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sysinfo::System;
use tokio::time::{sleep, timeout};

use crate::stripe_frame::{
    fill_stripe_address_oopif_frame, fill_stripe_payment_oopif_field, StripeOopifError,
    StripePaymentField,
};
use crate::{
    parse_page_websocket_url, BrowserAutoLoginInput, BrowserAutoLoginResult, BrowserAutomation,
    BrowserAutomationError, BrowserAutomationResult, BrowserSessionPolicy, BrowserTaskKind,
    CdpLoginAutomation, CdpLoginOptions, CdpLoginState, CdpSession, CdpWebSocketTransport,
    ChromeLaunchOptions, ChromeLaunchPlan, ChromeProxyAttempt, ChromeProxyPolicy,
    CredentialsLoginInput, GitTokenInput, GitTokenPageState, GitTokenResult, LoginResult,
    PasswordChangeInput, PasswordChangeResult, RegistrationInput, RegistrationResult,
    StripeAddressInput, StripeFrameActionResult, StripePaymentFramesResult, StripePaymentInput,
    CDP_LIST_ENDPOINT_PATH, OVERLEAF_LOGIN_URL, OVERLEAF_PROJECT_URL, OVERLEAF_REGISTER_URL,
    OVERLEAF_SETTINGS_URL,
};

pub const ENV_CHROME_PATH: &str = "OVERLEAF_SWITCHER_CHROME_PATH";
pub const ENV_CDP_PORT_START: &str = "OVERLEAF_SWITCHER_CDP_PORT_START";
pub const DEFAULT_CDP_PORT_START: u16 = 9222;
pub const DEFAULT_CDP_PORT_ATTEMPTS: u16 = 128;
pub const DEFAULT_CHROME_STARTUP_TIMEOUT_MS: u64 = 15_000;
pub const DEFAULT_CHROME_STARTUP_POLL_MS: u64 = 250;
pub const DEFAULT_CHROME_EXIT_WAIT_MS: u64 = 5_000;
const MAX_DEBUGGER_HTTP_RESPONSE_BYTES: usize = 1024 * 1024;
const STRIPE_TARGET_WAIT_ATTEMPTS: usize = 60;
const STRIPE_TARGET_WAIT_DELAY_MS: u64 = 250;
const STRIPE_ADDRESS_FIELD_READY_RETRY_ATTEMPTS: usize = 2;
// 支付 iframe 可能已创建，但重新挂载的输入字段尚未就绪。
const STRIPE_PAYMENT_FIELD_READY_RETRY_ATTEMPTS: usize = 20;
const COOKIE_LOGIN_STATUS_CHECKS: usize = 10;
const COOKIE_LOGIN_POLL_DELAY_MS: u64 = 250;
const CDP_CLEANUP_STEP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalChromeAutomationConfig {
    pub chrome_executable: PathBuf,
    pub tmp_dir: PathBuf,
    pub debug_port_start: u16,
    pub startup_timeout_ms: u64,
    pub startup_poll_ms: u64,
    pub exit_wait_ms: u64,
    pub proxy_policy: ChromeProxyPolicy,
}

#[derive(Debug)]
pub struct LocalChromeBrowserAutomation {
    config: LocalChromeAutomationConfig,
    next_session: AtomicU64,
    debug_ports: Arc<DebugPortRegistry>,
}

pub struct LocalChromeCdpSession {
    launched: Option<LaunchedChrome>,
    automation: Option<CdpLoginAutomation<CdpWebSocketTransport>>,
}

#[derive(Clone, Copy)]
enum StripeFillStep<'a> {
    Address(&'a StripeAddressInput),
    Payment {
        input: &'a StripePaymentInput,
        field: StripePaymentField,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalChromeAutomationError {
    ChromeNotFound,
    NoDebugPortAvailable { start: u16, attempts: u16 },
    Io { operation: String, message: String },
    Debugger { message: String },
    Timeout { operation: String },
}

struct LaunchedChrome {
    plan: ChromeLaunchPlan,
    child: Child,
    websocket_url: String,
    exit_wait_ms: u64,
    _debug_port_lease: DebugPortLease,
}

#[derive(Debug, Default)]
struct DebugPortRegistry {
    reserved: Mutex<BTreeSet<u16>>,
}

struct DebugPortLease {
    port: u16,
    registry: Arc<DebugPortRegistry>,
}

impl DebugPortRegistry {
    fn reserve(self: &Arc<Self>, start: u16, attempts: u16) -> io::Result<DebugPortLease> {
        let mut reserved = self
            .reserved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if start == 0 {
            for _ in 0..attempts.max(1) {
                let listener = TcpListener::bind(("127.0.0.1", 0))?;
                let port = listener.local_addr()?.port();
                drop(listener);
                if reserved.insert(port) {
                    return Ok(DebugPortLease {
                        port,
                        registry: Arc::clone(self),
                    });
                }
            }
        } else {
            for offset in 0..attempts {
                let Some(port) = start.checked_add(offset) else {
                    break;
                };
                if reserved.contains(&port) {
                    continue;
                }
                if TcpListener::bind(("127.0.0.1", port)).is_ok() {
                    reserved.insert(port);
                    return Ok(DebugPortLease {
                        port,
                        registry: Arc::clone(self),
                    });
                }
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "no available local debug port",
        ))
    }
}

impl DebugPortLease {
    fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for DebugPortLease {
    fn drop(&mut self) {
        let mut reserved = self
            .registry
            .reserved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        reserved.remove(&self.port);
    }
}

impl LocalChromeAutomationConfig {
    pub fn new(chrome_executable: impl Into<PathBuf>, tmp_dir: impl Into<PathBuf>) -> Self {
        Self {
            chrome_executable: chrome_executable.into(),
            tmp_dir: tmp_dir.into(),
            debug_port_start: env::var(ENV_CDP_PORT_START)
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(DEFAULT_CDP_PORT_START),
            startup_timeout_ms: DEFAULT_CHROME_STARTUP_TIMEOUT_MS,
            startup_poll_ms: DEFAULT_CHROME_STARTUP_POLL_MS,
            exit_wait_ms: DEFAULT_CHROME_EXIT_WAIT_MS,
            proxy_policy: ChromeProxyPolicy::default(),
        }
    }

    pub fn with_debug_port_start(mut self, debug_port_start: u16) -> Self {
        self.debug_port_start = debug_port_start;
        self
    }

    pub fn with_proxy_policy(mut self, proxy_policy: ChromeProxyPolicy) -> Self {
        self.proxy_policy = proxy_policy;
        self
    }
}

impl LocalChromeBrowserAutomation {
    pub fn new(config: LocalChromeAutomationConfig) -> Self {
        Self {
            config,
            next_session: AtomicU64::new(1),
            debug_ports: Arc::new(DebugPortRegistry::default()),
        }
    }

    pub fn from_environment(
        tmp_dir: impl Into<PathBuf>,
    ) -> Result<Self, LocalChromeAutomationError> {
        let chrome_executable =
            detect_chrome_executable().ok_or(LocalChromeAutomationError::ChromeNotFound)?;
        Ok(Self::new(LocalChromeAutomationConfig::new(
            chrome_executable,
            tmp_dir,
        )))
    }

    pub fn config(&self) -> &LocalChromeAutomationConfig {
        &self.config
    }

    pub async fn start_registration_cdp_session(
        &self,
    ) -> BrowserAutomationResult<LocalChromeCdpSession> {
        self.start_registration_cdp_session_for_attempt(ChromeProxyAttempt::Primary)
            .await
    }

    pub async fn start_registration_cdp_session_for_attempt(
        &self,
        proxy_attempt: ChromeProxyAttempt,
    ) -> BrowserAutomationResult<LocalChromeCdpSession> {
        self.start_cdp_session(
            BrowserTaskKind::Registration,
            OVERLEAF_REGISTER_URL,
            proxy_attempt,
        )
        .await
    }

    pub async fn start_account_login_cdp_session(
        &self,
    ) -> BrowserAutomationResult<LocalChromeCdpSession> {
        self.start_account_login_cdp_session_for_attempt(ChromeProxyAttempt::Primary)
            .await
    }

    pub async fn start_account_login_cdp_session_for_attempt(
        &self,
        proxy_attempt: ChromeProxyAttempt,
    ) -> BrowserAutomationResult<LocalChromeCdpSession> {
        self.start_cdp_session(
            BrowserTaskKind::AccountPasswordLogin,
            OVERLEAF_LOGIN_URL,
            proxy_attempt,
        )
        .await
    }

    async fn start_cdp_session(
        &self,
        task_kind: BrowserTaskKind,
        initial_url: &'static str,
        proxy_attempt: ChromeProxyAttempt,
    ) -> BrowserAutomationResult<LocalChromeCdpSession> {
        let mut launched = self
            .launch(task_kind, initial_url, proxy_attempt)
            .await
            .map_err(local_chrome_error)?;

        let transport = match CdpWebSocketTransport::connect(&launched.websocket_url).await {
            Ok(transport) => transport,
            Err(error) => {
                let _ = launched.cleanup().await;
                return Err(BrowserAutomationError::Other {
                    message: format!("connect CDP websocket: {}", error.message),
                });
            }
        };

        let automation = CdpLoginAutomation::new(CdpSession::new(transport));
        automation.use_current_page_for_next_submission();
        if task_kind == BrowserTaskKind::AccountPasswordLogin {
            automation.return_immediately_on_visible_challenge();
        }
        Ok(LocalChromeCdpSession::new(launched, automation))
    }

    async fn run_cdp_automation<T, F, Fut>(
        &self,
        task_kind: BrowserTaskKind,
        initial_url: &'static str,
        proxy_attempt: ChromeProxyAttempt,
        action: F,
    ) -> BrowserAutomationResult<T>
    where
        F: FnOnce(CdpLoginAutomation<CdpWebSocketTransport>) -> Fut + Send,
        Fut: Future<Output = BrowserAutomationResult<T>> + Send,
    {
        let mut launched = self
            .launch(task_kind, initial_url, proxy_attempt)
            .await
            .map_err(local_chrome_error)?;

        let result = match CdpWebSocketTransport::connect(&launched.websocket_url).await {
            Ok(transport) => {
                let automation = CdpLoginAutomation::new(CdpSession::new(transport));
                automation.use_current_page_for_next_submission();
                action(automation).await
            }
            Err(error) => Err(BrowserAutomationError::Other {
                message: format!("connect CDP websocket: {}", error.message),
            }),
        };

        let _ = launched.cleanup().await;
        result
    }

    async fn launch(
        &self,
        task_kind: BrowserTaskKind,
        initial_url: &'static str,
        proxy_attempt: ChromeProxyAttempt,
    ) -> Result<LaunchedChrome, LocalChromeAutomationError> {
        let debug_port_lease = self
            .debug_ports
            .reserve(self.config.debug_port_start, DEFAULT_CDP_PORT_ATTEMPTS)
            .map_err(|_| LocalChromeAutomationError::NoDebugPortAvailable {
                start: self.config.debug_port_start,
                attempts: DEFAULT_CDP_PORT_ATTEMPTS,
            })?;
        let debug_port = debug_port_lease.port();
        let profile_dir = BrowserSessionPolicy::for_task(task_kind)
            .profile_dir_in(&self.config.tmp_dir, self.next_profile_nonce(task_kind));

        fs::create_dir_all(&profile_dir).map_err(|error| io_error("create profile dir", error))?;

        let options = ChromeLaunchOptions::new(
            self.config.chrome_executable.clone(),
            task_kind,
            debug_port,
            profile_dir,
        )
        .with_initial_url(initial_url)
        .with_proxy_route(self.config.proxy_policy.route_for_attempt(proxy_attempt));
        let plan = ChromeLaunchPlan::from_options(options);
        let child = Command::new(&plan.chrome_executable)
            .args(&plan.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| io_error("launch chrome", error))?;

        let mut launched = LaunchedChrome {
            plan,
            child,
            websocket_url: String::new(),
            exit_wait_ms: self.config.exit_wait_ms,
            _debug_port_lease: debug_port_lease,
        };

        match wait_for_page_websocket_url(
            launched.plan.debug_port,
            initial_url,
            Duration::from_millis(self.config.startup_timeout_ms),
            Duration::from_millis(self.config.startup_poll_ms),
        )
        .await
        {
            Ok(websocket_url) => {
                launched.websocket_url = websocket_url;
                Ok(launched)
            }
            Err(error) => {
                let _ = launched.cleanup().await;
                Err(error)
            }
        }
    }

    fn next_profile_nonce(&self, task_kind: BrowserTaskKind) -> String {
        let sequence = self.next_session.fetch_add(1, Ordering::Relaxed);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        format!("{}-{}-{}", task_slug(task_kind), millis, sequence)
    }
}

#[async_trait::async_trait]
impl BrowserAutomation for LocalChromeBrowserAutomation {
    async fn login_with_credentials(
        &self,
        input: CredentialsLoginInput,
    ) -> BrowserAutomationResult<LoginResult> {
        let primary_input = input.clone();
        let result = self
            .run_cdp_automation(
                BrowserTaskKind::AccountPasswordLogin,
                OVERLEAF_LOGIN_URL,
                ChromeProxyAttempt::Primary,
                |automation| async move { automation.login_with_credentials(primary_input).await },
            )
            .await;
        if matches!(
            result,
            Err(BrowserAutomationError::RobotVerificationBlocked)
        ) && self.config.proxy_policy.has_fallback()
        {
            return self
                .run_cdp_automation(
                    BrowserTaskKind::AccountPasswordLogin,
                    OVERLEAF_LOGIN_URL,
                    ChromeProxyAttempt::Fallback,
                    |automation| async move { automation.login_with_credentials(input).await },
                )
                .await;
        }
        result
    }

    async fn register_and_subscribe(
        &self,
        input: RegistrationInput,
    ) -> BrowserAutomationResult<RegistrationResult> {
        let primary_input = input.clone();
        let result = self
            .run_cdp_automation(
                BrowserTaskKind::Registration,
                OVERLEAF_REGISTER_URL,
                ChromeProxyAttempt::Primary,
                |automation| async move { automation.register_and_subscribe(primary_input).await },
            )
            .await;
        if matches!(
            result,
            Err(BrowserAutomationError::RobotVerificationBlocked)
        ) && self.config.proxy_policy.has_fallback()
        {
            return self
                .run_cdp_automation(
                    BrowserTaskKind::Registration,
                    OVERLEAF_REGISTER_URL,
                    ChromeProxyAttempt::Fallback,
                    |automation| async move { automation.register_and_subscribe(input).await },
                )
                .await;
        }
        result
    }

    async fn extract_git_token_with_cookie(
        &self,
        input: GitTokenInput,
    ) -> BrowserAutomationResult<GitTokenResult> {
        let primary_input = input.clone();
        let result = self
            .run_cdp_automation(
                BrowserTaskKind::GitToken,
                OVERLEAF_SETTINGS_URL,
                ChromeProxyAttempt::Primary,
                |automation| async move {
                    automation
                        .extract_git_token_with_cookie(primary_input)
                        .await
                },
            )
            .await;
        if matches!(
            result,
            Err(BrowserAutomationError::RobotVerificationBlocked)
        ) && self.config.proxy_policy.has_fallback()
        {
            return self
                .run_cdp_automation(
                    BrowserTaskKind::GitToken,
                    OVERLEAF_SETTINGS_URL,
                    ChromeProxyAttempt::Fallback,
                    |automation| async move {
                        automation.extract_git_token_with_cookie(input).await
                    },
                )
                .await;
        }
        result
    }

    async fn read_git_token_state_with_cookie(
        &self,
        input: GitTokenInput,
    ) -> BrowserAutomationResult<GitTokenPageState> {
        let primary_input = input.clone();
        let result = self
            .run_cdp_automation(
                BrowserTaskKind::GitToken,
                OVERLEAF_SETTINGS_URL,
                ChromeProxyAttempt::Primary,
                |automation| async move {
                    automation
                        .read_git_token_state_with_cookie(primary_input)
                        .await
                },
            )
            .await;
        if matches!(
            result,
            Err(BrowserAutomationError::RobotVerificationBlocked)
        ) && self.config.proxy_policy.has_fallback()
        {
            return self
                .run_cdp_automation(
                    BrowserTaskKind::GitToken,
                    OVERLEAF_SETTINGS_URL,
                    ChromeProxyAttempt::Fallback,
                    |automation| async move {
                        automation.read_git_token_state_with_cookie(input).await
                    },
                )
                .await;
        }
        result
    }

    async fn change_password_with_cookie(
        &self,
        input: PasswordChangeInput,
    ) -> BrowserAutomationResult<PasswordChangeResult> {
        let primary_input = input.clone();
        let result = self
            .run_cdp_automation(
                BrowserTaskKind::PasswordChange,
                OVERLEAF_SETTINGS_URL,
                ChromeProxyAttempt::Primary,
                |automation| async move {
                    automation.change_password_with_cookie(primary_input).await
                },
            )
            .await;
        if matches!(
            result,
            Err(BrowserAutomationError::RobotVerificationBlocked)
        ) && self.config.proxy_policy.has_fallback()
        {
            return self
                .run_cdp_automation(
                    BrowserTaskKind::PasswordChange,
                    OVERLEAF_SETTINGS_URL,
                    ChromeProxyAttempt::Fallback,
                    |automation| async move { automation.change_password_with_cookie(input).await },
                )
                .await;
        }
        result
    }

    async fn open_user_window_with_cookie(
        &self,
        input: BrowserAutoLoginInput,
    ) -> BrowserAutomationResult<BrowserAutoLoginResult> {
        let mut launched = self
            .launch(
                BrowserTaskKind::BrowserAutoLogin,
                OVERLEAF_PROJECT_URL,
                ChromeProxyAttempt::Primary,
            )
            .await
            .map_err(local_chrome_error)?;

        let result = match CdpWebSocketTransport::connect(&launched.websocket_url).await {
            Ok(transport) => {
                let mut session = CdpSession::new(transport);
                match session
                    .set_overleaf_session_cookie(
                        input.overleaf_session.expose(),
                        input.cookie_expiry,
                    )
                    .await
                {
                    Ok(()) => {
                        session
                            .navigate(OVERLEAF_PROJECT_URL)
                            .await
                            .map_err(local_chrome_cdp_error)?;
                        verify_cookie_login_session(session).await
                    }
                    Err(error) => Err(local_chrome_cdp_error(error)),
                }
            }
            Err(error) => Err(BrowserAutomationError::Other {
                message: format!("connect CDP websocket: {}", error.message),
            }),
        };

        if let Err(error) = result {
            let _ = launched.cleanup().await;
            return Err(error);
        }

        Ok(launched.detach_user_window_cleanup())
    }
}

async fn verify_cookie_login_session(
    session: CdpSession<CdpWebSocketTransport>,
) -> BrowserAutomationResult<()> {
    let automation = CdpLoginAutomation::with_options(
        session,
        CdpLoginOptions {
            login_url: OVERLEAF_LOGIN_URL.to_string(),
            status_checks: 1,
            poll_delay_ms: 0,
        },
    );
    let mut consecutive_successes = 0;

    for attempt in 0..COOKIE_LOGIN_STATUS_CHECKS {
        match automation.current_login_state().await? {
            CdpLoginState::Success => {
                consecutive_successes += 1;
                if consecutive_successes >= 2 {
                    return Ok(());
                }
            }
            CdpLoginState::Pending => {
                consecutive_successes = 0;
            }
            CdpLoginState::ChallengeRequired
            | CdpLoginState::ChallengePossible
            | CdpLoginState::RobotVerificationBlocked
            | CdpLoginState::InvalidCredentials
            | CdpLoginState::RegisteredEmail => {
                return Err(BrowserAutomationError::InvalidCredentials);
            }
        }

        if attempt + 1 < COOKIE_LOGIN_STATUS_CHECKS {
            sleep(Duration::from_millis(COOKIE_LOGIN_POLL_DELAY_MS)).await;
        }
    }

    Err(BrowserAutomationError::InvalidCredentials)
}

impl LocalChromeCdpSession {
    fn new(
        launched: LaunchedChrome,
        automation: CdpLoginAutomation<CdpWebSocketTransport>,
    ) -> Self {
        Self {
            launched: Some(launched),
            automation: Some(automation),
        }
    }

    pub fn automation(&self) -> &CdpLoginAutomation<CdpWebSocketTransport> {
        self.automation
            .as_ref()
            .expect("local chrome CDP session already cleaned up")
    }

    pub fn profile_dir(&self) -> &Path {
        &self.launched().plan.user_data_dir
    }

    pub fn debug_port(&self) -> u16 {
        self.launched().plan.debug_port
    }

    pub fn process_id(&self) -> Option<u32> {
        Some(self.launched().child.id())
    }

    pub async fn fill_registration_payment_frames(
        &self,
        address: StripeAddressInput,
        payment: StripePaymentInput,
    ) -> BrowserAutomationResult<StripePaymentFramesResult> {
        let address_result = self
            .fill_stripe_step(StripeFillStep::Address(&address))
            .await?;
        let mut payment_result = StripeFrameActionResult::empty_success();
        for field in StripePaymentField::ALL {
            payment_result.merge(
                self.fill_stripe_step(StripeFillStep::Payment {
                    input: &payment,
                    field,
                })
                .await?,
            );
        }
        Ok(StripePaymentFramesResult {
            address: address_result,
            payment: payment_result,
        })
    }

    async fn fill_stripe_step(
        &self,
        step: StripeFillStep<'_>,
    ) -> BrowserAutomationResult<StripeFrameActionResult> {
        let mut field_retry_attempts = 0;
        for attempt in 1..=STRIPE_TARGET_WAIT_ATTEMPTS {
            let response = match fetch_debugger_targets(self.debug_port()) {
                Ok(response) => response,
                Err(_) if attempt < STRIPE_TARGET_WAIT_ATTEMPTS => {
                    sleep(Duration::from_millis(STRIPE_TARGET_WAIT_DELAY_MS)).await;
                    continue;
                }
                Err(error) => {
                    return Err(BrowserAutomationError::ExternalDriver {
                        message: format!("read Chrome OOPIF targets: {error}"),
                    });
                }
            };
            let targets_json = debugger_http_response_body(&response);
            let fill_result = match step {
                StripeFillStep::Address(input) => {
                    fill_stripe_address_oopif_frame(targets_json, input).await
                }
                StripeFillStep::Payment { input, field } => {
                    fill_stripe_payment_oopif_field(targets_json, input, field).await
                }
            };
            match fill_result {
                Ok(result) => return Ok(result),
                Err(error)
                    if stripe_field_error_is_retryable(&error, field_retry_attempts)
                        && attempt < STRIPE_TARGET_WAIT_ATTEMPTS =>
                {
                    field_retry_attempts += 1;
                    sleep(Duration::from_millis(STRIPE_TARGET_WAIT_DELAY_MS)).await;
                }
                Err(error)
                    if stripe_target_error_is_retryable(&error)
                        && attempt < STRIPE_TARGET_WAIT_ATTEMPTS =>
                {
                    sleep(Duration::from_millis(STRIPE_TARGET_WAIT_DELAY_MS)).await;
                }
                Err(error) if stripe_target_error_is_retryable(&error) => {
                    return Err(BrowserAutomationError::Timeout {
                        operation: format!("wait for Stripe payment frames: {error}"),
                    });
                }
                Err(error) => {
                    return Err(BrowserAutomationError::ExternalDriver {
                        message: error.to_string(),
                    });
                }
            }
        }

        unreachable!("Stripe target wait loop always returns")
    }

    pub fn is_active(&self) -> bool {
        scoped_chrome_processes_running(self.profile_dir())
    }

    pub async fn cleanup(mut self) -> Result<(), LocalChromeAutomationError> {
        self.automation.take();
        match self.launched.take() {
            Some(mut launched) => launched.cleanup().await,
            None => Ok(()),
        }
    }

    fn launched(&self) -> &LaunchedChrome {
        self.launched
            .as_ref()
            .expect("local chrome CDP session already cleaned up")
    }
}

fn stripe_target_error_is_retryable(error: &StripeOopifError) -> bool {
    match error {
        StripeOopifError::MissingTarget { .. }
        | StripeOopifError::Connect { .. }
        | StripeOopifError::TargetReload { .. } => true,
        StripeOopifError::Session { message, .. } => message
            .to_ascii_lowercase()
            .contains("cdp transport failed"),
        StripeOopifError::InvalidTargetList { .. } | StripeOopifError::MissingField { .. } => false,
    }
}

fn stripe_field_error_is_retryable(error: &StripeOopifError, attempts: usize) -> bool {
    match error {
        StripeOopifError::MissingField {
            component: "address",
            ..
        } => attempts < STRIPE_ADDRESS_FIELD_READY_RETRY_ATTEMPTS,
        StripeOopifError::MissingField {
            component: "payment",
            ..
        } => attempts < STRIPE_PAYMENT_FIELD_READY_RETRY_ATTEMPTS,
        _ => false,
    }
}

impl Drop for LocalChromeCdpSession {
    fn drop(&mut self) {
        self.automation.take();
        if let Some(mut launched) = self.launched.take() {
            let exited = matches!(launched.child.try_wait(), Ok(Some(_)));
            if !exited && launched.child.kill().is_ok() {
                let _ = launched.child.wait();
            }
            terminate_scoped_chrome_processes(&launched.plan.user_data_dir);
            let _ = wait_for_scoped_chrome_exit_blocking(
                &launched.plan.user_data_dir,
                Duration::from_secs(2),
            );
            let _ = remove_profile_dir_with_retry_blocking(
                &launched.plan.user_data_dir,
                crate::PROFILE_REMOVE_RETRY_ATTEMPTS,
                Duration::from_millis(crate::PROFILE_REMOVE_RETRY_DELAY_MS),
            );
        }
    }
}

impl LaunchedChrome {
    async fn cleanup(&mut self) -> Result<(), LocalChromeAutomationError> {
        if !self.websocket_url.is_empty() {
            if let Ok(Ok(transport)) = timeout(
                CDP_CLEANUP_STEP_TIMEOUT,
                CdpWebSocketTransport::connect(&self.websocket_url),
            )
            .await
            {
                let mut session = CdpSession::new(transport);
                let _ = timeout(CDP_CLEANUP_STEP_TIMEOUT, session.close_browser()).await;
            }
        }

        wait_for_child_exit(&mut self.child, Duration::from_millis(self.exit_wait_ms)).await?;
        if !wait_for_scoped_chrome_exit_blocking(
            &self.plan.user_data_dir,
            Duration::from_millis(self.exit_wait_ms),
        ) {
            terminate_scoped_chrome_processes(&self.plan.user_data_dir);
            let _ = wait_for_scoped_chrome_exit_blocking(
                &self.plan.user_data_dir,
                Duration::from_secs(2),
            );
        }
        remove_profile_dir_with_retry(
            &self.plan.user_data_dir,
            crate::PROFILE_REMOVE_RETRY_ATTEMPTS,
            Duration::from_millis(crate::PROFILE_REMOVE_RETRY_DELAY_MS),
        )
        .await
    }

    fn detach_user_window_cleanup(mut self) -> BrowserAutoLoginResult {
        let profile_dir = self.plan.user_data_dir.clone();
        let debug_port = self.plan.debug_port;
        let process_id = Some(self.child.id());
        let cleanup_profile_dir = profile_dir.clone();
        std::thread::Builder::new()
            .name("overleaf-user-window-cleanup".to_string())
            .spawn(move || {
                let _ = self.child.wait();
                wait_for_scoped_chrome_start_blocking(&cleanup_profile_dir, Duration::from_secs(5));
                while scoped_chrome_processes_running(&cleanup_profile_dir) {
                    std::thread::sleep(Duration::from_millis(500));
                }
                let _ = remove_profile_dir_with_retry_blocking(
                    &cleanup_profile_dir,
                    crate::PROFILE_REMOVE_RETRY_ATTEMPTS,
                    Duration::from_millis(crate::PROFILE_REMOVE_RETRY_DELAY_MS),
                );
            })
            .ok();

        BrowserAutoLoginResult {
            url: OVERLEAF_PROJECT_URL.to_string(),
            profile_dir,
            debug_port,
            process_id,
        }
    }
}

impl fmt::Display for LocalChromeAutomationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChromeNotFound => write!(
                formatter,
                "Chrome executable not found; set {ENV_CHROME_PATH} to chrome.exe"
            ),
            Self::NoDebugPortAvailable { start, attempts } => write!(
                formatter,
                "no available CDP debug port from {start} across {attempts} attempts"
            ),
            Self::Io { operation, message } => write!(formatter, "{operation}: {message}"),
            Self::Debugger { message } => write!(formatter, "Chrome debugger: {message}"),
            Self::Timeout { operation } => write!(formatter, "timeout: {operation}"),
        }
    }
}

impl std::error::Error for LocalChromeAutomationError {}

pub fn detect_chrome_executable() -> Option<PathBuf> {
    chrome_executable_candidates()
        .into_iter()
        .find(|path| path.is_file())
}

pub fn chrome_executable_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(path) = env::var(ENV_CHROME_PATH) {
        if !path.trim().is_empty() {
            candidates.push(PathBuf::from(path));
        }
    }
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local_app_data)
                .join("Google")
                .join("Chrome")
                .join("Application")
                .join("chrome.exe"),
        );
    }
    if let Ok(program_files) = env::var("PROGRAMFILES") {
        candidates.push(
            PathBuf::from(program_files)
                .join("Google")
                .join("Chrome")
                .join("Application")
                .join("chrome.exe"),
        );
    }
    if let Ok(program_files_x86) = env::var("PROGRAMFILES(X86)") {
        candidates.push(
            PathBuf::from(program_files_x86)
                .join("Google")
                .join("Chrome")
                .join("Application")
                .join("chrome.exe"),
        );
    }

    candidates.extend([
        PathBuf::from("/usr/bin/google-chrome"),
        PathBuf::from("/usr/bin/google-chrome-stable"),
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/usr/bin/chromium-browser"),
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
    ]);
    dedupe_paths(candidates)
}

pub fn find_available_debug_port(start: u16, attempts: u16) -> io::Result<u16> {
    if start == 0 {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        return Ok(listener.local_addr()?.port());
    }

    for offset in 0..attempts {
        let Some(port) = start.checked_add(offset) else {
            break;
        };
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AddrNotAvailable,
        "no available local debug port",
    ))
}

pub fn page_websocket_url_from_debugger_http_response(
    response: &str,
    preferred_url: Option<&str>,
) -> Result<String, LocalChromeAutomationError> {
    let body = debugger_http_response_body(response);
    parse_page_websocket_url(body, preferred_url).map_err(|error| {
        LocalChromeAutomationError::Debugger {
            message: format!("{error:?}"),
        }
    })
}

fn debugger_http_response_body(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .or_else(|| response.split_once("\n\n").map(|(_, body)| body))
        .unwrap_or(response)
}

async fn wait_for_page_websocket_url(
    debug_port: u16,
    preferred_url: &str,
    timeout: Duration,
    poll_delay: Duration,
) -> Result<String, LocalChromeAutomationError> {
    let started_at = Instant::now();
    let mut last_error = None;

    while started_at.elapsed() <= timeout {
        match fetch_debugger_targets(debug_port) {
            Ok(response) => {
                match page_websocket_url_from_debugger_http_response(&response, Some(preferred_url))
                {
                    Ok(websocket_url) => return Ok(websocket_url),
                    Err(error) => last_error = Some(error.to_string()),
                }
            }
            Err(error) => {
                last_error = Some(error.to_string());
            }
        }
        sleep(poll_delay).await;
    }

    Err(LocalChromeAutomationError::Timeout {
        operation: format!(
            "wait for Chrome debugger on port {debug_port}{}",
            last_error
                .map(|message| format!("; last error: {message}"))
                .unwrap_or_default()
        ),
    })
}

fn fetch_debugger_targets(debug_port: u16) -> io::Result<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", debug_port))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(
        format!(
            "GET {CDP_LIST_ENDPOINT_PATH} HTTP/1.1\r\nHost: 127.0.0.1:{debug_port}\r\nConnection: close\r\n\r\n"
        )
        .as_bytes(),
    )?;

    let mut response = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                response.extend_from_slice(&buffer[..read]);
                if response.len() > MAX_DEBUGGER_HTTP_RESPONSE_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Chrome debugger response exceeded size limit",
                    ));
                }
                if debugger_http_response_complete(&response) {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) && !response.is_empty() =>
            {
                break;
            }
            Err(error) => return Err(error),
        }
    }

    String::from_utf8(response)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
}

fn debugger_http_response_complete(response: &[u8]) -> bool {
    let Some(header_end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let body_start = header_end + 4;
    let Ok(headers) = std::str::from_utf8(&response[..header_end]) else {
        return false;
    };
    let content_length = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    });

    content_length.is_some_and(|length| response.len().saturating_sub(body_start) >= length)
}

async fn wait_for_child_exit(
    child: &mut Child,
    timeout: Duration,
) -> Result<(), LocalChromeAutomationError> {
    let started_at = Instant::now();
    while started_at.elapsed() <= timeout {
        match child
            .try_wait()
            .map_err(|error| io_error("wait for chrome", error))?
        {
            Some(_) => return Ok(()),
            None => sleep(Duration::from_millis(200)).await,
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

async fn remove_profile_dir_with_retry(
    user_data_dir: &Path,
    attempts: u32,
    delay: Duration,
) -> Result<(), LocalChromeAutomationError> {
    for attempt in 0..attempts.max(1) {
        if !user_data_dir.exists() {
            return Ok(());
        }
        match fs::remove_dir_all(user_data_dir) {
            Ok(()) => return Ok(()),
            Err(error) if attempt + 1 < attempts.max(1) => {
                let _ = error;
                sleep(delay).await;
            }
            Err(error) => return Err(io_error("remove chrome profile dir", error)),
        }
    }
    Ok(())
}

fn local_chrome_error(error: LocalChromeAutomationError) -> BrowserAutomationError {
    match error {
        LocalChromeAutomationError::Timeout { operation } => {
            BrowserAutomationError::Timeout { operation }
        }
        LocalChromeAutomationError::ChromeNotFound
        | LocalChromeAutomationError::NoDebugPortAvailable { .. }
        | LocalChromeAutomationError::Io { .. }
        | LocalChromeAutomationError::Debugger { .. } => BrowserAutomationError::ExternalDriver {
            message: error.to_string(),
        },
    }
}

fn local_chrome_cdp_error(error: crate::CdpSessionError) -> BrowserAutomationError {
    BrowserAutomationError::ExternalDriver {
        message: error.to_string(),
    }
}

fn io_error(operation: impl Into<String>, error: io::Error) -> LocalChromeAutomationError {
    LocalChromeAutomationError::Io {
        operation: operation.into(),
        message: error.to_string(),
    }
}

fn remove_profile_dir_with_retry_blocking(
    user_data_dir: &Path,
    attempts: u32,
    delay: Duration,
) -> io::Result<()> {
    for attempt in 0..attempts.max(1) {
        if !user_data_dir.exists() {
            return Ok(());
        }
        match fs::remove_dir_all(user_data_dir) {
            Ok(()) => return Ok(()),
            Err(error) if attempt + 1 < attempts.max(1) => {
                let _ = error;
                std::thread::sleep(delay);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn scoped_chrome_processes_running(profile_dir: &Path) -> bool {
    !scoped_chrome_process_ids(profile_dir).is_empty()
}

fn scoped_chrome_process_ids(profile_dir: &Path) -> Vec<sysinfo::Pid> {
    let marker = profile_dir.to_string_lossy().to_ascii_lowercase();
    if marker.trim().is_empty() {
        return Vec::new();
    }

    let mut system = System::new_all();
    system.refresh_all();
    system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            let command_line = process.cmd().join(" ").to_ascii_lowercase();
            command_line.contains(&marker).then_some(*pid)
        })
        .collect()
}

fn terminate_scoped_chrome_processes(profile_dir: &Path) -> usize {
    let process_ids = scoped_chrome_process_ids(profile_dir);
    if process_ids.is_empty() {
        return 0;
    }

    let mut system = System::new_all();
    system.refresh_all();
    process_ids
        .into_iter()
        .filter(|pid| system.process(*pid).is_some_and(|process| process.kill()))
        .count()
}

pub fn cleanup_owned_chrome_profile(profile_dir: impl AsRef<Path>) -> io::Result<usize> {
    let profile_dir = profile_dir.as_ref();
    if !crate::is_owned_temp_profile_path(profile_dir) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refuse to clean non-owned Chrome profile: {}",
                profile_dir.display()
            ),
        ));
    }

    let terminated_process_count = terminate_scoped_chrome_processes(profile_dir);
    let _ = wait_for_scoped_chrome_exit_blocking(profile_dir, Duration::from_secs(2));
    remove_profile_dir_with_retry_blocking(
        profile_dir,
        crate::PROFILE_REMOVE_RETRY_ATTEMPTS,
        Duration::from_millis(crate::PROFILE_REMOVE_RETRY_DELAY_MS),
    )?;
    Ok(terminated_process_count)
}

fn wait_for_scoped_chrome_start_blocking(profile_dir: &Path, timeout: Duration) {
    let started_at = Instant::now();
    while started_at.elapsed() <= timeout {
        if scoped_chrome_processes_running(profile_dir) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_scoped_chrome_exit_blocking(profile_dir: &Path, timeout: Duration) -> bool {
    let started_at = Instant::now();
    loop {
        if !scoped_chrome_processes_running(profile_dir) {
            return true;
        }
        if started_at.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique = Vec::new();
    for path in paths {
        if !unique.iter().any(|existing| existing == &path) {
            unique.push(path);
        }
    }
    unique
}

fn task_slug(task_kind: BrowserTaskKind) -> &'static str {
    match task_kind {
        BrowserTaskKind::AddressFetch => "address",
        BrowserTaskKind::AccountPasswordLogin => "login",
        BrowserTaskKind::CookieRefresh => "cookie",
        BrowserTaskKind::GitToken => "token",
        BrowserTaskKind::PasswordChange => "password",
        BrowserTaskKind::Registration => "registration",
        BrowserTaskKind::ProjectMigration => "migration",
        BrowserTaskKind::BrowserAutoLogin => "user",
    }
}
