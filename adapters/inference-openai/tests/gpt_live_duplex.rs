//! The GPT-Live binding to the duplex contract, offline: a local WebSocket server replays the
//! provider. No network and no key: the credential is a fixed test value.
//!
//! The replays follow what the Live primary WebSocket reference documents and what two live
//! sessions on 2026-10-20 showed: one 100 ms output frame for each 100 ms input frame, silent
//! (peak below 50) whenever the model does not speak, and output transcript fragments arriving
//! just before the audio they transcribe, their intervals on the output audio's timeline.

#![cfg(all(feature = "gpt-live", feature = "duplex"))]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_core::model::Capability;
use inference_duplex::conformance::{NativeDuplexProbe, NoNativeDuplex, native_duplex_evidence};
use inference_duplex::{
    NativeSpeechModel, OpenError, Quantity, QuantityKind, ResponseId, ResponseUsage, SessionConfig,
    SessionError, SessionEvent, SessionFailureKind, SpeechSessionHandle, TokenCounts, TuningOption,
    TuningSupport, TurnDetection, TurnSensitivity, TurnTuning, Unsupported, UsageRecorded,
};
use inference_openai::live::{LIVE_MODEL, LiveClosed, LiveFormat};
use inference_openai::live_duplex::{
    DEFAULT_BACKEND_MODEL, OpenAiLive, Segmentation, capabilities, claims, live_format,
    native_duplex_verdict,
};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
};
use inference_tools::{DeclareTool, SideEffectClass, Tool};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const WAIT: Duration = Duration::from_millis(100);

/// The samples of one 100 ms frame at 24 kHz.
const FRAME: u32 = 2_400;

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

