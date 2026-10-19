//! The Realtime binding to the duplex contract, offline: a local WebSocket server replays the
//! provider. No network and no key: the credential is a fixed test value.
//!
//! The cases are those the testkit's mock passes through the contract (audio in and out on one
//! response id, interruption, barge-in decided by the caller with truncation, tool calls, usage,
//! liveness, a closed transport), run here against replayed server events. Every replayed event is
//! written in the shape of the published Realtime server events reference, as `tests/offline.rs`
//! writes them; every client event the scripts expect follows its client events reference.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_duplex::conformance::{NativeDuplexProbe, NoNativeDuplex, native_duplex_evidence};
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, ResponseUsage, SessionConfig, SessionEvent,
    SessionFailureKind, SpeechSessionHandle, TokenCounts, TuningOption, TurnDetection,
    TurnSensitivity, TurnTuning, Unsupported,
};
use inference_openai::duplex::{OpenAiRealtime, capabilities, claims, wire_format};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Eagerness, Endpoint,
    OPENAI_KEY_NAME, WireFormat,
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

/// Session setup: `session.created`, the binding's `session.update`, `session.updated`. Returns
/// the `session` object the binding sent.
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

fn transcript_delta(response: &str, item: &str, delta: &str) -> Value {
    json!({"type": "response.output_audio_transcript.delta", "event_id": "event_td", "response_id": response, "item_id": item, "output_index": 0, "content_index": 0, "delta": delta})
}

fn audio_delta(response: &str, item: &str, audio: &[u8]) -> Value {
    json!({"type": "response.output_audio.delta", "event_id": "event_ad", "response_id": response, "item_id": item, "output_index": 0, "content_index": 0, "delta": STANDARD.encode(audio)})
}

fn response_done(id: &str, status: &str) -> Value {
    json!({"type": "response.done", "event_id": "event_rd", "response": {
        "id": id, "object": "realtime.response", "status": status, "output": [],
        "usage": {
            "total_tokens": 40, "input_tokens": 25, "output_tokens": 15,
            "input_token_details": {"text_tokens": 5, "audio_tokens": 20, "cached_tokens": 4},
            "output_token_details": {"text_tokens": 3, "audio_tokens": 12}
        }
    }})
}

fn speech_started(item: &str, at_ms: u64) -> Value {
    json!({"type": "input_audio_buffer.speech_started", "event_id": "event_ss", "audio_start_ms": at_ms, "item_id": item})
}

fn speech_stopped(item: &str, at_ms: u64) -> Value {
    json!({"type": "input_audio_buffer.speech_stopped", "event_id": "event_sp", "audio_end_ms": at_ms, "item_id": item})
}

fn committed(item: &str) -> Value {
    json!({"type": "input_audio_buffer.committed", "event_id": "event_cm", "previous_item_id": null, "item_id": item})
}

