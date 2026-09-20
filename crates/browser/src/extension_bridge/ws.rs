use std::collections::BTreeMap;
use std::fmt;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::{
    handshake::server::{Callback, ErrorResponse, Request, Response},
    Message,
};
use tokio_tungstenite::{accept_hdr_async, WebSocketStream};

use crate::extension_bridge::{
    parse_extension_event, ExtensionBridgeError, ExtensionBridgeEvent, ExtensionBridgeSession,
    ExtensionCommand, ExtensionResponse,
};

pub const ENV_EXTENSION_BRIDGE_HOST: &str = "OVERLEAF_SWITCHER_EXTENSION_BRIDGE_HOST";
pub const ENV_EXTENSION_BRIDGE_PORT: &str = "OVERLEAF_SWITCHER_EXTENSION_BRIDGE_PORT";
pub const DEFAULT_EXTENSION_BRIDGE_BIND_HOST: &str = "127.0.0.1";
pub const DEFAULT_EXTENSION_BRIDGE_PORT: u16 = 9876;
pub const DEFAULT_EXTENSION_BRIDGE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct ExtensionBridgeWebSocketServer {
    state: Arc<ExtensionBridgeWebSocketState>,
    local_addr: SocketAddr,
    response_timeout: Duration,
}

#[derive(Debug)]
struct ExtensionBridgeWebSocketState {
    clients: Mutex<BTreeMap<u64, mpsc::UnboundedSender<Message>>>,
    session: Mutex<ExtensionBridgeSession>,
    pending_responses: Mutex<BTreeMap<String, PendingWebSocketResponse>>,
    next_client_id: AtomicU64,
    command_lock: Mutex<()>,
}

#[derive(Debug)]
struct PendingWebSocketResponse {
    client_id: u64,
    sender: oneshot::Sender<ExtensionResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionBridgeWebSocketError {
    Bind { message: String },
    Runtime { message: String },
    Accept { message: String },
    NotConnected,
    Serialize { message: String },
    Send { message: String },
    ResponseTimeout { request_id: String },
    ResponseDropped { request_id: String },
    Protocol { message: String },
}

impl ExtensionBridgeWebSocketServer {
    pub fn from_environment() -> Result<Self, ExtensionBridgeWebSocketError> {
        Self::spawn(bind_addr_from_environment())
    }

    pub fn spawn(bind_addr: SocketAddr) -> Result<Self, ExtensionBridgeWebSocketError> {
        let listener = StdTcpListener::bind(bind_addr).map_err(|error| {
            ExtensionBridgeWebSocketError::Bind {
                message: format!("bind extension bridge websocket {bind_addr}: {error}"),
            }
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|error| ExtensionBridgeWebSocketError::Bind {
                message: format!("set extension bridge listener nonblocking: {error}"),
            })?;
        Self::spawn_with_listener(listener)
    }

    pub fn spawn_with_listener(
        listener: StdTcpListener,
    ) -> Result<Self, ExtensionBridgeWebSocketError> {
        let local_addr =
            listener
                .local_addr()
                .map_err(|error| ExtensionBridgeWebSocketError::Bind {
                    message: format!("read extension bridge listener address: {error}"),
                })?;
        let state = Arc::new(ExtensionBridgeWebSocketState {
            clients: Mutex::new(BTreeMap::new()),
            session: Mutex::new(ExtensionBridgeSession::new()),
            pending_responses: Mutex::new(BTreeMap::new()),
            next_client_id: AtomicU64::new(1),
            command_lock: Mutex::new(()),
        });

        let thread_state = Arc::clone(&state);
        std::thread::Builder::new()
            .name("overleaf-extension-bridge".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        eprintln!("[extension-bridge] failed to create runtime: {error}");
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener = match TcpListener::from_std(listener) {
                        Ok(listener) => listener,
                        Err(error) => {
                            eprintln!(
                                "[extension-bridge] failed to adopt listener {local_addr}: {error}"
                            );
                            return;
                        }
                    };
                    accept_loop(listener, thread_state).await;
                });
            })
            .map_err(|error| ExtensionBridgeWebSocketError::Runtime {
                message: format!("spawn extension bridge thread: {error}"),
            })?;

