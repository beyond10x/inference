//! `story:sdk-duplex-node`: a native speech model runs as a graph node, `DuplexNode`, inside a
//! run and a live session.
//!
//! The test pipeline is the node alone over the duplex mock: its `audio` and `control` inputs
//! exposed as ingresses and its one output, `events`, as the egress; the tool cases also expose
//! `tool_call` and `tool_result`. Every case feeds `utterance(..).frames()` and never commits:
//! the mock's server turn detection ends each turn at the silence after speech. Every wait in a
//! case is bounded by the case's own wall-clock deadline.

#![cfg(all(feature = "run", feature = "duplex"))]

use std::cell::Cell;
use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, Locality};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_core::usage::UsageRecorded;
use inference_duplex::inference_audio::AudioFrame;
use inference_duplex::{
    InputItemId, NativeSpeechModel, OpenError, ResponseFailed, SessionConfig, SessionError,
    SessionEvent as ModelEvent, SessionFailureKind, SessionLedger, SpeechCallId,
    SpeechCapabilities, SpeechSessionClosed, SpeechSessionFailed, SpeechSessionHandle,
    SpeechStarted, TurnDetection, TurnTuning,
};
use inference_facade::events::{SessionEvent, SessionPayload};
use inference_facade::live::{LiveSession, LiveSessions};
use inference_pipeline::nodes::duplex::{CallIds, DuplexNode, DuplexPayload, DuplexPorts};
use inference_pipeline::nodes::{RunLog, StreamEvent, ToolCallEvent, ToolCallUpdate, ToolPayload};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineId, PipelineSpec, PlannedPipeline, PortDirection, PortTraits,
    modality,
};
use inference_runtime::{RunController, RunError, RunRequests};
use inference_testkit::duplex::mock::{
    AUDIO_FRAMES, Faults, MockSpeechModel, MockSpeechSession, Reply, ScriptedTurn, pcm, utterance,
};
use inference_tools::{AuthorizeToolCall, DeclareTool, Json, SideEffectClass, Tool, ToolCall};
use serde_json::json;

/// How long a whole case may take on the wall clock.
const CASE: Duration = Duration::from_secs(20);

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Starts the case's wall-clock deadline.
fn start() {
    DEADLINE.with(|deadline| deadline.set(Some(Instant::now() + CASE)));
}

/// What is left of the case's deadline; a case past it fails.
fn remaining() -> Duration {
    let deadline = DEADLINE
        .with(Cell::get)
        .expect("the case started its deadline");
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

/// Polls `future` on this thread until it completes, within the case's deadline.
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

/// Polls the future `make` makes on a thread of its own; the answer arrives on the returned
/// receiver.
fn spawn<T, F>(make: impl FnOnce() -> F + Send + 'static) -> mpsc::Receiver<T>
where
    T: Send + 'static,
    F: Future<Output = T>,
{
    let (sent, received) = mpsc::channel();
    thread::spawn(move || {
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut future = pin!(make());
        let answer = loop {
            if let Poll::Ready(answer) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
                break answer;
            }
            thread::park_timeout(Duration::from_millis(50));
        };
        let _ = sent.send(answer);
    });
    received
}

/// The answer of a spawned future, within the case's deadline.
fn joined<T>(answer: &mpsc::Receiver<T>) -> T {
    answer
        .recv_timeout(remaining())
        .expect("the spawned work answers within the case's deadline")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// The payload of the test pipeline.

/// What the test pipeline's edges carry.
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

impl SessionPayload for Voice {
    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }

    fn session_event(&self) -> Option<SessionEvent> {
        Some(match self.clone() {
            Self::Audio(frame) => SessionEvent::AudioChunk(frame),
            Self::Stream(event) => match event {
                StreamEvent::TranscriptHypothesis(event) => {
                    SessionEvent::TranscriptHypothesis(event)
                }
                StreamEvent::TranscriptRevision(event) => SessionEvent::TranscriptRevision(event),
                StreamEvent::TranscriptCommitted(event) => SessionEvent::TranscriptCommitted(event),
                StreamEvent::GenerationStarted(event) => SessionEvent::GenerationStarted(event),
                StreamEvent::GenerationCompleted(event) => SessionEvent::GenerationCompleted(event),
                StreamEvent::GenerationSuperseded(event) => {
                    SessionEvent::GenerationSuperseded(event)
                }
                StreamEvent::GenerationCancelled(event) => SessionEvent::GenerationCancelled(event),
                StreamEvent::GenerationRefused(event) => SessionEvent::GenerationRefused(event),
                StreamEvent::TextDelta(event) => SessionEvent::TextDelta(event),
                StreamEvent::TextSegmentCommitted(event) => {
                    SessionEvent::TextSegmentCommitted(event)
                }
                StreamEvent::SynthesisCompleted(event) => SessionEvent::SynthesisCompleted(event),
                StreamEvent::AudioPlayed(event) => SessionEvent::AudioPlayed(event),
                _ => return None,
            },
            Self::Usage(recorded) => SessionEvent::UsageRecorded(recorded),
            Self::Tool(update) => match update.event {
                ToolCallEvent::Proposed(event) => SessionEvent::ToolCallProposed(event),
                ToolCallEvent::Authorized(event) => SessionEvent::ToolCallAuthorized(event),
                ToolCallEvent::Rejected(event) => SessionEvent::ToolCallRejected(event),
                ToolCallEvent::Started(event) => SessionEvent::ToolCallStarted(event),
                ToolCallEvent::Result(event) => SessionEvent::ToolCallResult(event),
                ToolCallEvent::Failed(event) => SessionEvent::ToolCallFailed(event),
            },
            Self::SpeechStarted(event) => SessionEvent::SpeechStarted(event),
            Self::ResponseFailed(event) => SessionEvent::SpeechResponseFailed(event),
            Self::Closed(event) => SessionEvent::SpeechSessionClosed(event),
            Self::Failed(event) => SessionEvent::SpeechSessionFailed(event),
            Self::Control(_) => return None,
        })
    }
}

// The model: the duplex mock, wrapped so a case can hold a response in progress and see what
// the node asked of the session.

/// What a session of [`Held`] saw.
#[derive(Debug, Default)]
struct Seen {
    closes: usize,
    dropped: bool,
    submitted: Vec<(SpeechCallId, Json)>,
    yielded: Vec<ModelEvent>,
}

/// The duplex mock behind a gate: a holding model's session, once its inner session yielded
/// `ResponseCreated`, answers none after `wait` without calling the inner one until the case
/// opens the gate, so the response stays in progress with no output while the case acts. Its
/// sessions pass every operation through and record closes, tool results, their drop and every
/// event they yielded; a model built `untranscribed` hides every `InputTranscribed`; a model
/// built `reordering` yields the first turn's transcript only after the third turn's commit and
/// never yields the third turn's.
#[derive(Debug, Clone)]
struct Held {
    inner: MockSpeechModel,
    gate: Arc<AtomicBool>,
    holds: bool,
    untranscribed: bool,
    reordering: bool,
    seen: Arc<Mutex<Seen>>,
}

impl Held {
    fn over(inner: MockSpeechModel) -> Self {
        Self {
            inner,
            gate: Arc::new(AtomicBool::new(true)),
            holds: false,
            untranscribed: false,
            reordering: false,
            seen: Arc::default(),
        }
    }

    fn reordering(mut self) -> Self {
        self.reordering = true;
        self
    }

    fn holding(mut self) -> Self {
        self.holds = true;
        self.gate.store(false, Ordering::Release);
        self
    }

    fn untranscribed(mut self) -> Self {
        self.untranscribed = true;
        self
    }

    fn open_gate(&self) {
        self.gate.store(true, Ordering::Release);
    }

    fn seen(&self) -> MutexGuard<'_, Seen> {
        lock(&self.seen)
    }
}

