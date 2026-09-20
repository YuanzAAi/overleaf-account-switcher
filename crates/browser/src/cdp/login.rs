use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::Duration;

use overleaf_core::registration_trial_plan_code;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::automation::{
    overleaf_session_cookie, BrowserAutoLoginInput, BrowserAutomation, BrowserAutomationError,
    BrowserAutomationResult, CookieCapture, CredentialsLoginInput, GitTokenInput,
    GitTokenPageState, GitTokenResult, LoginResult, PasswordChangeInput, PasswordChangeResult,
    RegistrationInput, RegistrationResult, SecretText,
};
use crate::cdp_session::{CdpSession, CdpSessionError, CdpTransport};

pub const OVERLEAF_LOGIN_URL: &str = "https://www.overleaf.com/login";
pub const OVERLEAF_REGISTER_URL: &str = "https://www.overleaf.com/register";
pub const OVERLEAF_PROJECT_URL: &str = "https://www.overleaf.com/project";
pub const OVERLEAF_SETTINGS_URL: &str = "https://www.overleaf.com/user/settings";
pub const OVERLEAF_SUBSCRIPTION_NEW_URL: &str = "https://www.overleaf.com/user/subscription/new";
pub const OVERLEAF_SUBSCRIPTION_DASHBOARD_URL: &str = "https://www.overleaf.com/user/subscription";
pub const DEFAULT_LOGIN_STATUS_CHECKS: usize = 30;
pub const DEFAULT_LOGIN_POLL_DELAY_MS: u64 = 1_000;
pub const CAPTCHA_WAIT_SECONDS_PER_CHALLENGE: u64 = 60;
pub const CAPTCHA_NEAR_DEADLINE_SECONDS: u64 = 5;
pub const DEFAULT_ROBOT_VERIFICATION_RETRIES: usize = 2;
pub const ROBOT_VERIFICATION_RETRY_DELAY_MS: u64 = 750;
const RUNTIME_NAVIGATION_RETRY_ATTEMPTS: usize = 80;
const RUNTIME_NAVIGATION_RETRY_DELAY_MS: u64 = 250;
const EMAIL_CODE_AUTO_ADVANCE_GRACE_CHECKS: usize = 10;
const EMAIL_CODE_AUTO_ADVANCE_GRACE_DELAY_MS: u64 = 200;

static CAPTCHA_WAITER_ID: AtomicU64 = AtomicU64::new(1);
static CAPTCHA_WAIT_STATE: OnceLock<StdMutex<CaptchaWaitState>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdpLoginOptions {
    pub login_url: String,
    pub status_checks: usize,
    pub poll_delay_ms: u64,
}

