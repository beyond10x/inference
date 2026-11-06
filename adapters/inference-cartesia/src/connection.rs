//! The WebSocket connection of one synthesis: the handshake with the key, bounded writes and
//! reads, and the close.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::credential::{CredentialError, CredentialSource};
use crate::error::{CartesiaError, ConnectError};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The most a cancel or an orderly close waits for the close frame to be written.
pub const CLOSE_BOUND: Duration = Duration::from_secs(1);

/// An open connection and when it last read a message.
pub(crate) struct Connection {
    socket: Socket,
    read_timeout: Duration,
    last_message: Instant,
}

impl Connection {
    /// Resolves the secret `credential` names through `source` and opens a WebSocket to `url`
    /// with it in the `X-API-Key` header, marked sensitive. The credential is resolved before
    /// any connection is made, is sent only in the handshake and is never part of an error.
    pub(crate) async fn open(
        url: &str,
        source: &dyn CredentialSource,
        credential: &str,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<Self, CartesiaError> {
        let resolved = source
            .resolve(credential)
            .await
            .map_err(CartesiaError::Credential)?;
        let mut header = HeaderValue::from_bytes(resolved.expose())
            .map_err(|_| CartesiaError::Credential(CredentialError::Invalid))?;
        drop(resolved);
        header.set_sensitive(true);

        let mut request = url.into_client_request().map_err(|error| {
            CartesiaError::Connect(ConnectError {
                status: None,
                detail: error.to_string(),
            })
        })?;
        request.headers_mut().insert("x-api-key", header);
        let connector = if url.starts_with("wss://") {
            Some(Connector::Rustls(tls_config()?))
        } else {
            None
        };
        let handshake =
            tokio_tungstenite::connect_async_tls_with_config(request, None, true, connector);
        let (socket, _) = tokio::time::timeout(connect_timeout, handshake)
            .await
            .map_err(|_| {
                CartesiaError::Connect(ConnectError {
                    status: None,
                    detail: format!("no handshake within {connect_timeout:?}"),
                })
            })?
            .map_err(|error| CartesiaError::Connect(handshake_error(&error)))?;
        Ok(Self {
            socket,
            read_timeout,
            last_message: Instant::now(),
        })
    }

    /// Writes `text` as one text message, waiting at most the read timeout, and starts the wait
    /// for the next message afresh: a request just sent is what the next read waits on.
    pub(crate) async fn send(&mut self, text: String) -> Result<(), CartesiaError> {
        self.send_within(text, self.read_timeout).await?;
        self.last_message = Instant::now();
        Ok(())
    }

    /// Writes `text` as one text message, waiting at most `bound`.
    pub(crate) async fn send_within(
        &mut self,
        text: String,
        bound: Duration,
    ) -> Result<(), CartesiaError> {
        tokio::time::timeout(bound, self.socket.send(Message::text(text)))
            .await
            .map_err(|_| CartesiaError::Stalled(bound))?
            .map_err(|error| CartesiaError::Transport(error.to_string()))
    }

    /// The next text message, waiting at most until the read timeout has passed since the last
    /// message read. Pings and pongs are skipped and move nothing.
    pub(crate) async fn read(&mut self) -> Result<String, CartesiaError> {
        loop {
            let deadline = self.last_message.checked_add(self.read_timeout);
            let next = match deadline {
                Some(deadline) => tokio::time::timeout_at(deadline, self.socket.next())
                    .await
                    .map_err(|_| CartesiaError::Stalled(self.read_timeout))?,
                None => self.socket.next().await,
            };
            let message = match next {
                None => {
                    return Err(CartesiaError::Closed {
                        code: None,
                        reason: String::new(),
                    });
                }
                Some(Err(error)) => return Err(CartesiaError::Transport(error.to_string())),
                Some(Ok(message)) => message,
            };
            match message {
                Message::Text(text) => {
                    self.last_message = Instant::now();
                    return Ok(text.as_str().to_owned());
                }
                Message::Close(frame) => {
                    return Err(CartesiaError::Closed {
                        code: frame.as_ref().map(|frame| u16::from(frame.code)),
                        reason: frame
                            .map(|frame| frame.reason.as_str().to_owned())
                            .unwrap_or_default(),
                    });
                }
                Message::Binary(data) => {
                    return Err(CartesiaError::Protocol(format!(
                        "a binary message of {} bytes; the endpoint sends text",
                        data.len()
                    )));
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    }

    /// Writes a close frame, waiting at most [`CLOSE_BOUND`], and drops the connection without
    /// reading anything more from it.
    pub(crate) async fn close(mut self) {
        let _ = tokio::time::timeout(CLOSE_BOUND, self.socket.close(None)).await;
    }
}

/// TLS with the ring provider and the webpki roots, independent of any process-wide default
/// provider.
pub(crate) fn tls_config() -> Result<Arc<rustls::ClientConfig>, CartesiaError> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| {
        CartesiaError::Connect(ConnectError {
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