struct HeldSession {
    inner: MockSpeechSession,
    gate: Arc<AtomicBool>,
    holds: bool,
    untranscribed: bool,
    reordering: bool,
    commits: usize,
    transcripts: usize,
    late: Option<ModelEvent>,
    due: Option<ModelEvent>,
    created: bool,
    seen: Arc<Mutex<Seen>>,
}

impl Drop for HeldSession {
    fn drop(&mut self) {
        lock(&self.seen).dropped = true;
    }
}

impl NativeSpeechModel for Held {
    type Session = HeldSession;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        self.inner.capabilities()
    }

    fn open(&mut self, config: SessionConfig) -> Result<HeldSession, OpenError> {
        Ok(HeldSession {
            inner: self.inner.open(config)?,
            gate: Arc::clone(&self.gate),
            holds: self.holds,
            untranscribed: self.untranscribed,
            reordering: self.reordering,
            commits: 0,
            transcripts: 0,
            late: None,
            due: None,
            created: false,
            seen: Arc::clone(&self.seen),
        })
    }
}

impl SpeechSessionHandle for HeldSession {
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

    fn cancel_response(
        &mut self,
        response_id: inference_duplex::ResponseId,
    ) -> Result<(), SessionError> {
        self.inner.cancel_response(response_id)
    }

    fn truncate_output(
        &mut self,
        response_id: inference_duplex::ResponseId,
        heard: inference_duplex::inference_audio::SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.inner.truncate_output(response_id, heard)
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        lock(&self.seen)
            .submitted
            .push((call_id.clone(), result.clone()));
        self.inner.submit_tool_result(call_id, result)
    }

    fn close(&mut self) -> Result<(), SessionError> {
        lock(&self.seen).closes += 1;
        self.inner.close()
    }

    fn next_event(&mut self, wait: Duration) -> Option<ModelEvent> {
        if let Some(event) = self.due.take() {
            lock(&self.seen).yielded.push(event.clone());
            return Some(event);
        }
        loop {
            if self.holds && self.created && !self.gate.load(Ordering::Acquire) {
                thread::sleep(wait);
                return None;
            }
            let event = self.inner.next_event(wait)?;
            if self.reordering {
                if matches!(event, ModelEvent::InputTranscribed(_)) {
                    self.transcripts += 1;
                    if self.transcripts == 1 {
                        self.late = Some(event);
                        continue;
                    }
                    if self.transcripts == 3 {
                        continue;
                    }
                } else if matches!(event, ModelEvent::InputCommitted(_)) {
                    self.commits += 1;
                    if self.commits == 3 {
                        self.due = self.late.take();
                    }
                }
            }
            if matches!(event, ModelEvent::ResponseCreated(_)) {
                self.created = true;
            }
            if self.untranscribed && matches!(event, ModelEvent::InputTranscribed(_)) {
                continue;
            }
            lock(&self.seen).yielded.push(event.clone());
            return Some(event);
        }
    }
}

// The test pipeline.

const AUDIO: &str = "audio";
const CONTROL: &str = "control";
const EVENTS: &str = "events";
const TOOL_CALL: &str = "tool_call";
const TOOL_RESULT: &str = "tool_result";

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

/// The node alone over `adapter`, declared, its ports, and the pipeline's identity and catalog;
/// with `tools`, its tool ports exposed too.
fn declared(adapter: &Adapter, tools: bool) -> (Declaration, Fixture, PipelineId, DuplexPorts) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let adapter_id = adapter.adapter_id();
    let catalog = Fixture {
        profile,
        adapter: adapter.clone(),
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
    let spec = PipelineSpec {
        name: "native voice".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::Duplex,
        required_capabilities: vec![Capability::NativeDuplex],
    };
    let room = NonZeroUsize::new(64).expect("64");
    let mut declaration = Declaration::new();
    let (pipeline_id, ports) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let speech = pipeline.node("speech", NodeRole::Model, vec![Capability::NativeDuplex]);
            let audio = pipeline.input::<modality::Audio>(&speech, AUDIO, stream);
            let control = pipeline.input::<modality::Control>(&speech, CONTROL, control);
            let events = pipeline.output::<modality::Structured>(&speech, EVENTS, stream);
            let tool_call = pipeline.output::<modality::Structured>(&speech, TOOL_CALL, structured);
            let tool_result =
                pipeline.input::<modality::Structured>(&speech, TOOL_RESULT, structured);
            pipeline.expose(&audio, AUDIO, room, OverloadPolicy::Pause);
            pipeline.expose(&control, CONTROL, room, OverloadPolicy::Pause);
            pipeline.expose(&events, EVENTS, room, OverloadPolicy::Pause);
            if tools {
                pipeline.expose(&tool_call, TOOL_CALL, room, OverloadPolicy::Pause);
                pipeline.expose(&tool_result, TOOL_RESULT, room, OverloadPolicy::Pause);
            }
            pipeline
                .bind(&speech, adapter_id)
                .expect("the catalog holds the mock's adapter");
            DuplexPorts {
                audio: audio.id(),
                control: control.id(),
                events: events.id(),
                tool_call: Some(tool_call.id()),
                tool_result: Some(tool_result.id()),
            }
        })
        .expect("the profile is declared");
    (declaration, catalog, pipeline_id, ports)
}

/// The test pipeline, validated and planned.
fn planned(adapter: &Adapter, tools: bool) -> (Declaration, PlannedPipeline, DuplexPorts) {
    let (mut declaration, catalog, pipeline_id, ports) = declared(adapter, tools);
    let validated = declaration
        .validate(pipeline_id)
        .expect("the node alone between its ingresses and its egress is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the node is bound to the mock's adapter");
    (declaration, planned, ports)
}

/// The one `weather` tool of the cases: a declaration takes a fresh identity, so the session
/// and the node are given this same one.
fn weather() -> Tool {
    static WEATHER: OnceLock<Tool> = OnceLock::new();
    WEATHER.get_or_init(|| tool("weather")).clone()
}

