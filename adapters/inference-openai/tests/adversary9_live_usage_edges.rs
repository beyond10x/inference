//! Adversary case, pass 9, story `duplex-late-usage-counted` at 7ec1aa4: GPT-Live backend usage
//! read field by field in both directions, and a backend end without its `response` object.
//!
//! The module documentation of `live_duplex` says a backend response end whose `response` or
//! `usage` was read only in part is counted as a frame the binding could not read, "and none
//! refreshes the liveness bound". It also says such a frame is counted "after what it
//! recorded", so a stall after it is a provider error. The cases below drive both statements,
//! the usage with neither count, the input left out before a known one, and the closing path.
//! No network and no key.
#![cfg(all(feature = "gpt-live", feature = "duplex"))]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, QuantityKind, ResponseId, SessionConfig, SessionEvent, SessionFailureKind,
    SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_openai::live::LIVE_MODEL;
use inference_openai::live_duplex::{OpenAiLive, OpenAiLiveSession};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const WAIT: Duration = Duration::from_millis(100);

struct FixedSource;

impl CredentialSource for FixedSource {
    fn resolve<'a>(&'a self, name: &'a str) -> CredentialFuture<'a> {
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

struct Server {
    endpoint: Endpoint,
    _runtime: Runtime,
    _task: JoinHandle<()>,
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
        let task = runtime.spawn(async move {
            let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .expect("the binding did not connect within five seconds")
                .unwrap();
            let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            script(socket).await;
        });
        let endpoint = Endpoint::live(LIVE_MODEL)
            .with_url(format!("ws://{address}/v1/live/sessions"))
            .with_read_timeout(Duration::from_secs(5));
        Self {
            endpoint,
            _runtime: runtime,
            _task: task,
        }
    }

    fn model(&self) -> OpenAiLive {
        OpenAiLive::new(
            self.endpoint.clone(),
            Box::new(FixedSource),
            OPENAI_KEY_NAME,
        )
    }
}

async fn send(socket: &mut Socket, event: Value) {
    socket.send(Message::text(event.to_string())).await.unwrap();
}

async fn receive(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("the binding sent nothing for five seconds")
            .expect("the binding closed the connection")
            .unwrap();
        match message {
            Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}

async fn accept_start(socket: &mut Socket) {
    let start = receive(socket).await;
    assert_eq!(start["type"], "session.start", "{start}");
    let mut resolved = start["session"].clone();
    resolved["id"] = json!("live_123");
    resolved["status"] = json!("active");
    send(
        socket,
        json!({"type": "session.started", "event_id": "event_started_1", "session": resolved}),
    )
    .await;
}

fn session_closed() -> Value {
    json!({"type": "session.closed", "event_id": "evt_c", "reason": "close_requested", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 1.5}})
}

/// Answers the binding's `session.close` with `session.closed`.
async fn closed(socket: &mut Socket) {
    let close = receive(socket).await;
    assert_eq!(close["type"], "session.close", "{close}");
    send(socket, session_closed()).await;
    while let Some(Ok(_)) = socket.next().await {}
}

fn said(delta: &str, start_ms: u64) -> Value {
    json!({"type": "session.output_transcript.delta", "event_id": "evt_out", "delta": delta, "start_ms": start_ms, "end_ms": start_ms + 200})
}

fn backend_end(kind: &str, response: Value) -> Value {
    json!({"type": "response.event", "event_id": "evt_backend", "delegation_id": "del_1", "event": {
        "type": kind, "sequence_number": 9, "response": response
    }})
}

/// A backend `response.completed` without any `response`.
fn completed_without_response() -> Value {
    json!({"type": "response.event", "event_id": "evt_backend", "delegation_id": "del_1", "event": {
        "type": "response.completed", "sequence_number": 9
    }})
}

fn completed_with(usage: Value) -> Value {
    backend_end(
        "response.completed",
        json!({"id": "resp_1", "status": "completed", "output": [], "usage": usage}),
    )
}

fn config(idle_limit: Duration) -> SessionConfig {
    let pcm24 = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap(),
        channels: NonZeroU16::MIN,
    };
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm24,
        output_format: pcm24,
        turn_detection: TurnDetection::SemanticVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit,
    }
}

