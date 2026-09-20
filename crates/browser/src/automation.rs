use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

#[derive(Clone, PartialEq, Eq)]
pub struct SecretText(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialsLoginInput {
    pub email: String,
    pub password: SecretText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationInput {
    pub alias_hint: Option<String>,
    pub email: String,
    pub password: SecretText,
    pub trial_days: u32,
    pub auto_fetch_git_token: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitTokenInput {
    pub overleaf_session: SecretText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordChangeInput {
    pub email: String,
    pub overleaf_session: SecretText,
    pub current_password: SecretText,
    pub new_password: SecretText,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrowserAutoLoginInput {
    pub overleaf_session: SecretText,
    pub cookie_expiry: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserAutoLoginResult {
    pub url: String,
    pub profile_dir: PathBuf,
    pub debug_port: u16,
    pub process_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieCapture {
    pub name: String,
    pub value: SecretText,
    pub expiration_date: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginResult {
    pub email: Option<String>,
    pub user_id: Option<String>,
    pub cookies: Vec<CookieCapture>,
    pub trial_expiry: Option<i64>,
    pub subscription_status: Option<String>,
    pub subscription_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitTokenResult {
    pub token: Option<SecretText>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitTokenPageState {
    pub visible_token: Option<SecretText>,
    pub expires_at: Option<i64>,
    pub has_existing_masked_token: bool,
    pub can_generate_token: bool,
    pub can_add_another_token: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordChangeResult {
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationResult {
    pub email: String,
    pub trial_days: u32,
    pub subscription_succeeded: bool,
    pub cookies: Vec<CookieCapture>,
    pub git_token: Option<GitTokenResult>,
    pub trial_expiry: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserAutomationError {
    Navigation { message: String },
    ElementNotFound { selector: String },
    Timeout { operation: String },
    InvalidCredentials,
    RobotVerificationBlocked,
    RegisteredEmail { email: String },
    EmailCodeRequired { email: String },
    ChallengeRequired { challenge: String },
    Canceled,
    ExternalDriver { message: String },
    Other { message: String },
}

pub type BrowserAutomationResult<T> = Result<T, BrowserAutomationError>;

#[async_trait::async_trait]
pub trait BrowserAutomation {
    async fn login_with_credentials(
        &self,
        input: CredentialsLoginInput,
    ) -> BrowserAutomationResult<LoginResult>;

    async fn register_and_subscribe(
        &self,
        input: RegistrationInput,
    ) -> BrowserAutomationResult<RegistrationResult>;

    async fn extract_git_token_with_cookie(
        &self,
        input: GitTokenInput,
    ) -> BrowserAutomationResult<GitTokenResult>;

    async fn read_git_token_state_with_cookie(
        &self,
        _input: GitTokenInput,
    ) -> BrowserAutomationResult<GitTokenPageState> {
        Err(BrowserAutomationError::ExternalDriver {
            message: "Git token page state is not implemented for this automation backend"
                .to_string(),
        })
    }

    async fn change_password_with_cookie(
        &self,
        input: PasswordChangeInput,
    ) -> BrowserAutomationResult<PasswordChangeResult>;

    async fn open_user_window_with_cookie(
        &self,
        _input: BrowserAutoLoginInput,
    ) -> BrowserAutomationResult<BrowserAutoLoginResult> {
        Err(BrowserAutomationError::ExternalDriver {
            message: "browser auto-login window is not implemented for this automation backend"
                .to_string(),
        })
    }
}

impl SecretText {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretText(<redacted>)")
    }
}

impl fmt::Display for BrowserAutomationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Navigation { message }
            | Self::ExternalDriver { message }
            | Self::Other { message } => f.write_str(message),
            Self::ElementNotFound { selector } => {
                write!(f, "browser element not found: {selector}")
            }
            Self::Timeout { operation } => write!(f, "browser operation timed out: {operation}"),
            Self::InvalidCredentials => f.write_str("invalid email or password"),
            Self::RobotVerificationBlocked => {
                f.write_str("robot verification could not be initialized")
            }
            Self::RegisteredEmail { email } => write!(f, "email already registered: {email}"),
            Self::EmailCodeRequired { email } => {
                write!(f, "email verification code required: {email}")
            }
            Self::ChallengeRequired { challenge } => {
                write!(f, "manual challenge required: {challenge}")
            }
            Self::Canceled => f.write_str("browser automation canceled"),
        }
    }
}

impl std::error::Error for BrowserAutomationError {}

pub fn cookies_to_map(cookies: &[CookieCapture]) -> BTreeMap<String, String> {
    cookies
        .iter()
        .map(|cookie| (cookie.name.clone(), cookie.value.expose().to_string()))
        .collect()
}

pub fn overleaf_session_cookie(
    value: impl Into<String>,
    expiration_date: Option<i64>,
) -> CookieCapture {
    CookieCapture {
        name: crate::cdp::OVERLEAF_SESSION_COOKIE_NAME.to_string(),
        value: SecretText::new(value),
        expiration_date,
    }
}