fn tool(name: &str) -> Tool {
    Tool::declare(DeclareTool {
        name: name.to_owned(),
        description: String::new(),
        schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

/// The configuration the SDK runs a native model in: server turn detection, each turn answered
/// at once, detected speech interrupting a reply, offering `tools`.
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

fn say(heard: &str, reply: &str) -> ScriptedTurn {
    ScriptedTurn {
        heard: heard.to_owned(),
        reply: Reply::Say(reply.to_owned()),
    }
}

fn call(tool: &str, then: &str) -> ScriptedTurn {
    ScriptedTurn {
        heard: "how warm is it in Berlin".to_owned(),
        reply: Reply::Call {
            tool: tool.to_owned(),
            arguments: json!({"city": "Berlin"}),
            then: then.to_owned(),
        },
    }
}

/// The application's side of a stream it sends on.
struct Sender {
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Sender {
    fn new() -> Self {
        Self {
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
            sequence: 0,
        }
    }

    fn envelope(&mut self, port: &str, kind: &str, payload: Voice) -> EventEnvelope<Voice> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: inference_core::id::RunId::generate(),
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

    fn frame(&mut self, frame: AudioFrame) -> EventEnvelope<Voice> {
        self.envelope(AUDIO, "inference.audio.AudioFrame", Voice::Audio(frame))
    }

    fn control(&mut self, port: &str, kind: ControlKind) -> EventEnvelope<Voice> {
        self.envelope(port, kind.as_str(), Voice::Control(kind))
    }

    fn update(&mut self, update: ToolCallUpdate) -> EventEnvelope<Voice> {
        let kind = update.event.name();
        self.envelope(TOOL_RESULT, kind, Voice::Tool(update))
    }
}

// A live session over the test pipeline.

struct Live {
    session: LiveSession<Voice>,
    driver: mpsc::Receiver<Result<inference_runtime::Run, RunError>>,
    sender: Sender,
    node_id: NodeId,
    delivered: Vec<SessionEvent>,
}

/// Opens a live session over the test pipeline whose node runs `model` in `config`, proposing
/// `tools`, and drives its run on a thread of its own.
fn live(model: &Held, config: SessionConfig, tools: Vec<Tool>) -> Live {
    let (declaration, planned, ports) = planned(model.adapter(), false);
    let node_id = planned_node(&declaration);
    let implement = {
        let model = model.clone();
        move |run_id, _: &Node| -> Box<dyn NodeImplementation<Voice>> {
            let node = DuplexNode::new(run_id, model.clone(), config.clone(), ports, RunLog::new())
                .with_tools(tools.clone());
            Box::new(node)
        }
    };
    let sessions = LiveSessions::new();
    let (session, driver) =
        wait(sessions.open(&declaration, &planned, InteractionMode::Duplex, implement))
            .unwrap_or_else(|error| panic!("the session opens: {error}"));
    Live {
        session,
        driver: spawn(move || driver.drive()),
        sender: Sender::new(),
        node_id,
        delivered: Vec::new(),
    }
}

/// The identity of the one node `declaration` holds.
fn planned_node(declaration: &Declaration) -> NodeId {
    declaration.nodes().next().expect("one node").node_id()
}

impl Live {
    fn feed(&mut self, frames: Vec<AudioFrame>) {
        for frame in frames {
            let envelope = self.sender.frame(frame);
            wait(self.session.send(envelope)).unwrap_or_else(|error| panic!("sent: {error}"));
        }
    }

    fn control(&mut self, port: &str, kind: ControlKind) {
        let envelope = self.sender.control(port, kind);
        wait(self.session.send(envelope)).unwrap_or_else(|error| panic!("sent: {error}"));
    }

    /// The next event of the session, none once it ended.
    fn next(&mut self) -> Option<SessionEvent> {
        let event = wait(self.session.receive());
        self.delivered.extend(event.clone());
        event
    }

    /// The events up to and including the first that `stop` answers.
    fn until(&mut self, stop: impl Fn(&SessionEvent) -> bool) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        loop {
            let event = self
                .next()
                .unwrap_or_else(|| panic!("the session ended first: {events:#?}"));
            let done = stop(&event);
            events.push(event);
            if done {
                return events;
            }
        }
    }

    /// Every event left, to the session's end.
    fn rest(&mut self) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.next() {
            events.push(event);
        }
        events
    }

    fn close(&mut self) {
        wait(self.session.close()).unwrap_or_else(|error| panic!("closes: {error}"));
    }

    fn ended(&self) -> Result<inference_runtime::Run, RunError> {
        joined(&self.driver)
    }
}

/// The run's events of `events`: none of the session's own lifecycle.
fn run_events(events: &[SessionEvent]) -> Vec<&SessionEvent> {
    events
        .iter()
        .filter(|event| {
            !matches!(
                event,
                SessionEvent::SessionOpened(_)
                    | SessionEvent::RunAttached(_)
                    | SessionEvent::SessionReady(_)
                    | SessionEvent::InputSent(_)
                    | SessionEvent::InputCommitted(_)
                    | SessionEvent::SessionInterrupted(_)
                    | SessionEvent::SessionDraining(_)
                    | SessionEvent::SessionClosed(_)
                    | SessionEvent::HistoryRecorded(_)
                    | SessionEvent::RunCompleted(_)
                    | SessionEvent::RunCancelled(_)
                    | SessionEvent::RunFailed(_)
            )
        })
        .collect()
}

fn name(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::SpeechStarted(_) => "SpeechStarted",
        SessionEvent::TranscriptCommitted(_) => "TranscriptCommitted",
        SessionEvent::UsageRecorded(_) => "UsageRecorded",
        SessionEvent::GenerationStarted(_) => "GenerationStarted",
        SessionEvent::TextDelta(_) => "TextDelta",
        SessionEvent::TextSegmentCommitted(_) => "TextSegmentCommitted",
        SessionEvent::AudioChunk(_) => "AudioChunk",
        SessionEvent::GenerationCompleted(_) => "GenerationCompleted",
        SessionEvent::GenerationCancelled(_) => "GenerationCancelled",
        SessionEvent::GenerationSuperseded(_) => "GenerationSuperseded",
        SessionEvent::SpeechResponseFailed(_) => "SpeechResponseFailed",
        SessionEvent::SpeechSessionClosed(_) => "SpeechSessionClosed",
        SessionEvent::SpeechSessionFailed(_) => "SpeechSessionFailed",
        SessionEvent::ToolCallProposed(_) => "ToolCallProposed",
        SessionEvent::SessionClosed(_) => "SessionClosed",
        SessionEvent::RunCompleted(_) => "RunCompleted",
        SessionEvent::RunCancelled(_) => "RunCancelled",
        SessionEvent::RunFailed(_) => "RunFailed",
        _ => "other",
    }
}

fn count(events: &[SessionEvent], wanted: &str) -> usize {
    events.iter().filter(|event| name(event) == wanted).count()
}

fn is(wanted: &'static str) -> impl Fn(&SessionEvent) -> bool {
    move |event| name(event) == wanted
}

fn started(events: &[SessionEvent]) -> Vec<GenerationId> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::GenerationStarted(started) => Some(started.generation_id),
            _ => None,
        })
        .collect()
}

fn utterances(count: usize) -> Vec<Vec<AudioFrame>> {
    let mut at = 0;
    (0..count)
        .map(|_| {
            let spoken = utterance(pcm(16_000), at);
            at = spoken.next();
            spoken.frames()
        })
        .collect()
}