/// A local server running one script on its own runtime, apart from the binding's.
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
        let endpoint = Endpoint::live(LIVE_MODEL)
            .with_url(format!("ws://{address}/v1/live/sessions"))
            .with_read_timeout(Duration::from_secs(5));
        Self {
            endpoint,
            runtime,
            task,
        }
    }

    fn model(&self) -> OpenAiLive {
        OpenAiLive::new(
            self.endpoint.clone(),
            Box::new(FixedSource),
            OPENAI_KEY_NAME,
        )
    }

    /// Waits for the script, failing the test with the script's own assertion failure.
    fn finish(self) {
        self.runtime.block_on(self.task).unwrap();
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
            Message::Text(text) => {
                let mut event: Value = serde_json::from_str(text.as_str()).unwrap();
                if let Some(object) = event.as_object_mut() {
                    object.remove("event_id");
                }
                return event;
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}

/// Startup: reads `session.start`, answers `session.started`; returns the `session` sent.
async fn accept_start(socket: &mut Socket) -> Value {
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
    start["session"].clone()
}

/// Reads `count` audio appends.
async fn appends(socket: &mut Socket, count: usize) {
    for _ in 0..count {
        let append = receive(socket).await;
        assert_eq!(append["type"], "session.input_audio.append", "{append}");
    }
}

/// A 100 ms output frame of PCM16 at 24 kHz, every sample `level`.
fn pcm_frame(level: i16) -> Vec<u8> {
    level
        .to_le_bytes()
        .iter()
        .copied()
        .cycle()
        .take(FRAME as usize * 2)
        .collect()
}

fn audio(bytes: &[u8]) -> Value {
    json!({"type": "session.output_audio.delta", "delta": STANDARD.encode(bytes)})
}

/// The silence the model sends while it does not speak.
fn silent() -> Value {
    audio(&pcm_frame(12))
}

fn voiced() -> Value {
    audio(&pcm_frame(3_000))
}

fn said(delta: &str, start_ms: u64) -> Value {
    json!({"type": "session.output_transcript.delta", "event_id": "evt_out", "delta": delta, "start_ms": start_ms, "end_ms": start_ms + 200})
}

fn heard(delta: &str, start_ms: u64) -> Value {
    json!({"type": "session.input_transcript.delta", "event_id": "evt_in", "delta": delta, "start_ms": start_ms, "end_ms": start_ms + 200})
}

fn backend(delegation: &str, event: Value) -> Value {
    json!({"type": "response.event", "event_id": "evt_backend", "delegation_id": delegation, "event": event})
}

fn backend_completed(response: &str, input: u64, cached: u64, output: u64) -> Value {
    json!({"type": "response.completed", "sequence_number": 9, "response": {
        "id": response, "status": "completed", "output": [],
        "usage": {"input_tokens": input, "input_tokens_details": {"cached_tokens": cached}, "output_tokens": output, "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": input + output}
    }})
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

fn frame(format: AudioFormat, position: u64, data: Vec<u8>) -> AudioFrame {
    let sample_count = u32::try_from(data.len() as u64 / format.encoding.width()).unwrap();
    AudioFrame {
        format,
        position: SampleClockPosition::new(position).unwrap(),
        sample_count,
        data,
    }
}

fn input(position: u64) -> AudioFrame {
    frame(pcm24(), position, vec![0; FRAME as usize * 2])
}

fn weather() -> Tool {
    Tool::declare(DeclareTool {
        name: "weather".to_owned(),
        schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

/// What GPT-Live takes: its own turn-taking, interrupting itself, creating every response.
fn config() -> SessionConfig {
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm24(),
        output_format: pcm24(),
        turn_detection: TurnDetection::SemanticVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

/// Events until one `stop` accepts, or until twenty waits in a row deliver nothing.
fn until(
    session: &mut impl SpeechSessionHandle,
    stop: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let mut idle = 0;
    while idle < 20 {
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

/// Events until `count` responses completed.
fn completed(session: &mut impl SpeechSessionHandle, count: usize) -> Vec<SessionEvent> {
    let seen = std::cell::Cell::new(0);
    until(session, |event| {
        if matches!(event, SessionEvent::ResponseCompleted(_)) {
            seen.set(seen.get() + 1);
        }
        seen.get() == count
    })
}

fn created(events: &[SessionEvent]) -> Vec<ResponseId> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ResponseCreated(created) => Some(created.response_id),
            _ => None,
        })
        .collect()
}

fn transcript(events: &[SessionEvent], response_id: ResponseId) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputTranscriptDelta(delta) if delta.response_id == response_id => {
                Some(delta.text.as_str())
            }
            _ => None,
        })
        .collect()
}

/// The output frames of a response: position, sample count, whether any sample is loud.
fn frames(events: &[SessionEvent], response_id: ResponseId) -> Vec<(u64, u32, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputAudio(output) if output.response_id == response_id => Some((
                output.frame.position.get(),
                output.frame.sample_count,
                output
                    .frame
                    .data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .any(|sample| i16::from_le_bytes(*sample).unsigned_abs() >= 1_000),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn the_declared_capabilities_are_what_gpt_live_takes() {
    let declared = capabilities();
    assert_eq!(
        declared.input_formats,
        vec![
            pcm24(),
            format(AudioEncoding::PcmS16le, 16_000),
            format(AudioEncoding::PcmMulaw, 8_000),
            format(AudioEncoding::PcmAlaw, 8_000),
        ]
    );
    assert_eq!(declared.output_formats, declared.input_formats);
    assert_eq!(declared.turn_detection, vec![TurnDetection::SemanticVad]);
    assert_eq!(declared.turn_tuning, TuningSupport::NONE);
    assert!(!declared.manual_commit && !declared.manual_response && !declared.manual_interrupt);
    assert!(!declared.client_cancel && !declared.truncation);
    assert!(declared.tool_calling);
    let claimed = claims();
    for capability in [
        Capability::OutputStreaming,
        Capability::IncrementalInput,
        Capability::ConcurrentInputOutput,
        Capability::NativeDuplex,
        Capability::ToolCalling,
        Capability::Interruptible,
    ] {
        assert!(claimed.contains(&capability), "{capability:?}");
    }
    assert_eq!(live_format(&pcm24()), Some(LiveFormat::Pcm24k));
    assert_eq!(
        live_format(&format(AudioEncoding::PcmS16le, 16_000)),
        Some(LiveFormat::Pcm16k)
    );
    assert_eq!(
        live_format(&format(AudioEncoding::PcmAlaw, 8_000)),
        Some(LiveFormat::Pcma)
    );
    assert_eq!(live_format(&format(AudioEncoding::PcmS16le, 48_000)), None);
    assert_eq!(Segmentation::default().gap, Duration::from_millis(800));
}

#[test]
fn a_configuration_maps_onto_session_start_with_tools_on_the_responses_backend() {
    let model = OpenAiLive::new(
        Endpoint::live(LIVE_MODEL),
        Box::new(FixedSource),
        OPENAI_KEY_NAME,
    );
    let mut telephone = config();
    telephone.input_format = format(AudioEncoding::PcmMulaw, 8_000);
    telephone.output_format = telephone.input_format;
    telephone.tools = vec![weather()];
    let session = model
        .client_config(&telephone)
        .expect("mu-law both ways, the provider's own turn-taking")
        .to_session_json(LIVE_MODEL);
    assert_eq!(
        session,
        json!({
            "model": "gpt-live-1",
            "instructions": "answer briefly",
            "audio": {"format": {"type": "audio/pcmu", "rate": 8000}, "output": {"voice": "marin"}},
            "delegation": {"type": "responses", "responses": {
                "model": DEFAULT_BACKEND_MODEL,
                "tools": [{"type": "function", "name": "weather", "description": "", "parameters": weather().schema().clone(), "strict": false}],
                "tool_choice": "auto"
            }}
        })
    );
    let plain = model
        .client_config(&config())
        .expect("no tools")
        .to_session_json(LIVE_MODEL);
    assert_eq!(
        plain["delegation"],
        json!({"type": "responses", "responses": {"model": DEFAULT_BACKEND_MODEL}})
    );
}

#[test]
fn what_gpt_live_does_not_take_is_refused_before_connecting() {
    let nowhere = Endpoint::live(LIVE_MODEL).with_url("ws://127.0.0.1:1/v1/live/sessions");
    let mut model = OpenAiLive::new(nowhere, Box::new(FixedSource), OPENAI_KEY_NAME);
    let mut mixed = config();
    mixed.output_format = format(AudioEncoding::PcmMulaw, 8_000);
    let mut wide = config();
    wide.input_format = format(AudioEncoding::PcmS16le, 48_000);
    wide.output_format = wide.input_format;
    let mut vad = config();
    vad.turn_detection = TurnDetection::ServerVad;
    let mut manual = config();
    manual.turn_detection = TurnDetection::Disabled;
    let mut caller_responds = config();
    caller_responds.auto_response = false;
    let mut caller_interrupts = config();
    caller_interrupts.auto_interrupt = false;
    let mut tuned = config();
    tuned.turn_tuning.sensitivity = Some(TurnSensitivity::High);
    for (config, refusal) in [
        (
            mixed,
            Unsupported::OutputFormat(format(AudioEncoding::PcmMulaw, 8_000)),
        ),
        (
            wide,
            Unsupported::InputFormat(format(AudioEncoding::PcmS16le, 48_000)),
        ),
        (vad, Unsupported::TurnDetection(TurnDetection::ServerVad)),
        (manual, Unsupported::TurnDetection(TurnDetection::Disabled)),
        (caller_responds, Unsupported::ManualResponse),
        (caller_interrupts, Unsupported::ManualInterrupt),
        (tuned, Unsupported::TurnTuning(TuningOption::Sensitivity)),
    ] {
        assert_eq!(
            model.open(config).err(),
            Some(OpenError::Unsupported(refusal))
        );
    }
}

#[test]
fn spoken_output_is_one_response_from_the_first_voiced_frame_to_a_silence_gap() {
    let server = Server::start(|mut socket| async move {
        accept_start(&mut socket).await;
        appends(&mut socket, 3).await;
        send(&mut socket, silent()).await;
        send(&mut socket, silent()).await;
        send(&mut socket, heard("What is the capital of France", 600)).await;
        send(&mut socket, said(" Paris.", 200)).await;
        send(&mut socket, voiced()).await;
        send(&mut socket, silent()).await;
        send(&mut socket, voiced()).await;
        for _ in 0..10 {
            send(&mut socket, silent()).await;
        }
        send(&mut socket, said(" Anything else?", 1_500)).await;
        send(&mut socket, voiced()).await;
        for _ in 0..8 {
            send(&mut socket, silent()).await;
        }
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    for at in 0..3 {
        session
            .append_audio(input(at * u64::from(FRAME)))
            .expect("PCM16 at 24 kHz");
    }
    let events = completed(&mut session, 2);
    session.close().expect("open");
    server.finish();

    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:?}");
    assert_ne!(ids[0], ids[1]);
    assert_eq!(transcript(&events, ids[0]), " Paris.");
    assert_eq!(transcript(&events, ids[1]), " Anything else?");
    let first = frames(&events, ids[0]);
    assert_eq!(
        first.len(),
        3 + 8,
        "the voiced run, its short pause, then a gap of 800 ms"
    );
    assert_eq!(
        first[0],
        (0, FRAME, true),
        "leading silence is no part of a response"
    );
    assert_eq!(first[1], (u64::from(FRAME), FRAME, false));
    assert_eq!(first[2], (2 * u64::from(FRAME), FRAME, true));
    assert_eq!(first[10].0, 10 * u64::from(FRAME));
    assert_eq!(frames(&events, ids[1]).len(), 1 + 8);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::InputTranscribed(heard) if heard.text == "What is the capital of France"
    )));
    let completions: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ResponseCompleted(done) => Some(done.response_id),
            _ => None,
        })
        .collect();
    assert_eq!(completions, ids);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ResponseUsageRecorded(_))),
        "a session with no backend work records no usage"
    );
}

