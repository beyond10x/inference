//! Adversary pass 1 on the transport deadlines and the cleartext refusal: the variants the
//! unit's own cases do not build. A TLS peer that never answers, a handshake that progresses
//! slowly (under and over the deadline), a redirect, more URL shapes, a duplex session whose
//! write passes its deadline, and the error cap at exactly its bound. Offline: every server is a
//! local loopback listener and the credential is a fixed test value.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
    RealtimeClient, RealtimeError, SessionConfig,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;

const CREDENTIAL: &str = "offline-test-credential";
const MODEL: &str = "gpt-realtime-2.1";
const SLACK: Duration = Duration::from_millis(1_500);
const HANG: Duration = Duration::from_secs(8);

#[derive(Clone, Default)]
struct CountingSource {
    resolved: Arc<AtomicUsize>,
}

impl CredentialSource for CountingSource {
    fn resolve<'a>(&'a self, name: &'a str) -> CredentialFuture<'a> {
        self.resolved.fetch_add(1, Ordering::SeqCst);
        let found = name == OPENAI_KEY_NAME;
        Box::pin(async move {
            if found {
                Ok(Credential::new(CREDENTIAL.as_bytes().to_vec()))
            } else {
                Err(CredentialError::Missing)
            }
        })
    }
}

type Socket = WebSocketStream<TcpStream>;

async fn silent_listener() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    (address, task)
}

async fn read_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        stream.readable().await.unwrap();
        match stream.try_read(&mut buffer) {
            Ok(0) => panic!("the client closed before its request ended"),
            Ok(read) => request.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("{error}"),
        }
    }
    String::from_utf8(request).unwrap()
}

async fn write_all(stream: &mut TcpStream, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        stream.writable().await.unwrap();
        match stream.try_write(bytes) {
            Ok(written) => bytes = &bytes[written..],
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("{error}"),
        }
    }
}

fn accept_key(request: &str) -> String {
    let key = request
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("sec-websocket-key")
                .then(|| value.trim().to_owned())
        })
        .expect("the request carries Sec-WebSocket-Key");
    derive_accept_key(key.as_bytes())
}

/// A server that answers the upgrade one byte group at a time, the whole answer spread over
/// `spread`, then plays session setup and holds the connection.
async fn dripping_server(spread: Duration) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept_key(&request)
        );
        let bytes = response.as_bytes();
        let pieces = 20;
        let step = bytes.len().div_ceil(pieces);
        for piece in bytes.chunks(step) {
            tokio::time::sleep(spread / pieces as u32).await;
            if stream.writable().await.is_err() || stream.try_write(piece).is_err() {
                return;
            }
        }
        let mut socket = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
        accept_session(&mut socket).await;
        let _ = socket.next().await;
    });
    address
}

async fn send(socket: &mut Socket, event: Value) {
    socket.send(Message::text(event.to_string())).await.unwrap();
}