// The cases.

#[test]
fn the_test_pipeline_validates_with_no_finding() {
    start();
    let model = MockSpeechModel::native(vec![say("hello", "Hello there")]);
    for tools in [false, true] {
        let (mut declaration, _, pipeline_id, _) = declared(model.adapter(), tools);
        let validated = declaration.validate(pipeline_id);
        assert!(validated.is_ok(), "{validated:?}");
    }
}

/// R139, R134: transcript, reply audio and usage reach the session; two usage records per `Say`
/// turn, none for the duplex's own usage events, and a `SpeechStarted` per turn before its
/// transcript.
#[test]
fn a_session_delivers_the_transcript_the_reply_and_its_usage() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![
        say("hello", "Hello there"),
        say("and you", "Fine"),
    ]));
    let mut live = live(&model, config(Vec::new()), Vec::new());
    let mut turns = utterances(2).into_iter();
    live.feed(turns.next().expect("one"));
    let first = live.until(is("GenerationCompleted"));
    live.feed(turns.next().expect("two"));
    let second = live.until(is("GenerationCompleted"));
    live.close();
    let tail = live.rest();

    let mut one = first.clone();
    one.extend(second.clone());
    let frames = usize::try_from(AUDIO_FRAMES).expect("a few frames");
    assert_eq!(count(&one, "AudioChunk"), 2 * frames);
    for turn in [&first, &second] {
        let records: Vec<&UsageRecorded> = turn
            .iter()
            .filter_map(|event| match event {
                SessionEvent::UsageRecorded(recorded) => Some(recorded),
                _ => None,
            })
            .collect();
        assert_eq!(records.len(), 2, "{turn:#?}");
        assert_eq!(
            records
                .iter()
                .filter(|recorded| recorded.response_id.is_some())
                .count(),
            1,
            "one record names the response and one names none: {records:#?}"
        );
        let speech = turn
            .iter()
            .position(is("SpeechStarted"))
            .unwrap_or_else(|| panic!("speech started: {turn:#?}"));
        let heard = turn
            .iter()
            .position(is("TranscriptCommitted"))
            .unwrap_or_else(|| panic!("the turn was heard: {turn:#?}"));
        assert!(speech < heard, "{turn:#?}");
        assert_eq!(count(turn, "SpeechStarted"), 1, "{turn:#?}");
    }
    assert!(one.iter().any(|event| matches!(
        event,
        SessionEvent::TranscriptCommitted(committed) if committed.text == "hello"
    )));
    assert!(one.iter().any(|event| matches!(
        event,
        SessionEvent::TextSegmentCommitted(segment) if segment.text == "Hello there"
    )));
    assert_eq!(count(&tail, "UsageRecorded"), 0, "{tail:#?}");
    let yielded = model.seen().yielded.clone();
    assert!(
        yielded
            .iter()
            .any(|event| matches!(event, ModelEvent::ResponseUsageRecorded(_)))
    );
    assert!(
        yielded
            .iter()
            .any(|event| matches!(event, ModelEvent::TranscriptionUsageRecorded(_)))
    );
    assert_eq!(count(&live.delivered, "UsageRecorded"), 4);
    assert!(live.ended().is_ok());
}

/// R142: one egress, and one `Say` turn in the node's emission order.
#[test]
fn one_say_turn_reaches_the_session_in_the_order_the_node_emits_it() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![say("hello", "Hello there")]));
    let (declaration, _, _) = planned(model.adapter(), false);
    let egresses: Vec<&str> = declaration
        .exposures()
        .filter(|exposure| {
            declaration
                .port(exposure.port_id())
                .is_some_and(|port| port.direction() == PortDirection::Output)
        })
        .map(|exposure| exposure.name())
        .collect();
    assert_eq!(
        egresses,
        [EVENTS],
        "the node's events port is the one egress"
    );

    let mut live = live(&model, config(Vec::new()), Vec::new());
    live.feed(utterances(1).remove(0));
    let turn = live.until(is("GenerationCompleted"));
    let order: Vec<&str> = run_events(&turn).into_iter().map(name).collect();
    assert_eq!(
        order,
        [
            "SpeechStarted",
            "TranscriptCommitted",
            "UsageRecorded",
            "GenerationStarted",
            "TextDelta",
            "TextSegmentCommitted",
            "AudioChunk",
            "AudioChunk",
            "UsageRecorded",
            "GenerationCompleted",
        ]
    );
    live.close();
    live.rest();
    assert!(live.ended().is_ok());
}

/// R143: a failed response reaches the session as `SpeechResponseFailed`, then the cancel of
/// its generation, and never completes.
#[test]
fn a_failed_response_reaches_the_session_before_its_generation_is_cancelled() {
    start();
    let model =
        Held::over(MockSpeechModel::native(vec![say("hello", "Hello there")]).fails_responses());
    let mut live = live(&model, config(Vec::new()), Vec::new());
    live.feed(utterances(1).remove(0));
    let turn = live.until(is("GenerationCancelled"));
    let generation = started(&turn)[0];
    let response = model
        .seen()
        .yielded
        .iter()
        .find_map(|event| match event {
            ModelEvent::ResponseCreated(created) => Some(created.response_id),
            _ => None,
        })
        .expect("a response was created");
    let failed = turn
        .iter()
        .position(|event| {
            matches!(
                event,
                SessionEvent::SpeechResponseFailed(failed)
                    if failed.response_id == response
                        && failed.failure.kind == SessionFailureKind::ProviderError
            )
        })
        .unwrap_or_else(|| panic!("the failure reaches the session: {turn:#?}"));
    let cancelled = turn
        .iter()
        .position(|event| matches!(
            event,
            SessionEvent::GenerationCancelled(cancelled) if cancelled.generation_id == generation
        ))
        .expect("the generation is cancelled");
    assert!(failed < cancelled, "{turn:#?}");
    live.close();
    live.rest();
    assert_eq!(count(&live.delivered, "GenerationCompleted"), 0);
    assert!(live.ended().is_ok());
}

/// `barge-in-cancels-response`: speech started while the reply is in progress cancels it, and
/// the session sees one `GenerationCancelled` of the interrupted reply's generation.
#[test]
fn a_barge_in_reaches_the_session_as_one_cancelled_generation() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![
        say("tell me a story", "Once upon a time"),
        say("stop", "Sure"),
    ]))
    .holding();
    let mut live = live(&model, config(Vec::new()), Vec::new());
    let mut turns = utterances(2).into_iter();
    live.feed(turns.next().expect("one"));
    let first = live.until(is("GenerationStarted"));
    let interrupted = started(&first)[0];
    live.feed(turns.next().expect("two"));
    model.open_gate();
    live.until(|event| {
        matches!(event, SessionEvent::GenerationCompleted(completed) if completed.generation_id != interrupted)
    });
    live.close();
    live.rest();
    let cancelled: Vec<GenerationId> = live
        .delivered
        .iter()
        .filter_map(|event| match event {
            SessionEvent::GenerationCancelled(cancelled) => Some(cancelled.generation_id),
            _ => None,
        })
        .collect();
    assert_eq!(cancelled, [interrupted], "{:#?}", live.delivered);
    assert!(live.ended().is_ok());
}

