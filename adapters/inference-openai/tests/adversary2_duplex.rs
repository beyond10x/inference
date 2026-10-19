//! Adversary cases, pass 2 (final review): the Realtime binding through the duplex contract,
//! against a local WebSocket server replaying the provider. No network and no key.
//!
//! Replayed server events follow the published Realtime reference: a client
//! `input_audio_buffer.commit` "will not create a response from the model"; the
//! `conversation.item.input_audio_transcription.completed` event carries a required `usage`.
#![cfg(feature = "duplex")]

use std::num::{NonZeroU16, NonZeroU32};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_duplex::conformance::native_duplex_evidence;
use inference_duplex::conformance::{NativeDuplexProbe, NoNativeDuplex};
use inference_duplex::{
    NativeSpeechModel, ResponseUsage, SessionConfig, SessionEvent, SessionFailureKind,
    SpeechSessionHandle, TokenCounts, TranscriptionUsage, TurnDetection, TurnTuning,
};
use inference_openai::duplex::{OpenAiRealtime, native_duplex_verdict};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "adversary2-test-credential";
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
    runtime: Runtime,
    task: JoinHandle<()>,
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
        let endpoint = Endpoint::new("gpt-realtime-2.1")
            .with_url(format!("ws://{address}/v1/realtime"))
            .with_read_timeout(Duration::from_secs(5));
        Self {
            endpoint,
            runtime,
            task,
        }
    }

    fn model(&self) -> OpenAiRealtime {
        OpenAiRealtime::new(
            self.endpoint.clone(),
            Box::new(FixedSource),
            OPENAI_KEY_NAME,
        )
    }

    fn finish(self) {
        self.runtime.block_on(self.task).unwrap();
    }
}

async fn send(socket: &mut Socket, event: Value) {
    socket.send(Message::text(event.to_string())).await.unwrap();
}