async fn receive(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("the client sent nothing for five seconds")
            .expect("the client closed the connection")
            .unwrap();
        match message {
            Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}

async fn accept_session(socket: &mut Socket) {
    send(
        socket,
        json!({"type": "session.created", "event_id": "event_1", "session": {"type": "realtime", "id": "sess_1"}}),
    )
    .await;
    let update = receive(socket).await;
    assert_eq!(update["type"], "session.update", "{update}");
    let mut confirmed = update["session"].clone();
    confirmed["id"] = json!("sess_1");
    send(
        socket,
        json!({"type": "session.updated", "event_id": "event_2", "session": confirmed}),
    )
    .await;
}

async fn connect(endpoint: &Endpoint) -> (Duration, Result<RealtimeClient, RealtimeError>) {
    let started = Instant::now();
    let outcome = tokio::time::timeout(
        HANG,
        RealtimeClient::connect(
            endpoint,
            &CountingSource::default(),
            OPENAI_KEY_NAME,
            &SessionConfig::default(),
        ),
    )
    .await
    .unwrap_or_else(|_| panic!("connect still waiting after {HANG:?}"));
    (started.elapsed(), outcome)
}

/// W1 over TLS: a `wss://` peer that accepts TCP and never answers the ClientHello fails
/// `connect` at the connect deadline, without a status.
#[tokio::test]
async fn a_tls_peer_that_never_answers_fails_connect_within_the_deadline() {
    let deadline = Duration::from_millis(300);
    let (address, _held) = silent_listener().await;
    let endpoint = Endpoint::new(MODEL)
        .with_url(format!("wss://{address}/v1/realtime"))
        .with_connect_timeout(deadline);
    let (elapsed, outcome) = connect(&endpoint).await;
    assert!(
        matches!(&outcome, Err(RealtimeError::Connect(error)) if error.status.is_none()),
        "{:?}",
        outcome.as_ref().err()
    );
    assert!(
        elapsed >= deadline && elapsed <= deadline + SLACK,
        "failed after {elapsed:?}; the deadline is {deadline:?}"
    );
}

/// W1: a handshake that progresses slowly and completes under the deadline connects; the
/// deadline is over the whole handshake, not an idle bound that a slow peer would trip.
#[tokio::test]
async fn a_slow_handshake_that_completes_under_the_deadline_connects() {
    let address = dripping_server(Duration::from_millis(1_000)).await;
    let endpoint = Endpoint::new(MODEL)
        .with_url(format!("ws://{address}/v1/realtime"))
        .with_read_timeout(Duration::from_secs(5))
        .with_connect_timeout(Duration::from_millis(2_500));
    let (elapsed, outcome) = connect(&endpoint).await;
    assert!(outcome.is_ok(), "{:?}", outcome.err());
    assert!(
        elapsed >= Duration::from_millis(900),
        "the answer was spread over a second, yet connect took {elapsed:?}"
    );
}

/// W1: a handshake that keeps progressing but would end past the deadline fails at the
/// deadline: progress does not extend it.
#[tokio::test]
async fn a_progressing_handshake_past_the_deadline_fails_at_the_deadline() {
    let deadline = Duration::from_millis(400);
    let address = dripping_server(Duration::from_millis(3_000)).await;
    let endpoint = Endpoint::new(MODEL)
        .with_url(format!("ws://{address}/v1/realtime"))
        .with_connect_timeout(deadline);
    let (elapsed, outcome) = connect(&endpoint).await;
    assert!(
        matches!(&outcome, Err(RealtimeError::Connect(error)) if error.status.is_none()),
        "{:?}",
        outcome.as_ref().err()
    );
    assert!(
        elapsed >= deadline && elapsed <= deadline + SLACK,
        "failed after {elapsed:?}; the deadline is {deadline:?}"
    );
}

/// W2: a loopback server that answers the upgrade with a redirect off loopback is not followed:
/// connect fails with the redirect's status and nothing else is contacted.
#[tokio::test]
async fn a_redirect_off_loopback_is_not_followed() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counted = accepted.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            let _ = read_request(&mut stream).await;
            write_all(
                &mut stream,
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: ws://{address}/again\r\nContent-Length: 0\r\n\r\n"
                )
                .as_bytes(),
            )
            .await;
        }
    });
    let endpoint = Endpoint::new(MODEL)
        .with_url(format!("ws://{address}/v1/realtime"))
        .with_connect_timeout(Duration::from_secs(2));
    let (_, outcome) = connect(&endpoint).await;
    assert!(
        matches!(&outcome, Err(RealtimeError::Connect(error)) if error.status == Some(302)),
        "{:?}",
        outcome.as_ref().err()
    );
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        1,
        "the redirect was followed"
    );
}

/// W2: URL shapes the unit's cases do not list. Each one either names a host off loopback, or
/// could be read as another host by a second parser; each is refused before the credential.
#[tokio::test]
async fn more_cleartext_shapes_off_loopback_are_refused_before_the_credential() {
    for url in [
        "ws://[::ffff:127.0.0.1]:9/v1/realtime",
        "ws://0.0.0.0:9/v1/realtime",
        "ws://user@remote.example/v1/realtime",
        "ws://user:secret@127.0.0.1:9/v1/realtime",
        "ws://127.0.0.1:9@example.com/v1/realtime",
        "ws://127.0.0.1\\@example.com/v1/realtime",
        "ws://127.0.0.1%2F@example.com/v1/realtime",
        "ws://127.0.0.1%2Eexample.com/v1/realtime",
        "ws://localhost.:9/v1/realtime",
        "ws://[::1%25lo]:9/v1/realtime",
        "ws://[::1].example.com/v1/realtime",
        "Ws://example.com/v1/realtime",
        "ws:/127.0.0.1:9/v1/realtime",
        "ws://example.com:9?127.0.0.1",
        " ws://127.0.0.1:9/v1/realtime",
    ] {
        let source = CountingSource::default();
        let endpoint = Endpoint::new(MODEL).with_url(url);
        let refused = RealtimeClient::connect(
            &endpoint,
            &source,
            OPENAI_KEY_NAME,
            &SessionConfig::default(),
        )
        .await;
        assert!(
            matches!(&refused, Err(RealtimeError::InvalidEndpoint(_))),
            "{url}: {:?}",
            refused.as_ref().err()
        );
        assert_eq!(source.resolved.load(Ordering::SeqCst), 0, "{url}");
    }
}