/// `input-during-output-continues-response`: input during the reply is answered on the same
/// generation, with nothing superseded or cancelled.
#[test]
fn input_during_output_continues_the_same_generation() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![
        say("what is the weather", "The weather in"),
        say("Berlin", " Berlin is mild"),
    ]))
    .holding();
    let mut answering = config(Vec::new());
    answering.auto_interrupt = false;
    let mut live = live(&model, answering, Vec::new());
    let mut turns = utterances(2).into_iter();
    live.feed(turns.next().expect("one"));
    live.until(is("GenerationStarted"));
    live.feed(turns.next().expect("two"));
    model.open_gate();
    live.until(is("GenerationCompleted"));
    live.close();
    live.rest();

    let delivered = &live.delivered;
    assert_eq!(
        count(delivered, "GenerationSuperseded"),
        0,
        "{delivered:#?}"
    );
    assert_eq!(count(delivered, "GenerationCancelled"), 0, "{delivered:#?}");
    let deltas: Vec<(GenerationId, &str)> = delivered
        .iter()
        .filter_map(|event| match event {
            SessionEvent::TextDelta(delta) => Some((delta.generation_id, delta.text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(deltas.len(), 2, "{delivered:#?}");
    assert_eq!(deltas[0].1, "The weather in");
    assert_eq!(deltas[1].1, " Berlin is mild");
    assert_eq!(deltas[0].0, deltas[1].0, "both on one generation");
    assert!(live.ended().is_ok());
}

/// Closing the session delivers every event the binding received before the close, then
/// `SessionClosed`.
#[test]
fn closing_the_session_delivers_what_the_binding_received_then_session_closed() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![say("hello", "Hello there")]));
    let mut live = live(&model, config(Vec::new()), Vec::new());
    live.feed(utterances(1).remove(0));
    live.until(is("TranscriptCommitted"));
    live.close();
    live.rest();

    let delivered = &live.delivered;
    assert!(
        matches!(delivered.last(), Some(SessionEvent::SessionClosed(_))),
        "{delivered:#?}"
    );
    let yielded = model.seen().yielded.clone();
    let of = |wanted: fn(&ModelEvent) -> bool| yielded.iter().filter(|event| wanted(event)).count();
    assert_eq!(
        count(delivered, "UsageRecorded"),
        of(|event| matches!(event, ModelEvent::UsageRecorded(_)))
    );
    assert_eq!(
        count(delivered, "AudioChunk"),
        of(|event| matches!(event, ModelEvent::OutputAudio(_)))
    );
    assert_eq!(
        count(delivered, "TextDelta"),
        of(|event| matches!(event, ModelEvent::OutputTranscriptDelta(_)))
    );
    assert_eq!(
        count(delivered, "GenerationStarted"),
        of(|event| matches!(event, ModelEvent::ResponseCreated(_)))
    );
    assert_eq!(
        count(delivered, "GenerationCompleted") + count(delivered, "GenerationCancelled"),
        count(delivered, "GenerationStarted"),
        "every reply ends: {delivered:#?}"
    );
    assert_eq!(count(delivered, "SpeechSessionClosed"), 0, "{delivered:#?}");
    assert!(live.ended().is_ok());
}

/// R160, R176: a `Cancel` on the audio ingress and another on the control ingress cancel the
/// reply in progress once; a `Cancel` with no reply in progress does nothing.
#[test]
fn a_cancel_cancels_the_reply_in_progress_once() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![
        say("hello", "Hello there"),
        say("again", "Hello again"),
    ]))
    .holding();
    let mut live = live(&model, config(Vec::new()), Vec::new());
    let mut turns = utterances(2).into_iter();
    live.feed(turns.next().expect("one"));
    let first = live.until(is("GenerationStarted"));
    let cancelled = started(&first)[0];
    live.control(AUDIO, ControlKind::Cancel);
    live.control(CONTROL, ControlKind::Cancel);
    model.open_gate();
    let cut = live.until(is("GenerationCancelled"));
    assert!(matches!(
        cut.last(),
        Some(SessionEvent::GenerationCancelled(event)) if event.generation_id == cancelled
    ));

    live.control(CONTROL, ControlKind::Cancel);
    live.feed(turns.next().expect("two"));
    let second = live.until(is("GenerationCompleted"));
    live.close();
    live.rest();

    assert_eq!(
        count(&live.delivered, "GenerationCancelled"),
        1,
        "{:#?}",
        live.delivered
    );
    let of_first = live
        .delivered
        .iter()
        .filter(|event| match event {
            SessionEvent::GenerationCompleted(event) => event.generation_id == cancelled,
            _ => false,
        })
        .count();
    assert_eq!(of_first, 0);
    let reply: Vec<&str> = run_events(&second)
        .into_iter()
        .map(name)
        .filter(|name| *name != "SpeechStarted" && *name != "TranscriptCommitted")
        .skip_while(|name| *name != "GenerationStarted")
        .collect();
    assert_eq!(
        reply,
        [
            "GenerationStarted",
            "TextDelta",
            "TextSegmentCommitted",
            "AudioChunk",
            "AudioChunk",
            "UsageRecorded",
            "GenerationCompleted",
        ],
        "{second:#?}"
    );
    let audio_of_first = cut
        .iter()
        .chain(&first)
        .filter(|event| matches!(event, SessionEvent::AudioChunk(_)))
        .count();
    assert_eq!(audio_of_first, 0);
    assert!(live.ended().is_ok());
}

/// R160, R176: closing while a reply is in progress cancels it, delivers its cancel and its
/// usage before `SessionClosed`, and nothing after.
#[test]
fn a_drain_cancels_the_reply_in_progress_and_delivers_its_usage_before_session_closed() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![say("hello", "Hello there")])).holding();
    let mut live = live(&model, config(Vec::new()), Vec::new());
    live.feed(utterances(1).remove(0));
    let first = live.until(is("GenerationStarted"));
    let generation = started(&first)[0];
    live.close();
    model.open_gate();
    let rest = live.rest();

    let closed = rest
        .iter()
        .position(is("SessionClosed"))
        .unwrap_or_else(|| panic!("the session closes: {rest:#?}"));
    assert_eq!(
        closed,
        rest.len() - 1,
        "nothing after SessionClosed: {rest:#?}"
    );
    let before = &rest[..closed];
    assert!(
        before.iter().any(|event| matches!(
            event,
            SessionEvent::GenerationCancelled(cancelled) if cancelled.generation_id == generation
        )),
        "{rest:#?}"
    );
    assert!(
        before.iter().any(|event| matches!(
            event,
            SessionEvent::UsageRecorded(recorded) if recorded.response_id.is_some()
        )),
        "{rest:#?}"
    );
    assert_eq!(count(&rest, "SpeechSessionClosed"), 0, "{rest:#?}");
    assert_eq!(model.seen().closes, 1);
    assert!(live.ended().is_ok());
}

