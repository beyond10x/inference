//! The transport's deadlines and its refusal of cleartext, offline: a local TCP or WebSocket
//! server plays a peer that never completes the opening handshake, or one that stops reading.
//! No network and no key: every URL that is refused is refused before anything connects, and the
//! credential is a fixed test value.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, DEFAULT_CONNECT_TIMEOUT,
    DEFAULT_WRITE_TIMEOUT, Endpoint, OPENAI_KEY_NAME, RealtimeClient, RealtimeError, SessionConfig,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const MODEL: &str = "gpt-realtime-2.1";

/// The deadlines the tests set: short, so a hang shows within the test's own bound.
const DEADLINE: Duration = Duration::from_millis(300);

/// How much later than its deadline a failure may arrive: scheduling, not waiting.
const SLACK: Duration = Duration::from_millis(1_500);

/// The test's own bound on an operation that, without a deadline, never ends.
const HANG: Duration = Duration::from_secs(8);

/// Serves the fixed test value under [`OPENAI_KEY_NAME`] and counts every resolution.
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

/// A source that refuses every name, so a client that consults it stops before connecting.
#[derive(Clone, Default)]
struct RefusingSource {
    asked: Arc<AtomicUsize>,
}

impl CredentialSource for RefusingSource {
    fn resolve<'a>(&'a self, _: &'a str) -> CredentialFuture<'a> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(CredentialError::Missing) })
    }
}

type Socket = WebSocketStream<TcpStream>;

/// A listener that accepts TCP connections and never answers on them: no TLS, no HTTP upgrade.
/// The accepted streams are held until the returned task ends.
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

/// A WebSocket server that runs `script` on its one connection.
async fn serve<F, Fut>(script: F) -> std::net::SocketAddr
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        script(socket).await;
    });
    address
}

/// Realtime session setup from the server's side.
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

/// After setup the peer neither reads nor closes: its receive buffer and then the client's send
/// buffer fill, and a write can no longer complete.
async fn stop_reading(socket: Socket) {
    tokio::time::sleep(Duration::from_secs(30)).await;
    drop(socket);
}

#[test]
fn the_deadlines_are_stated_and_on_the_endpoint() {
    let endpoint = Endpoint::new(MODEL);
    assert_eq!(endpoint.connect_timeout, DEFAULT_CONNECT_TIMEOUT);
    assert_eq!(endpoint.write_timeout, DEFAULT_WRITE_TIMEOUT);
    assert!(DEFAULT_CONNECT_TIMEOUT > Duration::ZERO && DEFAULT_CONNECT_TIMEOUT < HANG * 10);
    assert!(DEFAULT_WRITE_TIMEOUT > Duration::ZERO && DEFAULT_WRITE_TIMEOUT < HANG * 10);
    let set = endpoint
        .with_connect_timeout(DEADLINE)
        .with_write_timeout(DEADLINE * 2);
    assert_eq!(set.connect_timeout, DEADLINE);
    assert_eq!(set.write_timeout, DEADLINE * 2);
}

/// W1: a server that accepts TCP and never answers the upgrade fails `connect` at the endpoint's
/// connect deadline, as a connection failure without a status.
#[tokio::test]
async fn a_server_that_never_answers_the_upgrade_fails_connect_within_the_deadline() {
    let (address, _held) = silent_listener().await;
    let endpoint = Endpoint::new(MODEL)
        .with_url(format!("ws://{address}/v1/realtime"))
        .with_connect_timeout(DEADLINE);
    let started = Instant::now();
    let outcome = tokio::time::timeout(
        HANG,
        RealtimeClient::connect(
            &endpoint,
            &CountingSource::default(),
            OPENAI_KEY_NAME,
            &SessionConfig::default(),
        ),
    )
    .await;
    let elapsed = started.elapsed();
    let outcome = outcome.unwrap_or_else(|_| {
        panic!("connect was still waiting on the handshake after {HANG:?}; the deadline is {DEADLINE:?}")
    });
    match outcome {
        Err(RealtimeError::Connect(error)) => {
            assert_eq!(error.status, None, "{error}");
            assert!(!error.detail.contains(CREDENTIAL), "{error}");
        }
        other => panic!("expected a connection failure, got {other:?}"),
    }
    assert!(
        elapsed >= DEADLINE && elapsed <= DEADLINE + SLACK,
        "failed after {elapsed:?}; the deadline is {DEADLINE:?}"
    );
}

