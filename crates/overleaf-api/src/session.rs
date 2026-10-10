use std::collections::BTreeMap;
use std::fmt;

use crate::project_api::{classify_project_api_status, ProjectApiStatusKind};
use crate::{
    body_preview, classify_trial_eligibility, extract_csrf_token, extract_user_json,
    format_cookie_header, header_map_from_strings, meta_boolean, parse_git_token_page_state,
    parse_subscription_status, parse_trial_plan_availability, GitTokenPageState, SubscriptionState,
    SubscriptionStatus, TrialEligibility, TrialEligibilityStatus, TrialPlanAvailability,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPageRequest {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPageResponse {
    pub status: u16,
    pub body: String,
    pub effective_url: Option<String>,
}

impl SessionPageResponse {
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            effective_url: None,
        }
    }

    pub fn with_effective_url(mut self, effective_url: impl Into<String>) -> Self {
        self.effective_url = Some(effective_url.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSessionRequest {
    pub url: String,
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverleafUserInfo {
    pub email: Option<String>,
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHttpError {
    pub status: u16,
    pub kind: ProjectApiStatusKind,
    pub body_preview: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTransportErrorKind {
    RequestFailed,
    InvalidHeader,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTransportError {
    pub kind: SessionTransportErrorKind,
    message: String,
}

impl SessionTransportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            kind: SessionTransportErrorKind::RequestFailed,
            message: message.into(),
        }
    }

    fn invalid_header(message: impl Into<String>) -> Self {
        Self {
            kind: SessionTransportErrorKind::InvalidHeader,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    Http(SessionHttpError),
    Transport(SessionTransportError),
    MissingCsrfToken,
    MissingUserInfo,
    LoginVerificationRequired,
}

impl fmt::Display for SessionHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "协议请求返回状态码 {}", self.status)
    }
}

impl std::error::Error for SessionHttpError {}

impl fmt::Display for SessionTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SessionTransportError {}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(err) => write!(f, "{err}"),
            Self::Transport(err) => write!(f, "{err}"),
            Self::MissingCsrfToken => f.write_str("missing csrf token"),
            Self::MissingUserInfo => f.write_str("missing user info"),
            Self::LoginVerificationRequired => f.write_str("登录接口不允许跳过验证"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<SessionHttpError> for SessionError {
    fn from(value: SessionHttpError) -> Self {
        Self::Http(value)
    }
}

impl From<SessionTransportError> for SessionError {
    fn from(value: SessionTransportError) -> Self {
        Self::Transport(value)
    }
}

pub type SessionResult<T> = Result<T, SessionError>;

#[async_trait::async_trait]
pub trait SessionTransport {
    async fn get(
        &self,
        request: &SessionPageRequest,
    ) -> Result<SessionPageResponse, SessionTransportError>;
}

#[derive(Debug, Clone)]
pub struct ReqwestSessionTransport {
    client: reqwest::Client,
    cookies: BTreeMap<String, String>,
    session_expiry: Option<i64>,
}

impl ReqwestSessionTransport {
    pub fn new(cookies: BTreeMap<String, String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            cookies,
            session_expiry: None,
        }
    }

    pub fn cookies(&self) -> &BTreeMap<String, String> {
        &self.cookies
    }

    pub fn session_expiry(&self) -> Option<i64> {
        self.session_expiry
    }

    fn capture_response_cookies(&mut self, response: &reqwest::Response) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        for value in response.headers().get_all(reqwest::header::SET_COOKIE) {
            let Ok(value) = value.to_str() else { continue };
            let Ok(cookie) = cookie::Cookie::parse(value) else {
                continue;
            };
            let expiry = cookie
                .max_age()
                .map(|age| now.saturating_add(age.whole_seconds()))
                .or_else(|| {
                    cookie
                        .expires_datetime()
                        .map(|value| value.unix_timestamp())
                });
            if cookie.name() == crate::OVERLEAF_SESSION_COOKIE {
                self.session_expiry = expiry;
            }
            if cookie.value().is_empty() || expiry.is_some_and(|expiry| expiry <= now) {
                self.cookies.remove(cookie.name());
            } else {
                self.cookies
                    .insert(cookie.name().to_string(), cookie.value().to_string());
            }
        }
    }

    pub async fn fetch_git_token_state(&self) -> SessionResult<GitTokenPageState> {
        let response = self
            .get(&SessionPageRequest {
                path: "/oauth/personal-access-tokens".to_string(),
            })
            .await?;
        ensure_session_success(response.status, &response.body)?;
        let tokens: Vec<serde_json::Value> = serde_json::from_str(&response.body)
            .map_err(|error| SessionTransportError::new(error.to_string()))?;
        let expiry = tokens
            .first()
            .filter(|_| tokens.len() == 1)
            .and_then(|item| item.get("accessTokenExpiresAt")?.as_str())
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.timestamp());
        Ok(GitTokenPageState {
            visible_token: None,
            expiry,
            has_existing_masked_token: !tokens.is_empty(),
            can_generate_token: tokens.is_empty(),
            can_add_another_token: !tokens.is_empty(),
        })
    }

    async fn post_session_json(
        &self,
        path: &str,
        referer: &str,
        body: Option<serde_json::Value>,
    ) -> SessionResult<reqwest::Response> {
        let csrf = OverleafSessionClient::new(self.clone())
            .fetch_csrf_token()
            .await?;
        let request = self
            .client
            .post(format!("https://www.overleaf.com{path}"))
            .timeout(std::time::Duration::from_secs(20))
            .header("cookie", format_cookie_header(&self.cookies))
            .header("x-csrf-token", csrf)
            .header("origin", "https://www.overleaf.com")
            .header("referer", format!("https://www.overleaf.com{referer}"))
            .header("content-type", "application/json");
        let request = if let Some(body) = body {
            request.json(&body)
        } else {
            request
        };
        let response = request
            .send()
            .await
            .map_err(|error| SessionTransportError::new(error.to_string()))?;
        let status = response.status();
        if status.is_server_error()
            || status == reqwest::StatusCode::REQUEST_TIMEOUT
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            return Err(SessionHttpError {
                status: status.as_u16(),
                kind: classify_project_api_status(status.as_u16()),
                body_preview: None,
            }
            .into());
        }
        Ok(response)
    }

    pub async fn create_git_token(&self) -> SessionResult<Option<(String, Option<i64>)>> {
        let response = self
            .post_session_json("/oauth/personal-access-tokens", "/user/settings", None)
            .await?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|error| SessionTransportError::new(error.to_string()))?;
        let Some(token) = value
            .get("accessToken")
            .and_then(serde_json::Value::as_str)
            .filter(|token| token.starts_with("olp_") && token.len() > 8)
        else {
            return Err(SessionTransportError::new("Git token response was not confirmed").into());
        };
        let token = token.to_string();
        let response = self
            .get(&SessionPageRequest {
                path: "/oauth/personal-access-tokens".to_string(),
            })
            .await;
        let expiry = response.ok().and_then(|response| {
            serde_json::from_str::<Vec<serde_json::Value>>(&response.body)
                .ok()?
                .into_iter()
                .find(|item| {
                    item.get("accessTokenPartial")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|partial| token.starts_with(partial))
                })?
                .get("accessTokenExpiresAt")?
                .as_str()
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.timestamp())
        });
        Ok(Some((token, expiry)))
    }

    pub async fn change_password(&self, current: &str, new: &str) -> SessionResult<bool> {
        let response = self
            .post_session_json(
                "/user/password/update",
                "/user/settings",
                Some(serde_json::json!({
                    "currentPassword": current,
                    "newPassword1": new,
                    "newPassword2": new,
                })),
            )
            .await?;
        if !response.status().is_success() {
            return Ok(false);
        }
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|error| SessionTransportError::new(error.to_string()))?;
        match value
            .pointer("/message/type")
            .and_then(serde_json::Value::as_str)
        {
            Some("success") => Ok(true),
            Some("error") => Ok(false),
            _ => {
                Err(SessionTransportError::new("password change response was not confirmed").into())
            }
        }
    }

    pub async fn try_login_with_credentials(
        email: &str,
        password: &str,
    ) -> SessionResult<Option<(Self, OverleafUserInfo)>> {
        let mut transport = Self::new(BTreeMap::new());
        let client = transport.client.clone();
        let login_page = client
            .get("https://www.overleaf.com/login")
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .map_err(|error| SessionTransportError::new(error.to_string()))?;
        transport.capture_response_cookies(&login_page);
        if !login_page.status().is_success() {
            return Ok(None);
        }
        let page = login_page
            .text()
            .await
            .map_err(|error| SessionTransportError::new(error.to_string()))?;
        let Some(csrf) = extract_csrf_token(&page) else {
            return Ok(None);
        };

        let cookie_header = format_cookie_header(&transport.cookies);
        let can_skip = client
            .post("https://www.overleaf.com/login/can-skip-captcha")
            .timeout(std::time::Duration::from_secs(20))
            .header("x-csrf-token", &csrf)
            .header("cookie", &cookie_header)
            .header("origin", "https://www.overleaf.com")
            .header("referer", "https://www.overleaf.com/login")
            .json(&serde_json::json!({ "email": email }))
            .send()
            .await
            .map_err(|error| SessionTransportError::new(error.to_string()))?;
        transport.capture_response_cookies(&can_skip);
        if !can_skip.status().is_success() {
            return Ok(None);
        }
        let can_skip = can_skip
            .json::<bool>()
            .await
            .map_err(|error| SessionTransportError::new(error.to_string()))?;
        if !can_skip {
            return Err(SessionError::LoginVerificationRequired);
        }

        let login = client
            .post("https://www.overleaf.com/login")
            .timeout(std::time::Duration::from_secs(20))
            .header("x-csrf-token", &csrf)
            .header("cookie", format_cookie_header(&transport.cookies))
            .header("origin", "https://www.overleaf.com")
            .header("referer", "https://www.overleaf.com/login")
            .json(&serde_json::json!({
                "_csrf": csrf,
                "email": email,
                "password": password,
                "g-recaptcha-response": "",
            }))
            .send()
            .await
            .map_err(|error| SessionTransportError::new(error.to_string()))?;
        transport.capture_response_cookies(&login);
        if !login.status().is_success() {
            return Ok(None);
        }
        let body = login
            .json::<serde_json::Value>()
            .await
            .map_err(|error| SessionTransportError::new(error.to_string()))?;
        if body.get("redir").and_then(serde_json::Value::as_str) != Some("/project")
            || !transport
                .cookies
                .contains_key(crate::OVERLEAF_SESSION_COOKIE)
        {
            return Ok(None);
        }

        let user = OverleafSessionClient::new(transport.clone())
            .fetch_user_info()
            .await?;
        if user
            .email
            .as_deref()
            .is_none_or(|actual| !actual.eq_ignore_ascii_case(email))
        {
            return Ok(None);
        }
        Ok(Some((transport, user)))
    }

    pub fn prepare_request(
        &self,
        request: &SessionPageRequest,
    ) -> SessionResult<PreparedSessionRequest> {
        let mut headers = BTreeMap::from([
            (
                "accept".to_string(),
                "application/json, text/plain, */*".to_string(),
            ),
            (
                "user-agent".to_string(),
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36".to_string(),
            ),
        ]);

        if !self.cookies.is_empty() {
            headers.insert("cookie".to_string(), format_cookie_header(&self.cookies));
        }

        Ok(PreparedSessionRequest {
            url: format!("https://www.overleaf.com{}", request.path),
            headers,
        })
    }
}

