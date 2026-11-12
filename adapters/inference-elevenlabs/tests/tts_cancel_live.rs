//! A billed live probe of the multi-stream text-to-speech endpoint, behind feature `live` and
//! ignored. It asserts nothing about the endpoint: it prints raw measurements that decide how the
//! binding closes and cancels a context (story:elevenlabs-tts-provider-cancel). It talks to the
//! socket itself through the crate's protocol helpers, not through the synthesiser, so what it
//! prints is the endpoint's behaviour and not the binding's.
//!
//! It needs a key with text-to-speech access in `ELEVENLABS_API_KEY` (or the keychain secret
//! `elevenlabs-api-key`), a voice in `ELEVENLABS_VOICE_ID`, and the key's residency in
//! `ELEVENLABS_RESIDENCY` (`Endpoint::from_env`, global when unset). Each probe opens a connection
//! of its own in `ulaw_8000`, so an audio byte is one sample and 8 bytes are one millisecond.
//! About 900 characters are synthesised in all. Run it with
//! `cargo test -p inference-elevenlabs --features live --test tts_cancel_live -- --ignored --nocapture`.
//!
//! - P1: a short text with `flush` and no close. Does a final message for the context arrive,
//!   and how long after its last audio? It waits up to `P1_WAIT`, past the endpoint's default
//!   20 s context inactivity timeout.
//! - P2: a long text with `flush`, and `close_context` right after its first audio message. How
//!   many audio messages and bytes arrive after the close, and how much audio in all against the
//!   full text's audio (P3)?
//! - P3: the same long text with `flush` and no close. Every gap between two audio messages and
//!   the largest, to size a quiet window, and the full text's audio; then a close, and how long
//!   its final message takes.
//! - P4: a text of punctuation only with `flush`. Does any message arrive for it, with and then
//!   without a close?

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_elevenlabs::protocol::{
    CLOSE_SOCKET, close_context_message, context_message, request_url, text_message,
};
use inference_elevenlabs::{
    CredentialSource, ELEVENLABS_KEY_NAME, ELEVENLABS_KEY_VARIABLE, Endpoint, EnvironmentSource,
    KeychainSource, MAX_MESSAGE_SIZE, OutputFormat, VoiceConfig,
};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The variable naming the voice.
const VOICE_VARIABLE: &str = "ELEVENLABS_VOICE_ID";

/// The short text of P1.
const SHORT: &str = "This is a short sentence for the first probe.";

/// The long text of P2 and P3, about 400 characters.
const LONG: &str = "The harbour was quiet that morning, and the fishing boats rocked gently against \
     the old stone pier while gulls circled overhead. A few early walkers wandered along the \
     promenade, stopping now and then to watch the light change over the water. Somewhere a bell \
     rang the hour, and the baker on the corner opened his shutters, letting the smell of fresh \
     bread drift down the narrow street toward the sea.";

/// The punctuation-only text of P4.
const PUNCTUATION: &str = "…";

/// How long P1 waits for a final message after its last message.
const P1_WAIT: Duration = Duration::from_secs(30);

/// How long P2, P3 and P4 wait for the next message before they take the endpoint as done.
const QUIET: Duration = Duration::from_secs(8);

/// The key from the environment when it is set, else from the keychain, as the live example
/// reads it.
fn source() -> Arc<dyn CredentialSource> {
    if std::env::var_os(ELEVENLABS_KEY_VARIABLE).is_some() {
        Arc::new(EnvironmentSource::elevenlabs())
    } else {
        Arc::new(KeychainSource::native().expect("a keychain"))
    }
}

/// The URL of the multi-stream endpoint for the voice in `ulaw_8000`.
fn url() -> String {
    let endpoint = Endpoint::from_env().unwrap_or_else(|error| panic!("{error}"));
    let voice =
        std::env::var(VOICE_VARIABLE).unwrap_or_else(|_| panic!("{VOICE_VARIABLE} is unset"));
    request_url(&endpoint, &VoiceConfig::new(voice, OutputFormat::Ulaw8000))
        .unwrap_or_else(|error| panic!("{error}"))
}

/// TLS with the ring provider and the webpki roots, as the binding connects.
fn tls() -> Arc<rustls::ClientConfig> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth(),
    )
}

/// Opens one connection, the key in the `xi-api-key` header only, marked sensitive.
async fn open() -> Socket {
    let url = url();
    let credential = source()
        .resolve(ELEVENLABS_KEY_NAME)
        .await
        .unwrap_or_else(|error| panic!("no key: {error}"));
    let mut header = HeaderValue::from_bytes(credential.expose()).expect("a header value");
    header.set_sensitive(true);
    drop(credential);
    let mut request = url.as_str().into_client_request().expect("a request");
    request.headers_mut().insert("xi-api-key", header);
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_SIZE))
        .max_frame_size(Some(MAX_MESSAGE_SIZE));
    let connector = url.starts_with("wss://").then(|| Connector::Rustls(tls()));
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async_tls_with_config(request, Some(config), true, connector),
    )
    .await
    .expect("a handshake within 10 s")
    .unwrap_or_else(|error| panic!("the handshake failed: {error}"));
    socket
}