/// W2: `ws://` sends the bearer key in the clear, so it is taken only to a loopback host; any
/// other URL that is not `wss://` is refused as an invalid endpoint before the credential is
/// resolved and before anything connects. The source here refuses every name, so a client that
/// consulted it would stop before connecting too: nothing in this test reaches the network.
#[tokio::test]
async fn cleartext_off_loopback_is_refused_before_the_credential_or_a_connection() {
    for url in [
        "ws://api.openai.com/v1/realtime",
        "ws://10.0.0.5:8080/v1/realtime",
        "ws://127.0.0.1@example.com/v1/realtime",
        "ws://user@127.0.0.1/v1/realtime",
        "ws://localhost.example.com/v1/realtime",
        "ws://127.0.0.1.example.com/v1/realtime",
        "ws://[::2]:8080/v1/realtime",
        "WS://example.com/v1/realtime",
        "http://127.0.0.1:9/v1/realtime",
        "https://api.openai.com/v1/realtime",
        "api.openai.com/v1/realtime",
    ] {
        let source = RefusingSource::default();
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
            "{url}: {refused:?}"
        );
        assert_eq!(
            source.asked.load(Ordering::SeqCst),
            0,
            "{url}: the credential was resolved before the URL was refused"
        );
    }
}

/// W2: `ws://` to a loopback host still connects, by address and by name.
#[tokio::test]
async fn cleartext_to_loopback_still_connects() {
    for host in ["127.0.0.1", "localhost"] {
        let address = serve(|mut socket| async move {
            accept_session(&mut socket).await;
            let _ = socket.next().await;
        })
        .await;
        let source = CountingSource::default();
        let endpoint = Endpoint::new(MODEL)
            .with_url(format!("ws://{host}:{}/v1/realtime", address.port()))
            .with_read_timeout(Duration::from_secs(5));
        let client = RealtimeClient::connect(
            &endpoint,
            &source,
            OPENAI_KEY_NAME,
            &SessionConfig::default(),
        )
        .await
        .unwrap_or_else(|error| panic!("{host}: {error}"));
        assert_eq!(source.resolved.load(Ordering::SeqCst), 1);
        drop(client);
    }
}

/// N7: a peer that stops reading fails a write at the endpoint's write deadline instead of
/// blocking it, and every write before it completed at once.
#[tokio::test]
async fn a_peer_that_stops_reading_fails_a_write_within_the_deadline() {
    let address = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        stop_reading(socket).await;
    })
    .await;
    let endpoint = Endpoint::new(MODEL)
        .with_url(format!("ws://{address}/v1/realtime"))
        .with_read_timeout(Duration::from_secs(5))
        .with_write_timeout(DEADLINE);
    let mut client = RealtimeClient::connect(
        &endpoint,
        &CountingSource::default(),
        OPENAI_KEY_NAME,
        &SessionConfig::default(),
    )
    .await
    .expect("the session opens");
    let chunk = vec![0_u8; 512 * 1024];
    let outcome = tokio::time::timeout(HANG, async {
        for written in 0..400 {
            let started = Instant::now();
            if let Err(error) = client.append_encoded(&chunk).await {
                return (written, started.elapsed(), error);
            }
        }
        panic!("400 writes of 512 KiB completed against a peer that reads nothing");
    })
    .await;
    let (written, elapsed, error) = outcome.unwrap_or_else(|_| {
        panic!("a write was still blocked after {HANG:?}; the deadline is {DEADLINE:?}")
    });
    assert!(written > 0, "the first write already failed: {error}");
    assert!(
        matches!(&error, RealtimeError::Transport(_)),
        "expected a transport failure, got {error:?}"
    );
    assert!(
        elapsed >= DEADLINE && elapsed <= DEADLINE + SLACK,
        "the write failed after {elapsed:?}; the deadline is {DEADLINE:?}"
    );
}

#[cfg(feature = "gpt-live")]
mod live {
    use inference_openai::live::{LIVE_MODEL, LiveClient, LiveSessionConfig};

    use super::*;

    /// Live start from the server's side.
    async fn accept_start(socket: &mut Socket) {
        let start = receive(socket).await;
        assert_eq!(start["type"], "session.start", "{start}");
        send(
            socket,
            json!({"type": "session.started", "event_id": "event_started_1", "session": {"id": "live_123", "status": "active", "model": "gpt-live-1"}}),
        )
        .await;
    }

    /// W1 for GPT-Live.
    #[tokio::test]
    async fn a_server_that_never_answers_the_upgrade_fails_live_connect_within_the_deadline() {
        let (address, _held) = silent_listener().await;
        let endpoint = Endpoint::live(LIVE_MODEL)
            .with_url(format!("ws://{address}/v1/live/sessions"))
            .with_connect_timeout(DEADLINE);
        let started = Instant::now();
        let outcome = tokio::time::timeout(
            HANG,
            LiveClient::connect(
                &endpoint,
                &CountingSource::default(),
                OPENAI_KEY_NAME,
                &LiveSessionConfig::default(),
            ),
        )
        .await;
        let elapsed = started.elapsed();
        let outcome = outcome.unwrap_or_else(|_| {
            panic!("connect was still waiting on the handshake after {HANG:?}; the deadline is {DEADLINE:?}")
        });
        assert!(
            matches!(&outcome, Err(RealtimeError::Connect(error)) if error.status.is_none()),
            "{outcome:?}"
        );
        assert!(
            elapsed >= DEADLINE && elapsed <= DEADLINE + SLACK,
            "failed after {elapsed:?}; the deadline is {DEADLINE:?}"
        );
    }