#[derive(Debug)]
pub struct CdpLoginAutomation<T> {
    session: Mutex<CdpSession<T>>,
    options: CdpLoginOptions,
    use_current_page_for_next_submission: AtomicBool,
    return_immediately_on_visible_challenge: AtomicBool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdpLoginActionResult {
    pub ok: bool,
    #[serde(default)]
    pub missing: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub has_existing_masked_token: bool,
    #[serde(default)]
    pub can_generate_token: bool,
    #[serde(default)]
    pub can_add_another_token: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdpLoginPageState {
    pub url: String,
    pub title: String,
    pub text: String,
    #[serde(default)]
    pub visible_error_messages: Vec<String>,
    #[serde(default)]
    pub invalid_form_fields: Vec<String>,
    #[serde(default)]
    pub credential_error_detected: bool,
    #[serde(default)]
    pub registered_email_detected: bool,
    #[serde(default)]
    pub robot_verification_blocked: bool,
    #[serde(default)]
    pub recaptcha_detected: bool,
    #[serde(default)]
    pub recaptcha_possible: bool,
    #[serde(default)]
    pub recaptcha_iframe_count: usize,
    #[serde(default)]
    pub recaptcha_visible_count: usize,
    #[serde(default)]
    pub recaptcha_challenge_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdpLoginState {
    Pending,
    Success,
    ChallengeRequired,
    ChallengePossible,
    RobotVerificationBlocked,
    InvalidCredentials,
    RegisteredEmail,
}

#[derive(Debug, Default)]
struct CaptchaWaitState {
    active_waiters: HashSet<u64>,
    peak_waiters: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CaptchaWaitBudget {
    active_count: usize,
    wait_seconds: u64,
}

#[derive(Debug)]
struct CaptchaWaitPermit {
    id: u64,
    released: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdpRegistrationState {
    Pending,
    RegisteredEmail,
    ChallengeRequired,
    EmailVerificationRequired,
    EmailVerificationFailed,
    AccountCreated,
    SubscriptionSucceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdpTrialPurchaseAvailability {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdpPasswordChangeState {
    Pending,
    Success,
    Failed,
}

impl Default for CdpLoginOptions {
    fn default() -> Self {
        Self {
            login_url: OVERLEAF_LOGIN_URL.to_string(),
            status_checks: DEFAULT_LOGIN_STATUS_CHECKS,
            poll_delay_ms: DEFAULT_LOGIN_POLL_DELAY_MS,
        }
    }
}

impl<T> CdpLoginAutomation<T>
where
    T: CdpTransport,
{
    pub fn new(session: CdpSession<T>) -> Self {
        Self::with_options(session, CdpLoginOptions::default())
    }

    pub fn with_options(session: CdpSession<T>, options: CdpLoginOptions) -> Self {
        Self {
            session: Mutex::new(session),
            options,
            use_current_page_for_next_submission: AtomicBool::new(false),
            return_immediately_on_visible_challenge: AtomicBool::new(false),
        }
    }

    pub fn use_current_page_for_next_submission(&self) {
        self.use_current_page_for_next_submission
            .store(true, Ordering::Release);
    }

    pub fn return_immediately_on_visible_challenge(&self) {
        self.return_immediately_on_visible_challenge
            .store(true, Ordering::Release);
    }

    pub fn options(&self) -> &CdpLoginOptions {
        &self.options
    }

    pub async fn submit_login_credentials(
        &self,
        input: &CredentialsLoginInput,
    ) -> BrowserAutomationResult<()> {
        let mut session = self.session.lock().await;
        if !self
            .use_current_page_for_next_submission
            .swap(false, Ordering::AcqRel)
        {
            session
                .navigate(self.options.login_url.clone())
                .await
                .map_err(cdp_error)?;
        }

        let fill_result = evaluate_action_with_await(
            &mut session,
            &fill_login_form_expression(&input.email, input.password.expose()),
            true,
        )
        .await?;
        ensure_action_ok(fill_result)?;

        let submit_result =
            evaluate_action_with_await(&mut session, &submit_login_form_submit_expression(), true)
                .await?;
        ensure_action_ok(submit_result)
    }

    pub async fn submit_registration_credentials(
        &self,
        input: &RegistrationInput,
    ) -> BrowserAutomationResult<()> {
        let mut session = self.session.lock().await;
        if !self
            .use_current_page_for_next_submission
            .swap(false, Ordering::AcqRel)
        {
            session
                .navigate(OVERLEAF_REGISTER_URL)
                .await
                .map_err(cdp_error)?;
        }

        let fill_result = evaluate_action_with_await(
            &mut session,
            &fill_registration_form_expression(&input.email, input.password.expose()),
            true,
        )
        .await?;
        ensure_action_ok(fill_result)?;

        let submit_result =
            evaluate_action_with_await(&mut session, &submit_registration_form_expression(), true)
                .await?;
        ensure_action_ok(submit_result)
    }

    pub async fn submit_registration_email_code(
        &self,
        code: &str,
    ) -> BrowserAutomationResult<CdpRegistrationState> {
        let mut session = self.session.lock().await;

        let current_state =
            classify_registration_page_state(&read_login_page_state(&mut session).await?);
        if !matches!(
            current_state,
            CdpRegistrationState::Pending
                | CdpRegistrationState::EmailVerificationRequired
                | CdpRegistrationState::EmailVerificationFailed
        ) {
            return Ok(current_state);
        }

        let fill_result = evaluate_action_with_await(
            &mut session,
            &fill_registration_email_code_expression(code),
            true,
        )
        .await?;
        ensure_action_ok(fill_result)?;

        let mut state_after_fill =
            classify_registration_page_state(&read_login_page_state(&mut session).await?);
        for _ in 0..EMAIL_CODE_AUTO_ADVANCE_GRACE_CHECKS {
            if !matches!(
                state_after_fill,
                CdpRegistrationState::Pending
                    | CdpRegistrationState::EmailVerificationRequired
                    | CdpRegistrationState::EmailVerificationFailed
            ) {
                return Ok(state_after_fill);
            }
            sleep_after_pending_check(
                self.options
                    .poll_delay_ms
                    .min(EMAIL_CODE_AUTO_ADVANCE_GRACE_DELAY_MS),
            )
            .await;
            state_after_fill =
                classify_registration_page_state(&read_login_page_state(&mut session).await?);
        }
        if matches!(
            state_after_fill,
            CdpRegistrationState::EmailVerificationRequired
                | CdpRegistrationState::EmailVerificationFailed
        ) {
            let submit_result =
                evaluate_action(&mut session, &submit_registration_email_code_expression()).await?;
            ensure_action_ok(submit_result)?;
        } else if state_after_fill != CdpRegistrationState::Pending {
            return Ok(state_after_fill);
        }

        for _ in 0..self.options.status_checks {
            let page_state = read_login_page_state(&mut session).await?;
            if page_state.robot_verification_blocked {
                return Err(BrowserAutomationError::RobotVerificationBlocked);
            }
            match classify_registration_page_state(&page_state) {
                CdpRegistrationState::Pending | CdpRegistrationState::EmailVerificationRequired => {
                    sleep_after_pending_check(self.options.poll_delay_ms).await;
                }
                CdpRegistrationState::EmailVerificationFailed => {
                    return Ok(CdpRegistrationState::EmailVerificationFailed);
                }
                state => return Ok(state),
            }
        }

        Err(BrowserAutomationError::Timeout {
            operation: "wait for Overleaf email verification".to_string(),
        })
    }

    pub async fn open_registration_subscription_page(
        &self,
        trial_days: u32,
    ) -> BrowserAutomationResult<CdpRegistrationState> {
        let subscription_url = registration_subscription_url(trial_days).ok_or_else(|| {
            BrowserAutomationError::Other {
                message: format!("unsupported trial_days for subscription page: {trial_days}"),
            }
        })?;
        let mut session = self.session.lock().await;

        session
            .navigate(subscription_url)
            .await
            .map_err(cdp_error)?;

        for _ in 0..self.options.status_checks {
            let page_state = read_login_page_state(&mut session).await?;
            if page_state.url.to_ascii_lowercase().contains("/login") {
                return Err(BrowserAutomationError::Navigation {
                    message: "registration session redirected to login before subscription"
                        .to_string(),
                });
            }
            match classify_registration_page_state(&page_state) {
                CdpRegistrationState::AccountCreated
                | CdpRegistrationState::SubscriptionSucceeded => {
                    return Ok(classify_registration_page_state(&page_state));
                }
                CdpRegistrationState::ChallengeRequired => {
                    return Err(BrowserAutomationError::ChallengeRequired {
                        challenge: "reCAPTCHA or robot verification".to_string(),
                    });
                }
                CdpRegistrationState::Pending => {
                    sleep_after_pending_check(self.options.poll_delay_ms).await;
                }
                state => return Ok(state),
            }
        }

        Err(BrowserAutomationError::Timeout {
            operation: "wait for Overleaf subscription page".to_string(),
        })
    }

    pub async fn inspect_registration_trial_purchase_availability(
        &self,
        trial_days: u32,
    ) -> BrowserAutomationResult<CdpTrialPurchaseAvailability> {
        let subscription_url = registration_subscription_url(trial_days).ok_or_else(|| {
            BrowserAutomationError::Other {
                message: format!("unsupported trial_days for subscription page: {trial_days}"),
            }
        })?;
        let mut session = self.session.lock().await;

        session
            .navigate(subscription_url)
            .await
            .map_err(cdp_error)?;

        for _ in 0..self.options.status_checks {
            let page_state = read_login_page_state(&mut session).await?;
            if page_state.url.to_ascii_lowercase().contains("/login") {
                return Err(BrowserAutomationError::Navigation {
                    message:
                        "registration session redirected to login before trial eligibility check"
                            .to_string(),
                });
            }
            let availability = classify_registration_trial_purchase_page(&page_state, trial_days);
            if availability != CdpTrialPurchaseAvailability::Unknown {
                return Ok(availability);
            }
            if classify_registration_page_state(&page_state)
                == CdpRegistrationState::ChallengeRequired
            {
                return Err(BrowserAutomationError::ChallengeRequired {
                    challenge: "reCAPTCHA or robot verification".to_string(),
                });
            }

            let url = page_state.url.to_ascii_lowercase();
            if url.contains("/project")
                || (url.contains("/user/subscription") && !url.contains("/user/subscription/new"))
            {
                return Ok(CdpTrialPurchaseAvailability::Unknown);
            }
            sleep_after_pending_check(self.options.poll_delay_ms).await;
        }

        Ok(CdpTrialPurchaseAvailability::Unknown)
    }

    pub async fn submit_registration_payment_and_wait_thank_you(
        &self,
    ) -> BrowserAutomationResult<CdpRegistrationState> {
        let mut session = self.session.lock().await;

        let submit_result =
            evaluate_action(&mut session, &submit_registration_payment_expression()).await?;
        ensure_action_ok(submit_result)?;

        let mut last_page_state = None;
        for _ in 0..self.options.status_checks {
            let page_state = read_login_page_state(&mut session).await?;
            if is_registration_payment_post_checkout_candidate(&page_state) {
                drop(session);
                return self.open_registration_subscription_management().await;
            }
            if let Some(error) = detect_registration_payment_error(&page_state) {
                return Err(BrowserAutomationError::Other { message: error });
            }
            last_page_state = Some(page_state.clone());
            match classify_registration_page_state(&page_state) {
                CdpRegistrationState::ChallengeRequired => {
                    return Err(BrowserAutomationError::ChallengeRequired {
                        challenge: "reCAPTCHA or robot verification".to_string(),
                    });
                }
                CdpRegistrationState::Pending | CdpRegistrationState::AccountCreated => {
                    sleep_after_pending_check(self.options.poll_delay_ms).await;
                }
                state => return Ok(state),
            }
        }

        Err(BrowserAutomationError::Timeout {
            operation: format!(
                "wait for Overleaf subscription confirmation after payment{}",
                last_page_state
                    .as_ref()
                    .map(registration_payment_state_diagnostic)
                    .unwrap_or_default()
            ),
        })
    }

    pub async fn open_registration_subscription_management(
        &self,
    ) -> BrowserAutomationResult<CdpRegistrationState> {
        let mut session = self.session.lock().await;

        let action =
            evaluate_action(&mut session, &open_subscription_management_expression()).await?;
        if !action.ok {
            session
                .navigate(OVERLEAF_SUBSCRIPTION_DASHBOARD_URL)
                .await
                .map_err(cdp_error)?;
        }

        for _ in 0..self.options.status_checks {
            let page_state = read_login_page_state(&mut session).await?;
            if is_confirmed_active_subscription_page(&page_state) {
                return Ok(CdpRegistrationState::SubscriptionSucceeded);
            }
            match classify_registration_page_state(&page_state) {
                CdpRegistrationState::ChallengeRequired => {
                    return Err(BrowserAutomationError::ChallengeRequired {
                        challenge: "reCAPTCHA or robot verification".to_string(),
                    });
                }
                CdpRegistrationState::Pending
                | CdpRegistrationState::AccountCreated
                | CdpRegistrationState::SubscriptionSucceeded => {
                    sleep_after_pending_check(self.options.poll_delay_ms).await;
                }
                state => return Ok(state),
            }
        }

        Err(BrowserAutomationError::Timeout {
            operation: "wait for Overleaf subscription management".to_string(),
        })
    }

    pub async fn accept_registration_extra_trial_offer(&self) -> BrowserAutomationResult<()> {
        let mut session = self.session.lock().await;

        let action =
            evaluate_action_with_await(&mut session, &accept_extra_trial_offer_expression(), true)
                .await?;
        ensure_action_ok(action)
    }

    pub async fn change_registration_plan_to_pro_annual(
        &self,
    ) -> BrowserAutomationResult<CdpRegistrationState> {
        let mut session = self.session.lock().await;

        let action =
            evaluate_action_with_await(&mut session, &change_to_pro_annual_plan_expression(), true)
                .await?;
        ensure_action_ok(action)?;

        session
            .navigate(OVERLEAF_SUBSCRIPTION_DASHBOARD_URL)
            .await
            .map_err(cdp_error)?;

        for _ in 0..self.options.status_checks {
            let page_state = read_login_page_state(&mut session).await?;
            if is_pro_annual_subscription_page(&page_state) {
                return Ok(CdpRegistrationState::SubscriptionSucceeded);
            }
            match classify_registration_page_state(&page_state) {
                CdpRegistrationState::ChallengeRequired => {
                    return Err(BrowserAutomationError::ChallengeRequired {
                        challenge: "reCAPTCHA or robot verification".to_string(),
                    });
                }
                CdpRegistrationState::Pending
                | CdpRegistrationState::AccountCreated
                | CdpRegistrationState::SubscriptionSucceeded => {
                    sleep_after_pending_check(self.options.poll_delay_ms).await;
                }
                state => return Ok(state),
            }
        }

        Err(BrowserAutomationError::Timeout {
            operation: "wait for Overleaf Pro annual plan".to_string(),
        })
    }

    pub async fn cancel_registration_subscription_final(
        &self,
    ) -> BrowserAutomationResult<CdpRegistrationState> {
        let mut session = self.session.lock().await;

        let action =
            evaluate_action_with_await(&mut session, &cancel_subscription_final_expression(), true)
                .await?;
        ensure_action_ok(action)?;

        for _ in 0..self.options.status_checks {
            let page_state = read_login_page_state(&mut session).await?;
            if is_subscription_cancellation_confirmed(&page_state) {
                return Ok(CdpRegistrationState::SubscriptionSucceeded);
            }
            match classify_registration_page_state(&page_state) {
                CdpRegistrationState::ChallengeRequired => {
                    return Err(BrowserAutomationError::ChallengeRequired {
                        challenge: "reCAPTCHA or robot verification".to_string(),
                    });
                }
                CdpRegistrationState::Pending
                | CdpRegistrationState::AccountCreated
                | CdpRegistrationState::SubscriptionSucceeded => {
                    sleep_after_pending_check(self.options.poll_delay_ms).await;
                }
                state => return Ok(state),
            }
        }

        Err(BrowserAutomationError::Timeout {
            operation: "wait for Overleaf subscription cancellation".to_string(),
        })
    }

    pub async fn current_page_state(&self) -> BrowserAutomationResult<CdpLoginPageState> {
        let mut session = self.session.lock().await;
        read_login_page_state(&mut session).await
    }

    pub async fn current_login_state(&self) -> BrowserAutomationResult<CdpLoginState> {
        let mut session = self.session.lock().await;
        let page_state = read_login_page_state(&mut session).await?;
        Ok(classify_login_page_state(&page_state))
    }

    pub async fn finalize_login_result(
        &self,
        email: Option<String>,
    ) -> BrowserAutomationResult<LoginResult> {
        let mut session = self.session.lock().await;
        login_result_from_session(&mut session, email).await
    }

    pub async fn login_with_session_cookie(
        &self,
        input: BrowserAutoLoginInput,
        email: Option<String>,
    ) -> BrowserAutomationResult<LoginResult> {
        let mut session = self.session.lock().await;
        session
            .set_overleaf_session_cookie(input.overleaf_session.expose(), input.cookie_expiry)
            .await
            .map_err(cdp_error)?;
        session
            .navigate(OVERLEAF_PROJECT_URL)
            .await
            .map_err(cdp_error)?;
        wait_for_authenticated_project_page(&mut session, &self.options).await?;
        login_result_from_session(&mut session, email).await
    }

    pub async fn current_registration_state(
        &self,
    ) -> BrowserAutomationResult<CdpRegistrationState> {
        let mut session = self.session.lock().await;
        let page_state = read_login_page_state(&mut session).await?;
        Ok(classify_registration_page_state(&page_state))
    }

    pub async fn finalize_registration_result(
        &self,
        email: String,
        trial_days: u32,
        auto_fetch_git_token: bool,
    ) -> BrowserAutomationResult<RegistrationResult> {
        let mut session = self.session.lock().await;
        session
            .navigate(OVERLEAF_PROJECT_URL)
            .await
            .map_err(cdp_error)?;
        wait_for_authenticated_project_page(&mut session, &self.options).await?;
        let cookies = read_optional_overleaf_session_cookie(&mut session).await;
        let git_token = if auto_fetch_git_token {
            Some(generate_git_token_in_current_session(&mut session).await?)
        } else {
            None
        };

        Ok(RegistrationResult {
            email,
            trial_days,
            subscription_succeeded: true,
            cookies,
            git_token,
            trial_expiry: None,
        })
    }

    pub async fn read_git_token_state_with_cookie(
        &self,
        input: GitTokenInput,
    ) -> BrowserAutomationResult<GitTokenPageState> {
        let mut session = self.session.lock().await;

        session
            .set_overleaf_session_cookie(input.overleaf_session.expose(), None)
            .await
            .map_err(cdp_error)?;

        read_git_token_page_state_in_current_session(&mut session).await
    }
}

#[async_trait::async_trait]
impl<T> BrowserAutomation for CdpLoginAutomation<T>
where
    T: CdpTransport,
{
    async fn login_with_credentials(
        &self,
        input: CredentialsLoginInput,
    ) -> BrowserAutomationResult<LoginResult> {
        let mut robot_retry_count = 0;

        'login_attempts: loop {
            self.submit_login_credentials(&input).await?;
            let mut session = self.session.lock().await;

            let mut max_checks = self.options.status_checks.max(1);
            let mut check_index = 0;
            let mut captcha_wait_permit = None;
            let mut captcha_started_check = None;
            let mut captcha_seen = false;

            while check_index < max_checks {
                let page_state = read_login_page_state(&mut session).await?;
                let state = classify_login_page_state(&page_state);
                let near_deadline = is_near_login_deadline(
                    check_index,
                    max_checks,
                    self.options.poll_delay_ms,
                    CAPTCHA_NEAR_DEADLINE_SECONDS,
                );
                match state {
                    CdpLoginState::Success => {
                        return login_result_from_session(&mut session, Some(input.email)).await;
                    }
                    CdpLoginState::ChallengeRequired => {
                        captcha_seen = true;
                        if self
                            .return_immediately_on_visible_challenge
                            .load(Ordering::Acquire)
                        {
                            return Err(BrowserAutomationError::ChallengeRequired {
                                challenge: "reCAPTCHA or robot verification".to_string(),
                            });
                        }
                        if captcha_wait_permit.is_none() {
                            let (permit, budget) = register_captcha_waiter();
                            let started_check = check_index + 1;
                            captcha_started_check = Some(started_check);
                            max_checks = extend_login_checks_for_captcha(
                                max_checks,
                                started_check,
                                self.options.poll_delay_ms,
                                budget.wait_seconds,
                            );
                            captcha_wait_permit = Some(permit);
                        } else {
                            let budget = current_captcha_wait_budget();
                            max_checks = extend_login_checks_for_captcha(
                                max_checks,
                                captcha_started_check.unwrap_or(check_index + 1),
                                self.options.poll_delay_ms,
                                budget.wait_seconds,
                            );
                        }
                        sleep_after_pending_check(self.options.poll_delay_ms).await;
                    }
                    CdpLoginState::ChallengePossible if near_deadline => {
                        captcha_seen = true;
                        if captcha_wait_permit.is_none() {
                            let (permit, budget) = register_captcha_waiter();
                            let started_check = check_index + 1;
                            captcha_started_check = Some(started_check);
                            max_checks = extend_login_checks_for_captcha(
                                max_checks,
                                started_check,
                                self.options.poll_delay_ms,
                                budget.wait_seconds,
                            );
                            captcha_wait_permit = Some(permit);
                        } else {
                            let budget = current_captcha_wait_budget();
                            max_checks = extend_login_checks_for_captcha(
                                max_checks,
                                captcha_started_check.unwrap_or(check_index + 1),
                                self.options.poll_delay_ms,
                                budget.wait_seconds,
                            );
                        }
                        sleep_after_pending_check(self.options.poll_delay_ms).await;
                    }
                    CdpLoginState::ChallengePossible => {
                        sleep_after_pending_check(self.options.poll_delay_ms).await;
                    }
                    CdpLoginState::InvalidCredentials => {
                        return Err(BrowserAutomationError::InvalidCredentials);
                    }
                    CdpLoginState::RobotVerificationBlocked => {
                        if robot_retry_count < DEFAULT_ROBOT_VERIFICATION_RETRIES {
                            robot_retry_count += 1;
                            drop(session);
                            sleep_after_pending_check(ROBOT_VERIFICATION_RETRY_DELAY_MS).await;
                            continue 'login_attempts;
                        }
                        return Err(BrowserAutomationError::RobotVerificationBlocked);
                    }
                    CdpLoginState::RegisteredEmail => {
                        return Err(BrowserAutomationError::RegisteredEmail { email: input.email });
                    }
                    CdpLoginState::Pending => {
                        sleep_after_pending_check(self.options.poll_delay_ms).await;
                    }
                }
                check_index += 1;
            }

            return if captcha_seen {
                Err(BrowserAutomationError::ChallengeRequired {
                    challenge: "reCAPTCHA or robot verification".to_string(),
                })
            } else {
                Err(BrowserAutomationError::Timeout {
                    operation: "wait for Overleaf login".to_string(),
                })
            };
        }
    }

    async fn register_and_subscribe(
        &self,
        input: RegistrationInput,
    ) -> BrowserAutomationResult<RegistrationResult> {
        self.submit_registration_credentials(&input).await?;
        let mut session = self.session.lock().await;

        for _ in 0..self.options.status_checks {
            let page_state = read_login_page_state(&mut session).await?;
            if page_state.robot_verification_blocked {
                return Err(BrowserAutomationError::RobotVerificationBlocked);
            }
            match classify_registration_page_state(&page_state) {
                CdpRegistrationState::RegisteredEmail => {
                    return Err(BrowserAutomationError::RegisteredEmail { email: input.email });
                }
                CdpRegistrationState::ChallengeRequired => {
                    return Err(BrowserAutomationError::ChallengeRequired {
                        challenge: "reCAPTCHA or robot verification".to_string(),
                    });
                }
                CdpRegistrationState::EmailVerificationRequired => {
                    return Err(BrowserAutomationError::EmailCodeRequired { email: input.email });
                }
                CdpRegistrationState::EmailVerificationFailed => {
                    return Err(BrowserAutomationError::Other {
                        message: "email verification failed".to_string(),
                    });
                }
                CdpRegistrationState::SubscriptionSucceeded => {
                    let cookies = read_optional_overleaf_session_cookie(&mut session).await;
                    return Ok(RegistrationResult {
                        email: input.email,
                        trial_days: input.trial_days,
                        subscription_succeeded: true,
                        cookies,
                        git_token: None,
                        trial_expiry: None,
                    });
                }
                CdpRegistrationState::AccountCreated => {
                    let cookies = read_optional_overleaf_session_cookie(&mut session).await;
                    return Ok(RegistrationResult {
                        email: input.email,
                        trial_days: input.trial_days,
                        subscription_succeeded: false,
                        cookies,
                        git_token: None,
                        trial_expiry: None,
                    });
                }
                CdpRegistrationState::Pending => {
                    sleep_after_pending_check(self.options.poll_delay_ms).await;
                }
            }
        }

        Err(BrowserAutomationError::Timeout {
            operation: "wait for Overleaf registration".to_string(),
        })
    }

    async fn extract_git_token_with_cookie(
        &self,
        input: GitTokenInput,
    ) -> BrowserAutomationResult<GitTokenResult> {
        let mut session = self.session.lock().await;

        session
            .set_overleaf_session_cookie(input.overleaf_session.expose(), None)
            .await
            .map_err(cdp_error)?;

        generate_git_token_in_current_session(&mut session).await
    }

    async fn read_git_token_state_with_cookie(
        &self,
        input: GitTokenInput,
    ) -> BrowserAutomationResult<GitTokenPageState> {
        CdpLoginAutomation::read_git_token_state_with_cookie(self, input).await
    }

    async fn change_password_with_cookie(
        &self,
        input: PasswordChangeInput,
    ) -> BrowserAutomationResult<PasswordChangeResult> {
        let mut session = self.session.lock().await;

        session
            .set_overleaf_session_cookie(input.overleaf_session.expose(), None)
            .await
            .map_err(cdp_error)?;
        session
            .navigate(OVERLEAF_SETTINGS_URL)
            .await
            .map_err(cdp_error)?;

        let fill_result = evaluate_action_with_await(
            &mut session,
            &fill_password_change_form_expression(
                input.current_password.expose(),
                input.new_password.expose(),
            ),
            true,
        )
        .await?;
        ensure_action_ok(fill_result)?;

        let submit_result =
            evaluate_action(&mut session, &submit_password_change_form_expression()).await?;
        ensure_action_ok(submit_result)?;

        for _ in 0..self.options.status_checks {
            let page_state = read_login_page_state(&mut session).await?;
            match classify_password_change_page_state(&page_state) {
                CdpPasswordChangeState::Success => {
                    return Ok(PasswordChangeResult { changed: true });
                }
                CdpPasswordChangeState::Failed => {
                    return Ok(PasswordChangeResult { changed: false });
                }
                CdpPasswordChangeState::Pending => {
                    sleep_after_pending_check(self.options.poll_delay_ms).await;
                }
            }
        }

        Err(BrowserAutomationError::Timeout {
            operation: "wait for Overleaf password change".to_string(),
        })
    }
}

pub fn fill_login_form_expression(email: &str, password: &str) -> String {
    let email_json = serde_json::to_string(email).expect("email string should serialize");
    let password_json = serde_json::to_string(password).expect("password string should serialize");

    format!(
        r#"(() => new Promise(resolve => {{
  const email = {email_json};
  const password = {password_json};
  const deadline = Date.now() + 15000;
  const done = value => resolve(JSON.stringify(value));
  const findFirst = selectors => {{
    for (const selector of selectors) {{
      try {{
        const node = document.querySelector(selector);
        if (node) return node;
      }} catch (_) {{}}
    }}
    return null;
  }};
  const setValue = (el, value) => {{
    const prototype = Object.getPrototypeOf(el);
    const descriptor = Object.getOwnPropertyDescriptor(prototype, 'value')
      || (window.HTMLInputElement
        ? Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')
        : null);
    if (descriptor && descriptor.set) {{
      descriptor.set.call(el, value);
    }} else {{
      el.value = value;
    }}
    try {{
      el.dispatchEvent(new InputEvent('input', {{
        bubbles: true,
        data: String(value),
        inputType: 'insertText'
      }}));
    }} catch (_) {{
      el.dispatchEvent(new Event('input', {{ bubbles: true }}));
    }}
    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
  }};
  const tick = () => {{
    const emailInput = findFirst([
      'input[name="email"]',
      'input[type="email"]',
      '#email',
      '#email-input',
      'input[autocomplete="email"]',
      'input[id*="email"]',
      'input[placeholder*="email" i]'
    ]);
    const passwordInput = findFirst([
      'input[name="password"]',
      'input[type="password"]',
      '#password',
      '#password-input',
      'input[autocomplete="current-password"]',
      'input[id*="password"]',
      'input[placeholder*="password" i]'
    ]);
    if (!emailInput) {{
      if (Date.now() > deadline) return done({{ ok: false, missing: 'email' }});
      return setTimeout(tick, 100);
    }}
    if (!passwordInput) {{
      if (Date.now() > deadline) return done({{ ok: false, missing: 'password' }});
      return setTimeout(tick, 100);
    }}
    emailInput.focus();
    setValue(emailInput, email);
    passwordInput.focus();
    setValue(passwordInput, password);
    const emailOk = String(emailInput.value || '').trim().toLowerCase() === String(email).trim().toLowerCase();
    const passwordOk = String(passwordInput.value || '') === String(password);
    if (emailOk && passwordOk) return done({{ ok: true }});
    if (Date.now() > deadline) return done({{ ok: false, missing: 'login value write' }});
    setTimeout(tick, 100);
  }};
  tick();
}}))()"#
    )
}

pub fn submit_login_form_submit_expression() -> String {
    r#"(() => new Promise(resolve => {
  const deadline = Date.now() + 15000;
  const done = value => resolve(JSON.stringify(value));
  let recaptchaReady = false;
  let readyRequested = false;
  const visible = element => {
    if (!element) return false;
    const style = window.getComputedStyle ? window.getComputedStyle(element) : null;
    const rect = element.getBoundingClientRect ? element.getBoundingClientRect() : null;
    return (!style || (style.display !== 'none' && style.visibility !== 'hidden' && Number(style.opacity || 1) > 0))
      && (!rect || (rect.width > 0 && rect.height > 0));
  };
  const findSubmit = () => Array.from(document.querySelectorAll('button[type="submit"], input[type="submit"]'))
    .find(button => visible(button)) || document.querySelector('button[type="submit"], input[type="submit"]');
  const tick = () => {
    const submit = findSubmit();
    if (!submit) {
      if (Date.now() > deadline) return done({ ok: false, missing: 'submit' });
      return setTimeout(tick, 100);
    }
    if (submit.disabled || submit.getAttribute('aria-disabled') === 'true') {
      if (Date.now() > deadline) return done({ ok: false, missing: 'submit enabled' });
      return setTimeout(tick, 100);
    }
    const recaptchaScript = Array.from(document.scripts || []).some(script => /recaptcha/i.test(String(script.src || '')));
    if (recaptchaScript) {
      const recaptcha = window.grecaptcha;
      if (!recaptcha || typeof recaptcha.ready !== 'function') {
        if (Date.now() > deadline) return done({ ok: false, missing: 'reCAPTCHA initialization' });
        return setTimeout(tick, 100);
      }
      if (!readyRequested) {
        readyRequested = true;
        recaptcha.ready(() => { recaptchaReady = true; });
      }
      if (!recaptchaReady) {
        if (Date.now() >= deadline) return done({ ok: false, missing: 'reCAPTCHA ready callback timeout' });
        return setTimeout(tick, 100);
      }
    }
    window.__overleafSwitcherLoginSubmitted = Date.now();
    submit.scrollIntoView({ block: 'center', inline: 'center' });
    submit.click();
    return done({ ok: true });
  };
  tick();
}))()"#
        .to_string()
}