fn transcribed(item: &str, transcript: &str) -> Value {
    json!({"type": "conversation.item.input_audio_transcription.completed", "event_id": "event_tc", "item_id": item, "content_index": 0, "transcript": transcript})
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

fn config(turn_detection: TurnDetection, auto: bool) -> SessionConfig {
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm24(),
        output_format: pcm24(),
        turn_detection,
        turn_tuning: TurnTuning::default(),
        auto_response: auto,
        auto_interrupt: auto,
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

fn any_completed(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::ResponseCompleted(_))
}

#[test]
fn the_declared_capabilities_are_the_realtime_apis() {
    let declared = capabilities();
    assert_eq!(
        declared.input_formats,
        vec![
            pcm24(),
            format(AudioEncoding::PcmMulaw, 8_000),
            format(AudioEncoding::PcmAlaw, 8_000)
        ]
    );
    assert_eq!(declared.output_formats, declared.input_formats);
    assert_eq!(declared.turn_detection, TurnDetection::ALL.to_vec());
    assert!(declared.turn_tuning.silence && declared.turn_tuning.sensitivity);
    assert!(declared.turn_tuning.prefix_padding);
    assert!(declared.manual_commit && declared.manual_response && declared.manual_interrupt);
    assert!(declared.client_cancel && declared.truncation && declared.tool_calling);
    assert!(
        !claims().contains(&inference_core::model::Capability::NativeDuplex),
        "no native_duplex claim without the evidence"
    );
    assert!(claims().contains(&inference_core::model::Capability::ConcurrentInputOutput));
    assert_eq!(wire_format(&pcm24()), Some(WireFormat::Pcm16));
    assert_eq!(
        wire_format(&format(AudioEncoding::PcmAlaw, 8_000)),
        Some(WireFormat::Pcma)
    );
    assert_eq!(wire_format(&format(AudioEncoding::PcmS16le, 16_000)), None);
}

#[test]
fn a_configuration_maps_onto_the_session_update_and_refuses_what_a_mode_cannot_tune() {
    let model = OpenAiRealtime::new(
        Endpoint::new("gpt-realtime-2.1"),
        Box::new(FixedSource),
        OPENAI_KEY_NAME,
    );
    let mut semantic = config(TurnDetection::SemanticVad, false);
    semantic.turn_tuning.sensitivity = Some(TurnSensitivity::High);
    semantic.tools = vec![weather()];
    let session = model
        .client_config(&semantic)
        .expect("semantic detection takes a sensitivity")
        .to_session_json();
    assert_eq!(
        session["audio"]["input"]["turn_detection"],
        json!({"type": "semantic_vad", "eagerness": "high", "create_response": false, "interrupt_response": false})
    );
    assert_eq!(
        session["audio"]["input"]["format"],
        json!({"type": "audio/pcm", "rate": 24000})
    );
    assert_eq!(session["audio"]["output"]["voice"], "marin");
    assert_eq!(session["instructions"], "answer briefly");
    assert_eq!(
        session["tools"],
        json!([{"type": "function", "name": "weather", "description": "", "parameters": weather().schema().clone()}])
    );
    let _ = Eagerness::High;

    let mut server = config(TurnDetection::ServerVad, true);
    server.turn_tuning = TurnTuning {
        silence: Some(Duration::from_millis(500)),
        sensitivity: None,
        prefix_padding: Some(Duration::from_millis(300)),
    };
    let session = model
        .client_config(&server)
        .expect("server detection")
        .to_session_json();
    assert_eq!(
        session["audio"]["input"]["turn_detection"],
        json!({"type": "server_vad", "prefix_padding_ms": 300, "silence_duration_ms": 500, "create_response": true, "interrupt_response": true})
    );

    let mut alaw = config(TurnDetection::Disabled, false);
    alaw.input_format = format(AudioEncoding::PcmAlaw, 8_000);
    alaw.output_format = alaw.input_format;
    let session = model.client_config(&alaw).expect("A-law").to_session_json();
    assert_eq!(
        session["audio"]["input"]["format"],
        json!({"type": "audio/pcma"})
    );
    assert_eq!(
        session["audio"]["output"]["format"],
        json!({"type": "audio/pcma"})
    );

    server.turn_tuning.sensitivity = Some(TurnSensitivity::Low);
    assert_eq!(
        model.client_config(&server).err(),
        Some(Unsupported::TurnTuning(TuningOption::Sensitivity)),
        "server_vad has no sensitivity, only a loudness threshold"
    );
    semantic.turn_tuning.silence = Some(Duration::from_millis(500));
    assert_eq!(
        model.client_config(&semantic).err(),
        Some(Unsupported::TurnTuning(TuningOption::Silence))
    );
}

#[test]
fn a_format_the_api_does_not_take_is_refused_before_connecting() {
    let mut model = OpenAiRealtime::new(
        Endpoint::new("gpt-realtime-2.1").with_url("ws://127.0.0.1:9/v1/realtime"),
        Box::new(FixedSource),
        OPENAI_KEY_NAME,
    );
    let mut wide = config(TurnDetection::Disabled, false);
    wide.input_format = format(AudioEncoding::PcmS16le, 16_000);
    assert_eq!(
        model.open(wide).err(),
        Some(OpenError::Unsupported(Unsupported::InputFormat(format(
            AudioEncoding::PcmS16le,
            16_000
        ))))
    );
}

#[test]
fn audio_in_gives_audio_and_transcript_out_on_one_response_id() {
    let input = vec![7_u8; 480];
    let sent = input.clone();
    let server = Server::start(move |mut socket| async move {
        let session = accept_session(&mut socket).await;
        assert_eq!(session["audio"]["input"]["turn_detection"], Value::Null);
        assert_eq!(
            session["audio"]["input"]["transcription"],
            json!({"model": "gpt-4o-mini-transcribe"})
        );
        let append = receive(&mut socket).await;
        assert_eq!(append["type"], "input_audio_buffer.append");
        assert_eq!(
            STANDARD.decode(append["audio"].as_str().unwrap()).unwrap(),
            sent,
            "the frame goes out as it is"
        );
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "input_audio_buffer.commit"})
        );
        send(&mut socket, committed("item_in")).await;
        send(&mut socket, transcribed("item_in", "what time is it")).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create"})
        );
        send(&mut socket, response_created("resp_1")).await;
        send(
            &mut socket,
            transcript_delta("resp_1", "item_out", "It is noon"),
        )
        .await;
        send(&mut socket, audio_delta("resp_1", "item_out", &[1; 480])).await;
        send(&mut socket, audio_delta("resp_1", "item_out", &[2; 480])).await;
        send(&mut socket, response_done("resp_1", "completed")).await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut session = model
        .open(config(TurnDetection::Disabled, false))
        .expect("the session opens");
    session
        .append_audio(frame(pcm24(), 0, input))
        .expect("in the session's format");
    session.commit_input().expect("a manual commit");
    session.create_response().expect("none in progress");
    let events = until(&mut session, any_completed);
    session.close().expect("open");
    server.finish();

    let responses = created(&events);
    assert_eq!(responses.len(), 1, "{events:#?}");
    let response = responses[0];
    assert_eq!(session.provider_response_id(response), Some("resp_1"));
    let commits = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::InputCommitted(_)))
        .count();
    assert_eq!(
        commits, 1,
        "the server's acknowledgement of the caller's commit is not a second commit"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::InputTranscribed(heard) if heard.text == "what time is it"
    )));
    assert_eq!(transcript(&events, response), "It is noon");
    let frames: Vec<&AudioFrame> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputAudio(audio) if audio.response_id == response => Some(&audio.frame),
            _ => None,
        })
        .collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(
        frames
            .iter()
            .map(|frame| (frame.format, frame.position.get(), frame.sample_count))
            .collect::<Vec<_>>(),
        vec![(pcm24(), 0, 240), (pcm24(), 240, 240)]
    );
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::ResponseUsageRecorded(recorded)
            if recorded.response_id == response
                && recorded.usage == ResponseUsage {
                    input: TokenCounts { text_tokens: 5, audio_tokens: 20, cached_tokens: 4 },
                    output: TokenCounts { text_tokens: 3, audio_tokens: 12, cached_tokens: 0 },
                }
    )));
    assert!(matches!(
        events.last(),
        Some(SessionEvent::ResponseCompleted(done)) if done.response_id == response
    ));
}

