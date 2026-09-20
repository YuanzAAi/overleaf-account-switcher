use std::fmt;

use serde_json::Value;

use crate::cdp::{
    browser_close_command, dom_text_command, execution_context_id_from_response,
    frames_from_tree_response, input_insert_text_command, network_get_overleaf_cookies_command,
    network_set_overleaf_session_cookie_command, overleaf_session_cookie_from_response,
    page_create_isolated_world_command, page_get_frame_tree_command, page_navigate_command,
    runtime_evaluate_command, runtime_evaluate_in_context_command, runtime_evaluate_string,
    CdpCommand, CdpCookie, CdpFrame, CdpResponse,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdpTransportError {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdpSessionError {
    Transport {
        message: String,
    },
    CommandFailed {
        code: i64,
        message: String,
    },
    UnexpectedResponseId {
        expected: u64,
        actual: Option<u64>,
    },
    RuntimeEvaluationFailed {
        message: String,
    },
    UnexpectedRuntimeResult {
        value_type: Option<String>,
        subtype: Option<String>,
    },
    MissingRuntimeValue,
    MissingOverleafSessionCookie,
    MissingFrameTree,
    MissingExecutionContextId,
    CookieSetRejected,
}

#[async_trait::async_trait]
pub trait CdpTransport: Send + Sync {
    async fn send(&self, command: &CdpCommand) -> Result<CdpResponse, CdpTransportError>;
}

#[derive(Debug, Clone)]
pub struct CdpSession<T> {
    transport: T,
    next_id: u64,
}

impl CdpTransportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CdpTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CDP transport error: {}", self.message)
    }
}

impl std::error::Error for CdpTransportError {}

impl fmt::Display for CdpSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { message } => write!(f, "CDP transport failed: {message}"),
            Self::CommandFailed { code, message } => {
                write!(f, "CDP command failed ({code}): {message}")
            }
            Self::UnexpectedResponseId { expected, actual } => {
                write!(
                    f,
                    "CDP response id mismatch: expected {expected}, got {actual:?}"
                )
            }
            Self::RuntimeEvaluationFailed { message } => {
                write!(f, "CDP Runtime.evaluate failed: {message}")
            }
            Self::UnexpectedRuntimeResult {
                value_type,
                subtype,
            } => {
                write!(
                    f,
                    "CDP Runtime.evaluate returned no string value (type={}, subtype={})",
                    value_type.as_deref().unwrap_or("unknown"),
                    subtype.as_deref().unwrap_or("none")
                )
            }
            Self::MissingRuntimeValue => write!(f, "CDP runtime response did not contain a value"),
            Self::MissingOverleafSessionCookie => {
                write!(f, "CDP response did not contain overleaf_session2 cookie")
            }
            Self::MissingFrameTree => write!(f, "CDP response did not contain a frame tree"),
            Self::MissingExecutionContextId => {
                write!(f, "CDP response did not contain an execution context id")
            }
            Self::CookieSetRejected => write!(f, "CDP rejected Network.setCookie"),
        }
    }
}

impl std::error::Error for CdpSessionError {}

impl CdpSessionError {
    pub fn is_transient_navigation_context_error(&self) -> bool {
        let message = match self {
            Self::CommandFailed { message, .. } | Self::RuntimeEvaluationFailed { message } => {
                message.as_str()
            }
            _ => return false,
        }
        .to_ascii_lowercase();

        message.contains("execution context was destroyed")
            || message.contains("cannot find default execution context")
            || message.contains("cannot find context with specified id")
            || message.contains("inspected target navigated or closed")
    }
}

impl From<CdpTransportError> for CdpSessionError {
    fn from(error: CdpTransportError) -> Self {
        Self::Transport {
            message: error.message,
        }
    }
}