fn until(
    session: &mut impl SpeechSessionHandle,
    stop: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let mut idle = 0;
    while idle < 10 {
        match session.next_event(WAIT) {
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

/// Opens a session against `script`, reads until the first transcript delta, waits for the rest
/// of what the script sends to arrive, closes, and returns every event and the session.
fn run<F, Fut>(script: F) -> (Vec<SessionEvent>, OpenAiLiveSession)
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let server = Server::start(script);
    let mut model = server.model();
    let mut session = model
        .open(config(Duration::from_secs(5)))
        .expect("the session opens");
    let mut events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(_))
    });
    std::thread::sleep(Duration::from_millis(300));
    session.close().expect("open");
    events.extend(std::iter::from_fn(|| session.next_event(Duration::ZERO)));
    drop(server);
    (events, session)
}

fn first_response(events: &[SessionEvent]) -> ResponseId {
    events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ResponseCreated(created) => Some(created.response_id),
            _ => None,
        })
        .expect("a response was created")
}

fn values(events: &[SessionEvent], response: ResponseId, kind: QuantityKind) -> Vec<Option<i64>> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(record) if record.response_id == Some(response) => {
                Some(record)
            }
            _ => None,
        })
        .flat_map(|record| record.quantities.iter())
        .filter(|quantity| quantity.kind() == kind)
        .map(|quantity| quantity.value())
        .collect()
}

/// Reads until the session fails or `bound` passes; returns the events, and the time from the
/// first transcript delta to the failure.
fn until_failed(
    session: &mut OpenAiLiveSession,
    bound: Duration,
) -> (Vec<SessionEvent>, Option<Duration>) {
    let start = Instant::now();
    let mut spoke = None;
    let mut events = Vec::new();
    while start.elapsed() < bound {
        let Some(event) = session.next_event(WAIT) else {
            continue;
        };
        if spoke.is_none() && matches!(event, SessionEvent::OutputTranscriptDelta(_)) {
            spoke = Some(Instant::now());
        }
        let failed = matches!(event, SessionEvent::SpeechSessionFailed(_));
        events.push(event);
        if failed {
            return (events, spoke.map(|spoke| spoke.elapsed()));
        }
    }
    (events, None)
}

/// A usage object carrying neither `input_tokens` nor `output_tokens` (empty, and with only a
/// total and cached tokens) is read in part twice: each frame counts once, and both directions'
/// totals of the response are unknown.
#[test]
fn a_usage_with_neither_count_counts_once_and_leaves_both_totals_unknown() {
    let (events, session) = run(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        send(&mut socket, completed_with(json!({}))).await;
        send(
            &mut socket,
            completed_with(
                json!({"total_tokens": 9, "input_tokens_details": {"cached_tokens": 1}}),
            ),
        )
        .await;
        send(
            &mut socket,
            completed_with(json!({"input_tokens": 4, "output_tokens": 2})),
        )
        .await;
        closed(&mut socket).await;
    });
    let response = first_response(&events);
    assert_eq!(
        session.ledger().unreadable_frames(),
        2,
        "two usages without either count, each read in part: {events:#?}"
    );
    assert_eq!(
        values(&events, response, QuantityKind::InputTextTokens),
        vec![None],
        "{events:#?}"
    );
    assert_eq!(
        values(&events, response, QuantityKind::OutputTextTokens),
        vec![None],
        "{events:#?}"
    );
}

/// The input rule mirrors the output rule: an input count left out keeps the response's input
/// total unknown after a later known one, and the output counts, all readable, sum to 8.
#[test]
fn an_input_count_left_out_keeps_the_input_sum_unknown_and_the_output_sums() {
    let (events, session) = run(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        send(&mut socket, completed_with(json!({"output_tokens": 3}))).await;
        send(
            &mut socket,
            completed_with(json!({"input_tokens": 10, "output_tokens": 5, "total_tokens": 15})),
        )
        .await;
        closed(&mut socket).await;
    });
    let response = first_response(&events);
    assert_eq!(session.ledger().unreadable_frames(), 1, "{events:#?}");
    assert_eq!(
        values(&events, response, QuantityKind::InputTextTokens),
        vec![None],
        "a later known count made the input sum known again: {events:#?}"
    );
    assert_eq!(
        values(&events, response, QuantityKind::OutputTextTokens),
        vec![Some(8)],
        "{events:#?}"
    );
}