#[test]
fn the_providers_interruption_cancels_the_response_and_the_next_has_a_new_id() {
    let server = Server::start(|mut socket| async move {
        let session = accept_session(&mut socket).await;
        assert_eq!(
            session["audio"]["input"]["turn_detection"],
            json!({"type": "server_vad", "create_response": true, "interrupt_response": true})
        );
        send(&mut socket, speech_started("item_1", 0)).await;
        send(&mut socket, speech_stopped("item_1", 900)).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(
            &mut socket,
            transcript_delta("resp_1", "item_o1", "Once upon"),
        )
        .await;
        send(&mut socket, speech_started("item_2", 1_000)).await;
        send(&mut socket, response_done("resp_1", "cancelled")).await;
        send(&mut socket, audio_delta("resp_1", "item_o1", &[3; 480])).await;
        send(&mut socket, speech_stopped("item_2", 1_500)).await;
        send(&mut socket, committed("item_2")).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, transcript_delta("resp_2", "item_o2", "Sure")).await;
        send(&mut socket, response_done("resp_2", "completed")).await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut session = model
        .open(config(TurnDetection::ServerVad, true))
        .expect("the session opens");
    let mut events = until(&mut session, any_completed);
    events.extend(until(&mut session, any_completed));
    session.close().expect("open");
    server.finish();

    let responses = created(&events);
    assert_eq!(responses.len(), 2, "{events:#?}");
    let (first, second) = (responses[0], responses[1]);
    assert_ne!(first, second);
    let cancelled = events
        .iter()
        .position(
            |event| matches!(event, SessionEvent::ResponseCancelled(c) if c.response_id == first),
        )
        .expect("the first response is cancelled");
    assert!(events[..cancelled].iter().any(|event| matches!(
        event,
        SessionEvent::SpeechStarted(started) if started.position.get() == 24_000
    )));
    assert!(
        events[cancelled..].iter().all(|event| !matches!(
            event,
            SessionEvent::OutputAudio(audio) if audio.response_id == first
        )),
        "late audio of the cancelled response is dropped: {events:#?}"
    );
    assert_eq!(transcript(&events, second), "Sure");
    let commits = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::InputCommitted(_)))
        .count();
    assert_eq!(commits, 2, "the provider's own commits");
}