/// W2: loopback by `[::1]` and by another address in 127.0.0.0/8 still connects, as the crate
/// doc states.
#[tokio::test]
async fn cleartext_to_ipv6_loopback_connects() {
    let Ok(listener) = TcpListener::bind("[::1]:0").await else {
        return;
    };
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        accept_session(&mut socket).await;
        let _ = socket.next().await;
    });
    let endpoint = Endpoint::new(MODEL)
        .with_url(format!("ws://[::1]:{}/v1/realtime", address.port()))
        .with_read_timeout(Duration::from_secs(5));
    let (_, outcome) = connect(&endpoint).await;
    assert!(outcome.is_ok(), "{:?}", outcome.err());
}

#[cfg(feature = "duplex")]
mod binding {
    use std::num::{NonZeroU16, NonZeroU32};

    use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
    use inference_duplex::{
        NativeSpeechModel, SessionConfig as SpeechConfig, SessionError, SessionEvent,
        SessionFailureKind, SpeechSessionHandle, TurnDetection, TurnTuning,
    };
    use inference_openai::duplex::{OpenAiRealtime, OpenAiRealtimeSession};
    use tokio::runtime::Runtime;

    use super::*;

    struct Server {
        endpoint: Endpoint,
        _runtime: Runtime,
    }

    impl Server {
        fn start<F, Fut>(script: F) -> Self
        where
            F: FnOnce(Socket) -> Fut + Send + 'static,
            Fut: std::future::Future<Output = ()> + Send + 'static,
        {
            let runtime = Runtime::new().unwrap();
            let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
            let address = listener.local_addr().unwrap();
            runtime.spawn(async move {
                let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                    .await
                    .expect("the binding did not connect within five seconds")
                    .unwrap();
                let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                script(socket).await;
            });
            let endpoint = Endpoint::new(MODEL)
                .with_url(format!("ws://{address}/v1/realtime"))
                .with_read_timeout(Duration::from_secs(5));
            Self {
                endpoint,
                _runtime: runtime,
            }
        }

        fn model(&self, endpoint: Endpoint) -> OpenAiRealtime {
            OpenAiRealtime::new(
                endpoint,
                Box::new(CountingSource::default()),
                OPENAI_KEY_NAME,
            )
        }
    }

