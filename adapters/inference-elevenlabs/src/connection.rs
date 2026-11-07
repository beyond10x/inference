//! One WebSocket connection of one synthesis: the handshake with the key, bounded writes and
//! reads, and the close.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::credential::CredentialSource;
use crate::error::{ConnectError, ElevenLabsError};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The most a cancel or an orderly close waits for the close frame to be written.
pub const CLOSE_BOUND: Duration = Duration::from_secs(1);

/// An open connection and the bound on its writes and on each wait for the server.
pub(crate) struct Connection {
    socket: Socket,
    read_timeout: Duration,
}

impl Connection {
    /// Resolves the secret `credential` names through `source` and opens a WebSocket to `url`
    /// with it in the `xi-api-key` header, marked sensitive. The credential is resolved before
    /// any connection is made, is sent only in the handshake and is never part of an error.
    pub(crate) async fn open(
        url: &str,
        source: &dyn CredentialSource,
        credential: &str,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<Self, ElevenLabsError> {
        let resolved = source
            .resolve(credential)
            .await
            .map_err(ElevenLabsError::Credential)?;
        let header = resolved
            .into_header(b"")
            .map_err(ElevenLabsError::Credential)?;

        let mut request = url.into_client_request().map_err(|error| {
            ElevenLabsError::Connect(ConnectError {
                status: None,
                detail: error.to_string(),
            })
        })?;
        request.headers_mut().insert("xi-api-key", header);
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
                ElevenLabsError::Connect(ConnectError {
                    status: None,
                    detail: format!("no handshake within {connect_timeout:?}"),
                })
            })?
            .map_err(|error| ElevenLabsError::Connect(handshake_error(&error)))?;
        Ok(Self {
            socket,
            read_timeout,
        })
    }

    /// The instant the read timeout ends when it starts now.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        Instant::now().checked_add(self.read_timeout)
    }

    /// Writes `text` as one text message, waiting at most the read timeout.
    pub(crate) async fn send(&mut self, text: String) -> Result<(), ElevenLabsError> {
        tokio::time::timeout(self.read_timeout, self.socket.send(Message::text(text)))
            .await
            .map_err(|_| ElevenLabsError::Stalled(self.read_timeout))?
            .map_err(|error| ElevenLabsError::Transport(error.to_string()))
    }

    /// The next text message, waiting at most until `deadline`, which the caller moves on the
    /// messages it counts. Pings and pongs are skipped and move nothing.
    pub(crate) async fn read_until(
        &mut self,
        deadline: Option<Instant>,
    ) -> Result<String, ElevenLabsError> {
        loop {
            let next = match deadline {
                Some(deadline) => tokio::time::timeout_at(deadline, self.socket.next())
                    .await
                    .map_err(|_| ElevenLabsError::Stalled(self.read_timeout))?,
                None => self.socket.next().await,
            };
            let message = match next {
                None => {
                    return Err(ElevenLabsError::Closed {
                        code: None,
                        reason: String::new(),
                    });
                }
                Some(Err(error)) => return Err(ElevenLabsError::Transport(error.to_string())),
                Some(Ok(message)) => message,
            };
            match message {
                Message::Text(text) => return Ok(text.as_str().to_owned()),
                Message::Close(frame) => {
                    return Err(ElevenLabsError::Closed {
                        code: frame.as_ref().map(|frame| u16::from(frame.code)),
                        reason: frame
                            .map(|frame| frame.reason.as_str().to_owned())
                            .unwrap_or_default(),
                    });
                }
                Message::Binary(data) => {
                    return Err(ElevenLabsError::Protocol(format!(
                        "a binary message of {} bytes; the endpoint sends text",
                        data.len()
                    )));
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    }

    /// Writes `farewell`, when there is one, then a close frame, waiting at most [`CLOSE_BOUND`]
    /// for both, and drops the connection without reading anything more from it.
    pub(crate) async fn close(mut self, farewell: Option<&str>) {
        let socket = &mut self.socket;
        let _ = tokio::time::timeout(CLOSE_BOUND, async move {
            if let Some(farewell) = farewell {
                let _ = socket.send(Message::text(farewell)).await;
            }
            let _ = socket.close(None).await;
        })
        .await;
    }
}

/// TLS with the ring provider and the webpki roots, independent of any process-wide default
/// provider.
pub(crate) fn tls_config() -> Result<Arc<rustls::ClientConfig>, ElevenLabsError> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| {
        ElevenLabsError::Connect(ConnectError {
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