        Ok(Self {
            state,
            local_addr,
            response_timeout: DEFAULT_EXTENSION_BRIDGE_RESPONSE_TIMEOUT,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn connected_clients(&self) -> usize {
        self.state.clients.lock().await.len()
    }

    pub async fn is_connected(&self) -> bool {
        self.connected_clients().await > 0
    }

    pub async fn execute_command(
        &self,
        command: ExtensionCommand,
    ) -> Result<ExtensionResponse, ExtensionBridgeWebSocketError> {
        let _command_guard = self.state.command_lock.lock().await;
        let request_id = command
            .request_id()
            .ok_or_else(|| ExtensionBridgeWebSocketError::Protocol {
                message: "extension command does not expect a response".to_string(),
            })?
            .to_string();
        let payload =
            command
                .to_json()
                .map_err(|error| ExtensionBridgeWebSocketError::Serialize {
                    message: format!("serialize extension command: {error}"),
                })?;
        let (client_id, sender) = self.first_client().await?;
        let (response_tx, response_rx) = oneshot::channel();

        {
            let mut session = self.state.session.lock().await;
            session
                .queue_command(command)
                .map_err(extension_bridge_protocol_error)?;
        }
        self.state.pending_responses.lock().await.insert(
            request_id.clone(),
            PendingWebSocketResponse {
                client_id,
                sender: response_tx,
            },
        );

        if sender.send(Message::Text(payload.into())).is_err() {
            self.remove_pending_request(&request_id).await;
            return Err(ExtensionBridgeWebSocketError::Send {
                message: "extension websocket client disconnected before command send".to_string(),
            });
        }

        match tokio::time::timeout(self.response_timeout, response_rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(ExtensionBridgeWebSocketError::ResponseDropped { request_id }),
            Err(_) => {
                self.remove_pending_request(&request_id).await;
                Err(ExtensionBridgeWebSocketError::ResponseTimeout { request_id })
            }
        }
    }

    async fn first_client(
        &self,
    ) -> Result<(u64, mpsc::UnboundedSender<Message>), ExtensionBridgeWebSocketError> {
        self.state
            .clients
            .lock()
            .await
            .iter()
            .next()
            .map(|(client_id, sender)| (*client_id, sender.clone()))
            .ok_or(ExtensionBridgeWebSocketError::NotConnected)
    }

    async fn remove_pending_request(&self, request_id: &str) {
        let _ = self.state.pending_responses.lock().await.remove(request_id);
        self.state.session.lock().await.reset_connection();
    }
}

impl fmt::Display for ExtensionBridgeWebSocketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind { message }
            | Self::Runtime { message }
            | Self::Accept { message }
            | Self::Serialize { message }
            | Self::Send { message }
            | Self::Protocol { message } => write!(formatter, "{message}"),
            Self::NotConnected => write!(formatter, "extension bridge has no connected clients"),
            Self::ResponseTimeout { request_id } => {
                write!(
                    formatter,
                    "extension bridge response timeout for {request_id}"
                )
            }
            Self::ResponseDropped { request_id } => {
                write!(
                    formatter,
                    "extension bridge response channel dropped for {request_id}"
                )
            }
        }
    }
}

impl std::error::Error for ExtensionBridgeWebSocketError {}

struct ExtensionOriginGuard;

impl Callback for ExtensionOriginGuard {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        if extension_origin_allowed(request) {
            Ok(response)
        } else {
            let mut denied = ErrorResponse::new(Some("extension origin required".into()));
            *denied.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
            Err(denied)
        }
    }
}

async fn accept_loop(listener: TcpListener, state: Arc<ExtensionBridgeWebSocketState>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let handshake = accept_hdr_async(stream, ExtensionOriginGuard);
                    match tokio::time::timeout(Duration::from_secs(10), handshake).await {
                        Ok(Ok(websocket)) => {
                            let client_id = state.next_client_id.fetch_add(1, Ordering::SeqCst);
                            handle_client(client_id, websocket, state).await;
                        }
                        Ok(Err(error)) => {
                            eprintln!("[extension-bridge] websocket accept failed: {error}")
                        }
                        Err(_) => eprintln!("[extension-bridge] websocket handshake timed out"),
                    }
                });
            }
            Err(error) => {
                eprintln!("[extension-bridge] tcp accept failed: {error}");
                break;
            }
        }
    }
}

