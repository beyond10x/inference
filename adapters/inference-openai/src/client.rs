//! The Realtime WebSocket session.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_audio::AudioFrame;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use zeroize::Zeroizing;

use crate::audio::encode_pcm16_24k;
use crate::config::{Endpoint, SessionConfig, WireFormat};
use crate::credential::{CredentialError, CredentialSource};
use crate::error::{ConnectError, RealtimeError};
use crate::event::ServerEvent;

pub(crate) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// One open Realtime session over WebSocket.
///
/// Every send method writes one client event and returns once it is written; answers arrive as
/// server events through [`RealtimeClient::next_event`]. The client holds no credential after the
/// handshake.
pub struct RealtimeClient {
    socket: Socket,
    read_timeout: std::time::Duration,
    input_format: WireFormat,
    session: Value,
    pending: VecDeque<ServerEvent>,
}

impl fmt::Debug for RealtimeClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RealtimeClient")
            .field("read_timeout", &self.read_timeout)
            .field("input_format", &self.input_format)
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

impl RealtimeClient {
    /// Opens a session: resolves the secret `credential` names through `source`, connects to
    /// `<endpoint.url>?model=<endpoint.model>` with it as the bearer token, waits for
    /// `session.created`, sends `session` as `session.update` and waits for `session.updated`.
    /// Any other server event that arrives during setup is kept and returned first by
    /// [`RealtimeClient::next_event`].
    ///
    /// The credential is resolved before any connection is made, is sent only in the opening
    /// handshake and is never part of an error.
    ///
    /// # Errors
    /// [`RealtimeError::Credential`] when the source refuses or the credential is not a valid
    /// header value; [`RealtimeError::InvalidEndpoint`], [`RealtimeError::Connect`];
    /// [`RealtimeError::Server`] when the server answers setup with `error`; and the errors of
    /// [`RealtimeClient::next_event`].
    pub async fn connect(
        endpoint: &Endpoint,
        source: &dyn CredentialSource,
        credential: &str,
        session: &SessionConfig,
    ) -> Result<Self, RealtimeError> {
        let url = request_url(endpoint)?;
        let socket = open_socket(&url, source, credential).await?;

        let mut client = Self {
            socket,
            read_timeout: endpoint.read_timeout,
            input_format: session.input_format,
            session: Value::Null,
            pending: VecDeque::new(),
        };
        client
            .await_setup(|event| matches!(event, ServerEvent::SessionCreated { .. }))
            .await?;
        client.update_session(session).await?;
        if let ServerEvent::SessionUpdated { session } = client
            .await_setup(|event| matches!(event, ServerEvent::SessionUpdated { .. }))
            .await?
        {
            client.session = session;
        }
        Ok(client)
    }

    /// Reads until an event `wanted` accepts, keeping every other event for
    /// [`RealtimeClient::next_event`]; an `error` event fails setup.
    async fn await_setup(
        &mut self,
        wanted: impl Fn(&ServerEvent) -> bool,
    ) -> Result<ServerEvent, RealtimeError> {
        loop {
            let event = self.read_event().await?;
            if wanted(&event) {
                return Ok(event);
            }
            if let ServerEvent::Error(error) = event {
                return Err(RealtimeError::Server(error));
            }
            self.pending.push_back(event);
        }
    }

    /// The session as the server confirmed it in `session.updated` during
    /// [`RealtimeClient::connect`].
    pub fn session(&self) -> &Value {
        &self.session
    }

    /// Sends `session.update` with `session`. Later appends follow its input format; the
    /// server's `session.updated` arrives through [`RealtimeClient::next_event`].
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn update_session(&mut self, session: &SessionConfig) -> Result<(), RealtimeError> {
        self.send(json!({"type": "session.update", "session": session.to_session_json()}))
            .await?;
        self.input_format = session.input_format;
        Ok(())
    }

    /// Sends `input_audio_buffer.append` with `frame` as PCM16 at 24 kHz, resampled and mixed
    /// down by [`encode_pcm16_24k`].
    ///
    /// # Errors
    /// [`RealtimeError::Audio`] when the session's input format is not PCM16 (G.711 goes through
    /// [`RealtimeClient::append_encoded`]) or the frame is malformed;
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn append_audio(&mut self, frame: &AudioFrame) -> Result<(), RealtimeError> {
        if self.input_format != WireFormat::Pcm16 {
            return Err(RealtimeError::Audio(format!(
                "the session takes {:?} input; send it encoded with append_encoded",
                self.input_format
            )));
        }
        let pcm = encode_pcm16_24k(frame)?;
        self.append_encoded(&pcm).await
    }