#[async_trait::async_trait]
impl SessionTransport for ReqwestSessionTransport {
    async fn get(
        &self,
        request: &SessionPageRequest,
    ) -> Result<SessionPageResponse, SessionTransportError> {
        let prepared = self.prepare_request(request).map_err(|err| match err {
            SessionError::Transport(err) => err,
            other => SessionTransportError::new(other.to_string()),
        })?;

        let response = self
            .client
            .get(&prepared.url)
            .timeout(std::time::Duration::from_secs(20))
            .headers(
                header_map_from_strings(&prepared.headers)
                    .map_err(SessionTransportError::invalid_header)?,
            )
            .send()
            .await
            .map_err(|err| SessionTransportError::new(err.to_string()))?;
        let status = response.status().as_u16();
        let effective_url = response.url().as_str().to_string();
        let body = response
            .text()
            .await
            .map_err(|err| SessionTransportError::new(err.to_string()))?;

        Ok(SessionPageResponse::new(status, body).with_effective_url(effective_url))
    }
}

#[derive(Debug, Clone)]
pub struct OverleafSessionClient<T> {
    transport: T,
}

impl<T> OverleafSessionClient<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn into_inner(self) -> T {
        self.transport
    }
}

impl<T: SessionTransport> OverleafSessionClient<T> {
    pub async fn fetch_project_page(&self) -> SessionResult<String> {
        let response = self.get(project_page_request()).await?;
        ensure_session_success(response.status, &response.body)?;
        Ok(response.body)
    }