pub fn fill_registration_form_expression(email: &str, password: &str) -> String {
    let email_json = serde_json::to_string(email).expect("email string should serialize");
    let password_json = serde_json::to_string(password).expect("password string should serialize");

    format!(
        r#"(() => new Promise(resolve => {{
  const email = {email_json};
  const password = {password_json};
  const deadline = Date.now() + 15000;
  const done = value => resolve(JSON.stringify(value));
  const findFirst = selectors => {{
    for (const selector of selectors) {{
      const node = document.querySelector(selector);
      if (node) return node;
    }}
    return null;
  }};
  const setValue = (el, value) => {{
    el.focus();
    const prototype = Object.getPrototypeOf(el);
    const descriptor = Object.getOwnPropertyDescriptor(prototype, 'value')
      || (window.HTMLInputElement && el instanceof window.HTMLInputElement
        ? Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')
        : null);
    if (descriptor && descriptor.set) {{
      descriptor.set.call(el, value);
    }} else {{
      el.value = value;
    }}
    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
  }};
  const tick = () => {{
    const emailInput = findFirst([
      'input[name="email"]',
      'input[type="email"]',
      'input[autocomplete="email"]',
      'input[placeholder*="Email"]',
      '#email'
    ]);
    const passwordInput = findFirst([
      'input[name="password"]',
      'input[type="password"]',
      'input[autocomplete="new-password"]',
      'input[placeholder*="Password"]',
      '#password'
    ]);
    if (!emailInput || !passwordInput) {{
      if (Date.now() >= deadline) {{
        return done({{ ok: false, missing: !emailInput ? 'registration email' : 'registration password' }});
      }}
      return setTimeout(tick, 100);
    }}
    setValue(emailInput, email);
    setValue(passwordInput, password);
    if (emailInput.value !== email || passwordInput.value !== password) {{
      if (Date.now() >= deadline) {{
        return done({{ ok: false, missing: emailInput.value !== email ? 'registration email value' : 'registration password value' }});
      }}
      return setTimeout(tick, 100);
    }}
    const submit = findRegistrationSubmit();
    if (!submit) {{
      if (Date.now() >= deadline) return done({{ ok: false, missing: 'registration submit' }});
      return setTimeout(tick, 100);
    }}
    if (!submit.disabled) return done({{ ok: true }});
    if (Date.now() >= deadline) return done({{ ok: false, missing: 'registration submit enabled' }});
    setTimeout(tick, 100);
  }};
  const findRegistrationSubmit = () => {{
    const primary = document.querySelector('button[type="submit"], input[type="submit"]');
    if (primary) return primary;
    for (const node of document.querySelectorAll('button, a')) {{
      const text = (node.innerText || node.textContent || '').trim().toLowerCase();
      if (text.includes('create') || text.includes('sign up') || text.includes('register')) return node;
    }}
    return null;
  }};
  tick();
}}))()"#
    )
}

