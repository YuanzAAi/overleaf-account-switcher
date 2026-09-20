pub mod login;
pub mod session;
pub mod stripe;
pub mod ws;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const CDP_VERSION_ENDPOINT_PATH: &str = "/json/version";
pub const CDP_LIST_ENDPOINT_PATH: &str = "/json/list";
pub const DEFAULT_OVERLEAF_COOKIE_URL: &str = "https://www.overleaf.com";
pub const OVERLEAF_SESSION_COOKIE_NAME: &str = "overleaf_session2";
pub const OVERLEAF_SESSION_COOKIE_DOMAIN: &str = ".overleaf.com";
pub const OVERLEAF_SESSION_COOKIE_PATH: &str = "/";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CdpCommand {
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CdpResponse {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<CdpError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CdpError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CdpCookie {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub expires: Option<f64>,
    #[serde(default)]
    pub secure: Option<bool>,
    #[serde(default, rename = "httpOnly")]
    pub http_only: Option<bool>,
    #[serde(default, rename = "sameSite")]
    pub same_site: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdpFrame {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdpParseError {
    InvalidJson(String),
    MissingField(&'static str),
}

impl CdpCommand {
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            id,
            method: method.into(),
            params,
        }
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

impl CdpCookie {
    pub fn overleaf_session(value: impl Into<String>, expires: Option<f64>) -> Self {
        Self {
            name: OVERLEAF_SESSION_COOKIE_NAME.to_string(),
            value: value.into(),
            domain: Some(OVERLEAF_SESSION_COOKIE_DOMAIN.to_string()),
            path: Some(OVERLEAF_SESSION_COOKIE_PATH.to_string()),
            expires,
            secure: Some(true),
            http_only: Some(true),
            same_site: Some("None".to_string()),
        }
    }
}

pub fn browser_close_command(id: u64) -> CdpCommand {
    CdpCommand::new(id, "Browser.close", None)
}

pub fn page_navigate_command(id: u64, url: impl Into<String>) -> CdpCommand {
    CdpCommand::new(id, "Page.navigate", Some(json!({ "url": url.into() })))
}

pub fn page_get_frame_tree_command(id: u64) -> CdpCommand {
    CdpCommand::new(id, "Page.getFrameTree", None)
}

pub fn input_insert_text_command(id: u64, text: impl Into<String>) -> CdpCommand {
    CdpCommand::new(id, "Input.insertText", Some(json!({ "text": text.into() })))
}

pub fn page_create_isolated_world_command(
    id: u64,
    frame_id: impl Into<String>,
    world_name: impl Into<String>,
    grant_universal_access: bool,
) -> CdpCommand {
    CdpCommand::new(
        id,
        "Page.createIsolatedWorld",
        Some(json!({
            "frameId": frame_id.into(),
            "worldName": world_name.into(),
            "grantUniveralAccess": grant_universal_access,
        })),
    )
}

pub fn runtime_evaluate_command(
    id: u64,
    expression: impl Into<String>,
    await_promise: bool,
) -> CdpCommand {
    CdpCommand::new(
        id,
        "Runtime.evaluate",
        Some(json!({
            "expression": expression.into(),
            "awaitPromise": await_promise,
            "returnByValue": true,
        })),
    )
}

pub fn runtime_evaluate_in_context_command(
    id: u64,
    expression: impl Into<String>,
    await_promise: bool,
    context_id: i64,
) -> CdpCommand {
    let mut command = runtime_evaluate_command(id, expression, await_promise);
    if let Some(params) = command.params.as_mut() {
        params["contextId"] = json!(context_id);
    }
    command
}

pub fn dom_text_command(id: u64) -> CdpCommand {
    runtime_evaluate_command(id, "document.body ? document.body.innerText : ''", false)
}

pub fn network_get_overleaf_cookies_command(id: u64) -> CdpCommand {
    network_get_cookies_command(id, [DEFAULT_OVERLEAF_COOKIE_URL])
}

pub fn network_get_cookies_command<I, S>(id: u64, urls: I) -> CdpCommand
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let urls: Vec<String> = urls
        .into_iter()
        .map(|url| url.as_ref().to_string())
        .collect();
    CdpCommand::new(id, "Network.getCookies", Some(json!({ "urls": urls })))
}

pub fn network_set_cookie_command(id: u64, cookie: &CdpCookie, url: Option<&str>) -> CdpCommand {
    let mut params = json!({
        "name": cookie.name,
        "value": cookie.value,
    });

    if let Some(url) = url {
        params["url"] = Value::String(url.to_string());
    }
    if let Some(domain) = cookie.domain.as_deref() {
        params["domain"] = Value::String(domain.to_string());
    }
    if let Some(path) = cookie.path.as_deref() {
        params["path"] = Value::String(path.to_string());
    }
    if let Some(expires) = cookie.expires {
        params["expires"] = json!(expires);
    }
    if let Some(secure) = cookie.secure {
        params["secure"] = Value::Bool(secure);
    }
    if let Some(http_only) = cookie.http_only {
        params["httpOnly"] = Value::Bool(http_only);
    }
    if let Some(same_site) = cookie.same_site.as_deref() {
        params["sameSite"] = Value::String(same_site.to_string());
    }

    CdpCommand::new(id, "Network.setCookie", Some(params))
}

pub fn network_set_overleaf_session_cookie_command(
    id: u64,
    value: impl Into<String>,
    expires: Option<f64>,
) -> CdpCommand {
    let cookie = CdpCookie::overleaf_session(value, expires);
    network_set_cookie_command(id, &cookie, Some(DEFAULT_OVERLEAF_COOKIE_URL))
}

pub fn parse_cdp_response(input: &str) -> Result<CdpResponse, CdpParseError> {
    serde_json::from_str(input).map_err(|error| CdpParseError::InvalidJson(error.to_string()))
}

pub fn parse_browser_websocket_url(input: &str) -> Result<String, CdpParseError> {
    let value: Value = serde_json::from_str(input)
        .map_err(|error| CdpParseError::InvalidJson(error.to_string()))?;
    value
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
        .ok_or(CdpParseError::MissingField("webSocketDebuggerUrl"))
}

pub fn parse_page_websocket_url(
    input: &str,
    preferred_url: Option<&str>,
) -> Result<String, CdpParseError> {
    let targets: Value = serde_json::from_str(input)
        .map_err(|error| CdpParseError::InvalidJson(error.to_string()))?;
    let targets = targets
        .as_array()
        .ok_or(CdpParseError::MissingField("page target array"))?;
    let preferred_url = preferred_url.map(str::trim).filter(|url| !url.is_empty());
    let mut first_page = None;
    let mut same_origin_page = None;

    for target in targets {
        if target.get("type").and_then(Value::as_str) != Some("page") {
            continue;
        }
        let Some(websocket_url) = target
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|url| !url.is_empty())
        else {
            continue;
        };
        let target_url = target
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();

        if preferred_url.is_some_and(|preferred| target_url.starts_with(preferred)) {
            return Ok(websocket_url.to_string());
        }
        if same_origin_page.is_none()
            && preferred_url.is_some_and(|preferred| same_http_origin(target_url, preferred))
        {
            same_origin_page = Some(websocket_url.to_string());
        }
        if first_page.is_none() {
            first_page = Some(websocket_url.to_string());
        }
    }

    if preferred_url.is_some() {
        same_origin_page.ok_or(CdpParseError::MissingField(
            "preferred page webSocketDebuggerUrl",
        ))
    } else {
        first_page.ok_or(CdpParseError::MissingField("page webSocketDebuggerUrl"))
    }
}

fn same_http_origin(left: &str, right: &str) -> bool {
    fn origin(value: &str) -> Option<&str> {
        let scheme_end = value.find("://")? + 3;
        let path_start = value[scheme_end..]
            .find('/')
            .map(|offset| scheme_end + offset)
            .unwrap_or(value.len());
        Some(&value[..path_start])
    }

    matches!((origin(left), origin(right)), (Some(left), Some(right)) if left.eq_ignore_ascii_case(right))
}

pub fn runtime_evaluate_string(response: &CdpResponse) -> Option<String> {
    response
        .result
        .as_ref()?
        .get("result")?
        .get("value")?
        .as_str()
        .map(str::to_string)
}

pub fn execution_context_id_from_response(response: &CdpResponse) -> Option<i64> {
    response
        .result
        .as_ref()?
        .get("executionContextId")?
        .as_i64()
}

pub fn frames_from_tree_response(response: &CdpResponse) -> Vec<CdpFrame> {
    let Some(frame_tree) = response
        .result
        .as_ref()
        .and_then(|result| result.get("frameTree"))
    else {
        return Vec::new();
    };

    let mut frames = Vec::new();
    collect_frames(frame_tree, None, &mut frames);
    frames
}

pub fn cookies_from_response(response: &CdpResponse) -> Vec<CdpCookie> {
    let Some(cookies) = response
        .result
        .as_ref()
        .and_then(|result| result.get("cookies"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    cookies
        .iter()
        .filter_map(|cookie| serde_json::from_value(cookie.clone()).ok())
        .collect()
}

pub fn overleaf_session_cookie_from_response(response: &CdpResponse) -> Option<CdpCookie> {
    cookies_from_response(response).into_iter().find(|cookie| {
        cookie.name == OVERLEAF_SESSION_COOKIE_NAME && !cookie.value.trim().is_empty()
    })
}

fn collect_frames(frame_tree: &Value, parent_id: Option<String>, frames: &mut Vec<CdpFrame>) {
    let Some(frame) = frame_tree.get("frame") else {
        return;
    };
    let Some(id) = frame.get("id").and_then(Value::as_str) else {
        return;
    };
    let parent_id = frame
        .get("parentId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(parent_id);

    frames.push(CdpFrame {
        id: id.to_string(),
        parent_id: parent_id.clone(),
        name: frame
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        url: frame
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    });

    if let Some(children) = frame_tree.get("childFrames").and_then(Value::as_array) {
        for child in children {
            collect_frames(child, Some(id.to_string()), frames);
        }
    }
}