    pub async fn fetch_csrf_token(&self) -> SessionResult<String> {
        let page = self.fetch_project_page().await?;
        extract_csrf_token(&page).ok_or(SessionError::MissingCsrfToken)
    }

    pub async fn fetch_user_info(&self) -> SessionResult<OverleafUserInfo> {
        let page = self.fetch_project_page().await?;
        parse_user_info_from_page(&page).ok_or(SessionError::MissingUserInfo)
    }

    pub async fn fetch_subscription_status(&self) -> SessionResult<SubscriptionStatus> {
        self.fetch_subscription_status_with_page()
            .await
            .map(|(status, _)| status)
    }

    pub async fn fetch_trial_expiry(&self) -> SessionResult<Option<i64>> {
        let (subscription, page) = self.fetch_subscription_status_with_page().await?;
        Ok(trial_status(&subscription, &page).trial_expiry)
    }

    async fn fetch_subscription_status_with_page(
        &self,
    ) -> SessionResult<(SubscriptionStatus, String)> {
        let subscription = self.get(subscription_page_request()).await?;
        ensure_session_success(subscription.status, &subscription.body)?;
        let status = parse_subscription_status(&subscription.body, None);
        if status.trial_expiry.is_some() || status.state == super::SubscriptionState::Free {
            return Ok((status, subscription.body));
        }

        let project_body = match self.get(project_page_request()).await {
            Ok(response)
                if classify_project_api_status(response.status)
                    == ProjectApiStatusKind::Success =>
            {
                Some(response.body)
            }
            _ => None,
        };

        let status = parse_subscription_status(&subscription.body, project_body.as_deref());
        Ok((status, subscription.body))
    }