pub fn submit_registration_form_expression() -> String {
    r#"(() => new Promise(resolve => {
  const deadline = Date.now() + 15000;
  const done = value => resolve(JSON.stringify(value));
  let recaptchaReady = false;
  let readyRequested = false;
  const findSubmit = () => {
    const primary = document.querySelector('button[type="submit"], input[type="submit"]');
    if (primary) return primary;
    for (const node of document.querySelectorAll('button, a')) {
      const text = (node.innerText || node.textContent || '').trim().toLowerCase();
      if (text.includes('create') || text.includes('sign up') || text.includes('register')) return node;
    }
    return null;
  };
  const tick = () => {
    const submit = findSubmit();
    if (!submit) {
      if (Date.now() >= deadline) return done({ ok: false, missing: 'registration submit' });
      return setTimeout(tick, 100);
    }
    if (submit.disabled || submit.getAttribute('aria-disabled') === 'true') {
      if (Date.now() >= deadline) return done({ ok: false, missing: 'registration submit enabled' });
      return setTimeout(tick, 100);
    }
    const recaptchaScript = Array.from(document.scripts || []).some(script => /recaptcha/i.test(String(script.src || '')));
    if (recaptchaScript) {
      const recaptcha = window.grecaptcha;
      if (!recaptcha || typeof recaptcha.ready !== 'function') {
        if (Date.now() >= deadline) return done({ ok: false, missing: 'reCAPTCHA initialization' });
        return setTimeout(tick, 100);
      }
      if (!readyRequested) {
        readyRequested = true;
        recaptcha.ready(() => { recaptchaReady = true; });
      }
      if (!recaptchaReady) {
        if (Date.now() >= deadline) return done({ ok: false, missing: 'reCAPTCHA ready callback timeout' });
        return setTimeout(tick, 100);
      }
    }
    submit.scrollIntoView({ block: 'center', inline: 'center' });
    submit.click();
    done({ ok: true });
  };
  tick();
}))()"#
        .to_string()
}

pub fn fill_registration_email_code_expression(code: &str) -> String {
    let code_json = serde_json::to_string(code).expect("email code string should serialize");

    format!(
        r#"(() => new Promise(resolve => {{
  const code = {code_json};
  const normalizedCode = String(code).replace(/\D/g, '');
  const deadline = Date.now() + 5000;
  const done = value => resolve(JSON.stringify(value));
  const findFirst = selectors => {{
    for (const selector of selectors) {{
      const node = document.querySelector(selector);
      if (node) return node;
    }}
    return null;
  }};
  const setValue = (el, value) => {{
    el.focus();
    const prototype = Object.getPrototypeOf(el);
    const descriptor = Object.getOwnPropertyDescriptor(prototype, 'value')
      || (window.HTMLInputElement && el instanceof window.HTMLInputElement
        ? Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')
        : null);
    if (descriptor && descriptor.set) {{
      descriptor.set.call(el, value);
    }} else {{
      el.value = value;
    }}
    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
  }};
  const findSubmit = () => {{
    const primary = document.querySelector('button[type="submit"], input[type="submit"]');
    if (primary) return primary;
    for (const node of document.querySelectorAll('button, a')) {{
      const text = (node.innerText || node.textContent || '').trim().toLowerCase();
      if (text.includes('verify') || text.includes('confirm') || text.includes('continue')) return node;
    }}
    return null;
  }};
  const tick = () => {{
    const input = findFirst([
      '#one-time-code',
      'input[id*="code"]',
      'input[name*="code"]',
      'input[autocomplete="one-time-code"]',
      'input[inputmode="numeric"]',
      'input[type="text"]'
    ]);
    if (!input) {{
      if (Date.now() >= deadline) return done({{ ok: false, missing: 'email verification code input' }});
      return setTimeout(tick, 100);
    }}
    setValue(input, code);
    let normalizedValue = String(input.value || '').replace(/\D/g, '');
    if (normalizedValue !== normalizedCode && typeof document.execCommand === 'function') {{
      input.focus();
      if (typeof input.select === 'function') input.select();
      document.execCommand('insertText', false, code);
      normalizedValue = String(input.value || '').replace(/\D/g, '');
    }}
    if (normalizedValue !== normalizedCode) {{
      if (Date.now() >= deadline) return done({{ ok: false, missing: 'email verification code value' }});
      return setTimeout(tick, 100);
    }}
    const submit = findSubmit();
    if (!submit) {{
      if (Date.now() >= deadline) return done({{ ok: false, missing: 'email verification submit' }});
      return setTimeout(tick, 100);
    }}
    if (!submit.disabled) return done({{ ok: true }});
    if (Date.now() >= deadline) return done({{ ok: false, missing: 'email verification submit enabled' }});
    setTimeout(tick, 100);
  }};
  tick();
}}))()"#
    )
}