/// Between the close and `session.closed` the server sends a backend end without its `response`
/// and one with a whole usage: the first counts once as unreadable, the second counts its 4
/// input and 1 output tokens on the response.
#[test]
fn a_backend_end_without_its_response_while_closing_counts_once() {
    let (events, session) = run(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        let close = receive(&mut socket).await;
        assert_eq!(close["type"], "session.close", "{close}");
        send(&mut socket, completed_without_response()).await;
        send(
            &mut socket,
            completed_with(json!({"input_tokens": 4, "output_tokens": 1})),
        )
        .await;
        send(&mut socket, session_closed()).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let response = first_response(&events);
    assert_eq!(session.ledger().unreadable_frames(), 1, "{events:#?}");
    assert_eq!(
        values(&events, response, QuantityKind::InputTextTokens),
        vec![Some(4)],
        "{events:#?}"
    );
    assert_eq!(
        values(&events, response, QuantityKind::OutputTextTokens),
        vec![Some(1)],
        "{events:#?}"
    );
}

/// A backend usage read in part, then silence while the response is in progress: the frame was
/// counted after what it recorded, so the stall that follows is a provider error naming it.
#[test]
fn a_stall_after_a_usage_read_in_part_is_a_provider_error() {
    let server = Server::start(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        send(&mut socket, completed_with(json!({"input_tokens": 3}))).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model
        .open(config(Duration::from_millis(500)))
        .expect("the session opens");
    let (events, _) = until_failed(&mut session, Duration::from_secs(5));
    let Some(SessionEvent::SpeechSessionFailed(failed)) = events.last() else {
        panic!("the session did not fail: {events:#?}");
    };
    assert_eq!(
        failed.failure.kind,
        SessionFailureKind::ProviderError,
        "{failed:?}"
    );
    assert_eq!(session.ledger().unreadable_frames(), 1, "{events:#?}");
}

/// The module documentation: a backend response end whose `response` was read only in part is
/// counted, "and none refreshes the liveness bound". The server speaks once, then sends only
/// backend ends without their `response`, one every 150 ms for 3 s, while the response is in
/// progress and the bound is 500 ms. Read as documented, nothing readable arrived after the
/// transcript, so the session fails within about 500 ms of it. 7ec1aa4 records every
/// `LiveEvent` as heard before counting it, so each such frame restarts the bound and the
/// session fails only after the frames stop.
#[test]
fn backend_ends_without_their_response_do_not_refresh_the_liveness_bound() {
    let server = Server::start(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(150)).await;
            if socket
                .send(Message::text(completed_without_response().to_string()))
                .await
                .is_err()
            {
                return;
            }
        }
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model
        .open(config(Duration::from_millis(500)))
        .expect("the session opens");
    let (events, after) = until_failed(&mut session, Duration::from_secs(8));
    let Some(SessionEvent::SpeechSessionFailed(failed)) = events.last() else {
        panic!("the session did not fail: {events:#?}");
    };
    let after = after.expect("the server spoke");
    assert!(
        after < Duration::from_millis(1_600),
        "frames the binding could not read kept the session alive for {after:?} past a 500 ms \
         bound; {} were counted; failure: {failed:?}",
        session.ledger().unreadable_frames()
    );
}

/// Two backend usages whose input counts sum past `u64::MAX`. The ledger clamps a count past
/// `i64::MAX` (`inference_duplex::reported`), so a large count is anticipated; the sum on the
/// response is not checked, and in a debug build the addition panics inside `next_event`. The
/// session should go on and close, its input total saturated or unknown.
#[test]
fn input_counts_summing_past_u64_max_do_not_panic() {
    let (events, _session) = run(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        send(
            &mut socket,
            completed_with(json!({"input_tokens": u64::MAX, "output_tokens": 0})),
        )
        .await;
        send(
            &mut socket,
            completed_with(json!({"input_tokens": 1, "output_tokens": 0})),
        )
        .await;
        closed(&mut socket).await;
    });
    let response = first_response(&events);
    let input = values(&events, response, QuantityKind::InputTextTokens);
    assert!(
        input == vec![Some(i64::MAX)] || input == vec![None],
        "{input:?}"
    );
}
