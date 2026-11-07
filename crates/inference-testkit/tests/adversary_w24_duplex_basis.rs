//! Adversary pass 1 on `story:sdk-duplex-node`: the basis span of a proposal on a later turn.
//!
//! The story: "At `InputCommitted` with no transcript yet, the node emits and commits an empty
//! span, so a proposal on an untranscribed turn still has a committed basis." The duplex mock
//! yields `InputCommitted` before `InputTranscribed`, as a provider whose transcription finishes
//! after the commit does. A second turn whose transcript has not arrived when its call is
//! proposed must not rest the call on the first turn's words.
//!
//! Every wait is bounded by the case's wall-clock deadline.

#![cfg(all(feature = "run", feature = "duplex"))]

use std::cell::Cell;
use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, Locality};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_core::usage::UsageRecorded;
use inference_duplex::inference_audio::{AudioFrame, SampleClockPosition};
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseFailed, ResponseId, SessionConfig, SessionError,
    SessionEvent as ModelEvent, SessionLedger, SpeechCallId, SpeechCapabilities,
    SpeechSessionClosed, SpeechSessionFailed, SpeechSessionHandle, SpeechStarted, TurnDetection,
    TurnTuning,
};
use inference_pipeline::nodes::duplex::{DuplexNode, DuplexPayload, DuplexPorts};
use inference_pipeline::nodes::{RunLog, StreamEvent, ToolCallEvent, ToolCallUpdate, ToolPayload};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PortTraits, modality,
};
use inference_runtime::{RunController, RunError};
use inference_testkit::duplex::mock::{
    MockSpeechModel, MockSpeechSession, Reply, ScriptedTurn, pcm, utterance,
};
use inference_tools::{DeclareTool, Json, SideEffectClass, Tool, ToolCall};
use serde_json::json;

const CASE: Duration = Duration::from_secs(20);

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn start() {
    DEADLINE.with(|deadline| deadline.set(Some(Instant::now() + CASE)));
}

fn remaining() -> Duration {
    let deadline = DEADLINE.with(Cell::get).expect("the case started");
    let left = deadline.saturating_duration_since(Instant::now());
    assert!(
        !left.is_zero(),
        "the case ran past its deadline of {CASE:?}"
    );
    left
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn wait<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park_timeout(remaining().min(Duration::from_millis(50)));
    }
}