fn extension_origin_allowed(request: &Request) -> bool {
    // Native clients have no Origin; browser clients must come from an extension.
    let Some(origin) = request.headers().get("origin") else {
        return true;
    };
    origin
        .to_str()
        .ok()
        .and_then(|value| value.strip_prefix("chrome-extension://"))
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte)))
}

async fn handle_client<S>(
    client_id: u64,
    websocket: WebSocketStream<S>,
    state: Arc<ExtensionBridgeWebSocketState>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut writer, mut reader) = websocket.split();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<Message>();
    state
        .clients
        .lock()
        .await
        .insert(client_id, outbound_tx.clone());

    let write_task = tokio::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            if writer.send(message).await.is_err() {
                break;
            }
        }
    });

    while let Some(message) = reader.next().await {
        let Ok(message) = message else {
            break;
        };
        match text_from_message(message) {
            Ok(Some(text)) => handle_client_text(client_id, &text, &outbound_tx, &state).await,
            Ok(None) => {}
            Err(error) => {
                eprintln!("[extension-bridge] invalid client message: {error}");
            }
        }
    }

    let no_clients_left = {
        let mut clients = state.clients.lock().await;
        clients.remove(&client_id);
        clients.is_empty()
    };
    if no_clients_left {
        state.session.lock().await.reset_connection();
        fail_all_pending_responses(&state, "extension websocket client disconnected").await;
    } else {
        fail_pending_responses_for_client(
            &state,
            client_id,
            "extension websocket client disconnected",
        )
        .await;
    }
    write_task.abort();
}

async fn handle_client_text(
    client_id: u64,
    text: &str,
    outbound: &mpsc::UnboundedSender<Message>,
    state: &Arc<ExtensionBridgeWebSocketState>,
) {
    if json_action(text).as_deref() == Some("ping") {
        return;
    }

    let event = match parse_extension_event(text) {
        Ok(event) => event,
        Err(error) => {
            let message = format!("parse extension event failed: {error}");
            if let Some(request_id) = json_request_id(text) {
                fail_pending_response_from_client(state, client_id, &request_id, message.clone())
                    .await;
            }
            eprintln!("[extension-bridge] {message}");
            return;
        }
    };

    if let Some(request_id) = event.request_id() {
        if !pending_response_belongs_to_client(state, client_id, request_id).await {
            let message =
                format!("extension response {request_id} came from a non-target websocket client");
            let _ = outbound.send(Message::Text(handshake_ack(false, Some(message)).into()));
            return;
        }
    }

    let matched = {
        let mut session = state.session.lock().await;
        session.handle_event(event)
    };

    match matched {
        Ok(ExtensionBridgeEvent::Handshake { .. }) => {
            let _ = outbound.send(Message::Text(handshake_ack(true, None).into()));
        }
        Ok(ExtensionBridgeEvent::ResponseMatched { response, .. }) => {
            let request_id = response.request_id.clone();
            if let Some(pending) = state.pending_responses.lock().await.remove(&request_id) {
                let _ = pending.sender.send(*response);
            }
        }
        Err(error) => {
            let message = extension_bridge_protocol_error(error.clone()).to_string();
            if let Some(request_id) = bridge_error_request_id(&error) {
                fail_pending_response(state, request_id, message.clone()).await;
            }
            let _ = outbound.send(Message::Text(handshake_ack(false, Some(message)).into()));
        }
    }
}

async fn fail_pending_response(
    state: &Arc<ExtensionBridgeWebSocketState>,
    request_id: &str,
    message: String,
) {
    let _ = state
        .session
        .lock()
        .await
        .discard_pending_request(request_id);
    if let Some(pending) = state.pending_responses.lock().await.remove(request_id) {
        let _ = pending
            .sender
            .send(extension_failure_response(request_id, message));
    }
}

async fn fail_pending_response_from_client(
    state: &Arc<ExtensionBridgeWebSocketState>,
    client_id: u64,
    request_id: &str,
    message: String,
) {
    if pending_response_belongs_to_client(state, client_id, request_id).await {
        fail_pending_response(state, request_id, message).await;
    }
}

async fn pending_response_belongs_to_client(
    state: &Arc<ExtensionBridgeWebSocketState>,
    client_id: u64,
    request_id: &str,
) -> bool {
    state
        .pending_responses
        .lock()
        .await
        .get(request_id)
        .map(|pending| pending.client_id == client_id)
        .unwrap_or(true)
}