async fn send(socket: &mut Socket, text: String) {
    socket
        .send(Message::text(text))
        .await
        .unwrap_or_else(|error| panic!("a write failed: {error}"));
}

/// One message the endpoint sent, as the probe records it.
#[derive(Debug)]
struct Heard {
    /// Since the probe sent its text.
    at: Duration,
    context: Option<String>,
    /// Decoded audio bytes; 0 for none.
    audio: usize,
    is_final: bool,
    /// The message when it is neither audio nor a final message, cut to 300 characters.
    other: Option<String>,
}

/// The next message, or `None` once `wait` passes without one or the connection ends.
async fn next(socket: &mut Socket, sent: Instant, wait: Duration) -> Option<Heard> {
    loop {
        let message = match tokio::time::timeout(wait, socket.next()).await {
            Err(_) => return None,
            Ok(None) => {
                println!("    the connection ended");
                return None;
            }
            Ok(Some(Err(error))) => {
                println!("    the connection failed: {error}");
                return None;
            }
            Ok(Some(Ok(message))) => message,
        };
        let text = match message {
            Message::Text(text) => text.as_str().to_owned(),
            Message::Close(frame) => {
                println!("    close frame: {frame:?}");
                return None;
            }
            Message::Binary(data) => format!("<binary message of {} bytes>", data.len()),
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
        };
        let at = sent.elapsed();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let audio = value
            .get("audio")
            .and_then(Value::as_str)
            .map_or(0, |audio| {
                STANDARD.decode(audio).map_or(0, |bytes| bytes.len())
            });
        let is_final = ["isFinal", "is_final"]
            .iter()
            .any(|name| value.get(*name) == Some(&Value::Bool(true)));
        let context = value
            .get("contextId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let other = (audio == 0 && !is_final).then(|| text.chars().take(300).collect());
        return Some(Heard {
            at,
            context,
            audio,
            is_final,
            other,
        });
    }
}

fn ms(duration: Duration) -> u128 {
    duration.as_millis()
}

/// Prints one message as a line of the probe's log.
fn log(heard: &Heard) {
    println!(
        "    +{:>6} ms  context {:<6} audio {:>6} B  final {:<5} {}",
        ms(heard.at),
        heard.context.as_deref().unwrap_or("-"),
        heard.audio,
        heard.is_final,
        heard.other.as_deref().unwrap_or("")
    );
}

/// Opens the context `context` and sends `text` to it with `flush`. Returns the instant the text
/// was sent.
async fn speak(socket: &mut Socket, context: &str, text: &str) -> Instant {
    send(socket, context_message(context, None)).await;
    let sent = Instant::now();
    send(
        socket,
        text_message(context, text).expect("the text is not blank"),
    )
    .await;
    sent
}

async fn finish(mut socket: Socket) {
    let _ = socket.send(Message::text(CLOSE_SOCKET)).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.close(None)).await;
}

/// The audio bytes as milliseconds of `ulaw_8000`.
fn audio_ms(bytes: usize) -> usize {
    bytes / 8
}

/// P1: flush, no close.
async fn p1() {
    println!(
        "P1: {} characters with flush, no close",
        SHORT.chars().count()
    );
    let mut socket = open().await;
    let sent = speak(&mut socket, "p1", SHORT).await;
    let (mut first, mut last_audio, mut final_at, mut bytes) = (None, None, None, 0);
    while let Some(heard) = next(&mut socket, sent, P1_WAIT).await {
        log(&heard);
        if heard.audio > 0 {
            first.get_or_insert(heard.at);
            last_audio = Some(heard.at);
            bytes += heard.audio;
        }
        if heard.is_final {
            final_at = Some(heard.at);
            break;
        }
    }
    println!(
        "P1 result: first audio {:?} ms, last audio {:?} ms, audio {} B ({} ms), final {}",
        first.map(ms),
        last_audio.map(ms),
        bytes,
        audio_ms(bytes),
        match (final_at, last_audio) {
            (Some(at), Some(last)) => format!(
                "arrived at {} ms, {} ms after the last audio",
                ms(at),
                ms(at.saturating_sub(last))
            ),
            (Some(at), None) => format!("arrived at {} ms, with no audio", ms(at)),
            (None, _) => format!("none within {} s of the last message", P1_WAIT.as_secs()),
        }
    );
    finish(socket).await;
}

/// The audio of P3: the long text spoken whole, with the gaps between its audio messages.
struct Whole {
    bytes: usize,
    first: Option<Duration>,
    last: Option<Duration>,
}