/// The next client event, `None` when the binding sends nothing within `within`.
async fn receive_within(socket: &mut Socket, within: Duration) -> Option<Value> {
    loop {
        let message = tokio::time::timeout(within, socket.next())
            .await
            .ok()??
            .ok()?;
        match message {
            Message::Text(text) => {
                let mut event: Value = serde_json::from_str(text.as_str()).unwrap();
                if let Some(object) = event.as_object_mut() {
                    object.remove("event_id");
                }
                return Some(event);
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => return None,
        }
    }
}

async fn receive(socket: &mut Socket) -> Value {
    receive_within(socket, Duration::from_secs(5))
        .await
        .expect("the binding sent nothing for five seconds")
}

async fn accept_session(socket: &mut Socket) -> Value {
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
    update["session"].clone()
}

fn response_created(id: &str) -> Value {
    json!({"type": "response.created", "event_id": "event_rc", "response": {"id": id, "object": "realtime.response", "status": "in_progress", "output": []}})
}

fn audio_delta(response: &str, item: &str, audio: &[u8]) -> Value {
    json!({"type": "response.output_audio.delta", "event_id": "event_ad", "response_id": response, "item_id": item, "output_index": 0, "content_index": 0, "delta": STANDARD.encode(audio)})
}

fn response_done(id: &str, status: &str, usage: Value) -> Value {
    json!({"type": "response.done", "event_id": "event_rd", "response": {
        "id": id, "object": "realtime.response", "status": status, "output": [], "usage": usage
    }})
}

fn committed(item: &str) -> Value {
    json!({"type": "input_audio_buffer.committed", "event_id": "event_cm", "previous_item_id": null, "item_id": item})
}

fn format(encoding: AudioEncoding, rate: u32) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn pcm24() -> AudioFormat {
    format(AudioEncoding::PcmS16le, 24_000)
}

fn mulaw() -> AudioFormat {
    format(AudioEncoding::PcmMulaw, 8_000)
}

fn frame(format: AudioFormat, position: u64, data: Vec<u8>) -> AudioFrame {
    let sample_count = u32::try_from(data.len() as u64 / format.encoding.width()).unwrap();
    AudioFrame {
        format,
        position: SampleClockPosition::new(position).unwrap(),
        sample_count,
        data,
    }
}

fn config(turn_detection: TurnDetection, auto: bool) -> SessionConfig {
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm24(),
        output_format: pcm24(),
        turn_detection,
        turn_tuning: TurnTuning::default(),
        auto_response: auto,
        auto_interrupt: false,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

fn until(
    session: &mut impl SpeechSessionHandle,
    waits: usize,
    stop: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let mut idle = 0;
    while idle < waits {
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

fn stalled(events: &[SessionEvent]) -> bool {
    events.iter().any(|event| {
        matches!(event, SessionEvent::SpeechSessionFailed(failed)
            if failed.failure.kind == SessionFailureKind::Stalled)
    })
}

/// Under server turn detection with `auto_response` on, the ledger takes a caller commit as a
/// request for a response and runs the liveness bound. The API creates no response for a client
/// commit, so the binding has to ask for one; otherwise the session fails as stalled although
/// the provider is healthy. The testkit mock answers the same call with a response.
#[test]
fn a_caller_commit_under_server_vad_with_auto_response_gets_a_response() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            receive(&mut socket).await["type"],
            "input_audio_buffer.append"
        );
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "input_audio_buffer.commit"})
        );
        send(&mut socket, committed("item_1")).await;
        let asked = receive_within(&mut socket, Duration::from_millis(1_500)).await;
        assert_eq!(
            asked,
            Some(json!({"type": "response.create"})),
            "a client commit creates no response; the binding must send response.create"
        );
        send(&mut socket, response_created("resp_1")).await;
        send(
            &mut socket,
            response_done("resp_1", "completed", Value::Null),
        )
        .await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut vad = config(TurnDetection::ServerVad, true);
    vad.idle_limit = Duration::from_millis(600);
    let mut session = model.open(vad).expect("the session opens");
    session
        .append_audio(frame(pcm24(), 0, vec![1; 4_800]))
        .expect("PCM16 at 24 kHz");
    session
        .commit_input()
        .expect("server VAD takes a manual commit");
    let events = until(&mut session, 20, |event| {
        matches!(
            event,
            SessionEvent::ResponseCompleted(_) | SessionEvent::SpeechSessionFailed(_)
        )
    });
    let _ = session.close();
    let script = catch_unwind(AssertUnwindSafe(|| server.finish()));
    assert!(
        !stalled(&events),
        "a healthy provider, the session failed as stalled: {events:#?}"
    );
    assert!(script.is_ok(), "the replay saw no response.create");
}

/// A caller commit the server refuses (`error`, an empty buffer) is never acknowledged. The
/// binding still counts it as owed an acknowledgement, so the next commit the server's own turn
/// detection makes is taken for that acknowledgement and no `InputCommitted` is recorded.
#[test]
fn a_refused_caller_commit_does_not_swallow_the_next_server_commit() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "input_audio_buffer.commit"})
        );
        send(
            &mut socket,
            json!({"type": "error", "event_id": "event_err", "error": {
                "type": "invalid_request_error", "code": "input_audio_buffer_commit_empty",
                "message": "Error committing input audio buffer: buffer too small.",
                "param": null, "event_id": null
            }}),
        )
        .await;
        send(
            &mut socket,
            json!({"type": "input_audio_buffer.speech_started", "event_id": "event_ss", "audio_start_ms": 100, "item_id": "item_2"}),
        )
        .await;
        send(
            &mut socket,
            json!({"type": "input_audio_buffer.speech_stopped", "event_id": "event_sp", "audio_end_ms": 900, "item_id": "item_2"}),
        )
        .await;
        send(&mut socket, committed("item_2")).await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut session = model
        .open(config(TurnDetection::ServerVad, false))
        .expect("the session opens");
    session.commit_input().expect("a manual commit");
    let events = until(&mut session, 10, |event| {
        matches!(event, SessionEvent::SpeechStopped(_))
    });
    let rest = until(&mut session, 5, |_| false);
    let _ = session.close();
    server.finish();
    assert_eq!(session.server_errors().len(), 1, "{events:#?}");
    let commits = events
        .iter()
        .chain(rest.iter())
        .filter(|event| matches!(event, SessionEvent::InputCommitted(_)))
        .count();
    assert_eq!(
        commits, 2,
        "the caller's commit and the server's turn-end commit: {events:#?} {rest:#?}"
    );
}