#[test]
fn operations_the_api_lacks_are_refused_and_nothing_is_sent() {
    let server = Server::start(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hello.", 0)).await;
        send(&mut socket, voiced()).await;
        let next = receive(&mut socket).await;
        assert_eq!(
            next["type"], "session.input_audio.append",
            "a refused operation sent {next}"
        );
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputAudio(_))
    });
    let response = created(&events)[0];
    assert_eq!(
        session.commit_input(),
        Err(SessionError::Unsupported(Unsupported::ManualCommit))
    );
    assert_eq!(
        session.create_response(),
        Err(SessionError::Unsupported(Unsupported::ManualResponse))
    );
    assert_eq!(
        session.cancel_response(response),
        Err(SessionError::Unsupported(Unsupported::ClientCancel))
    );
    assert_eq!(
        session.truncate_output(response, SampleClockPosition::new(0).unwrap()),
        Err(SessionError::Unsupported(Unsupported::Truncation))
    );
    session.append_audio(input(0)).expect("input is taken");
    session.close().expect("open");
    server.finish();
}

#[test]
fn a_function_call_of_the_responses_backend_is_proposed_answered_and_its_usage_recorded() {
    let server = Server::start(|mut socket| async move {
        let session = accept_start(&mut socket).await;
        assert_eq!(
            session["delegation"]["responses"]["tools"][0]["name"],
            "weather"
        );
        send(&mut socket, heard("Weather in Lisbon?", 400)).await;
        send(
            &mut socket,
            json!({"type": "session.delegation.created", "event_id": "evt_d", "offset_ms": 900, "delegation": {"id": "del_1", "type": "delegation", "target": "responses", "response_id": "resp_1"}}),
        )
        .await;
        send(
            &mut socket,
            backend("del_1", json!({"type": "response.created", "sequence_number": 0, "response": {"id": "resp_1", "status": "in_progress", "output": []}})),
        )
        .await;
        send(
            &mut socket,
            backend("del_1", json!({"type": "response.output_item.done", "sequence_number": 4, "output_index": 0, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "weather", "arguments": "{\"city\":\"Lisbon\"}", "status": "completed"}})),
        )
        .await;
        send(
            &mut socket,
            backend("del_1", backend_completed("resp_1", 120, 64, 30)),
        )
        .await;
        let output = receive(&mut socket).await;
        assert_eq!(
            output,
            json!({"type": "response.item.create", "item": {"type": "function_call_output", "call_id": "call_1", "output": "{\"sky\":\"clear\"}"}})
        );
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create"})
        );
        send(
            &mut socket,
            backend("del_1", backend_completed("resp_2", 200, 0, 20)),
        )
        .await;
        for _ in 0..8 {
            send(&mut socket, silent()).await;
        }
        send(&mut socket, said(" It is clear in Lisbon.", 2_000)).await;
        send(&mut socket, voiced()).await;
        for _ in 0..8 {
            send(&mut socket, silent()).await;
        }
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut with_tool = config();
    let tool = weather();
    with_tool.tools = vec![tool.clone()];
    let mut session = model.open(with_tool).expect("the session opens");
    let proposed = until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseUsageRecorded(_))
    });
    let call = proposed
        .iter()
        .find_map(|event| match event {
            SessionEvent::SpeechToolCallProposed(call) => Some(call.clone()),
            _ => None,
        })
        .expect("a proposed call");
    assert_eq!(call.call_id.as_str(), "call_1");
    assert_eq!(call.tool_id, tool.tool_id());
    assert_eq!(call.arguments, json!({"city": "Lisbon"}));
    assert_eq!(created(&proposed), vec![call.response_id]);
    let backend_usage = ResponseUsage {
        input: TokenCounts {
            text_tokens: 120,
            audio_tokens: 0,
            cached_tokens: 64,
        },
        output: TokenCounts {
            text_tokens: 30,
            audio_tokens: 0,
            cached_tokens: 0,
        },
    };
    assert_eq!(
        session.ledger().usage(call.response_id),
        Some(backend_usage)
    );

    session
        .submit_tool_result(&call.call_id, json!({"sky": "clear"}))
        .expect("a proposed call");
    let answered = completed(&mut session, 2);
    session.close().expect("open");
    server.finish();
    let summed = ResponseUsage {
        input: TokenCounts {
            text_tokens: 320,
            audio_tokens: 0,
            cached_tokens: 64,
        },
        output: TokenCounts {
            text_tokens: 50,
            audio_tokens: 0,
            cached_tokens: 0,
        },
    };
    assert_eq!(
        session.ledger().usage(call.response_id),
        Some(summed),
        "backend usage is summed per response"
    );
    let spoken = created(&answered);
    assert_eq!(spoken.len(), 1, "{answered:?}");
    assert_ne!(spoken[0], call.response_id);
    assert_eq!(transcript(&answered, spoken[0]), " It is clear in Lisbon.");
    assert_eq!(session.backend_usage().len(), 2);

    let created_at = answered
        .iter()
        .position(|event| matches!(event, SessionEvent::ResponseCreated(_)))
        .expect("the spoken response");
    assert!(
        matches!(
            &answered[created_at - 1],
            SessionEvent::UsageRecorded(record) if record.response_id == Some(call.response_id)
        ),
        "backend usage may follow a response's end, so the next response settles it: {answered:?}"
    );
    let recorded = records(&proposed)
        .into_iter()
        .chain(records(&answered))
        .filter(|record| record.response_id == Some(call.response_id))
        .collect::<Vec<_>>();
    assert_eq!(recorded.len(), 1, "one record per response");
    assert_eq!(
        recorded[0].quantities,
        vec![
            Quantity::provider_reported(QuantityKind::InputTextTokens, 320),
            Quantity::unknown(QuantityKind::InputAudioTokens),
            Quantity::provider_reported(QuantityKind::CachedInputTokens, 64),
            Quantity::provider_reported(QuantityKind::OutputTextTokens, 50),
            Quantity::unknown(QuantityKind::OutputAudioTokens),
            Quantity::measured(QuantityKind::Requests, 1),
        ],
        "the backend's tokens summed as text tokens; the voice model reports no audio tokens"
    );
}