    /// Sends `input_audio_buffer.append` with `audio` as it is: bytes already in the session's
    /// input format, such as G.711 mu-law at 8 kHz from a telephone line.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn append_encoded(&mut self, audio: &[u8]) -> Result<(), RealtimeError> {
        self.send(json!({"type": "input_audio_buffer.append", "audio": STANDARD.encode(audio)}))
            .await
    }

    /// Sends `input_audio_buffer.commit`: the buffered input becomes a user item. With turn
    /// detection off a response still needs [`RealtimeClient::create_response`].
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn commit_audio(&mut self) -> Result<(), RealtimeError> {
        self.send(json!({"type": "input_audio_buffer.commit"}))
            .await
    }

    /// Sends `input_audio_buffer.clear`.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn clear_audio(&mut self) -> Result<(), RealtimeError> {
        self.send(json!({"type": "input_audio_buffer.clear"})).await
    }

    /// Sends `conversation.item.create` with a user message of one `input_text` part.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn send_text(&mut self, text: &str) -> Result<(), RealtimeError> {
        self.send(json!({
            "type": "conversation.item.create",
            "item": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": text}],
            },
        }))
        .await
    }

    /// Sends `conversation.item.create` with a `function_call_output` item answering `call_id`.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn send_function_output(
        &mut self,
        call_id: &str,
        output: &str,
    ) -> Result<(), RealtimeError> {
        self.send(json!({
            "type": "conversation.item.create",
            "item": {"type": "function_call_output", "call_id": call_id, "output": output},
        }))
        .await
    }

    /// Sends `response.create`.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn create_response(&mut self) -> Result<(), RealtimeError> {
        self.send(json!({"type": "response.create"})).await
    }

    /// Sends `response.cancel`, for `response_id` or, when `None`, the response in progress. The
    /// server answers with `response.done` of status `cancelled`.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn cancel_response(
        &mut self,
        response_id: Option<&str>,
    ) -> Result<(), RealtimeError> {
        let mut event = json!({"type": "response.cancel"});
        if let Some(response_id) = response_id {
            event["response_id"] = json!(response_id);
        }
        self.send(event).await
    }

    /// Sends `conversation.item.truncate`: the assistant audio of `item_id` is cut at
    /// `audio_end_ms`, the position the listener heard, and its transcript with it.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn truncate(
        &mut self,
        item_id: &str,
        content_index: u32,
        audio_end_ms: u32,
    ) -> Result<(), RealtimeError> {
        self.send(json!({
            "type": "conversation.item.truncate",
            "item_id": item_id,
            "content_index": content_index,
            "audio_end_ms": audio_end_ms,
        }))
        .await
    }

    /// The next server event, events kept during setup first.
    ///
    /// # Errors
    /// [`RealtimeError::Stalled`] when no event arrives within the endpoint's read timeout, pings
    /// and other frames that are not events counted in it;
    /// [`RealtimeError::Closed`] when the server closed the connection;
    /// [`RealtimeError::Transport`] when the read fails; [`RealtimeError::Protocol`] for a
    /// message that is not a Realtime event.
    pub async fn next_event(&mut self) -> Result<ServerEvent, RealtimeError> {
        match self.pending.pop_front() {
            Some(event) => Ok(event),
            None => self.read_event().await,
        }
    }

    /// Sends a close frame and waits for the server's.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the close handshake fails;
    /// [`RealtimeError::Stalled`] when the server does not close within the read timeout.
    pub async fn close(mut self) -> Result<(), RealtimeError> {
        close_socket(&mut self.socket, self.read_timeout).await
    }

    async fn read_event(&mut self) -> Result<ServerEvent, RealtimeError> {
        let text = read_text(&mut self.socket, self.read_timeout).await?;
        ServerEvent::parse(&text)
    }

    async fn send(&mut self, event: Value) -> Result<(), RealtimeError> {
        send_json(&mut self.socket, &event).await
    }
}

