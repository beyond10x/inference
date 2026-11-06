//! The opening of one WebSocket connection to a Deepgram endpoint, provider-wide: the handshake
//! with the key in the `Authorization` header, bounded by the connect timeout, and a bounded
//! close.

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::credential::{CredentialError, CredentialSource};
use crate::error::{ConnectError, DeepgramError};
use crate::listen::MAX_MESSAGE_SIZE;

/// An open connection.
pub(crate) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The most a cancel or an orderly close waits for the close frame to be written.
pub const CLOSE_BOUND: Duration = Duration::from_secs(1);

/// Resolves the secret `credential` names through `source` and opens a WebSocket to `url` with
/// `Authorization: Token <key>`, marked sensitive, reading messages and frames of at most
/// [`MAX_MESSAGE_SIZE`]. The credential is resolved before any connection is made, an empty or
/// blank one is refused as invalid, and it is sent only in the handshake and is never part of an
/// error.
pub(crate) async fn open(
    url: &str,
    source: &dyn CredentialSource,
    credential: &str,
    connect_timeout: Duration,
) -> Result<Socket, DeepgramError> {
    let resolved = source
        .resolve(credential)
        .await
        .map_err(DeepgramError::Credential)?;
    if resolved.expose().iter().all(u8::is_ascii_whitespace) {
        return Err(DeepgramError::Credential(CredentialError::Invalid));
    }
    let mut value = b"Token ".to_vec();
    value.extend_from_slice(resolved.expose());
    drop(resolved);
    let value = zeroize::Zeroizing::new(value);
    let mut header = HeaderValue::from_bytes(&value)
        .map_err(|_| DeepgramError::Credential(CredentialError::Invalid))?;
    header.set_sensitive(true);

    let mut request = url.into_client_request().map_err(|error| {
        DeepgramError::Connect(ConnectError {
            status: None,
            detail: error.to_string(),
        })
    })?;
    request.headers_mut().insert("authorization", header);
    let connector = if url.starts_with("wss://") {
        Some(Connector::Rustls(tls_config()?))
    } else {
        None
    };
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_SIZE))
        .max_frame_size(Some(MAX_MESSAGE_SIZE));
    let handshake =
        tokio_tungstenite::connect_async_tls_with_config(request, Some(config), true, connector);
    let (socket, _) = tokio::time::timeout(connect_timeout, handshake)
        .await
        .map_err(|_| {
            DeepgramError::Connect(ConnectError {
                status: None,
                detail: format!("no handshake within {connect_timeout:?}"),
            })
        })?
        .map_err(|error| DeepgramError::Connect(handshake_error(&error)))?;
    Ok(socket)
}

/// Writes a close frame, waiting at most [`CLOSE_BOUND`], and drops the connection without
/// reading anything more from it.
pub(crate) async fn close(mut socket: Socket) {
    let _ = tokio::time::timeout(CLOSE_BOUND, socket.close(None)).await;
}

/// TLS with the ring provider and the webpki roots, independent of any process-wide default
/// provider.
fn tls_config() -> Result<Arc<rustls::ClientConfig>, DeepgramError> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| {
        DeepgramError::Connect(ConnectError {
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
