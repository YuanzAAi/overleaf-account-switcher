use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::cdp::{parse_cdp_response, CdpCommand, CdpParseError, CdpResponse};
use crate::cdp_session::{CdpTransport, CdpTransportError};

pub const DEFAULT_CDP_RESPONSE_SCAN_LIMIT: usize = 1024;

#[derive(Debug)]
pub struct CdpWebSocketTransport {
    stream: Mutex<WebSocketStream<MaybeTlsStream<TcpStream>>>,
}

impl CdpWebSocketTransport {
    pub async fn connect(websocket_url: impl AsRef<str>) -> Result<Self, CdpTransportError> {
        let (stream, _) = connect_async(websocket_url.as_ref())
            .await
            .map_err(|error| cdp_transport_error(format!("connect CDP websocket: {error}")))?;

        Ok(Self::from_stream(stream))
    }

    pub fn from_stream(stream: WebSocketStream<MaybeTlsStream<TcpStream>>) -> Self {
        Self {
            stream: Mutex::new(stream),
        }
    }
}

#[async_trait::async_trait]
impl CdpTransport for CdpWebSocketTransport {
    async fn send(&self, command: &CdpCommand) -> Result<CdpResponse, CdpTransportError> {
        let payload = command
            .to_json()
            .map_err(|error| cdp_transport_error(format!("serialize CDP command: {error}")))?;
        let mut stream = self.stream.lock().await;

        stream
            .send(Message::Text(payload.into()))
            .await
            .map_err(|error| cdp_transport_error(format!("send CDP command: {error}")))?;

        for _ in 0..DEFAULT_CDP_RESPONSE_SCAN_LIMIT {
            let Some(message) = stream.next().await else {
                return Err(cdp_transport_error(
                    "CDP websocket closed before command response",
                ));
            };
            let message = message
                .map_err(|error| cdp_transport_error(format!("read CDP response: {error}")))?;

            if let Some(response) = response_from_message(message)? {
                return Ok(response);
            }
        }

        Err(cdp_transport_error(format!(
            "CDP response scan limit exceeded for command id {}",
            command.id
        )))
    }
}

fn response_from_message(message: Message) -> Result<Option<CdpResponse>, CdpTransportError> {
    match message {
        Message::Text(text) => response_from_text(&text),
        Message::Binary(bytes) => {
            let text = String::from_utf8(bytes.to_vec()).map_err(|error| {
                cdp_transport_error(format!("decode binary CDP response: {error}"))
            })?;
            response_from_text(&text)
        }
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Ok(None),
        Message::Close(frame) => Err(cdp_transport_error(format!(
            "CDP websocket closed by peer: {frame:?}"
        ))),
    }
}

fn response_from_text(text: &str) -> Result<Option<CdpResponse>, CdpTransportError> {
    let response = parse_cdp_response(text).map_err(cdp_parse_error)?;

    if response.id.is_none() {
        return Ok(None);
    }

    Ok(Some(response))
}

fn cdp_parse_error(error: CdpParseError) -> CdpTransportError {
    match error {
        CdpParseError::InvalidJson(message) => {
            cdp_transport_error(format!("parse CDP response JSON: {message}"))
        }
        CdpParseError::MissingField(field) => {
            cdp_transport_error(format!("parse CDP response JSON: missing {field}"))
        }
    }
}

fn cdp_transport_error(message: impl Into<String>) -> CdpTransportError {
    CdpTransportError::new(message)
}