fn spawn<T, F>(make: impl FnOnce() -> F + Send + 'static) -> mpsc::Receiver<T>
where
    T: Send + 'static,
    F: Future<Output = T>,
{
    let (sent, received) = mpsc::channel();
    thread::spawn(move || {
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut future = pin!(make());
        let started = Instant::now();
        let answer = loop {
            if let Poll::Ready(answer) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
                break Some(answer);
            }
            if started.elapsed() > CASE {
                break None;
            }
            thread::park_timeout(Duration::from_millis(50));
        };
        if let Some(answer) = answer {
            let _ = sent.send(answer);
        }
    });
    received
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
enum Voice {
    Audio(AudioFrame),
    Stream(StreamEvent),
    Usage(UsageRecorded),
    Tool(ToolCallUpdate),
    SpeechStarted(SpeechStarted),
    ResponseFailed(ResponseFailed),
    Closed(SpeechSessionClosed),
    Failed(SpeechSessionFailed),
    Control(ControlKind),
}

impl ToolPayload for Voice {
    fn into_tool_call(self) -> Result<ToolCallUpdate, Self> {
        match self {
            Self::Tool(update) => Ok(update),
            other => Err(other),
        }
    }

    fn from_tool_call(update: ToolCallUpdate) -> Self {
        Self::Tool(update)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl DuplexPayload for Voice {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn audio(frame: AudioFrame) -> Self {
        Self::Audio(frame)
    }

    fn usage(recorded: UsageRecorded) -> Self {
        Self::Usage(recorded)
    }

    fn speech_started(event: SpeechStarted) -> Self {
        Self::SpeechStarted(event)
    }

    fn response_failed(event: ResponseFailed) -> Self {
        Self::ResponseFailed(event)
    }

    fn closed(event: SpeechSessionClosed) -> Self {
        Self::Closed(event)
    }

    fn failed(event: SpeechSessionFailed) -> Self {
        Self::Failed(event)
    }

    fn into_audio(self) -> Result<AudioFrame, Self> {
        match self {
            Self::Audio(frame) => Ok(frame),
            other => Err(other),
        }
    }
}

/// The duplex mock with every user turn's transcript after the first held back until the call
/// that turn's response proposes has been yielded: the transcription finishes late.
#[derive(Debug, Clone)]
struct LateTranscripts {
    inner: MockSpeechModel,
}

struct LateSession {
    inner: MockSpeechSession,
    transcripts: usize,
    held: Option<ModelEvent>,
    release: bool,
    yielded: Arc<Mutex<Vec<ModelEvent>>>,
}

impl NativeSpeechModel for LateTranscripts {
    type Session = LateSession;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        self.inner.capabilities()
    }

    fn open(&mut self, config: SessionConfig) -> Result<LateSession, OpenError> {
        Ok(LateSession {
            inner: self.inner.open(config)?,
            transcripts: 0,
            held: None,
            release: false,
            yielded: Arc::default(),
        })
    }
}

impl SpeechSessionHandle for LateSession {
    fn ledger(&self) -> &SessionLedger {
        self.inner.ledger()
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        self.inner.append_audio(frame)
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.inner.commit_input()
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.inner.create_response()
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.inner.cancel_response(response_id)
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.inner.truncate_output(response_id, heard)
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        self.inner.submit_tool_result(call_id, result)
    }

    fn close(&mut self) -> Result<(), SessionError> {
        self.inner.close()
    }

    fn next_event(&mut self, wait: Duration) -> Option<ModelEvent> {
        if self.release
            && let Some(held) = self.held.take()
        {
            self.release = false;
            return Some(held);
        }
        loop {
            let event = self.inner.next_event(wait)?;
            if matches!(event, ModelEvent::InputTranscribed(_)) {
                self.transcripts += 1;
                if self.transcripts > 1 {
                    self.held = Some(event);
                    continue;
                }
            }
            if matches!(event, ModelEvent::SpeechToolCallProposed(_)) {
                self.release = true;
            }
            self.yielded
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(event.clone());
            return Some(event);
        }
    }
}

struct Fixture {
    profile: DeploymentProfile,
    adapter: Adapter,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        (self.adapter.adapter_id() == adapter_id).then_some(&self.adapter)
    }
}

fn weather() -> Tool {
    Tool::declare(DeclareTool {
        name: "weather".to_owned(),
        description: String::new(),
        schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

fn config(tools: Vec<Tool>) -> SessionConfig {
    SessionConfig {
        voice: "calm".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(16_000),
        output_format: pcm(16_000),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools,
        idle_limit: Duration::from_secs(5),
    }
}

struct Sender {
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Sender {
    fn envelope(&mut self, port: &str, kind: &str, payload: Voice) -> EventEnvelope<Voice> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: RunId::generate(),
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: port.to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }
}

/// A transcript arriving after its turn's commit, and after the call that turn's response
/// proposes, leaves that call resting on the commit's empty span, never on the previous turn's
/// transcript.
#[test]
fn a_call_on_a_later_turn_does_not_rest_on_the_previous_turns_transcript() {
    start();
    let model = LateTranscripts {
        inner: MockSpeechModel::native(vec![
            ScriptedTurn {
                heard: "hello".to_owned(),
                reply: Reply::Say("Hi".to_owned()),
            },
            ScriptedTurn {
                heard: "how warm is it in Berlin".to_owned(),
                reply: Reply::Call {
                    tool: "weather".to_owned(),
                    arguments: json!({"city": "Berlin"}),
                    then: "It is {result} degrees".to_owned(),
                },
            },
        ]),
    };
    let tool = weather();

    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let adapter_id = model.adapter().adapter_id();
    let catalog = Fixture {
        profile,
        adapter: model.adapter().clone(),
    };
    let stream = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let control = PortTraits {
        essentiality: Essentiality::ControlOnly,
        incremental: false,
        ..stream
    };
    let structured = PortTraits {
        essentiality: Essentiality::Optional,
        incremental: false,
        ..stream
    };
    let room = NonZeroUsize::new(64).expect("64");
    let mut declaration = Declaration::new();
    let (pipeline_id, ports) = declaration
        .build(
            &catalog,
            profile_id,
            PipelineSpec {
                name: "native voice".to_owned(),
                schema_revision: "1".to_owned(),
                mode: InteractionMode::Duplex,
                required_capabilities: vec![Capability::NativeDuplex],
            },
            |pipeline| {
                let speech =
                    pipeline.node("speech", NodeRole::Model, vec![Capability::NativeDuplex]);
                let audio = pipeline.input::<modality::Audio>(&speech, "audio", stream);
                let control = pipeline.input::<modality::Control>(&speech, "control", control);
                let events = pipeline.output::<modality::Structured>(&speech, "events", stream);
                let tool_call =
                    pipeline.output::<modality::Structured>(&speech, "tool_call", structured);
                let tool_result =
                    pipeline.input::<modality::Structured>(&speech, "tool_result", structured);
                pipeline.expose(&audio, "audio", room, OverloadPolicy::Pause);
                pipeline.expose(&control, "control", room, OverloadPolicy::Pause);
                pipeline.expose(&events, "events", room, OverloadPolicy::Pause);
                pipeline.expose(&tool_call, "tool_call", room, OverloadPolicy::Pause);
                pipeline.expose(&tool_result, "tool_result", room, OverloadPolicy::Pause);
                pipeline.bind(&speech, adapter_id).expect("bound");
                DuplexPorts {
                    audio: audio.id(),
                    control: control.id(),
                    events: events.id(),
                    tool_call: Some(tool_call.id()),
                    tool_result: Some(tool_result.id()),
                }
            },
        )
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline_id).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");

    let run_id = RunId::generate();
    let node = DuplexNode::new(
        run_id,
        model.clone(),
        config(vec![tool.clone()]),
        ports,
        RunLog::new(),
    )
    .with_tools(vec![tool]);
    let mut node = Some(node);
    let mut controller = RunController::create_as(
        run_id,
        &planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        move |_: &Node| -> Box<dyn NodeImplementation<Voice>> {
            Box::new(node.take().expect("one instance"))
        },
    )
    .unwrap_or_else(|error| panic!("the run is created: {error:?}"));
    controller.validate().expect("validates");
    wait(controller.prepare()).expect("prepares");
    wait(controller.activate()).expect("activates");
    let mut ends = controller.exposed();
    let audio = ends.ingress("audio").expect("audio");
    let control_in = ends.ingress("control").expect("control");
    let results = ends.ingress("tool_result").expect("tool_result");
    let mut events = ends.egress("events").expect("events");
    let mut tool_call = ends.egress("tool_call").expect("tool_call");
    let driver: mpsc::Receiver<Result<(), RunError>> =
        spawn(move || async move { controller.drive().await });
    let mut sender = Sender {
        producer: NodeId::generate(),
        stream_id: StreamId::generate(),
        sequence: 0,
    };

    let first = utterance(pcm(16_000), 0);
    for frame in first.frames() {
        let envelope = sender.envelope("audio", "inference.audio.AudioFrame", Voice::Audio(frame));
        wait(audio.send(envelope)).unwrap_or_else(|_| panic!("taken"));
    }
    let mut hello = None;
    loop {
        let envelope = wait(events.recv()).expect("the first reply");
        match envelope.payload {
            Voice::Stream(StreamEvent::TranscriptCommitted(committed)) => {
                assert_eq!(committed.text, "hello");
                hello = Some(committed.span_id);
            }
            Voice::Stream(StreamEvent::GenerationCompleted(_)) => break,
            _ => {}
        }
    }
    let hello = hello.expect("the first turn's transcript was committed");

    for frame in utterance(pcm(16_000), first.next()).frames() {
        let envelope = sender.envelope("audio", "inference.audio.AudioFrame", Voice::Audio(frame));
        wait(audio.send(envelope)).unwrap_or_else(|_| panic!("taken"));
    }
    let proposed: ToolCall = loop {
        let envelope = wait(tool_call.recv()).expect("tool_call carries the proposal");
        if let Voice::Tool(ToolCallUpdate {
            call,
            event: ToolCallEvent::Proposed(_),
        }) = envelope.payload
        {
            break call;
        }
    };

    assert_ne!(
        proposed.basis_span_id(),
        hello,
        "the second turn's call rests on the first turn's transcript \"hello\": its own \
         transcript had not arrived, so its basis must be the empty span committed at its \
         InputCommitted (story:sdk-duplex-node, Planned scope, Basis span)"
    );

    for port in ["audio", "control", "tool_result"] {
        let stop = sender.envelope(
            port,
            ControlKind::Stop.as_str(),
            Voice::Control(ControlKind::Stop),
        );
        let endpoint = match port {
            "audio" => &audio,
            "control" => &control_in,
            _ => &results,
        };
        wait(endpoint.send(stop)).unwrap_or_else(|_| panic!("taken"));
    }
    while wait(events.recv()).is_some() {}
    while wait(tool_call.recv()).is_some() {}
    driver
        .recv_timeout(remaining())
        .expect("the run ends within the deadline")
        .expect("the run completes");
}