async fn fail_all_pending_responses(state: &Arc<ExtensionBridgeWebSocketState>, message: &str) {
    let pending = std::mem::take(&mut *state.pending_responses.lock().await);
    for (request_id, pending) in pending {
        let _ = pending
            .sender
            .send(extension_failure_response(&request_id, message.to_string()));
    }
}

async fn fail_pending_responses_for_client(
    state: &Arc<ExtensionBridgeWebSocketState>,
    client_id: u64,
    message: &str,
) {
    let failed = {
        let mut pending = state.pending_responses.lock().await;
        let request_ids = pending
            .iter()
            .filter(|(_, response)| response.client_id == client_id)
            .map(|(request_id, _)| request_id.clone())
            .collect::<Vec<_>>();
        request_ids
            .into_iter()
            .filter_map(|request_id| {
                pending
                    .remove(&request_id)
                    .map(|response| (request_id, response))
            })
            .collect::<Vec<_>>()
    };

    if failed.is_empty() {
        return;
    }

    let mut session = state.session.lock().await;
    for (request_id, pending) in failed {
        let _ = session.discard_pending_request(&request_id);
        let _ = pending
            .sender
            .send(extension_failure_response(&request_id, message.to_string()));
    }
}

fn extension_failure_response(request_id: &str, message: String) -> ExtensionResponse {
    ExtensionResponse {
        request_id: request_id.to_string(),
        success: false,
        message: Some(message),
        cookie: None,
        refreshed: None,
        expiration_date: None,
    }
}

fn text_from_message(message: Message) -> Result<Option<String>, ExtensionBridgeWebSocketError> {
    match message {
        Message::Text(text) => Ok(Some(text.to_string())),
        Message::Binary(bytes) => String::from_utf8(bytes.to_vec())
            .map(Some)
            .map_err(|error| ExtensionBridgeWebSocketError::Protocol {
                message: format!("decode extension websocket message: {error}"),
            }),
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Ok(None),
        Message::Close(_) => Ok(None),
    }
}

fn handshake_ack(success: bool, message: Option<String>) -> String {
    #[derive(Serialize)]
    struct HandshakeAck<'a> {
        action: &'static str,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<&'a str>,
    }

    serde_json::to_string(&HandshakeAck {
        action: "handshake_ack",
        success,
        message: message.as_deref(),
    })
    .expect("handshake ack is serializable")
}

fn json_action(text: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("action")
                .and_then(|action| action.as_str())
                .map(str::to_string)
        })
}

fn json_request_id(text: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("request_id")
                .and_then(|request_id| request_id.as_str())
                .map(str::to_string)
        })
}

fn bridge_error_request_id(error: &ExtensionBridgeError) -> Option<&str> {
    match error {
        ExtensionBridgeError::UnexpectedResponse { request_id }
        | ExtensionBridgeError::ActionMismatch { request_id, .. } => Some(request_id),
        ExtensionBridgeError::CommandDoesNotExpectResponse
        | ExtensionBridgeError::DuplicateRequestId { .. } => None,
    }
}

fn extension_bridge_protocol_error(error: ExtensionBridgeError) -> ExtensionBridgeWebSocketError {
    ExtensionBridgeWebSocketError::Protocol {
        message: match error {
            ExtensionBridgeError::CommandDoesNotExpectResponse => {
                "extension command does not expect a response".to_string()
            }
            ExtensionBridgeError::DuplicateRequestId { request_id } => {
                format!("duplicate extension request id: {request_id}")
            }
            ExtensionBridgeError::UnexpectedResponse { request_id } => {
                format!("unexpected extension response id: {request_id}")
            }
            ExtensionBridgeError::ActionMismatch {
                request_id,
                expected,
                actual,
            } => format!(
                "extension action mismatch for {request_id}: expected {expected:?}, got {actual:?}"
            ),
        },
    }
}

pub fn bind_addr_from_environment() -> SocketAddr {
    let host = std::env::var(ENV_EXTENSION_BRIDGE_HOST)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_EXTENSION_BRIDGE_BIND_HOST.to_string());
    let port = std::env::var(ENV_EXTENSION_BRIDGE_PORT)
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_EXTENSION_BRIDGE_PORT);
    format!("{host}:{port}")
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], DEFAULT_EXTENSION_BRIDGE_PORT)))
}