impl<T> CdpSession<T>
where
    T: CdpTransport,
{
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_id: 1,
        }
    }

    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    pub async fn navigate(&mut self, url: impl Into<String>) -> Result<(), CdpSessionError> {
        let id = self.allocate_id();
        self.send(page_navigate_command(id, url)).await?;
        Ok(())
    }

    pub async fn evaluate_string(
        &mut self,
        expression: impl Into<String>,
        await_promise: bool,
    ) -> Result<String, CdpSessionError> {
        let id = self.allocate_id();
        let response = self
            .send(runtime_evaluate_command(id, expression, await_promise))
            .await?;

        runtime_evaluate_result(&response)
    }

    pub async fn evaluate_string_in_context(
        &mut self,
        context_id: i64,
        expression: impl Into<String>,
        await_promise: bool,
    ) -> Result<String, CdpSessionError> {
        let id = self.allocate_id();
        let response = self
            .send(runtime_evaluate_in_context_command(
                id,
                expression,
                await_promise,
                context_id,
            ))
            .await?;

        runtime_evaluate_result(&response)
    }

    pub async fn read_body_text(&mut self) -> Result<String, CdpSessionError> {
        let id = self.allocate_id();
        let response = self.send(dom_text_command(id)).await?;

        runtime_evaluate_result(&response)
    }

    pub async fn insert_text(&mut self, text: impl Into<String>) -> Result<(), CdpSessionError> {
        let id = self.allocate_id();
        self.send(input_insert_text_command(id, text)).await?;
        Ok(())
    }

    pub async fn get_overleaf_session_cookie(&mut self) -> Result<CdpCookie, CdpSessionError> {
        let id = self.allocate_id();
        let response = self.send(network_get_overleaf_cookies_command(id)).await?;

        overleaf_session_cookie_from_response(&response)
            .ok_or(CdpSessionError::MissingOverleafSessionCookie)
    }

    pub async fn set_overleaf_session_cookie(
        &mut self,
        value: impl Into<String>,
        expires: Option<f64>,
    ) -> Result<(), CdpSessionError> {
        let id = self.allocate_id();
        let response = self
            .send(network_set_overleaf_session_cookie_command(
                id, value, expires,
            ))
            .await?;

        if response
            .result
            .as_ref()
            .and_then(|result| result.get("success"))
            .and_then(Value::as_bool)
            == Some(false)
        {
            return Err(CdpSessionError::CookieSetRejected);
        }

        Ok(())
    }

    pub async fn frames(&mut self) -> Result<Vec<CdpFrame>, CdpSessionError> {
        let id = self.allocate_id();
        let response = self.send(page_get_frame_tree_command(id)).await?;
        let frames = frames_from_tree_response(&response);
        if frames.is_empty() {
            return Err(CdpSessionError::MissingFrameTree);
        }
        Ok(frames)
    }

    pub async fn create_isolated_world(
        &mut self,
        frame_id: impl Into<String>,
        world_name: impl Into<String>,
        grant_universal_access: bool,
    ) -> Result<i64, CdpSessionError> {
        let id = self.allocate_id();
        let response = self
            .send(page_create_isolated_world_command(
                id,
                frame_id,
                world_name,
                grant_universal_access,
            ))
            .await?;

        execution_context_id_from_response(&response)
            .ok_or(CdpSessionError::MissingExecutionContextId)
    }

    pub async fn close_browser(&mut self) -> Result<(), CdpSessionError> {
        let id = self.allocate_id();
        self.send(browser_close_command(id)).await?;
        Ok(())
    }

    async fn send(&self, command: CdpCommand) -> Result<CdpResponse, CdpSessionError> {
        let response = self.transport.send(&command).await?;
        validate_response(&command, response)
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }
}

fn validate_response(
    command: &CdpCommand,
    response: CdpResponse,
) -> Result<CdpResponse, CdpSessionError> {
    if response.id != Some(command.id) {
        return Err(CdpSessionError::UnexpectedResponseId {
            expected: command.id,
            actual: response.id,
        });
    }

    if let Some(error) = response.error {
        return Err(CdpSessionError::CommandFailed {
            code: error.code,
            message: error.message,
        });
    }

    Ok(response)
}

fn runtime_evaluate_result(response: &CdpResponse) -> Result<String, CdpSessionError> {
    if let Some(value) = runtime_evaluate_string(response) {
        return Ok(value);
    }

    if let Some(details) = response
        .result
        .as_ref()
        .and_then(|result| result.get("exceptionDetails"))
    {
        let description = details
            .get("exception")
            .and_then(|exception| exception.get("description"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let text = details
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("JavaScript exception");
        let class_name = details
            .get("exception")
            .and_then(|exception| exception.get("className"))
            .and_then(serde_json::Value::as_str);
        let raw_message = format!("{text} {description}").to_ascii_lowercase();
        let message = if raw_message.contains("execution context was destroyed")
            || raw_message.contains("cannot find context with specified id")
            || raw_message.contains("inspected target navigated or closed")
        {
            "execution context was destroyed during navigation".to_string()
        } else {
            let line = details
                .get("lineNumber")
                .and_then(serde_json::Value::as_u64)
                .map(|value| value.saturating_add(1));
            let column = details
                .get("columnNumber")
                .and_then(serde_json::Value::as_u64)
                .map(|value| value.saturating_add(1));
            match (class_name, line, column) {
                (Some(class_name), Some(line), Some(column)) => {
                    format!("{text} {class_name} at line {line}, column {column}")
                }
                (Some(class_name), _, _) => format!("{text} {class_name}"),
                _ => text.to_string(),
            }
        };

        return Err(CdpSessionError::RuntimeEvaluationFailed { message });
    }

    let remote_result = response
        .result
        .as_ref()
        .and_then(|result| result.get("result"));
    if let Some(remote_result) = remote_result {
        return Err(CdpSessionError::UnexpectedRuntimeResult {
            value_type: remote_result
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            subtype: remote_result
                .get("subtype")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        });
    }

    Err(CdpSessionError::MissingRuntimeValue)
}