pub fn submit_registration_email_code_expression() -> String {
    r#"(() => {
  const codeInput = document.querySelector([
    '#one-time-code',
    'input[id*="code"]',
    'input[name*="code"]',
    'input[autocomplete="one-time-code"]',
    'input[inputmode="numeric"]'
  ].join(','));
  const onConfirmEmailPage = String(window.location.pathname || '').toLowerCase().includes('/registration/confirm-email');
  if (!onConfirmEmailPage && !codeInput) {
    return JSON.stringify({ ok: true, skipped: 'email verification page changed' });
  }
  const findSubmit = () => {
    const form = codeInput && codeInput.closest('form');
    const primary = form && form.querySelector('button[type="submit"], input[type="submit"]');
    if (primary) return primary;
    const scope = form || document;
    for (const node of scope.querySelectorAll('button, a')) {
      const text = (node.innerText || node.textContent || '').trim().toLowerCase();
      if (text.includes('verify') || text.includes('confirm') || text.includes('continue')) return node;
    }
    return null;
  };
  const submit = findSubmit();
  if (!submit) return JSON.stringify({ ok: false, missing: 'email verification submit' });
  if (submit.disabled) return JSON.stringify({ ok: false, missing: 'email verification submit enabled' });
  submit.click();
  return JSON.stringify({ ok: true });
})()"#
        .to_string()
}

pub fn submit_registration_payment_expression() -> String {
    r#"(() => {
  const findSubmit = () => {
    const candidates = Array.from(document.querySelectorAll('button, input[type="submit"], a'));
    const byText = candidates.find(node => {
      const text = (node.innerText || node.textContent || node.value || '').trim().toLowerCase();
      return text.includes('upgrade now, pay after 7 days') || text.includes('upgrade now');
    });
    if (byText) return byText;
    return document.querySelector('button[type="submit"], input[type="submit"]');
  };
  const submit = findSubmit();
  if (!submit) return JSON.stringify({ ok: false, missing: 'payment submit' });
  if (submit.disabled) return JSON.stringify({ ok: false, missing: 'payment submit enabled' });
  submit.click();
  return JSON.stringify({ ok: true });
})()"#
        .to_string()
}

pub fn open_subscription_management_expression() -> String {
    r#"(() => {
  const nodes = Array.from(document.querySelectorAll('a, button'));
  const byText = nodes.find(node => {
    const text = (node.innerText || node.textContent || '').trim().toLowerCase();
    return text.includes('manage subscription');
  });
  const byHref = nodes.find(node => {
    const href = String(node.getAttribute && node.getAttribute('href') || '');
    return href === '/user/subscription' || href.endsWith('/user/subscription');
  });
  const target = byText || byHref;
  if (!target) return JSON.stringify({ ok: false, missing: 'Manage subscription' });
  target.click();
  return JSON.stringify({ ok: true });
})()"#
        .to_string()
}

pub fn accept_extra_trial_offer_expression() -> String {
    r#"(() => new Promise(resolve => {
  const deadline = Date.now() + 8000;
  const done = value => resolve(JSON.stringify(value));
  const normalize = text => String(text || '').replace(/\u2019/g, "'").replace(/\s+/g, ' ').trim().toLowerCase();
  const visible = node => {
    if (!node) return false;
    const style = window.getComputedStyle ? window.getComputedStyle(node) : null;
    if (style && (style.visibility === 'hidden' || style.display === 'none')) return false;
    return !!(node.offsetWidth || node.offsetHeight || node.getClientRects().length);
  };
  const enabled = node => visible(node) && !node.disabled && node.getAttribute('aria-disabled') !== 'true' && node.getAttribute('data-ol-loading') !== 'true';
  const nodes = () => Array.from(document.querySelectorAll('button, a'));
  const findCancel = () => nodes().find(node => {
    const text = normalize(node.innerText || node.textContent || node.value);
    return enabled(node) && ['cancel your subscription', 'cancel subscription', 'cancel your trial'].includes(text);
  });
  const findOffer = () => nodes().find(node => {
    const text = normalize(node.innerText || node.textContent || node.value);
    return enabled(node) && (text.includes("i'll take it") || ['take it', 'accept', 'keep my subscription', 'extend free trial'].includes(text));
  });
  const hasOfferText = () => {
    const text = normalize(document.body ? document.body.innerText : '');
    return text.includes('another 14 days') || text.includes('14 days extra');
  };
  let cancelClicked = false;
  const tick = () => {
    const offer = hasOfferText() && findOffer();
    if (offer) {
      offer.click();
      // The caller verifies the server expiry before reporting success.
      return done({ ok: true });
    }
    if (!cancelClicked) {
      const cancel = findCancel();
      if (cancel) { cancel.click(); cancelClicked = true; }
    }
    if (Date.now() >= deadline) return done({ ok: false, missing: 'extra 14 days offer button' });
    setTimeout(tick, 150);
  };
  tick();
}))()"#
        .to_string()
}

pub fn change_to_pro_annual_plan_expression() -> String {
    r#"(() => new Promise(resolve => {
  const deadline = Date.now() + 20000;
  const done = value => resolve(JSON.stringify(value));
  const normalize = text => String(text || '').replace(/\u2019/g, "'").trim().toLowerCase();
  const visible = node => {
    if (!node) return false;
    const style = window.getComputedStyle ? window.getComputedStyle(node) : null;
    if (style && (style.visibility === 'hidden' || style.display === 'none')) return false;
    return !!(node.offsetWidth || node.offsetHeight || node.getClientRects().length);
  };
  const enabled = node => visible(node) && !node.disabled && node.getAttribute('aria-disabled') !== 'true';
  const nodes = selector => Array.from(document.querySelectorAll(selector));
  const visibleDialog = () => nodes('[role="dialog"], .modal').find(visible);
  const currentPlanIsProAnnual = () => {
    if (visibleDialog()) return false;
    const text = normalize(document.body ? document.body.innerText : '');
    return location.pathname === '/user/subscription'
      && text.includes('your subscriptions')
      && text.includes('plan\npro annual');
  };
  const findChangePlan = () => nodes('button, a').find(node => {
    const text = normalize(node.innerText || node.textContent || node.value);
    return enabled(node) && text === 'change plan' && !node.closest('[role="dialog"], .modal');
  });
  const findPlanButton = () => {
    const dialog = visibleDialog();
    if (!dialog) return null;
    const row = Array.from(dialog.querySelectorAll('tr')).find(node => {
      const label = normalize(node.querySelector('strong')?.textContent);
      return label === 'pro annual';
    });
    return Array.from(row?.querySelectorAll('button') || []).find(node => {
      const text = normalize(node.innerText || node.textContent || node.value);
      return enabled(node) && text === 'change to this plan';
    }) || null;
  };
  const findConfirm = () => {
    const dialog = visibleDialog();
    if (!dialog) return null;
    return Array.from(dialog.querySelectorAll('button')).find(node => {
      const text = normalize(node.innerText || node.textContent || node.value);
      return enabled(node) && text === 'change plan';
    }) || null;
  };
  let opened = false;
  let selected = false;
  let confirmed = false;
  const tick = () => {
    if (currentPlanIsProAnnual()) return done({ ok: true });
    if (!opened) {
      const change = findChangePlan();
      if (change) {
        change.click();
        opened = true;
        return setTimeout(tick, 150);
      }
      if (Date.now() >= deadline) return done({ ok: false, missing: 'Change plan' });
      return setTimeout(tick, 150);
    }
    if (!selected) {
      const planButton = findPlanButton();
      if (planButton) {
        planButton.click();
        selected = true;
        return setTimeout(tick, 150);
      }
      if (Date.now() >= deadline) return done({ ok: false, missing: 'Pro annual row action' });
      return setTimeout(tick, 150);
    }
    if (!confirmed) {
      const confirm = findConfirm();
      if (confirm) {
        confirm.click();
        confirmed = true;
        return setTimeout(tick, 150);
      }
      if (Date.now() >= deadline) return done({ ok: false, missing: 'Confirm Change plan' });
      return setTimeout(tick, 150);
    }
    if (Date.now() >= deadline) return done({ ok: false, missing: 'Pro annual verification' });
    setTimeout(tick, 150);
  };
  tick();
}))()"#
        .to_string()
}

pub fn cancel_subscription_final_expression() -> String {
    r#"(() => new Promise(resolve => {
  const deadline = Date.now() + 15000;
  const done = value => resolve(JSON.stringify(value));
  const normalize = text => String(text || '').replace(/\u2019/g, "'").replace(/\s+/g, ' ').trim().toLowerCase();
  const visible = node => {
    if (!node) return false;
    const style = window.getComputedStyle ? window.getComputedStyle(node) : null;
    if (style && (style.visibility === 'hidden' || style.display === 'none')) return false;
    return !!(node.offsetWidth || node.offsetHeight || node.getClientRects().length);
  };
  const enabled = node => visible(node) && !node.disabled && node.getAttribute('aria-disabled') !== 'true' && node.getAttribute('data-ol-loading') !== 'true';
  const nodes = () => Array.from(document.querySelectorAll('button, a'));
  const cancellationConfirmed = () => {
    const text = normalize(document.body ? document.body.innerText : '');
    return location.pathname.includes('/user/subscription/canceled')
      || text.includes('subscription has been canceled')
      || text.includes('subscription has been cancelled')
      || text.includes('no further payments will be taken');
  };
  const findCancel = () => nodes().find(node => {
    const text = normalize(node.innerText || node.textContent || node.value);
    return enabled(node) && ['cancel your subscription', 'cancel subscription', 'cancel your trial'].includes(text);
  });
  const findConfirm = () => nodes().find(node => {
    const text = normalize(node.innerText || node.textContent || node.value);
    return enabled(node) && ['cancel my subscription', 'cancel trial'].includes(text);
  });
  let cancelClicked = false;
  let confirmClicked = false;
  let offerDeclined = false;
  const tick = () => {
    if (cancellationConfirmed()) return done({ ok: true });
    if (!confirmClicked) {
      const decline = nodes().find(node => enabled(node) && normalize(node.innerText || node.textContent) === 'cancel anyway');
      if (decline && !offerDeclined) {
        decline.click();
        offerDeclined = true;
        return setTimeout(tick, 150);
      }
      const confirm = findConfirm();
      if (confirm) {
        confirm.click();
        confirmClicked = true;
        return setTimeout(tick, 150);
      }
      if (!cancelClicked) {
        const cancel = findCancel();
        if (cancel) { cancel.click(); cancelClicked = true; }
      }
      if (Date.now() >= deadline) return done({ ok: false, missing: 'Cancel my subscription / Cancel trial' });
      return setTimeout(tick, 150);
    }
    if (Date.now() >= deadline) return done({ ok: false, missing: 'Cancellation confirmation' });
    setTimeout(tick, 150);
  };
  tick();
}))()"#
        .to_string()
}