/// R160: cancelling the run mid-reply ends it cancelled; the node closed its model session
/// once and let go of it.
#[test]
fn cancelling_the_run_closes_the_model_session_once_and_drops_it() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![say("hello", "Hello there")])).holding();
    let mut live = live(&model, config(Vec::new()), Vec::new());
    live.feed(utterances(1).remove(0));
    live.until(is("GenerationStarted"));
    live.session.cancel().expect("the session is open");
    let rest = live.rest();
    assert!(rest.iter().any(is("RunCancelled")), "{rest:#?}");
    assert!(live.ended().is_ok());
    let seen = model.seen();
    assert_eq!(seen.closes, 1);
    assert!(seen.dropped);
}

/// The closed notification: a model that closes its own session reaches the session as one
/// `SpeechSessionClosed`; the application's close then completes the run.
#[test]
fn a_model_that_closes_its_session_is_reported_once_and_the_close_completes_the_run() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![say("hello", "Hello there")]).closes());
    let mut live = live(&model, config(Vec::new()), Vec::new());
    live.feed(utterances(1).remove(0));
    let turn = live.until(is("SpeechSessionClosed"));
    assert!(turn.iter().any(is("GenerationCompleted")), "{turn:#?}");
    live.close();
    let rest = live.rest();

    assert_eq!(count(&live.delivered, "SpeechSessionClosed"), 1);
    assert!(rest.iter().any(is("RunCompleted")), "{rest:#?}");
    assert!(
        matches!(rest.last(), Some(SessionEvent::SessionClosed(_))),
        "{rest:#?}"
    );
    assert!(live.ended().is_ok());
}

/// Audio after the close: frames the microphone keeps sending after the model closed are
/// dropped, and the run still completes.
#[test]
fn audio_after_the_model_closed_is_dropped_and_the_run_completes() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![say("hello", "Hello there")]).closes());
    let mut live = live(&model, config(Vec::new()), Vec::new());
    let mut turns = utterances(5).into_iter();
    live.feed(turns.next().expect("one"));
    live.until(is("SpeechSessionClosed"));
    let more: Vec<AudioFrame> = turns.flatten().take(10).collect();
    assert_eq!(more.len(), 10);
    live.feed(more);
    live.close();
    let rest = live.rest();
    assert!(rest.iter().any(is("RunCompleted")), "{rest:#?}");
    assert_eq!(count(&live.delivered, "RunFailed"), 0);
    assert!(live.ended().is_ok());
}

fn failing() -> Held {
    Held::over(
        MockSpeechModel::native(vec![say("hello", "Hello there")]).with_faults(Faults {
            close_after: Some(3),
            ..Faults::default()
        }),
    )
}

/// What a failed model session delivers, unprompted.
fn failure_reaches(live: &mut Live) {
    let events = live.until(is("SpeechSessionFailed"));
    let failed = events
        .iter()
        .position(is("SpeechResponseFailed"))
        .unwrap_or_else(|| panic!("the response fails first: {events:#?}"));
    assert!(failed < events.len() - 1);
    assert_eq!(count(&events, "SpeechSessionClosed"), 0);
}

/// R158, R170: a failed model session reaches the session at once; a cancel then ends the run
/// cancelled.
#[test]
fn a_failed_model_session_is_reported_and_a_cancel_ends_the_run() {
    start();
    let model = failing();
    let mut live = live(&model, config(Vec::new()), Vec::new());
    live.feed(utterances(1).remove(0));
    failure_reaches(&mut live);
    live.session.cancel().expect("the session is open");
    let rest = live.rest();
    assert!(rest.iter().any(is("RunCancelled")), "{rest:#?}");
    assert_eq!(count(&live.delivered, "SpeechSessionClosed"), 0);
    assert!(live.ended().is_ok());
}

/// R158, R170: after a failed model session the node took no more input; the next frame is
/// refused and the run fails naming the node.
#[test]
fn a_failed_model_session_fails_the_run_at_the_next_frame() {
    start();
    let model = failing();
    let mut live = live(&model, config(Vec::new()), Vec::new());
    let mut turns = utterances(2).into_iter();
    live.feed(turns.next().expect("one"));
    failure_reaches(&mut live);
    let frame = turns.next().expect("two").remove(0);
    let envelope = live.sender.frame(frame);
    assert!(
        wait(live.session.send(envelope)).is_err(),
        "the frame is refused"
    );
    let rest = live.rest();
    assert_eq!(count(&rest, "RunFailed"), 1, "{rest:#?}");
    match live.ended() {
        Err(RunError::NodeFailed(failure)) => {
            assert_eq!(failure.node_id, Some(live.node_id));
            assert!(failure.detail.contains("TransportClosed"), "{failure:?}");
        }
        other => panic!("the run fails naming the node: {other:?}"),
    }
}

// The tool ports, read off a run of the test pipeline.

struct Run {
    ingress: HashMap<String, Box<dyn OutputEndpoint<Voice>>>,
    events: Box<dyn InputEndpoint<Voice>>,
    tool_call: Box<dyn InputEndpoint<Voice>>,
    driver: mpsc::Receiver<Result<(), RunError>>,
    requests: RunRequests,
    sender: Sender,
    call_ids: CallIds,
}

/// A run of the test pipeline with its tool ports exposed, its node running `model` in
/// `config` and proposing `tools`.
fn run(model: &Held, config: SessionConfig, tools: Vec<Tool>) -> Run {
    let (declaration, planned, ports) = planned(model.adapter(), true);
    let run_id = inference_core::id::RunId::generate();
    let node =
        DuplexNode::new(run_id, model.clone(), config, ports, RunLog::new()).with_tools(tools);
    let call_ids = node.call_ids();
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
    let ingress = [AUDIO, CONTROL, TOOL_RESULT]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                ends.ingress(name).expect("the ingress is exposed"),
            )
        })
        .collect();
    let events = ends.egress(EVENTS).expect("events is exposed");
    let tool_call = ends.egress(TOOL_CALL).expect("tool_call is exposed");
    let requests = controller.requests();
    let driver = spawn(move || async move { controller.drive().await });
    Run {
        ingress,
        events,
        tool_call,
        driver,
        requests,
        sender: Sender::new(),
        call_ids,
    }
}

impl Run {
    fn send(&mut self, envelope: EventEnvelope<Voice>) {
        let port = envelope.port.clone();
        wait(self.ingress[&port].send(envelope))
            .unwrap_or_else(|_| panic!("the ingress {port} takes it"));
    }

    fn feed(&mut self, frames: Vec<AudioFrame>) {
        for frame in frames {
            let envelope = self.sender.frame(frame);
            self.send(envelope);
        }
    }

    fn answer(&mut self, update: ToolCallUpdate) {
        let envelope = self.sender.update(update);
        self.send(envelope);
    }

    /// The payloads of `events` up to and including the first that `stop` answers.
    fn events_until(&mut self, stop: impl Fn(&Voice) -> bool) -> Vec<Voice> {
        let mut payloads = Vec::new();
        loop {
            let envelope = wait(self.events.recv())
                .unwrap_or_else(|| panic!("events ended first: {payloads:#?}"));
            let done = stop(&envelope.payload);
            payloads.push(envelope.payload);
            if done {
                return payloads;
            }
        }
    }