#[test]
fn voice_seconds_are_the_running_total_and_close_confirms_the_final_usage() {
    let server = Server::start(|mut socket| async move {
        accept_start(&mut socket).await;
        send(
            &mut socket,
            json!({"type": "session.usage.updated", "event_id": "evt_u", "usage": {"seconds": 3.0}, "context_window": {"usage_ratio": 0.01}}),
        )
        .await;
        send(&mut socket, said(" Hi.", 0)).await;
        let close = receive(&mut socket).await;
        assert_eq!(close["type"], "session.close");
        send(
            &mut socket,
            json!({"type": "session.closed", "event_id": "evt_c", "reason": "close_requested", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 4.5}}),
        )
        .await;
        let _ = socket.close(None).await;
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    assert_eq!(session.voice_seconds(), None, "nothing reported yet");
    assert_eq!(session.session_id(), Some("live_123"));
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(_))
    });
    assert_eq!(session.voice_seconds(), Some(3.0));
    session.close().expect("open");
    server.finish();
    assert_eq!(
        session.final_usage(),
        Some(&LiveClosed {
            reason: "close_requested".to_owned(),
            seconds: 4.5
        })
    );
    assert_eq!(session.voice_seconds(), Some(4.5));
    let closing: Vec<_> = std::iter::from_fn(|| session.next_event(Duration::ZERO)).collect();
    let response = created(&events)[0];
    assert!(
        matches!(
            closing.as_slice(),
            [
                SessionEvent::UsageRecorded(of_response),
                SessionEvent::ResponseCancelled(_),
                SessionEvent::UsageRecorded(of_session),
                SessionEvent::SpeechSessionClosed(_),
            ] if of_response.response_id == Some(response) && of_session.response_id.is_none()
        ),
        "{closing:?}"
    );
    assert_eq!(created(&events).len(), 1);
    let recorded = records(&closing);
    assert!(
        recorded[0]
            .quantities
            .iter()
            .filter(|quantity| quantity.kind() != QuantityKind::Requests)
            .all(|quantity| quantity.value().is_none()),
        "a response without backend work reported no tokens: {:?}",
        recorded[0]
    );
    let of_session = recorded[1];
    assert_eq!(
        of_session.quantities,
        vec![Quantity::provider_reported(
            QuantityKind::InputAudioMs,
            4_500
        )],
        "the billed voice seconds that session.closed confirmed"
    );
    assert!(of_session.r#final);
    assert_eq!(
        of_session.speech_session_id,
        Some(session.ledger().session().session_id())
    );
}