pub fn registration_subscription_url(trial_days: u32) -> Option<String> {
    let plan_code = registration_trial_plan_code(trial_days)?;
    Some(format!(
        "{OVERLEAF_SUBSCRIPTION_NEW_URL}?planCode={plan_code}&currency=USD&itm_campaign=plans&itm_content=table-header&itm_referrer=welcome-page-prompt"
    ))
}

pub fn login_page_state_expression() -> String {
    r#"(() => {
  const visible = element => {
    if (!element) return false;
    const style = window.getComputedStyle ? window.getComputedStyle(element) : null;
    const rect = element.getBoundingClientRect ? element.getBoundingClientRect() : null;
    return (!style || (style.display !== 'none' && style.visibility !== 'hidden' && Number(style.opacity || 1) > 0))
      && (!rect || (rect.width > 0 && rect.height > 0));
  };
  const frames = Array.from(document.querySelectorAll('iframe'));
  const recaptchaFrames = frames.filter(frame => {
    const source = String(frame.src || '').toLowerCase();
    const title = String(frame.title || '').toLowerCase();
    return source.includes('recaptcha') || title.includes('recaptcha');
  });
  const visibleFrames = recaptchaFrames.filter(visible);
  const challengeFrames = visibleFrames.filter(frame => {
    const source = String(frame.src || '').toLowerCase();
    const title = String(frame.title || '').toLowerCase();
    return source.includes('/bframe') || title.includes('challenge');
  });
  const text = document.body ? String(document.body.innerText || '').toLowerCase() : '';
  const errorNodes = Array.from(document.querySelectorAll(
    '[role="alert"], .alert, .alert-danger, .invalid-feedback, .form-control-feedback, [data-testid*="error" i], [class*="error" i]'
  )).filter(visible);
  const visibleErrorMessages = Array.from(new Set(errorNodes
    .map(node => String(node.innerText || node.textContent || '').trim())
    .filter(Boolean)))
    .slice(0, 8)
    .map(message => message.slice(0, 240));
  const invalidFormFields = Array.from(document.querySelectorAll('input, select, textarea'))
    .filter(visible)
    .filter(field => field.getAttribute('aria-invalid') === 'true'
      || (typeof field.checkValidity === 'function' && !field.checkValidity()))
    .map(field => {
      const fieldName = field.name || field.id || field.getAttribute('autocomplete') || field.type || 'field';
      const describedBy = String(field.getAttribute('aria-describedby') || '')
        .split(/\s+/)
        .filter(Boolean)
        .map(id => document.getElementById(id))
        .filter(Boolean)
        .map(node => String(node.innerText || node.textContent || '').trim())
        .filter(Boolean)
        .join(' ');
      const message = String(field.validationMessage || describedBy || 'invalid').trim();
      return `${fieldName}: ${message}`.slice(0, 240);
    })
    .slice(0, 8);
  const signalText = `${visibleErrorMessages.join('\n')}\n${text}`.toLowerCase();
  const credentialErrorDetected = [
    'your email or password is incorrect',
    'email or password is incorrect',
    'invalid email or password',
    'incorrect email or password'
  ].some(marker => signalText.includes(marker));
  const registeredEmailDetected = [
    'already associated with a different overleaf account',
    'email is already registered',
    'email address is already registered'
  ].some(marker => signalText.includes(marker));
  const robotVerificationBlocked = text.includes('cannot_verify_user_not_robot')
    || text.includes('could not verify that you are not a robot')
    || text.includes('captcha check failed');
  const textSignal = text.includes('complete the recaptcha')
    || text.includes('please complete recaptcha')
    || text.includes('please complete the recaptcha')
    || text.includes('robot verification');
  const recaptchaControls = Array.from(document.querySelectorAll(
    '[data-sitekey], .g-recaptcha, textarea[name="g-recaptcha-response"], input[name="g-recaptcha-response"]'
  ));
  const detected = challengeFrames.length > 0 || textSignal;
  const possible = detected || recaptchaFrames.length > 0 || recaptchaControls.length > 0;
  return JSON.stringify({
    url: location.href || '',
    title: document.title || '',
    text: document.body ? document.body.innerText || '' : '',
    visible_error_messages: visibleErrorMessages,
    invalid_form_fields: invalidFormFields,
    credential_error_detected: credentialErrorDetected,
    registered_email_detected: registeredEmailDetected,
    robot_verification_blocked: robotVerificationBlocked,
    recaptcha_detected: detected,
    recaptcha_possible: possible,
    recaptcha_iframe_count: recaptchaFrames.length,
    recaptcha_visible_count: visibleFrames.length,
    recaptcha_challenge_count: challengeFrames.length
  });
})()"#
        .to_string()
}

pub fn git_token_action_expression() -> String {
    git_token_expression(true)
}

pub fn git_token_page_state_expression() -> String {
    git_token_expression(false)
}

fn git_token_expression(generate: bool) -> String {
    r#"(() => new Promise(resolve => {
  const generate = __GENERATE__;
  const deadline = Date.now() + 15000;
  let clicked = false;
  const done = value => resolve(JSON.stringify(value));
  const tokenPattern = /\bolp_[A-Za-z0-9_-]+\b/g;
  const monthIndex = name => {
    const months = {
      january: 0, jan: 0, february: 1, feb: 1, march: 2, mar: 2,
      april: 3, apr: 3, may: 4, june: 5, jun: 5, july: 6, jul: 6,
      august: 7, aug: 7, september: 8, sep: 8, sept: 8,
      october: 9, oct: 9, november: 10, nov: 10, december: 11, dec: 11
    };
    return months[String(name || '').toLowerCase().replace(/\.$/, '')];
  };
  const parseExpiry = text => {
    const candidates = [];
    const pushDate = (day, month, year) => {
      const idx = monthIndex(month);
      const numericDay = Number(day);
      const numericYear = Number(year);
      if (idx === undefined || !numericDay || !numericYear) return;
      candidates.push(Math.floor(Date.UTC(numericYear, idx, numericDay) / 1000));
    };
    for (const match of String(text || '').matchAll(/\b(\d{1,2})(?:st|nd|rd|th)?\s+([A-Za-z]+\.?)\s+(\d{4})\b/g)) {
      pushDate(match[1], match[2], match[3]);
    }
    for (const match of String(text || '').matchAll(/\b([A-Za-z]+\.?)\s+(\d{1,2})(?:st|nd|rd|th)?,?\s+(\d{4})\b/g)) {
      pushDate(match[2], match[1], match[3]);
    }
    if (!candidates.length) return null;
    const now = Math.floor(Date.now() / 1000);
    const future = candidates.filter(value => value > now);
    return Math.max(...(future.length ? future : candidates));
  };
  const visibleTokenFrom = text => {
    const source = String(text || '');
    tokenPattern.lastIndex = 0;
    for (const match of source.matchAll(tokenPattern)) {
      if (source[match.index + match[0].length] === '*') continue;
      return match[0];
    }
    return null;
  };
  const visible = node => {
    if (!node) return false;
    const style = window.getComputedStyle ? window.getComputedStyle(node) : null;
    const rect = node.getBoundingClientRect ? node.getBoundingClientRect() : null;
    return (!style || (style.display !== 'none' && style.visibility !== 'hidden' && Number(style.opacity || 1) > 0))
      && (!rect || (rect.width > 0 && rect.height > 0));
  };
  const findAction = needle => Array.from(document.querySelectorAll('button, a')).find(node => {
    if (!visible(node) || node.disabled || node.getAttribute('aria-disabled') === 'true') return false;
    const text = String(node.innerText || node.textContent || node.value || '').trim().toLowerCase();
    return text.includes(needle);
  });
  const collect = () => {
    const bodyText = document.body ? String(document.body.innerText || '') : '';
    const selectors = [
      "span[aria-label='Git authentication token'] code",
      "[aria-label='Git authentication token'] code",
      '.git-bridge-copy code',
      'code'
    ];
    let token = null;
    for (const selector of selectors) {
      for (const node of document.querySelectorAll(selector)) {
        if (!visible(node)) continue;
        token = visibleTokenFrom(node.textContent || '');
        if (token) break;
      }
      if (token) break;
    }
    token = token || visibleTokenFrom(bodyText);
    return {
      token,
      expires_at: parseExpiry(bodyText),
      has_existing_masked_token: /olp_[A-Za-z0-9_-]*\*+/.test(bodyText),
      can_generate_token: Boolean(findAction('generate token')),
      can_add_another_token: Boolean(findAction('add another token')),
      url: location.href || ''
    };
  };
  const tick = () => {
    const state = collect();
    const path = String(location.pathname || '').toLowerCase();
    const loginPage = path.includes('/login') || path.includes('/signin');
    if (generate && !loginPage) {
      if (state.token) return done({ ok: true, ...state });
      if (!clicked) {
        const button = findAction('generate token') || findAction('add another token');
        if (button) { button.click(); clicked = true; }
      }
      if (Date.now() >= deadline) return done({ ok: false, missing: 'Git authentication token', ...state });
      return setTimeout(tick, 100);
    }
    const settingsPage = path.includes('/user/settings');
    const pageReady = state.token
      || state.has_existing_masked_token
      || state.can_generate_token
      || state.can_add_another_token
      || state.expires_at !== null
      || (settingsPage && document.readyState === 'complete' && document.body && document.body.innerText);
    if (loginPage || pageReady || Date.now() >= deadline) {
      return done({
        ok: !loginPage,
        missing: loginPage ? 'authenticated Git settings page' : undefined,
        token: state.token,
        expires_at: state.expires_at,
        url: state.url,
        has_existing_masked_token: state.has_existing_masked_token,
        can_generate_token: state.can_generate_token,
        can_add_another_token: state.can_add_another_token
      });
    }
    setTimeout(tick, 100);
  };
  tick();
}))()"#
        .replace("__GENERATE__", if generate { "true" } else { "false" })
}