/// Resolves the secret `credential` names through `source` and opens a WebSocket to `url` with
/// it as the bearer token. The credential is resolved before any connection is made, is sent only
/// in the opening handshake, marked sensitive, and is never part of an error.
pub(crate) async fn open_socket(
    url: &str,
    source: &dyn CredentialSource,
    credential: &str,
) -> Result<Socket, RealtimeError> {
    let resolved = source
        .resolve(credential)
        .await
        .map_err(RealtimeError::Credential)?;
    let mut authorization = Zeroizing::new(Vec::with_capacity(7 + resolved.expose().len()));
    authorization.extend_from_slice(b"Bearer ");
    authorization.extend_from_slice(resolved.expose());
    drop(resolved);
    let mut header = HeaderValue::from_bytes(&authorization)
        .map_err(|_| RealtimeError::Credential(CredentialError::Invalid))?;
    drop(authorization);
    header.set_sensitive(true);

    let mut request = url
        .into_client_request()
        .map_err(|error| RealtimeError::InvalidEndpoint(error.to_string()))?;
    request.headers_mut().insert("authorization", header);
    let connector = if url.starts_with("wss://") {
        Some(Connector::Rustls(tls_config()?))
    } else {
        None
    };
    let (socket, _) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, true, connector)
            .await
            .map_err(|error| RealtimeError::Connect(handshake_error(&error)))?;
    Ok(socket)
}

/// The text of the next text message, waiting at most `read_timeout`; pings and other frames
/// that are not messages count in the wait.
pub(crate) async fn read_text(
    socket: &mut Socket,
    read_timeout: std::time::Duration,
) -> Result<String, RealtimeError> {
    let deadline = tokio::time::Instant::now() + read_timeout;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| RealtimeError::Stalled(read_timeout))?;
        let message = match message {
            None => {
                return Err(RealtimeError::Closed {
                    code: None,
                    reason: String::new(),
                });
            }
            Some(message) => {
                message.map_err(|error| RealtimeError::Transport(error.to_string()))?
            }
        };
        match message {
            Message::Text(text) => return Ok(text.as_str().to_owned()),
            Message::Close(frame) => {
                return Err(RealtimeError::Closed {
                    code: frame.as_ref().map(|frame| u16::from(frame.code)),
                    reason: frame
                        .map(|frame| frame.reason.as_str().to_owned())
                        .unwrap_or_default(),
                });
            }
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            Message::Binary(data) => {
                return Err(RealtimeError::Protocol(format!(
                    "a binary message of {} bytes; the API sends text",
                    data.len()
                )));
            }
        }
    }
}

/// Writes `event` as one text message.
pub(crate) async fn send_json(socket: &mut Socket, event: &Value) -> Result<(), RealtimeError> {
    socket
        .send(Message::text(event.to_string()))
        .await
        .map_err(|error| RealtimeError::Transport(error.to_string()))
}

/// Sends a close frame on `socket` and waits at most `read_timeout` for the server's.
pub(crate) async fn close_socket(
    socket: &mut Socket,
    read_timeout: std::time::Duration,
) -> Result<(), RealtimeError> {
    use tokio_tungstenite::tungstenite::Error;
    socket
        .close(None)
        .await
        .map_err(|error| RealtimeError::Transport(error.to_string()))?;
    let deadline = tokio::time::Instant::now() + read_timeout;
    loop {
        match tokio::time::timeout_at(deadline, socket.next()).await {
            Err(_) => return Err(RealtimeError::Stalled(read_timeout)),
            Ok(None | Some(Err(Error::ConnectionClosed | Error::AlreadyClosed))) => return Ok(()),
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(error))) => return Err(RealtimeError::Transport(error.to_string())),
        }
    }
}

/// Refuses a model id outside the characters model ids use.
pub(crate) fn check_model(model: &str) -> Result<(), RealtimeError> {
    if model.is_empty()
        || !model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_'))
    {
        return Err(RealtimeError::InvalidEndpoint(format!(
            "model id `{model}` is empty or holds a character outside A-Z, a-z, 0-9, '-', '.', '_'"
        )));
    }
    Ok(())
}

/// `<url>?model=<model>`, refusing a model id outside the characters model ids use.
fn request_url(endpoint: &Endpoint) -> Result<String, RealtimeError> {
    let model = &endpoint.model;
    check_model(model)?;
    if endpoint.url.contains('?') {
        return Err(RealtimeError::InvalidEndpoint(
            "the endpoint URL already has a query; the client adds `model`".to_owned(),
        ));
    }
    Ok(format!("{}?model={model}", endpoint.url))
}

/// TLS with the ring provider and the webpki roots, independent of any process-wide default
/// provider.
fn tls_config() -> Result<Arc<rustls::ClientConfig>, RealtimeError> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| {
        RealtimeError::Connect(ConnectError {
            status: None,
            detail: error.to_string(),
        })
    })?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The handshake failure of `error`, with the HTTP status when the server answered the upgrade
/// with one. The description is tungstenite's, which names the status and never the request.
fn handshake_error(error: &tokio_tungstenite::tungstenite::Error) -> ConnectError {
    let status = match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => Some(response.status().as_u16()),
        _ => None,
    };
    ConnectError {
        status,
        detail: error.to_string(),
    }
}