/// A backend response that completes before the model has spoken has no response to count
/// toward; its tokens are billed all the same, so the session's end records them as usage of the
/// session, naming no response, before `SpeechSessionClosed`.
#[test]
fn backend_usage_before_any_response_is_recorded_for_the_session_at_its_end() {
    let server = Server::start(|mut socket| async move {
        accept_start(&mut socket).await;
        send(
            &mut socket,
            backend("del_1", backend_completed("resp_1", 12, 4, 3)),
        )
        .await;
        send(&mut socket, heard(" Bye.", 0)).await;
        let close = receive(&mut socket).await;
        assert_eq!(close["type"], "session.close");
        send(
            &mut socket,
            json!({"type": "session.closed", "event_id": "evt_c", "reason": "close_requested", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 1.5}}),
        )
        .await;
        let _ = socket.close(None).await;
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::InputTranscribed(_))
    });
    assert!(created(&events).is_empty(), "{events:?}");
    assert_eq!(session.backend_usage().len(), 1);
    session.close().expect("open");
    server.finish();
    let closing: Vec<_> = std::iter::from_fn(|| session.next_event(Duration::ZERO)).collect();
    assert!(
        matches!(
            closing.as_slice(),
            [
                SessionEvent::UsageRecorded(held),
                SessionEvent::UsageRecorded(seconds),
                SessionEvent::SpeechSessionClosed(_),
            ] if held.response_id.is_none() && seconds.response_id.is_none()
        ),
        "{closing:?}"
    );
    assert_eq!(
        records(&closing)[0].quantities,
        vec![
            Quantity::provider_reported(QuantityKind::InputTextTokens, 12),
            Quantity::provider_reported(QuantityKind::CachedInputTokens, 4),
            Quantity::provider_reported(QuantityKind::OutputTextTokens, 3),
        ]
    );
    assert_eq!(session.ledger().usage_records().len(), 2);
}