    /// W2 for GPT-Live.
    #[tokio::test]
    async fn live_cleartext_off_loopback_is_refused_before_the_credential() {
        for url in [
            "ws://api.openai.com/v1/live/sessions",
            "ws://127.0.0.1@example.com/v1/live/sessions",
        ] {
            let source = RefusingSource::default();
            let endpoint = Endpoint::live(LIVE_MODEL).with_url(url);
            let refused = LiveClient::connect(
                &endpoint,
                &source,
                OPENAI_KEY_NAME,
                &LiveSessionConfig::default(),
            )
            .await;
            assert!(
                matches!(&refused, Err(RealtimeError::InvalidEndpoint(_))),
                "{url}: {refused:?}"
            );
            assert_eq!(source.asked.load(Ordering::SeqCst), 0, "{url}");
        }
    }

    /// N7 for GPT-Live.
    #[tokio::test]
    async fn a_live_peer_that_stops_reading_fails_a_write_within_the_deadline() {
        let address = serve(|mut socket| async move {
            accept_start(&mut socket).await;
            stop_reading(socket).await;
        })
        .await;
        let endpoint = Endpoint::live(LIVE_MODEL)
            .with_url(format!("ws://{address}/v1/live/sessions"))
            .with_read_timeout(Duration::from_secs(5))
            .with_write_timeout(DEADLINE);
        let mut client = LiveClient::connect(
            &endpoint,
            &CountingSource::default(),
            OPENAI_KEY_NAME,
            &LiveSessionConfig::default(),
        )
        .await
        .expect("the session starts");
        let chunk = vec![0_u8; 512 * 1024];
        let outcome = tokio::time::timeout(HANG, async {
            for written in 0..400 {
                let started = Instant::now();
                if let Err(error) = client.append_audio(&chunk).await {
                    return (written, started.elapsed(), error);
                }
            }
            panic!("400 writes of 512 KiB completed against a peer that reads nothing");
        })
        .await;
        let (written, elapsed, error) = outcome.unwrap_or_else(|_| {
            panic!("a write was still blocked after {HANG:?}; the deadline is {DEADLINE:?}")
        });
        assert!(written > 0, "the first write already failed: {error}");
        assert!(matches!(&error, RealtimeError::Transport(_)), "{error:?}");
        assert!(
            elapsed >= DEADLINE && elapsed <= DEADLINE + SLACK,
            "the write failed after {elapsed:?}; the deadline is {DEADLINE:?}"
        );
    }
}

#[cfg(feature = "duplex")]
mod binding {
    use std::num::{NonZeroU16, NonZeroU32};

    use inference_audio::{AudioEncoding, AudioFormat};
    use inference_duplex::{
        NativeSpeechModel, OpenError, SessionConfig as SpeechConfig, SessionFailureKind,
        TurnDetection, TurnTuning,
    };
    use inference_openai::duplex::OpenAiRealtime;

    use super::*;

    /// W1 through the binding: `OpenAiRealtime::open` blocks on the connect, and fails at the
    /// connect deadline as a closed transport. It runs on a thread of its own so that, without a
    /// deadline, the test still ends.
    #[test]
    fn the_binding_open_fails_within_the_connect_deadline() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (address, _held) = runtime.block_on(silent_listener());
        let endpoint = Endpoint::new(MODEL)
            .with_url(format!("ws://{address}/v1/realtime"))
            .with_connect_timeout(DEADLINE);
        let (done, outcome) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let started = Instant::now();
            let mut model = OpenAiRealtime::new(
                endpoint,
                Box::new(CountingSource::default()),
                OPENAI_KEY_NAME,
            );
            let pcm24 = AudioFormat {
                encoding: AudioEncoding::PcmS16le,
                sample_rate: NonZeroU32::new(24_000).unwrap(),
                channels: NonZeroU16::MIN,
            };
            let config = SpeechConfig {
                voice: "marin".to_owned(),
                instructions: "answer briefly".to_owned(),
                input_format: pcm24,
                output_format: pcm24,
                turn_detection: TurnDetection::ServerVad,
                turn_tuning: TurnTuning::default(),
                auto_response: false,
                auto_interrupt: false,
                tools: Vec::new(),
                idle_limit: Duration::from_secs(5),
            };
            let refused = model.open(config).err();
            let _ = done.send((started.elapsed(), refused));
        });
        let (elapsed, refused) = outcome.recv_timeout(HANG).unwrap_or_else(|_| {
            panic!("open was still waiting on the handshake after {HANG:?}; the deadline is {DEADLINE:?}")
        });
        assert!(
            matches!(&refused, Some(OpenError::Failed(failure)) if failure.kind == SessionFailureKind::TransportClosed),
            "{refused:?}"
        );
        assert!(
            elapsed >= DEADLINE && elapsed <= DEADLINE + SLACK,
            "failed after {elapsed:?}; the deadline is {DEADLINE:?}"
        );
    }
}