pub fn fill_password_change_form_expression(current_password: &str, new_password: &str) -> String {
    let current_json =
        serde_json::to_string(current_password).expect("password string should serialize");
    let new_json = serde_json::to_string(new_password).expect("password string should serialize");

    format!(
        r#"(() => new Promise(resolve => {{
  const currentPassword = {current_json};
  const newPassword = {new_json};
  const deadline = Date.now() + 5000;
  const setValue = (el, value) => {{
    el.focus();
    const prototype = Object.getPrototypeOf(el);
    const descriptor = Object.getOwnPropertyDescriptor(prototype, 'value')
      || (window.HTMLInputElement && el instanceof window.HTMLInputElement
        ? Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')
        : null);
    if (descriptor && descriptor.set) {{
      descriptor.set.call(el, value);
    }} else {{
      el.value = value;
    }}
    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
  }};
  const done = value => resolve(JSON.stringify(value));
  const tick = () => {{
    const currentInput = document.querySelector('#current-password-input');
    const newInput = document.querySelector('#new-password-1-input');
    const confirmInput = document.querySelector('#new-password-2-input');
    const submit = document.querySelector('button[form="password-change-form"][type="submit"], #password-change-form button[type="submit"]');
    if (!currentInput || !newInput || !confirmInput || !submit) {{
      if (Date.now() >= deadline) {{
        const missing = !currentInput
          ? '#current-password-input'
          : !newInput
            ? '#new-password-1-input'
            : !confirmInput
              ? '#new-password-2-input'
              : 'password change submit';
        return done({{ ok: false, missing }});
      }}
      return setTimeout(tick, 100);
    }}
    setValue(currentInput, currentPassword);
    setValue(newInput, newPassword);
    setValue(confirmInput, newPassword);
    if (currentInput.value !== currentPassword || newInput.value !== newPassword || confirmInput.value !== newPassword) {{
      if (Date.now() >= deadline) {{
        const missing = currentInput.value !== currentPassword
          ? '#current-password-input value'
          : newInput.value !== newPassword
            ? '#new-password-1-input value'
            : '#new-password-2-input value';
        return done({{ ok: false, missing }});
      }}
      return setTimeout(tick, 100);
    }}
    if (!submit.disabled) return done({{ ok: true }});
    if (Date.now() >= deadline) return done({{ ok: false, missing: 'password change submit enabled' }});
    setTimeout(tick, 100);
  }};
  tick();
}}))()"#
    )
}

pub fn submit_password_change_form_expression() -> String {
    r#"(() => {
  const submit = document.querySelector('button[form="password-change-form"][type="submit"], #password-change-form button[type="submit"]');
  if (!submit) return JSON.stringify({ ok: false, missing: 'password change submit' });
  if (submit.disabled) return JSON.stringify({ ok: false, missing: 'password change submit enabled' });
  submit.click();
  return JSON.stringify({ ok: true });
})()"#
        .to_string()
}

pub fn parse_action_result(input: &str) -> BrowserAutomationResult<CdpLoginActionResult> {
    serde_json::from_str(input).map_err(|error| BrowserAutomationError::ExternalDriver {
        message: format!("parse login action result: {error}"),
    })
}

pub fn parse_login_page_state(input: &str) -> BrowserAutomationResult<CdpLoginPageState> {
    serde_json::from_str(input).map_err(|error| BrowserAutomationError::ExternalDriver {
        message: format!("parse login page state: {error}"),
    })
}

pub fn classify_login_page_state(page_state: &CdpLoginPageState) -> CdpLoginState {
    let url = page_state.url.to_ascii_lowercase();
    let text = page_state.text.to_ascii_lowercase();

    if url.contains("/project") {
        return CdpLoginState::Success;
    }

    if page_state.registered_email_detected
        || text.contains("already associated with a different overleaf account")
    {
        return CdpLoginState::RegisteredEmail;
    }

    // 服务端明确的凭据错误优先于未激活的验证码 iframe。
    if page_state.credential_error_detected {
        return CdpLoginState::InvalidCredentials;
    }

    if page_state.robot_verification_blocked {
        return CdpLoginState::RobotVerificationBlocked;
    }

    if page_state.recaptcha_detected
        || text.contains("complete the recaptcha")
        || text.contains("please complete recaptcha")
        || text.contains("please complete the recaptcha")
        || text.contains("robot verification")
    {
        return CdpLoginState::ChallengeRequired;
    }

    if page_state.recaptcha_possible {
        return CdpLoginState::ChallengePossible;
    }

    CdpLoginState::Pending
}

pub fn classify_registration_page_state(page_state: &CdpLoginPageState) -> CdpRegistrationState {
    let url = page_state.url.to_ascii_lowercase();
    let text = page_state.text.to_ascii_lowercase();

    if text.contains("already associated with a different overleaf account") {
        return CdpRegistrationState::RegisteredEmail;
    }

    if text.contains("invalid code")
        || text.contains("incorrect code")
        || text.contains("code is invalid")
        || text.contains("expired code")
        || text.contains("verification code is incorrect")
    {
        return CdpRegistrationState::EmailVerificationFailed;
    }

    if url.contains("/registration/try-premium")
        || text.contains("your email is confirmed")
        || text.contains("email is confirmed")
    {
        return CdpRegistrationState::AccountCreated;
    }

    if url.contains("/registration/confirm-email")
        || url.contains("confirm-email")
        || text.contains("verify your email")
        || text.contains("confirm your email")
        || text.contains("verification code")
        || text.contains("6 digit code")
        || text.contains("6-digit code")
    {
        return CdpRegistrationState::EmailVerificationRequired;
    }

    if page_state.recaptcha_detected
        || text.contains("complete the recaptcha")
        || text.contains("please complete recaptcha")
        || text.contains("please complete the recaptcha")
        || text.contains("robot verification")
    {
        return CdpRegistrationState::ChallengeRequired;
    }

    if is_confirmed_active_subscription_page(page_state) {
        return CdpRegistrationState::SubscriptionSucceeded;
    }

    if url.contains("thank-you")
        || text.contains("thank you for subscribing")
        || url.contains("/project")
        || url.contains("/user/subscription/new")
        || text.contains("start your free trial")
        || text.contains("upgrade now")
    {
        return CdpRegistrationState::AccountCreated;
    }

    CdpRegistrationState::Pending
}

pub fn classify_registration_trial_purchase_page(
    page_state: &CdpLoginPageState,
    trial_days: u32,
) -> CdpTrialPurchaseAvailability {
    let Some(plan_code) = registration_trial_plan_code(trial_days) else {
        return CdpTrialPurchaseAvailability::Unknown;
    };
    let url = page_state.url.to_ascii_lowercase();
    if !url.contains("/user/subscription/new") {
        return CdpTrialPurchaseAvailability::Unknown;
    }
    let plan_code = plan_code.to_ascii_lowercase();
    if url.contains("plancode=") && !url.contains(&plan_code) {
        return CdpTrialPurchaseAvailability::Unknown;
    }

    let text = page_state.text.to_ascii_lowercase();
    let unavailable_markers = [
        "not as a trial",
        "completed a trial before",
        "not eligible for a trial",
        "no longer eligible for a free trial",
        "trial is not available",
        "trial not available",
    ];
    if unavailable_markers
        .iter()
        .any(|marker| text.contains(marker))
    {
        return CdpTrialPurchaseAvailability::Unavailable;
    }

    let available_markers = [
        "free 7-day trial",
        "free 7 day trial",
        "start your free trial",
        "pay after 7 days",
    ];
    if available_markers.iter().any(|marker| text.contains(marker)) {
        CdpTrialPurchaseAvailability::Available
    } else {
        CdpTrialPurchaseAvailability::Unknown
    }
}

pub fn detect_registration_payment_error(page_state: &CdpLoginPageState) -> Option<String> {
    let text = page_state.text.to_ascii_lowercase();
    let markers = [
        "card declined",
        "your card was declined",
        "payment was declined",
        "payment failed",
        "payment error",
        "unable to process your payment",
        "could not process your payment",
        "couldn't process your payment",
        "couldn’t process your payment",
        "payment details appear to be invalid",
        "invalid card",
        "incorrect card",
        "支付失败",
        "银行卡无效",
        "卡片无效",
        "支付信息无效",
        "付款信息无效",
    ];
    if !markers.iter().any(|marker| text.contains(marker)) {
        return None;
    }

    page_state
        .text
        .lines()
        .map(str::trim)
        .find(|line| {
            let line = line.to_ascii_lowercase();
            markers.iter().any(|marker| line.contains(marker))
        })
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .or_else(|| Some("payment failed".to_string()))
}

pub fn is_subscription_management_page(page_state: &CdpLoginPageState) -> bool {
    let url = page_state.url.to_ascii_lowercase();
    let text = page_state.text.to_ascii_lowercase();

    url.contains("/user/subscription")
        && !url.contains("/user/subscription/new")
        && (text.contains("your subscriptions")
            || text.contains("currently subscribed")
            || text.contains("cancel your subscription")
            || text.contains("reactivate your subscription")
            || text.contains("change plan"))
}

pub fn is_confirmed_active_subscription_page(page_state: &CdpLoginPageState) -> bool {
    if !is_subscription_management_page(page_state) {
        return false;
    }

    let text = page_state.text.to_ascii_lowercase();
    text.contains("currently subscribed")
        || text.contains("cancel your subscription")
        || text.contains("subscription will end")
        || text.contains("next billing")
}

pub fn is_registration_payment_post_checkout_candidate(page_state: &CdpLoginPageState) -> bool {
    let path = registration_page_path(&page_state.url);
    path.contains("thank-you")
        || path == "/project"
        || path.starts_with("/project/")
        || is_subscription_management_page(page_state)
}

fn registration_payment_state_diagnostic(page_state: &CdpLoginPageState) -> String {
    let title = page_state
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let title = title.chars().take(80).collect::<String>();
    let first_visible_error = page_state
        .visible_error_messages
        .first()
        .map(|message| message.split_whitespace().collect::<Vec<_>>().join(" "))
        .map(|message| message.chars().take(160).collect::<String>());
    format!(
        " (path={}, title={title:?}, state={:?}, recaptcha_visible={}, visible_errors={}, first_visible_error={first_visible_error:?}, invalid_fields={})",
        registration_page_path(&page_state.url),
        classify_registration_page_state(page_state),
        page_state.recaptcha_visible_count,
        page_state.visible_error_messages.len(),
        page_state.invalid_form_fields.len()
    )
}

fn registration_page_path(url: &str) -> &str {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    without_query
        .split_once("://")
        .and_then(|(_, remainder)| remainder.find('/').map(|index| &remainder[index..]))
        .unwrap_or(without_query)
}