fn records(events: &[SessionEvent]) -> Vec<&UsageRecorded> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) => Some(recorded),
            _ => None,
        })
        .collect()
}

#[test]
fn a_session_the_server_ends_or_drops_fails() {
    let expired = Server::start(|mut socket| async move {
        accept_start(&mut socket).await;
        send(
            &mut socket,
            json!({"type": "session.closed", "event_id": "evt_c", "reason": "expired", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 1800.0}}),
        )
        .await;
        let _ = socket.close(None).await;
    });
    let mut model = expired.model();
    let mut session = model.open(config()).expect("the session opens");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::SpeechSessionFailed(_))
    });
    expired.finish();
    assert!(
        matches!(events.last(), Some(SessionEvent::SpeechSessionFailed(failed)) if failed.failure.kind == SessionFailureKind::ProviderError && failed.failure.detail.contains("expired")),
        "{events:?}"
    );
    assert_eq!(
        session.final_usage().map(|closed| closed.seconds),
        Some(1800.0)
    );
    assert!(
        matches!(
            &events[events.len() - 2],
            SessionEvent::UsageRecorded(record)
                if record.response_id.is_none()
                    && record.r#final
                    && record.quantities == [Quantity::provider_reported(QuantityKind::InputAudioMs, 1_800_000)]
        ),
        "the seconds session.closed carried, before the failure: {events:?}"
    );
    let after: Vec<_> = std::iter::from_fn(|| session.next_event(Duration::ZERO)).collect();
    assert!(after.is_empty(), "nothing after the failure: {after:?}");

    let dropped = Server::start(|mut socket| async move {
        accept_start(&mut socket).await;
        let _ = socket.close(None).await;
    });
    let mut model = dropped.model();
    let mut session = model.open(config()).expect("the session opens");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::SpeechSessionFailed(_))
    });
    dropped.finish();
    assert!(
        matches!(events.last(), Some(SessionEvent::SpeechSessionFailed(failed)) if failed.failure.kind == SessionFailureKind::TransportClosed),
        "{events:?}"
    );
    assert_eq!(session.final_usage(), None, "final usage unconfirmed");
    assert!(
        matches!(
            &events[events.len() - 2],
            SessionEvent::UsageRecorded(record)
                if record.response_id.is_none()
                    && !record.r#final
                    && record.quantities == [Quantity::unknown(QuantityKind::InputAudioMs)]
        ),
        "no seconds reported, none confirmed, before the failure: {events:?}"
    );
    let after: Vec<_> = std::iter::from_fn(|| session.next_event(Duration::ZERO)).collect();
    assert!(after.is_empty(), "nothing after the failure: {after:?}");
}