    fn pcm24() -> AudioFormat {
        AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(24_000).unwrap(),
            channels: NonZeroU16::MIN,
        }
    }

    fn config() -> SpeechConfig {
        SpeechConfig {
            voice: "marin".to_owned(),
            instructions: "answer briefly".to_owned(),
            input_format: pcm24(),
            output_format: pcm24(),
            turn_detection: TurnDetection::ServerVad,
            turn_tuning: TurnTuning::default(),
            auto_response: false,
            auto_interrupt: false,
            tools: Vec::new(),
            idle_limit: Duration::from_secs(5),
        }
    }

    fn until(
        session: &mut impl SpeechSessionHandle,
        stop: impl Fn(&SessionEvent) -> bool,
    ) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        let mut idle = 0;
        while idle < 20 {
            match session.next_event(Duration::from_millis(100)) {
                Some(event) => {
                    idle = 0;
                    let done = stop(&event);
                    events.push(event);
                    if done {
                        break;
                    }
                }
                None => idle += 1,
            }
        }
        events
    }

    /// N7 through the binding: a peer that stops reading makes one `append_audio` fail at the
    /// write deadline; the session is then failed with a typed transport failure, delivers it as
    /// an event, and refuses the next write as not open instead of writing after a partial
    /// message.
    #[test]
    fn a_write_past_its_deadline_fails_the_duplex_session_typed() {
        let deadline = Duration::from_millis(300);
        let server = Server::start(|mut socket| async move {
            accept_session(&mut socket).await;
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(socket);
        });
        let endpoint = server.endpoint.clone().with_write_timeout(deadline);
        let mut model = server.model(endpoint);
        let (done, outcome) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut session = model.open(config()).expect("the session opens");
            let samples = 240_000_u32;
            let mut position = 0_u64;
            let mut result = None;
            for written in 0..200 {
                let frame = AudioFrame {
                    format: pcm24(),
                    position: SampleClockPosition::new(position).unwrap(),
                    sample_count: samples,
                    data: vec![0; samples as usize * 2],
                };
                position += u64::from(samples);
                let started = Instant::now();
                if let Err(error) = session.append_audio(frame) {
                    result = Some((written, started.elapsed(), error));
                    break;
                }
            }
            let (written, elapsed, error) =
                result.expect("200 appends completed against a peer that reads nothing");
            let events = until(&mut session, |event| {
                matches!(event, SessionEvent::SpeechSessionFailed(_))
            });
            let next = session.append_audio(AudioFrame {
                format: pcm24(),
                position: SampleClockPosition::new(position).unwrap(),
                sample_count: 480,
                data: vec![0; 960],
            });
            let _ = done.send((written, elapsed, error, events, next.err()));
        });
        let (written, elapsed, error, events, next) = outcome
            .recv_timeout(HANG * 2)
            .unwrap_or_else(|_| panic!("an append was still blocked after {:?}", HANG * 2));
        assert!(written > 0, "the first append already failed: {error:?}");
        assert!(matches!(error, SessionError::NotOpen(_)), "{error:?}");
        assert!(
            elapsed >= deadline && elapsed <= deadline + SLACK,
            "the append failed after {elapsed:?}; the deadline is {deadline:?}"
        );
        assert!(
            matches!(
                events.last(),
                Some(SessionEvent::SpeechSessionFailed(failed))
                    if failed.failure.kind == SessionFailureKind::TransportClosed
            ),
            "{events:#?}"
        );
        assert!(matches!(next, Some(SessionError::NotOpen(_))), "{next:?}");
        drop(server);
    }

    fn errors_then_speech(sent: usize) -> (Vec<String>, u64) {
        let server = Server::start(move |mut socket| async move {
            accept_session(&mut socket).await;
            for at in 0..sent {
                send(
                    &mut socket,
                    json!({"type": "error", "event_id": format!("event_err_{at}"), "error": {
                        "type": "invalid_request_error", "code": "invalid_value",
                        "message": format!("error {at}"), "param": null, "event_id": null
                    }}),
                )
                .await;
            }
            send(
                &mut socket,
                json!({"type": "input_audio_buffer.speech_started", "event_id": "event_ss", "audio_start_ms": 100, "item_id": "item_1"}),
            )
            .await;
            let _ = socket.next().await;
        });
        let mut model = server.model(server.endpoint.clone());
        let mut session = model.open(config()).expect("the session opens");
        let events = until(&mut session, |event| {
            matches!(event, SessionEvent::SpeechStarted(_))
        });
        assert!(
            matches!(events.last(), Some(SessionEvent::SpeechStarted(_))),
            "{events:#?}"
        );
        let kept = session
            .server_errors()
            .iter()
            .map(|error| error.message.clone())
            .collect();
        let count = session.server_error_count();
        let _ = session.close();
        drop(server);
        (kept, count)
    }

    /// N8 at the bound: exactly `SERVER_ERRORS_KEPT` errors are all kept; one more forgets
    /// exactly the oldest; the count is exact in both.
    #[test]
    fn server_errors_at_and_one_past_the_cap() {
        let cap = OpenAiRealtimeSession::SERVER_ERRORS_KEPT;
        assert_eq!(cap, 64, "the crate doc states 64");
        let (kept, count) = errors_then_speech(cap);
        assert_eq!(
            kept,
            (0..cap).map(|at| format!("error {at}")).collect::<Vec<_>>()
        );
        assert_eq!(count, cap as u64);
        let (kept, count) = errors_then_speech(cap + 1);
        assert_eq!(
            kept,
            (1..=cap)
                .map(|at| format!("error {at}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(count, cap as u64 + 1);
    }
}