    /// Ends the run's input and answers what `tool_call` carried, the run's end awaited.
    fn finish(mut self) -> Vec<Voice> {
        for port in [AUDIO, CONTROL, TOOL_RESULT] {
            let stop = self.sender.control(port, ControlKind::Stop);
            self.send(stop);
        }
        let mut carried = Vec::new();
        while let Some(envelope) = wait(self.tool_call.recv()) {
            match envelope.payload {
                Voice::Control(kind) => assert_eq!(kind, ControlKind::Stop, "only its end"),
                payload => carried.push(payload),
            }
        }
        while wait(self.events.recv()).is_some() {}
        joined(&self.driver).expect("the run completes");
        carried
    }
}

fn completed_stream(payload: &Voice) -> bool {
    matches!(payload, Voice::Stream(StreamEvent::GenerationCompleted(_)))
}

/// The proposal `tool_call` carries next.
fn proposal(run: &mut Run) -> ToolCall {
    let envelope = wait(run.tool_call.recv()).expect("tool_call carries the proposal");
    match envelope.payload {
        Voice::Tool(ToolCallUpdate {
            call,
            event: ToolCallEvent::Proposed(_),
        }) => call,
        other => panic!("a proposal: {other:?}"),
    }
}

/// `call` answered with `result`, through the states the tool nodes move it.
fn answered(call: &ToolCall, result: Json) -> ToolCallUpdate {
    let (call, _) = call
        .authorize(&AuthorizeToolCall {
            call_id: call.call_id(),
            confirmation_required: false,
            confirmed: false,
        })
        .expect("proposed");
    let (call, _) = call.start().expect("authorized");
    let (call, event) = call.complete(result).expect("started");
    ToolCallUpdate {
        call,
        event: ToolCallEvent::Result(event),
    }
}

/// Tool ports: the proposal leaves on `tool_call`, its result reaches the model by its own call
/// id, and an unknown or second result is dropped.
#[test]
fn a_proposed_call_leaves_on_tool_call_and_its_result_reaches_the_model_once() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![
        call("weather", "It is {result} degrees"),
        say("thanks", "You are welcome"),
    ]));
    let mut run = run(&model, config(vec![weather()]), vec![weather()]);
    let mut turns = utterances(2).into_iter();
    run.feed(turns.next().expect("one"));
    let call = proposal(&mut run);
    assert_eq!(call.tool_id(), weather().tool_id());
    assert_eq!(call.arguments(), &json!({"city": "Berlin"}));
    let speech_id = SpeechCallId::new("call_1");
    assert_eq!(
        run.call_ids.speech_call_id(call.call_id()),
        Some(speech_id.clone())
    );

    let stranger = proposal_of_another_call();
    run.answer(answered(&stranger, json!(0)));
    run.answer(answered(&call, json!(21)));
    let reply = run.events_until(|payload| {
        matches!(payload, Voice::Stream(StreamEvent::TextSegmentCommitted(segment)) if segment.text == "It is 21 degrees")
    });
    assert!(!reply.is_empty());
    run.events_until(completed_stream);
    assert_eq!(
        run.call_ids.speech_call_id(call.call_id()),
        Some(speech_id.clone())
    );

    run.answer(answered(&call, json!(22)));
    run.feed(turns.next().expect("two"));
    run.events_until(|payload| {
        matches!(payload, Voice::Stream(StreamEvent::TextSegmentCommitted(segment)) if segment.text == "You are welcome")
    });
    assert_eq!(model.seen().submitted, [(speech_id, json!(21))]);
    let carried = run.finish();
    assert!(carried.is_empty(), "one proposal only: {carried:#?}");
}

/// A transcript belongs to the committed turn it names, whenever it arrives: of three turns, the
/// first one's transcript arrives after the second's and only after the third turn's commit, and
/// the third turn's never arrives. Each call rests on the span of its own turn: the first and the
/// third on the empty span committed at their commit, the second on its transcript, and the first
/// turn's late transcript is committed as its own span, neither the third turn's basis nor the
/// second's.
#[test]
fn a_transcript_pairs_with_its_own_turn_when_transcripts_arrive_out_of_order_or_never() {
    start();
    let turn = |heard: &str, then: &str| ScriptedTurn {
        heard: heard.to_owned(),
        reply: Reply::Call {
            tool: "weather".to_owned(),
            arguments: json!({"city": "Berlin"}),
            then: then.to_owned(),
        },
    };
    let model = Held::over(MockSpeechModel::native(vec![
        turn("first words", "done one"),
        turn("second words", "done two"),
        turn("third words", "done three"),
    ]))
    .reordering();
    let mut run = run(&model, config(vec![weather()]), vec![weather()]);
    let mut spoken = utterances(3).into_iter();
    let mut payloads = Vec::new();
    let mut bases = Vec::new();
    for done in ["done one", "done two", "done three"] {
        run.feed(spoken.next().expect("a turn"));
        let call = proposal(&mut run);
        bases.push(call.basis_span_id());
        run.answer(answered(&call, json!(21)));
        payloads.extend(run.events_until(|payload| {
            matches!(payload, Voice::Stream(StreamEvent::TextSegmentCommitted(segment)) if segment.text == done)
        }));
        payloads.extend(run.events_until(completed_stream));
    }
    run.finish();

    let span_of = |text: &str| {
        let spans: Vec<_> = payloads
            .iter()
            .filter_map(|payload| match payload {
                Voice::Stream(StreamEvent::TranscriptCommitted(committed))
                    if committed.text == text =>
                {
                    Some(committed.span_id)
                }
                _ => None,
            })
            .collect();
        assert_eq!(spans.len(), 1, "{text:?} is committed once: {payloads:#?}");
        spans[0]
    };
    let first = span_of("first words");
    let second = span_of("second words");
    assert!(
        !payloads.iter().any(|payload| matches!(
            payload,
            Voice::Stream(StreamEvent::TranscriptCommitted(committed)) if committed.text == "third words"
        )),
        "the third turn's transcript never arrived"
    );
    assert_eq!(
        bases[1], second,
        "the second call rests on its own transcript"
    );
    assert!(
        bases[0] != first && bases[0] != second,
        "the first call rests on its own turn's empty span"
    );
    assert!(
        bases[2] != first && bases[2] != second,
        "the third call rests on its own turn's empty span, not on another turn's transcript"
    );
    assert_ne!(bases[0], bases[2]);
    let seen = model.seen();
    let items: Vec<&InputItemId> = seen
        .yielded
        .iter()
        .filter_map(|event| match event {
            ModelEvent::InputTranscribed(transcribed) => Some(&transcribed.item),
            _ => None,
        })
        .collect();
    assert_eq!(
        items,
        [&InputItemId::new("turn-2"), &InputItemId::new("turn-1")],
        "the transcripts arrived out of order"
    );
}