#[test]
fn barge_in_decided_by_the_caller_cancels_and_truncates_at_the_heard_sample() {
    let server = Server::start(|mut socket| async move {
        let session = accept_session(&mut socket).await;
        assert_eq!(
            session["audio"]["input"]["turn_detection"]["interrupt_response"],
            json!(false)
        );
        send(&mut socket, speech_started("item_1", 0)).await;
        send(&mut socket, speech_stopped("item_1", 900)).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, audio_delta("resp_1", "item_o1", &[4; 480])).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.cancel", "response_id": "resp_1"})
        );
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "conversation.item.truncate", "item_id": "item_o1", "content_index": 0, "audio_end_ms": 5})
        );
        send(&mut socket, response_done("resp_1", "cancelled")).await;
        send(
            &mut socket,
            json!({"type": "conversation.item.truncated", "event_id": "event_tr", "item_id": "item_o1", "content_index": 0, "audio_end_ms": 5}),
        )
        .await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut caller = config(TurnDetection::ServerVad, true);
    caller.auto_interrupt = false;
    let mut session = model.open(caller).expect("the session opens");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputAudio(_))
    });
    let response = created(&events)[0];
    session.cancel_response(response).expect("in progress");
    let heard = SampleClockPosition::new(120).unwrap();
    session
        .truncate_output(response, heard)
        .expect("within the emitted audio");
    let rest = until(&mut session, |_| false);
    session.close().expect("open");
    server.finish();

    assert_eq!(
        rest.iter()
            .filter(|event| matches!(event, SessionEvent::ResponseCancelled(_)))
            .count(),
        1,
        "the server's cancelled response.done is not a second cancellation: {rest:#?}"
    );
    assert!(rest.iter().any(|event| matches!(
        event,
        SessionEvent::OutputTruncated(truncated) if truncated.heard == heard
    )));
}