/// The reference's `conversation.item.input_audio_transcription.completed` carries a required
/// `usage` (tokens of the transcription model, billed apart); the contract records it as
/// `TranscriptionUsageRecorded`, as the testkit mock does.
#[test]
fn the_input_transcription_usage_is_recorded() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(
            &mut socket,
            json!({
                "type": "conversation.item.input_audio_transcription.completed",
                "event_id": "event_CCXGRvtUVrax5SJAnNOWZ",
                "item_id": "item_CCXGQ4e1ht4cOraEYcuR2",
                "content_index": 0,
                "transcript": "Hey, can you hear me?",
                "usage": {
                    "type": "tokens", "total_tokens": 22, "input_tokens": 13,
                    "input_token_details": {"text_tokens": 0, "audio_tokens": 13},
                    "output_tokens": 9
                }
            }),
        )
        .await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut session = model
        .open(config(TurnDetection::ServerVad, false))
        .expect("the session opens");
    let events = until(&mut session, 5, |_| false);
    let _ = session.close();
    server.finish();
    assert!(
        events.iter().any(|event| matches!(event, SessionEvent::InputTranscribed(heard) if heard.text == "Hey, can you hear me?")),
        "{events:#?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::TranscriptionUsageRecorded(recorded)
                if recorded.usage == TranscriptionUsage { input_tokens: 13, output_tokens: 9 }
        )),
        "the transcription usage the server reported is dropped: {events:#?}"
    );
}

/// Truncation converts the heard sample at the output rate: 80 mu-law samples at 8 kHz are
/// 10 ms, with PCM16 at 24 kHz going in. Guards a mutant that converts at the input rate.
#[test]
fn truncation_converts_the_heard_sample_at_the_output_rate() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create"})
        );
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, audio_delta("resp_1", "item_o1", &[0x7F; 160])).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.cancel", "response_id": "resp_1"})
        );
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "conversation.item.truncate", "item_id": "item_o1", "content_index": 0, "audio_end_ms": 10})
        );
        send(
            &mut socket,
            response_done("resp_1", "cancelled", Value::Null),
        )
        .await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut mixed = config(TurnDetection::Disabled, false);
    mixed.output_format = mulaw();
    let mut session = model.open(mixed).expect("the session opens");
    session.create_response().expect("none in progress");
    let events = until(&mut session, 20, |event| {
        matches!(event, SessionEvent::OutputAudio(_))
    });
    let response = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ResponseCreated(created) => Some(created.response_id),
            _ => None,
        })
        .expect("a response");
    session.cancel_response(response).expect("in progress");
    session
        .truncate_output(response, SampleClockPosition::new(80).unwrap())
        .expect("within the emitted audio");
    let _ = until(&mut session, 5, |_| false);
    let _ = session.close();
    server.finish();
}

/// Speech positions count input samples: 1000 ms of mu-law at 8 kHz is sample 8000, with PCM16
/// at 24 kHz coming out. Guards a mutant that converts at the output rate.
#[test]
fn speech_positions_count_at_the_input_rate() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(
            &mut socket,
            json!({"type": "input_audio_buffer.speech_started", "event_id": "event_ss", "audio_start_ms": 1000, "item_id": "item_1"}),
        )
        .await;
        send(
            &mut socket,
            json!({"type": "input_audio_buffer.speech_stopped", "event_id": "event_sp", "audio_end_ms": 1500, "item_id": "item_1"}),
        )
        .await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut mixed = config(TurnDetection::ServerVad, false);
    mixed.input_format = mulaw();
    let mut session = model.open(mixed).expect("the session opens");
    let events = until(&mut session, 10, |event| {
        matches!(event, SessionEvent::SpeechStopped(_))
    });
    let _ = session.close();
    server.finish();
    let positions: Vec<u64> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SpeechStarted(started) => Some(started.position.get()),
            SessionEvent::SpeechStopped(stopped) => Some(stopped.position.get()),
            _ => None,
        })
        .collect();
    assert_eq!(positions, vec![8_000, 12_000], "{events:#?}");
}