pub fn is_pro_annual_subscription_page(page_state: &CdpLoginPageState) -> bool {
    is_confirmed_active_subscription_page(page_state)
        && page_state.text.to_ascii_lowercase().contains("pro annual")
}

pub fn is_subscription_cancellation_confirmed(page_state: &CdpLoginPageState) -> bool {
    let url = page_state.url.to_ascii_lowercase();
    let text = page_state.text.to_ascii_lowercase();

    (url.contains("/user/subscription") || url.contains("/canceled"))
        && (url.contains("/canceled")
            || text.contains("subscription has been canceled")
            || text.contains("subscription has been cancelled")
            || text.contains("will terminate on")
            || text.contains("no further payments will be taken"))
}

pub fn classify_password_change_page_state(
    page_state: &CdpLoginPageState,
) -> CdpPasswordChangeState {
    let url = page_state.url.to_ascii_lowercase();
    let text = page_state.text.to_ascii_lowercase();

    if text.contains("password has been changed")
        || text.contains("password changed")
        || text.contains("password updated")
        || text.contains("successfully changed")
    {
        return CdpPasswordChangeState::Success;
    }

    if url.contains("/login")
        || text.contains("current password is incorrect")
        || text.contains("incorrect current password")
        || text.contains("wrong current password")
        || text.contains("invalid current password")
        || text.contains("passwords do not match")
        || text.contains("password does not match")
    {
        return CdpPasswordChangeState::Failed;
    }

    CdpPasswordChangeState::Pending
}

async fn evaluate_action<T>(
    session: &mut CdpSession<T>,
    expression: &str,
) -> BrowserAutomationResult<CdpLoginActionResult>
where
    T: CdpTransport,
{
    evaluate_action_with_await(session, expression, false).await
}

async fn evaluate_action_with_await<T>(
    session: &mut CdpSession<T>,
    expression: &str,
    await_promise: bool,
) -> BrowserAutomationResult<CdpLoginActionResult>
where
    T: CdpTransport,
{
    let result = evaluate_string_with_navigation_retry(session, expression, await_promise).await?;
    parse_action_result(&result)
}

async fn login_result_from_session<T>(
    session: &mut CdpSession<T>,
    email: Option<String>,
) -> BrowserAutomationResult<LoginResult>
where
    T: CdpTransport,
{
    let cookie = session
        .get_overleaf_session_cookie()
        .await
        .map_err(cdp_error)?;
    Ok(LoginResult {
        email,
        user_id: None,
        cookies: vec![overleaf_session_cookie(
            cookie.value,
            cookie.expires.map(|value| value as i64),
        )],
        trial_expiry: None,
        subscription_status: None,
        subscription_label: None,
    })
}

async fn read_login_page_state<T>(
    session: &mut CdpSession<T>,
) -> BrowserAutomationResult<CdpLoginPageState>
where
    T: CdpTransport,
{
    let expression = login_page_state_expression();
    let result = evaluate_string_with_navigation_retry(session, &expression, false).await?;
    parse_login_page_state(&result)
}

async fn wait_for_authenticated_project_page<T>(
    session: &mut CdpSession<T>,
    options: &CdpLoginOptions,
) -> BrowserAutomationResult<()>
where
    T: CdpTransport,
{
    for _ in 0..options.status_checks.max(1) {
        let page_state = read_login_page_state(session).await?;
        let url = page_state.url.to_ascii_lowercase();
        if url.contains("/login") {
            return Err(BrowserAutomationError::InvalidCredentials);
        }
        if url.contains("/project") {
            return Ok(());
        }
        sleep_after_pending_check(options.poll_delay_ms).await;
    }

    Err(BrowserAutomationError::Timeout {
        operation: "return to authenticated Overleaf project page after registration".to_string(),
    })
}

async fn evaluate_string_with_navigation_retry<T>(
    session: &mut CdpSession<T>,
    expression: &str,
    await_promise: bool,
) -> BrowserAutomationResult<String>
where
    T: CdpTransport,
{
    for attempt in 1..=RUNTIME_NAVIGATION_RETRY_ATTEMPTS {
        match session.evaluate_string(expression, await_promise).await {
            Ok(result) => return Ok(result),
            Err(error)
                if error.is_transient_navigation_context_error()
                    && attempt < RUNTIME_NAVIGATION_RETRY_ATTEMPTS =>
            {
                sleep_after_pending_check(RUNTIME_NAVIGATION_RETRY_DELAY_MS).await;
            }
            Err(error) => return Err(cdp_error(error)),
        }
    }

    unreachable!("runtime navigation retry loop always returns")
}

async fn read_git_token_expiry_after_reload<T>(
    session: &mut CdpSession<T>,
) -> BrowserAutomationResult<Option<i64>>
where
    T: CdpTransport,
{
    session
        .navigate(OVERLEAF_SETTINGS_URL)
        .await
        .map_err(cdp_error)?;
    let state =
        evaluate_action_with_await(session, &git_token_page_state_expression(), true).await?;
    Ok(state.expires_at)
}

async fn read_git_token_page_state_in_current_session<T>(
    session: &mut CdpSession<T>,
) -> BrowserAutomationResult<GitTokenPageState>
where
    T: CdpTransport,
{
    session
        .navigate(OVERLEAF_SETTINGS_URL)
        .await
        .map_err(cdp_error)?;
    let action =
        evaluate_action_with_await(session, &git_token_page_state_expression(), true).await?;
    if !action.ok {
        return Err(BrowserAutomationError::InvalidCredentials);
    }

    Ok(GitTokenPageState {
        visible_token: action
            .token
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
            .map(SecretText::new),
        expires_at: action.expires_at,
        has_existing_masked_token: action.has_existing_masked_token,
        can_generate_token: action.can_generate_token,
        can_add_another_token: action.can_add_another_token,
    })
}

async fn generate_git_token_in_current_session<T>(
    session: &mut CdpSession<T>,
) -> BrowserAutomationResult<GitTokenResult>
where
    T: CdpTransport,
{
    session
        .navigate(OVERLEAF_SETTINGS_URL)
        .await
        .map_err(cdp_error)?;
    let action = evaluate_action_with_await(session, &git_token_action_expression(), true).await?;
    if !action.ok {
        return Ok(GitTokenResult {
            token: None,
            expires_at: action.expires_at,
        });
    }

    let token = action
        .token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned);
    let refreshed_expiry = if token.is_some() {
        read_git_token_expiry_after_reload(session).await?
    } else {
        None
    };

    Ok(GitTokenResult {
        token: token.map(SecretText::new),
        expires_at: refreshed_expiry.or(action.expires_at),
    })
}

async fn read_optional_overleaf_session_cookie<T>(session: &mut CdpSession<T>) -> Vec<CookieCapture>
where
    T: CdpTransport,
{
    match session.get_overleaf_session_cookie().await {
        Ok(cookie) => vec![overleaf_session_cookie(
            cookie.value,
            cookie.expires.map(|value| value as i64),
        )],
        Err(_) => Vec::new(),
    }
}

fn ensure_action_ok(action_result: CdpLoginActionResult) -> BrowserAutomationResult<()> {
    if action_result.ok {
        return Ok(());
    }

    Err(BrowserAutomationError::ElementNotFound {
        selector: action_result
            .missing
            .unwrap_or_else(|| "login form element".to_string()),
    })
}

fn captcha_wait_state() -> &'static StdMutex<CaptchaWaitState> {
    CAPTCHA_WAIT_STATE.get_or_init(|| StdMutex::new(CaptchaWaitState::default()))
}

fn register_captcha_waiter() -> (CaptchaWaitPermit, CaptchaWaitBudget) {
    let id = CAPTCHA_WAITER_ID.fetch_add(1, Ordering::Relaxed);
    let mut state = captcha_wait_state()
        .lock()
        .expect("captcha wait state lock poisoned");
    if state.active_waiters.is_empty() {
        state.peak_waiters = 0;
    }
    state.active_waiters.insert(id);
    let active_count = state.active_waiters.len();
    state.peak_waiters = state.peak_waiters.max(active_count);
    (
        CaptchaWaitPermit {
            id,
            released: false,
        },
        CaptchaWaitBudget {
            active_count,
            wait_seconds: state.peak_waiters as u64 * CAPTCHA_WAIT_SECONDS_PER_CHALLENGE,
        },
    )
}

fn current_captcha_wait_budget() -> CaptchaWaitBudget {
    let state = captcha_wait_state()
        .lock()
        .expect("captcha wait state lock poisoned");
    let peak_waiters = state.peak_waiters.max(1);
    CaptchaWaitBudget {
        active_count: state.active_waiters.len(),
        wait_seconds: peak_waiters as u64 * CAPTCHA_WAIT_SECONDS_PER_CHALLENGE,
    }
}

fn release_captcha_waiter(id: u64) {
    let mut state = captcha_wait_state()
        .lock()
        .expect("captcha wait state lock poisoned");
    state.active_waiters.remove(&id);
    if state.active_waiters.is_empty() {
        state.peak_waiters = 0;
    }
}

fn extend_login_checks_for_captcha(
    current_max_checks: usize,
    completed_checks: usize,
    poll_delay_ms: u64,
    wait_seconds: u64,
) -> usize {
    current_max_checks
        .max(completed_checks + captcha_wait_extra_checks(wait_seconds, poll_delay_ms))
}

fn captcha_wait_extra_checks(wait_seconds: u64, poll_delay_ms: u64) -> usize {
    if wait_seconds == 0 || poll_delay_ms == 0 {
        return 1;
    }
    let wait_ms = wait_seconds.saturating_mul(1_000);
    wait_ms.div_ceil(poll_delay_ms).max(1) as usize
}

fn is_near_login_deadline(
    check_index: usize,
    max_checks: usize,
    poll_delay_ms: u64,
    near_deadline_seconds: u64,
) -> bool {
    let remaining_checks = max_checks.saturating_sub(check_index + 1);
    let remaining_ms = remaining_checks as u64 * poll_delay_ms.max(1);
    remaining_ms <= near_deadline_seconds.saturating_mul(1_000)
}

impl Drop for CaptchaWaitPermit {
    fn drop(&mut self) {
        if !self.released {
            release_captcha_waiter(self.id);
            self.released = true;
        }
    }
}

async fn sleep_after_pending_check(poll_delay_ms: u64) {
    if poll_delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(poll_delay_ms)).await;
    }
}

fn cdp_error(error: CdpSessionError) -> BrowserAutomationError {
    BrowserAutomationError::ExternalDriver {
        message: error.to_string(),
    }
}