#[test]
fn a_tool_call_answered_on_the_session_yields_a_response_that_uses_its_result() {
    let server = Server::start(|mut socket| async move {
        let session = accept_session(&mut socket).await;
        assert_eq!(session["tools"][0]["name"], "weather");
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create"})
        );
        send(&mut socket, response_created("resp_1")).await;
        send(
            &mut socket,
            json!({"type": "response.function_call_arguments.done", "event_id": "event_fc", "response_id": "resp_1", "item_id": "item_fc", "output_index": 0, "call_id": "call_abc", "name": "weather", "arguments": "{\"city\":\"Berlin\"}"}),
        )
        .await;
        send(&mut socket, response_done("resp_1", "completed")).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "conversation.item.create", "item": {"type": "function_call_output", "call_id": "call_abc", "output": "{\"celsius\":21}"}})
        );
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create"})
        );
        send(&mut socket, response_created("resp_2")).await;
        send(
            &mut socket,
            transcript_delta("resp_2", "item_o2", "It is 21 degrees"),
        )
        .await;
        send(&mut socket, response_done("resp_2", "completed")).await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut tooled = config(TurnDetection::Disabled, false);
    tooled.tools = vec![weather()];
    let tool_id = tooled.tools[0].tool_id();
    let mut session = model.open(tooled).expect("the session opens");
    session.create_response().expect("none in progress");
    let events = until(&mut session, any_completed);
    let (call_id, proposed_tool, arguments) = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::SpeechToolCallProposed(proposed) => Some((
                proposed.call_id.clone(),
                proposed.tool_id,
                proposed.arguments.clone(),
            )),
            _ => None,
        })
        .expect("a proposed call");
    assert_eq!(call_id.as_str(), "call_abc");
    assert_eq!(proposed_tool, tool_id);
    assert_eq!(arguments, json!({"city": "Berlin"}));
    session
        .submit_tool_result(&call_id, json!({"celsius": 21}))
        .expect("the call awaits its result");
    session
        .create_response()
        .expect("the first response completed");
    let answer = until(&mut session, any_completed);
    session.close().expect("open");
    server.finish();
    let second = created(&answer);
    assert_eq!(second.len(), 1, "{answer:#?}");
    assert_eq!(transcript(&answer, second[0]), "It is 21 degrees");
}

#[test]
fn a_silent_provider_fails_the_session_as_stalled_at_the_bound() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create"})
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    });
    let mut model = server.model();
    let mut bounded = config(TurnDetection::Disabled, false);
    bounded.idle_limit = Duration::from_millis(400);
    let mut session = model.open(bounded).expect("the session opens");
    let asked = Instant::now();
    session.create_response().expect("none in progress");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::SpeechSessionFailed(_))
    });
    let waited = asked.elapsed();
    server.finish();
    assert!(
        matches!(
            events.last(),
            Some(SessionEvent::SpeechSessionFailed(failed))
                if failed.failure.kind == SessionFailureKind::Stalled
        ),
        "{events:#?}"
    );
    assert!(waited >= Duration::from_millis(400), "{waited:?}");
}

#[test]
fn a_closed_transport_fails_the_session_at_once() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        socket.close(None).await.unwrap();
    });
    let mut model = server.model();
    let mut session = model
        .open(config(TurnDetection::Disabled, false))
        .expect("the session opens");
    let started = Instant::now();
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::SpeechSessionFailed(_))
    });
    server.finish();
    assert!(
        matches!(
            events.last(),
            Some(SessionEvent::SpeechSessionFailed(failed))
                if failed.failure.kind == SessionFailureKind::TransportClosed
        ),
        "{events:#?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "not the liveness bound"
    );
    assert!(
        session
            .append_audio(frame(pcm24(), 0, vec![0; 480]))
            .is_err()
    );
}