/// A `response.done` whose usage has no token details counts the rest of each total as text,
/// and `incomplete` fails the response. Guards mutants of the fallback and of the status map.
#[test]
fn usage_without_details_and_an_incomplete_response() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create"})
        );
        send(&mut socket, response_created("resp_1")).await;
        send(
            &mut socket,
            response_done(
                "resp_1",
                "incomplete",
                json!({"total_tokens": 30, "input_tokens": 20, "output_tokens": 10}),
            ),
        )
        .await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut session = model
        .open(config(TurnDetection::Disabled, false))
        .expect("the session opens");
    session.create_response().expect("none in progress");
    let events = until(&mut session, 20, |event| {
        matches!(
            event,
            SessionEvent::ResponseFailed(_) | SessionEvent::ResponseCompleted(_)
        )
    });
    let _ = session.close();
    server.finish();
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::ResponseUsageRecorded(recorded) if recorded.usage == ResponseUsage {
                input: TokenCounts { text_tokens: 20, audio_tokens: 0, cached_tokens: 0 },
                output: TokenCounts { text_tokens: 10, audio_tokens: 0, cached_tokens: 0 },
            }
        )),
        "{events:#?}"
    );
    assert!(
        matches!(events.last(), Some(SessionEvent::ResponseFailed(failed)) if failed.failure.kind == SessionFailureKind::ProviderError),
        "{events:#?}"
    );
}

/// The session owns a Tokio runtime. A session opened on a blocking thread and dropped in async
/// code (the pattern the module doc points an async caller to) panics in the runtime's drop.
#[test]
fn dropping_a_session_inside_an_async_context_does_not_panic() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let session = model
        .open(config(TurnDetection::Disabled, false))
        .expect("the session opens");
    let caller = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dropped = catch_unwind(AssertUnwindSafe(|| {
        caller.block_on(async move {
            drop(session);
        });
    }));
    server.finish();
    assert!(
        dropped.is_ok(),
        "dropping the session inside an async task panicked"
    );
}

/// The oracle of the ignored live test `native_duplex_evidence_of_the_hosted_model`, applied to
/// a model whose session never opens (the handshake is refused, as a revoked key or no network
/// refuses it). The oracle `claims().contains(NativeDuplex) == outcome.is_ok()` holds, so the
/// live test passes without any exchange with the hosted model.
#[test]
fn the_live_evidence_oracle_refuses_a_session_that_never_opened() {
    let mut unreachable = OpenAiRealtime::new(
        Endpoint::new("gpt-realtime-2.1")
            .with_url("ws://127.0.0.1:9/v1/realtime")
            .with_read_timeout(Duration::from_secs(1)),
        Box::new(FixedSource),
        OPENAI_KEY_NAME,
    );
    let mut probe_config = config(TurnDetection::ServerVad, true);
    probe_config.idle_limit = Duration::from_secs(30);
    let probe = NativeDuplexProbe {
        config: probe_config,
        opening: vec![frame(pcm24(), 0, vec![0; 480])],
        during: vec![frame(pcm24(), 240, vec![0; 480])],
        marker: "portugal".to_owned(),
        wait: Duration::from_millis(500),
        patience: 60,
    };
    let outcome = native_duplex_evidence(&mut unreachable, &probe);
    assert!(
        matches!(outcome, Err(NoNativeDuplex::Open(_))),
        "{outcome:?}"
    );
    let live_oracle_passes = native_duplex_verdict(&outcome) == Ok(true);
    assert!(
        !live_oracle_passes,
        "the live test's assertion passes on a session that never opened: {outcome:?}"
    );
}