/// P3: flush, no close, then a close once it has gone quiet.
async fn p3() -> Whole {
    println!(
        "P3: {} characters with flush, no close",
        LONG.chars().count()
    );
    let mut socket = open().await;
    let sent = speak(&mut socket, "p3", LONG).await;
    let mut whole = Whole {
        bytes: 0,
        first: None,
        last: None,
    };
    let mut gaps = Vec::new();
    let mut final_unasked = None;
    while let Some(heard) = next(&mut socket, sent, QUIET).await {
        log(&heard);
        if heard.audio > 0 {
            if let Some(last) = whole.last {
                gaps.push(heard.at.saturating_sub(last));
            }
            whole.first.get_or_insert(heard.at);
            whole.last = Some(heard.at);
            whole.bytes += heard.audio;
        }
        if heard.is_final {
            final_unasked = Some(heard.at);
            break;
        }
    }
    let largest = gaps.iter().max().copied();
    let mut sorted = gaps.clone();
    sorted.sort();
    println!(
        "P3 result: first audio {:?} ms, last audio {:?} ms, {} audio messages, audio {} B ({} ms); \
         gaps between audio messages in ms {:?}; largest {:?} ms; median {:?} ms",
        whole.first.map(ms),
        whole.last.map(ms),
        gaps.len() + usize::from(whole.first.is_some()),
        whole.bytes,
        audio_ms(whole.bytes),
        gaps.iter().map(|gap| ms(*gap)).collect::<Vec<_>>(),
        largest.map(ms),
        sorted.get(sorted.len() / 2).map(|gap| ms(*gap)),
    );
    match final_unasked {
        Some(at) => println!(
            "P3 result: a final message arrived unasked at {} ms",
            ms(at)
        ),
        None => {
            let closed = Instant::now();
            send(&mut socket, close_context_message("p3")).await;
            let mut answered = false;
            while let Some(heard) = next(&mut socket, closed, QUIET).await {
                log(&heard);
                if heard.is_final {
                    println!(
                        "P3 result: no final message in {} s of quiet; after the close one \
                         arrived {} ms later",
                        QUIET.as_secs(),
                        ms(heard.at)
                    );
                    answered = true;
                    break;
                }
            }
            if !answered {
                println!(
                    "P3 result: no final message, neither in {} s of quiet nor in {} s after the close",
                    QUIET.as_secs(),
                    QUIET.as_secs()
                );
            }
        }
    }
    finish(socket).await;
    whole
}

/// P2: flush, then a close right after the first audio message.
async fn p2(whole: &Whole) {
    println!(
        "P2: {} characters with flush, close after the first audio message",
        LONG.chars().count()
    );
    let mut socket = open().await;
    let sent = speak(&mut socket, "p2", LONG).await;
    let mut closed_at = None;
    let (mut before, mut after, mut messages_after) = (0, 0, 0);
    let mut final_at = None;
    while let Some(heard) = next(&mut socket, sent, QUIET).await {
        log(&heard);
        if closed_at.is_some() {
            after += heard.audio;
            messages_after += usize::from(heard.audio > 0);
        } else {
            before += heard.audio;
        }
        if heard.is_final {
            final_at = Some(heard.at);
            break;
        }
        if heard.audio > 0 && closed_at.is_none() {
            send(&mut socket, close_context_message("p2")).await;
            closed_at = Some(sent.elapsed());
            println!("    close sent at +{} ms", ms(sent.elapsed()));
        }
    }
    let total = before + after;
    println!(
        "P2 result: close at {:?} ms; before it {} B ({} ms); after it {} audio messages, {} B \
         ({} ms); final {}; audio in all {} ms against {} ms for the whole text (P3), {}%",
        closed_at.map(ms),
        before,
        audio_ms(before),
        messages_after,
        after,
        audio_ms(after),
        final_at.map_or_else(
            || format!("none within {} s of quiet", QUIET.as_secs()),
            |at| format!("at {} ms", ms(at))
        ),
        audio_ms(total),
        audio_ms(whole.bytes),
        (total * 100).checked_div(whole.bytes).unwrap_or(0),
    );
    finish(socket).await;
}

/// P4: punctuation only, flush, then a close.
async fn p4() {
    println!("P4: {PUNCTUATION:?} with flush, no close, then a close");
    let mut socket = open().await;
    let sent = speak(&mut socket, "p4", PUNCTUATION).await;
    let mut before = 0;
    while let Some(heard) = next(&mut socket, sent, QUIET).await {
        log(&heard);
        before += 1;
        if heard.is_final {
            break;
        }
    }
    println!(
        "P4 result: {before} message(s) without a close in {} s of quiet",
        QUIET.as_secs()
    );
    let closed = Instant::now();
    send(&mut socket, close_context_message("p4")).await;
    let mut after = 0;
    let mut final_at = None;
    while let Some(heard) = next(&mut socket, closed, QUIET).await {
        log(&heard);
        after += 1;
        if heard.is_final {
            final_at = Some(heard.at);
            break;
        }
    }
    println!(
        "P4 result: after the close {after} message(s); final {}",
        final_at.map_or_else(
            || format!("none within {} s", QUIET.as_secs()),
            |at| format!("{} ms after the close", ms(at))
        )
    );
    finish(socket).await;
}

#[tokio::test]
#[ignore = "billed: needs ELEVENLABS_API_KEY (or the keychain) with text-to-speech access, ELEVENLABS_VOICE_ID and ELEVENLABS_RESIDENCY for a residency key"]
async fn tts_cancel_probe_prints_the_endpoints_context_behaviour() {
    println!("endpoint: {}", url().split('?').next().unwrap_or_default());
    p1().await;
    let whole = p3().await;
    p2(&whole).await;
    p4().await;
}