#[test]
fn mulaw_at_8_khz_passes_both_ways_as_it_is() {
    let mulaw = format(AudioEncoding::PcmMulaw, 8_000);
    let line: Vec<u8> = (0..160_u16).map(|index| (index % 256) as u8).collect();
    let expected = line.clone();
    let server = Server::start(move |mut socket| async move {
        let session = accept_session(&mut socket).await;
        assert_eq!(
            session["audio"]["input"]["format"],
            json!({"type": "audio/pcmu"})
        );
        assert_eq!(
            session["audio"]["output"]["format"],
            json!({"type": "audio/pcmu"})
        );
        let append = receive(&mut socket).await;
        assert_eq!(
            STANDARD.decode(append["audio"].as_str().unwrap()).unwrap(),
            expected
        );
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create"})
        );
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, audio_delta("resp_1", "item_o1", &[0xFF; 160])).await;
        send(&mut socket, response_done("resp_1", "completed")).await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut telephony = config(TurnDetection::Disabled, false);
    telephony.input_format = mulaw;
    telephony.output_format = mulaw;
    let mut session = model.open(telephony).expect("the session opens");
    session
        .append_audio(frame(mulaw, 0, line))
        .expect("mu-law in");
    session.create_response().expect("none in progress");
    let events = until(&mut session, any_completed);
    session.close().expect("open");
    server.finish();
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::OutputAudio(audio)
            if audio.frame.format == mulaw && audio.frame.sample_count == 160 && audio.frame.data == vec![0xFF; 160]
    )));
}

/// The evidence procedure run through the binding against a replay. The replay stages the
/// dependency, so this shows the procedure's mechanics through the binding, not that the hosted
/// model gives the evidence; that is the ignored live test's.
fn evidence_replay(restart: bool) -> Server {
    Server::start(move |mut socket| async move {
        accept_session(&mut socket).await;
        for _ in 0..2 {
            assert_eq!(
                receive(&mut socket).await["type"],
                "input_audio_buffer.append"
            );
        }
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(
            &mut socket,
            transcript_delta("resp_1", "item_o1", "The weather is"),
        )
        .await;
        for _ in 0..2 {
            assert_eq!(
                receive(&mut socket).await["type"],
                "input_audio_buffer.append"
            );
        }
        send(&mut socket, committed("item_2")).await;
        if restart {
            send(&mut socket, response_done("resp_1", "cancelled")).await;
            send(&mut socket, response_created("resp_2")).await;
            send(
                &mut socket,
                transcript_delta("resp_2", "item_o2", "In Lisbon it is mild"),
            )
            .await;
            send(&mut socket, response_done("resp_2", "completed")).await;
        } else {
            send(
                &mut socket,
                transcript_delta("resp_1", "item_o1", " mild in Lisbon"),
            )
            .await;
            send(&mut socket, response_done("resp_1", "completed")).await;
        }
        let _ = socket.next().await;
    })
}

fn evidence_probe() -> NativeDuplexProbe {
    let mut probe_config = config(TurnDetection::ServerVad, true);
    probe_config.auto_interrupt = false;
    NativeDuplexProbe {
        config: probe_config,
        opening: vec![
            frame(pcm24(), 0, vec![9; 480]),
            frame(pcm24(), 240, vec![0; 480]),
        ],
        during: vec![
            frame(pcm24(), 480, vec![9; 480]),
            frame(pcm24(), 720, vec![0; 480]),
        ],
        marker: "lisbon".to_owned(),
        wait: WAIT,
        patience: 20,
    }
}

#[test]
fn the_evidence_procedure_runs_through_the_binding_against_a_replay() {
    let continuing = evidence_replay(false);
    let evidence = native_duplex_evidence(&mut continuing.model(), &evidence_probe());
    continuing.finish();
    let evidence = evidence.expect("a replay that answers on the same response");
    assert_eq!(evidence.after, " mild in Lisbon");

    let restarting = evidence_replay(true);
    let refused = native_duplex_evidence(&mut restarting.model(), &evidence_probe());
    restarting.finish();
    assert!(
        matches!(refused, Err(NoNativeDuplex::Restarted { .. })),
        "{refused:?}"
    );
}