#[test]
fn mulaw_passes_both_ways_and_its_silence_is_no_response() {
    let mulaw = format(AudioEncoding::PcmMulaw, 8_000);
    let server = Server::start(|mut socket| async move {
        let session = accept_start(&mut socket).await;
        assert_eq!(
            session["audio"]["format"],
            json!({"type": "audio/pcmu", "rate": 8000})
        );
        let append = receive(&mut socket).await;
        assert_eq!(append["audio"], STANDARD.encode([0xFF_u8; 800]));
        send(&mut socket, audio(&[0xFF; 800])).await;
        send(&mut socket, audio(&[0x10; 800])).await;
        for _ in 0..8 {
            send(&mut socket, audio(&[0xFF; 800])).await;
        }
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut telephone = config();
    telephone.input_format = mulaw;
    telephone.output_format = mulaw;
    let mut session = model.open(telephone).expect("the session opens");
    session
        .append_audio(frame(mulaw, 0, vec![0xFF; 800]))
        .expect("mu-law in");
    let events = completed(&mut session, 1);
    session.close().expect("open");
    server.finish();
    let ids = created(&events);
    assert_eq!(ids.len(), 1);
    let outputs: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputAudio(output) => Some(output.frame.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(outputs.len(), 1 + 8);
    assert_eq!(outputs[0].format, mulaw);
    assert_eq!(outputs[0].data, vec![0x10; 800]);
    assert_eq!(outputs[0].position.get(), 0);
}

/// A server that refuses the opening handshake with `status`, as the API refuses a bad key.
#[expect(
    clippy::result_large_err,
    reason = "the handshake callback's error type is tungstenite's, fixed by its `Callback` trait"
)]
fn refusing(status: u16) -> (Endpoint, Runtime, JoinHandle<()>) {
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
    use tokio_tungstenite::tungstenite::http::StatusCode;
    let runtime = Runtime::new().unwrap();
    let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let address = listener.local_addr().unwrap();
    let task = runtime.spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = tokio_tungstenite::accept_hdr_async(stream, move |_: &Request, _: Response| {
            let mut refusal = ErrorResponse::new(Some("refused".to_owned()));
            *refusal.status_mut() = StatusCode::from_u16(status).unwrap();
            Err(refusal)
        })
        .await;
    });
    let endpoint = Endpoint::live(LIVE_MODEL).with_url(format!("ws://{address}/v1/live/sessions"));
    (endpoint, runtime, task)
}

#[test]
fn a_refused_key_is_a_rejection_and_an_unreachable_server_a_closed_transport() {
    for (status, kind) in [
        (401, SessionFailureKind::Rejected),
        (403, SessionFailureKind::Rejected),
        (503, SessionFailureKind::TransportClosed),
    ] {
        let (endpoint, runtime, task) = refusing(status);
        let refused = OpenAiLive::new(endpoint, Box::new(FixedSource), OPENAI_KEY_NAME)
            .open(config())
            .err();
        runtime.block_on(task).unwrap();
        assert!(
            matches!(&refused, Some(OpenError::Failed(failure)) if failure.kind == kind),
            "HTTP {status}: {refused:?}"
        );
    }
}

/// The evidence procedure run through the binding against a replay: the model either goes on
/// speaking and takes up the input during its output, or falls silent for longer than the gap
/// and answers it afterwards. The replay stages both, so this shows the procedure's mechanics
/// through the binding's segmentation, not what the hosted model does; that is the live test's.
fn evidence_replay(yields: bool) -> Server {
    Server::start(move |mut socket| async move {
        accept_start(&mut socket).await;
        appends(&mut socket, 2).await;
        send(&mut socket, said(" The capital of France is Paris.", 0)).await;
        send(&mut socket, voiced()).await;
        appends(&mut socket, 2).await;
        if yields {
            for _ in 0..8 {
                send(&mut socket, silent()).await;
            }
            send(&mut socket, said(" Portugal's is Lisbon.", 1_000)).await;
            send(&mut socket, voiced()).await;
        } else {
            send(&mut socket, said(" And Portugal's is Lisbon.", 100)).await;
            send(&mut socket, voiced()).await;
        }
        for _ in 0..8 {
            send(&mut socket, silent()).await;
        }
        let _ = socket.next().await;
    })
}

fn evidence_probe() -> NativeDuplexProbe {
    NativeDuplexProbe {
        config: config(),
        opening: vec![input(0), input(u64::from(FRAME))],
        during: vec![input(2 * u64::from(FRAME)), input(3 * u64::from(FRAME))],
        marker: "portugal".to_owned(),
        wait: WAIT,
        patience: 20,
    }
}

