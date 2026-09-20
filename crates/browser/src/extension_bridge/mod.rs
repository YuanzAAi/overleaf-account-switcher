pub mod ws;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const OVERLEAF_BRIDGE_WS_URL: &str = "ws://localhost:9876";
pub const DEFAULT_OVERLEAF_COOKIE_NAME: &str = "overleaf_session2";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CookiePayload {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub secure: Option<bool>,
    #[serde(default, rename = "httpOnly")]
    pub http_only: Option<bool>,
    #[serde(default, rename = "sameSite")]
    pub same_site: Option<String>,
    #[serde(default, rename = "expirationDate")]
    pub expiration_date: Option<f64>,
}

impl CookiePayload {
    pub fn overleaf_session(value: impl Into<String>, expiration_date: Option<f64>) -> Self {
        Self {
            name: DEFAULT_OVERLEAF_COOKIE_NAME.to_string(),
            value: value.into(),
            domain: Some(".overleaf.com".to_string()),
            path: Some("/".to_string()),
            secure: Some(true),
            http_only: Some(true),
            same_site: Some("no_restriction".to_string()),
            expiration_date,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ExtensionCommand {
    GetCookie {
        name: String,
        request_id: String,
    },
    SetCookie {
        cookie: CookiePayload,
        request_id: String,
    },
    RefreshTabs {
        request_id: String,
    },
    GetCookieExpiry {
        request_id: String,
    },
    Ping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExtensionCommandKind {
    GetCookie,
    SetCookie,
    RefreshTabs,
    GetCookieExpiry,
}

impl ExtensionCommand {
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::GetCookie { request_id, .. }
            | Self::SetCookie { request_id, .. }
            | Self::RefreshTabs { request_id }
            | Self::GetCookieExpiry { request_id } => Some(request_id),
            Self::Ping => None,
        }
    }

    pub fn kind(&self) -> Option<ExtensionCommandKind> {
        match self {
            Self::GetCookie { .. } => Some(ExtensionCommandKind::GetCookie),
            Self::SetCookie { .. } => Some(ExtensionCommandKind::SetCookie),
            Self::RefreshTabs { .. } => Some(ExtensionCommandKind::RefreshTabs),
            Self::GetCookieExpiry { .. } => Some(ExtensionCommandKind::GetCookieExpiry),
            Self::Ping => None,
        }
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ExtensionEvent {
    Handshake {
        session_id: String,
        timestamp: i64,
    },
    GetCookie {
        #[serde(flatten)]
        response: ExtensionResponse,
    },
    SetCookie {
        #[serde(flatten)]
        response: ExtensionResponse,
    },
    RefreshTabs {
        #[serde(flatten)]
        response: ExtensionResponse,
    },
    GetCookieExpiry {
        #[serde(flatten)]
        response: ExtensionResponse,
    },
}

impl ExtensionEvent {
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Handshake { .. } => None,
            Self::GetCookie { response }
            | Self::SetCookie { response }
            | Self::RefreshTabs { response }
            | Self::GetCookieExpiry { response } => Some(response.request_id.as_str()),
        }
    }

    pub fn kind(&self) -> Option<ExtensionCommandKind> {
        match self {
            Self::Handshake { .. } => None,
            Self::GetCookie { .. } => Some(ExtensionCommandKind::GetCookie),
            Self::SetCookie { .. } => Some(ExtensionCommandKind::SetCookie),
            Self::RefreshTabs { .. } => Some(ExtensionCommandKind::RefreshTabs),
            Self::GetCookieExpiry { .. } => Some(ExtensionCommandKind::GetCookieExpiry),
        }
    }

    pub fn response(&self) -> Option<&ExtensionResponse> {
        match self {
            Self::Handshake { .. } => None,
            Self::GetCookie { response }
            | Self::SetCookie { response }
            | Self::RefreshTabs { response }
            | Self::GetCookieExpiry { response } => Some(response),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ExtensionResponse {
    pub request_id: String,
    pub success: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub cookie: Option<CookiePayload>,
    #[serde(default)]
    pub refreshed: Option<u32>,
    #[serde(default, rename = "expirationDate")]
    pub expiration_date: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingExtensionRequest {
    pub request_id: String,
    pub kind: ExtensionCommandKind,
    pub command: ExtensionCommand,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionBridgeSession {
    session_id: Option<String>,
    connected_at: Option<i64>,
    next_request: u64,
    pending: BTreeMap<String, PendingExtensionRequest>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExtensionBridgeEvent {
    Handshake {
        session_id: String,
        timestamp: i64,
    },
    ResponseMatched {
        request: PendingExtensionRequest,
        response: Box<ExtensionResponse>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionBridgeError {
    CommandDoesNotExpectResponse,
    DuplicateRequestId {
        request_id: String,
    },
    UnexpectedResponse {
        request_id: String,
    },
    ActionMismatch {
        request_id: String,
        expected: ExtensionCommandKind,
        actual: ExtensionCommandKind,
    },
}

impl ExtensionBridgeSession {
    pub fn new() -> Self {
        Self {
            session_id: None,
            connected_at: None,
            next_request: 1,
            pending: BTreeMap::new(),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.session_id.is_some()
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn connected_at(&self) -> Option<i64> {
        self.connected_at
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn pending_request_ids(&self) -> Vec<&str> {
        self.pending.keys().map(String::as_str).collect()
    }

    pub fn reset_connection(&mut self) {
        self.session_id = None;
        self.connected_at = None;
        self.pending.clear();
    }

    pub fn discard_pending_request(&mut self, request_id: &str) -> bool {
        self.pending.remove(request_id).is_some()
    }

    pub fn next_request_id(&mut self) -> String {
        let request_id = format!("req-{}", self.next_request);
        self.next_request += 1;
        request_id
    }

    pub fn issue_get_cookie(&mut self, name: impl Into<String>) -> ExtensionCommand {
        let request_id = self.next_request_id();
        self.queue_command(get_cookie_command(request_id, name))
            .expect("generated request id is unique")
    }

    pub fn issue_set_cookie(&mut self, cookie: CookiePayload) -> ExtensionCommand {
        let request_id = self.next_request_id();
        self.queue_command(set_cookie_command(request_id, cookie))
            .expect("generated request id is unique")
    }

    pub fn issue_refresh_tabs(&mut self) -> ExtensionCommand {
        let request_id = self.next_request_id();
        self.queue_command(refresh_tabs_command(request_id))
            .expect("generated request id is unique")
    }

    pub fn issue_get_cookie_expiry(&mut self) -> ExtensionCommand {
        let request_id = self.next_request_id();
        self.queue_command(get_cookie_expiry_command(request_id))
            .expect("generated request id is unique")
    }

    pub fn queue_command(
        &mut self,
        command: ExtensionCommand,
    ) -> Result<ExtensionCommand, ExtensionBridgeError> {
        let Some(request_id) = command.request_id().map(ToOwned::to_owned) else {
            return Err(ExtensionBridgeError::CommandDoesNotExpectResponse);
        };
        let kind = command
            .kind()
            .expect("commands with request_id always have a kind");

        if self.pending.contains_key(&request_id) {
            return Err(ExtensionBridgeError::DuplicateRequestId { request_id });
        }

        self.pending.insert(
            request_id.clone(),
            PendingExtensionRequest {
                request_id,
                kind,
                command: command.clone(),
            },
        );
        Ok(command)
    }

    pub fn handle_event(
        &mut self,
        event: ExtensionEvent,
    ) -> Result<ExtensionBridgeEvent, ExtensionBridgeError> {
        match event {
            ExtensionEvent::Handshake {
                session_id,
                timestamp,
            } => {
                self.session_id = Some(session_id.clone());
                self.connected_at = Some(timestamp);
                Ok(ExtensionBridgeEvent::Handshake {
                    session_id,
                    timestamp,
                })
            }
            event => {
                let request_id = event
                    .request_id()
                    .expect("non-handshake extension events have request ids")
                    .to_string();
                let actual = event
                    .kind()
                    .expect("non-handshake extension events have kinds");
                let Some(pending) = self.pending.get(&request_id) else {
                    return Err(ExtensionBridgeError::UnexpectedResponse { request_id });
                };
                if pending.kind != actual {
                    return Err(ExtensionBridgeError::ActionMismatch {
                        request_id,
                        expected: pending.kind,
                        actual,
                    });
                }

                let pending = self
                    .pending
                    .remove(&request_id)
                    .expect("pending request exists after successful match");
                Ok(ExtensionBridgeEvent::ResponseMatched {
                    request: pending,
                    response: Box::new(
                        event
                            .response()
                            .expect("matched response event has response body")
                            .clone(),
                    ),
                })
            }
        }
    }
}

impl Default for ExtensionBridgeSession {
    fn default() -> Self {
        Self::new()
    }
}

pub fn get_cookie_command(
    request_id: impl Into<String>,
    name: impl Into<String>,
) -> ExtensionCommand {
    ExtensionCommand::GetCookie {
        name: name.into(),
        request_id: request_id.into(),
    }
}

pub fn set_cookie_command(
    request_id: impl Into<String>,
    cookie: CookiePayload,
) -> ExtensionCommand {
    ExtensionCommand::SetCookie {
        cookie,
        request_id: request_id.into(),
    }
}

pub fn refresh_tabs_command(request_id: impl Into<String>) -> ExtensionCommand {
    ExtensionCommand::RefreshTabs {
        request_id: request_id.into(),
    }
}

pub fn get_cookie_expiry_command(request_id: impl Into<String>) -> ExtensionCommand {
    ExtensionCommand::GetCookieExpiry {
        request_id: request_id.into(),
    }
}

pub fn parse_extension_event(input: &str) -> serde_json::Result<ExtensionEvent> {
    serde_json::from_str(input)
}