    pub async fn fetch_trial_eligibility(
        &self,
        now_unix: i64,
        trial_days: u32,
    ) -> SessionResult<TrialEligibilityStatus> {
        let (subscription, subscription_page) = self.fetch_subscription_status_with_page().await?;
        // 卡片有效期包含付费账期，试用续接只看真正的试用结束日。
        let trial = trial_status(&subscription, &subscription_page);
        let without_plan =
            classify_trial_eligibility(&trial, TrialPlanAvailability::Unknown, now_unix);
        if without_plan != TrialEligibility::Unknown {
            let mut subscription = subscription;
            if without_plan == TrialEligibility::ActiveTrial {
                subscription.trial_expiry = trial.trial_expiry;
            }
            return Ok(TrialEligibilityStatus {
                eligibility: without_plan,
                subscription,
                plan_availability: TrialPlanAvailability::Unknown,
            });
        }

        if meta_boolean(&subscription_page, "ol-hasSubscription") == Some(true) {
            return Ok(TrialEligibilityStatus {
                eligibility: TrialEligibility::Ineligible,
                subscription,
                plan_availability: TrialPlanAvailability::ExistingSubscription,
            });
        }
        if !matches!(
            subscription.state,
            SubscriptionState::Free | SubscriptionState::Unknown
        ) {
            return Ok(TrialEligibilityStatus {
                eligibility: TrialEligibility::Unknown,
                subscription,
                plan_availability: TrialPlanAvailability::Unknown,
            });
        }

        let Some(plan_request) = trial_plan_page_request(trial_days) else {
            return Ok(TrialEligibilityStatus {
                eligibility: TrialEligibility::Unknown,
                subscription,
                plan_availability: TrialPlanAvailability::Unknown,
            });
        };
        let response = self.get(plan_request).await?;
        ensure_session_success(response.status, &response.body)?;
        let plan_availability = parse_trial_plan_availability(
            response.status,
            response.effective_url.as_deref(),
            &response.body,
            trial_days,
        );
        Ok(TrialEligibilityStatus {
            eligibility: classify_trial_eligibility(&subscription, plan_availability, now_unix),
            subscription,
            plan_availability,
        })
    }