#[test]
fn the_evidence_procedure_runs_through_the_segmentation_against_a_replay() {
    let continuing = evidence_replay(false);
    let evidence = native_duplex_evidence(&mut continuing.model(), &evidence_probe());
    continuing.finish();
    let evidence = evidence.expect("output that takes up the input on the same response");
    assert_eq!(evidence.after, " And Portugal's is Lisbon.");
    assert_eq!(
        native_duplex_verdict(&Ok(evidence)),
        Ok(claims().contains(&Capability::NativeDuplex))
    );

    let yielding = evidence_replay(true);
    let refused = native_duplex_evidence(&mut yielding.model(), &evidence_probe());
    yielding.finish();
    assert!(
        matches!(refused, Err(NoNativeDuplex::NotDependent { .. })),
        "{refused:?}"
    );
    assert!(native_duplex_verdict(&Err(NoNativeDuplex::Silent)).is_err());
}

/// A server that speaks, completes one backend response with its usage, then does what `then`
/// says.
fn spoke_then<F, Fut>(then: F) -> Server
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    Server::start(move |mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        send(
            &mut socket,
            backend("del_1", backend_completed("resp_1", 12, 4, 3)),
        )
        .await;
        then(socket).await;
    })
}

/// The output text tokens of the usage record of `response` among `events`.
fn output_text_tokens(events: &[SessionEvent], response: ResponseId) -> Option<i64> {
    let recorded: Vec<_> = records(events)
        .into_iter()
        .filter(|record| record.response_id == Some(response))
        .collect();
    assert_eq!(recorded.len(), 1, "{events:#?}");
    recorded[0]
        .quantities
        .iter()
        .find(|quantity| quantity.kind() == QuantityKind::OutputTextTokens)
        .and_then(Quantity::value)
}

/// The backend's usage already reached the binding when the caller closes: the close records
/// it on the response, before `SpeechSessionClosed`.
#[test]
fn backend_usage_that_arrived_before_the_close_is_recorded_before_the_session_closed() {
    let server = spoke_then(|mut socket| async move {
        let close = receive(&mut socket).await;
        assert_eq!(close["type"], "session.close");
        send(
            &mut socket,
            json!({"type": "session.closed", "event_id": "evt_c", "reason": "close_requested", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 1.5}}),
        )
        .await;
        let _ = socket.close(None).await;
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(_))
    });
    let response = created(&events)[0];
    std::thread::sleep(Duration::from_millis(300));
    session.close().expect("open");
    server.finish();
    let closing: Vec<_> = std::iter::from_fn(|| session.next_event(Duration::ZERO)).collect();
    assert_eq!(session.backend_usage().len(), 1, "{closing:#?}");
    assert_eq!(output_text_tokens(&closing, response), Some(3));
    let closed = closing
        .iter()
        .position(|event| matches!(event, SessionEvent::SpeechSessionClosed(_)))
        .expect("the session closed");
    let of_response = closing
        .iter()
        .position(|event| {
            matches!(event, SessionEvent::UsageRecorded(record) if record.response_id == Some(response))
        })
        .expect("the response's record");
    assert!(
        of_response < closed && closed == closing.len() - 1,
        "{closing:#?}"
    );
}

/// The connection closed after the backend's usage reached the binding, so a send fails: the
/// usage is recorded before the session fails, and the failure is the closed connection.
#[test]
fn backend_usage_that_arrived_before_a_failed_send_is_recorded_before_the_failure() {
    let server = spoke_then(|mut socket| async move {
        let _ = socket.close(None).await;
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(_))
    });
    let response = created(&events)[0];
    server.finish();
    std::thread::sleep(Duration::from_millis(300));
    let mut refused = None;
    for index in 0..20 {
        if let Err(error) = session.append_audio(input(index * u64::from(FRAME))) {
            refused = Some(error);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(refused.is_some(), "a send on the closed connection fails");
    let rest: Vec<_> = std::iter::from_fn(|| session.next_event(Duration::ZERO)).collect();
    assert_eq!(session.backend_usage().len(), 1, "{rest:#?}");
    assert_eq!(output_text_tokens(&rest, response), Some(3));
    assert!(
        matches!(
            rest.last(),
            Some(SessionEvent::SpeechSessionFailed(failed))
                if failed.failure.kind == SessionFailureKind::TransportClosed
        ),
        "{rest:#?}"
    );
}