/// A call of another proposal, which the node never minted.
fn proposal_of_another_call() -> ToolCall {
    use inference_core::generation::{Generation, StartGeneration};
    use inference_text::{OpenTurn, Span, Turn};
    let run_id = inference_core::id::RunId::generate();
    let (generation, _) = Generation::start(StartGeneration {
        run_id,
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (turn, _) = Turn::open(OpenTurn { run_id });
    let (span, _) =
        Span::emit(&turn, run_id, StreamId::generate(), 1, "hi".to_owned()).expect("an open turn");
    let (span, _) = span.commit().expect("a hypothesis");
    ToolCall::propose(&weather(), &generation, &span, json!({"city": "Lisbon"}))
        .expect("active and committed")
        .0
}

/// R103: a proposal rests on the generation of its response and on the committed span of the
/// turn's transcript.
#[test]
fn a_proposal_names_its_responses_generation_and_the_transcript_it_rests_on() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![call(
        "weather",
        "It is {result} degrees",
    )]));
    let mut run = run(&model, config(vec![weather()]), vec![weather()]);
    run.feed(utterances(1).remove(0));
    let before = run.events_until(|payload| matches!(payload, Voice::Tool(_)));
    let call = match before.last() {
        Some(Voice::Tool(update)) => update.call.clone(),
        other => panic!("the proposal on events: {other:?}"),
    };
    let generation = before
        .iter()
        .find_map(|payload| match payload {
            Voice::Stream(StreamEvent::GenerationStarted(started)) => Some(started.generation_id),
            _ => None,
        })
        .expect("the response's generation started");
    let span = before
        .iter()
        .find_map(|payload| match payload {
            Voice::Stream(StreamEvent::TranscriptCommitted(committed)) => {
                Some((committed.span_id, committed.text.clone()))
            }
            _ => None,
        })
        .expect("the transcript is committed");
    assert_eq!(call.generation_id(), generation);
    assert_eq!(call.basis_span_id(), span.0);
    assert_eq!(span.1, "how warm is it in Berlin");
    assert_eq!(proposal(&mut run).call_id(), call.call_id());
    let after = run.events_until(completed_stream);
    assert!(matches!(
        after.last(),
        Some(Voice::Stream(StreamEvent::GenerationCompleted(completed))) if completed.generation_id == generation
    ));
    run.finish();
}

/// R103: with no transcript, a proposal rests on the empty span committed at the input's
/// commit.
#[test]
fn a_proposal_on_an_untranscribed_turn_rests_on_the_empty_committed_span() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![call(
        "weather",
        "It is {result} degrees",
    )]))
    .untranscribed();
    let log = RunLog::new();
    let (declaration, planned, ports) = planned(model.adapter(), true);
    let run_id = inference_core::id::RunId::generate();
    let node = DuplexNode::new(
        run_id,
        model.clone(),
        config(vec![weather()]),
        ports,
        log.clone(),
    )
    .with_tools(vec![weather()]);
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
    let audio = ends.ingress(AUDIO).expect("audio");
    let control = ends.ingress(CONTROL).expect("control");
    let results = ends.ingress(TOOL_RESULT).expect("tool_result");
    let mut events = ends.egress(EVENTS).expect("events");
    let mut tool_call = ends.egress(TOOL_CALL).expect("tool_call");
    let driver = spawn(move || async move { controller.drive().await });
    let mut sender = Sender::new();
    for frame in utterances(1).remove(0) {
        wait(audio.send(sender.frame(frame))).unwrap_or_else(|_| panic!("taken"));
    }
    let proposed = loop {
        let envelope = wait(tool_call.recv()).expect("a proposal");
        if let Voice::Tool(update) = envelope.payload {
            break update.call;
        }
    };
    let transcripts = log
        .envelopes()
        .into_iter()
        .filter(|envelope| matches!(envelope.payload, StreamEvent::TranscriptCommitted(_)))
        .count();
    assert_eq!(
        transcripts, 0,
        "no transcript was emitted, so the call's committed basis is the commit's empty span"
    );
    assert_eq!(proposed.generation_id(), started_generation(&mut events));
    for (endpoint, port) in [
        (&audio, AUDIO),
        (&control, CONTROL),
        (&results, TOOL_RESULT),
    ] {
        wait(endpoint.send(sender.control(port, ControlKind::Stop)))
            .unwrap_or_else(|_| panic!("taken"));
    }
    while wait(events.recv()).is_some() {}
    while wait(tool_call.recv()).is_some() {}
    joined(&driver).expect("the run completes");
}

/// The generation the next `GenerationStarted` on `events` names.
fn started_generation(events: &mut Box<dyn InputEndpoint<Voice>>) -> GenerationId {
    loop {
        let envelope = wait(events.recv()).expect("events carries the generation");
        if let Voice::Stream(StreamEvent::GenerationStarted(started)) = envelope.payload {
            return started.generation_id;
        }
    }
}

/// A tool outside `with_tools`: the model's proposal is answered as an error at once and never
/// leaves the node.
#[test]
fn a_proposal_of_a_tool_the_node_was_not_given_is_denied_to_the_model() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![call("unknown", "Sorry")]));
    let mut run = run(
        &model,
        config(vec![weather(), tool("unknown")]),
        vec![weather()],
    );
    run.feed(utterances(1).remove(0));
    let events = run.events_until(|payload| {
        matches!(payload, Voice::Stream(StreamEvent::TextSegmentCommitted(segment)) if segment.text == "Sorry")
    });
    assert!(
        !events
            .iter()
            .any(|payload| matches!(payload, Voice::Tool(_))),
        "{events:#?}"
    );
    let submitted = model.seen().submitted.clone();
    assert_eq!(submitted.len(), 1, "{submitted:#?}");
    assert_eq!(submitted[0].0, SpeechCallId::new("call_1"));
    assert!(submitted[0].1.get("error").is_some(), "{submitted:#?}");
    let carried = run.finish();
    assert!(carried.is_empty(), "{carried:#?}");
}

/// The run's drain, with a tool loop around the node: the node ends `events` and `tool_call`, and
/// still takes what arrives on `tool_result` until that edge has ended, as the loop's other nodes
/// send their end of input there only once `tool_call` ended. The run completes only then.
#[test]
fn under_the_runs_drain_the_node_ends_once_tool_result_has_ended() {
    start();
    let model = Held::over(MockSpeechModel::native(vec![say("hello", "Hi")]));
    let mut run = run(&model, config(vec![weather()]), vec![weather()]);
    run.feed(utterances(1).remove(0));
    run.events_until(completed_stream);
    run.requests.drain();
    let end = wait(run.tool_call.recv()).expect("tool_call carries its end");
    assert!(
        matches!(end.payload, Voice::Control(ControlKind::Stop)),
        "only its end: {:?}",
        end.payload
    );
    run.events_until(|payload| matches!(payload, Voice::Control(ControlKind::Stop)));
    assert!(
        run.driver.recv_timeout(Duration::from_millis(300)).is_err(),
        "the run does not complete before tool_result has ended"
    );
    let stop = run.sender.control(TOOL_RESULT, ControlKind::Stop);
    if wait(run.ingress[TOOL_RESULT].send(stop)).is_err() {
        panic!("tool_result still takes its end");
    }
    joined(&run.driver).expect("the run completes");
}