    pub async fn fetch_git_token_page_state(
        &self,
        now_unix: i64,
    ) -> SessionResult<GitTokenPageState> {
        let settings = self.get(settings_page_request()).await?;
        ensure_session_success(settings.status, &settings.body)?;
        Ok(parse_git_token_page_state(&settings.body, now_unix))
    }

    async fn get(&self, request: SessionPageRequest) -> SessionResult<SessionPageResponse> {
        Ok(self.transport.get(&request).await?)
    }
}

fn trial_status(subscription: &SubscriptionStatus, page: &str) -> SubscriptionStatus {
    if subscription.state == SubscriptionState::Free {
        subscription.clone()
    } else {
        super::parse_subscription_meta_status(page, false).unwrap_or_else(|| subscription.clone())
    }
}

pub fn project_page_request() -> SessionPageRequest {
    SessionPageRequest {
        path: "/project".to_string(),
    }
}

pub fn subscription_page_request() -> SessionPageRequest {
    SessionPageRequest {
        path: "/user/subscription".to_string(),
    }
}

pub fn trial_plan_page_request(trial_days: u32) -> Option<SessionPageRequest> {
    let plan_code = overleaf_core::registration_trial_plan_code(trial_days)?;
    Some(SessionPageRequest {
        path: format!("/user/subscription/new?planCode={plan_code}&currency=USD"),
    })
}

pub fn settings_page_request() -> SessionPageRequest {
    SessionPageRequest {
        path: "/user/settings".to_string(),
    }
}

pub fn parse_user_info_from_page(page_html: &str) -> Option<OverleafUserInfo> {
    let value = extract_user_json(page_html)?;
    let info = OverleafUserInfo {
        email: string_field(&value, "email"),
        user_id: string_field(&value, "_id").or_else(|| string_field(&value, "id")),
    };

    (info.email.is_some() || info.user_id.is_some()).then_some(info)
}

pub fn ensure_session_success(status: u16, body: &str) -> Result<(), SessionHttpError> {
    let kind = classify_project_api_status(status);
    if kind == ProjectApiStatusKind::Success {
        Ok(())
    } else {
        Err(SessionHttpError {
            status,
            kind,
            body_preview: body_preview(body),
        })
    }
}

fn string_field(value: &serde_json::Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
