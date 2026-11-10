//! Adversary pass 1 on `story:duplex-turn-ids`: `DuplexNode` pairs a transcript with the turn
//! its `InputItemId` names.
//!
//! The provider here is a scripted session over a `SessionLedger`: each step runs once the node
//! has appended a given number of audio frames, so every turn the node opens holds frames before
//! its commit, and the order of commits, transcripts, responses and calls is the script's.
//!
//! * A transcript that arrives before the commit naming it is the basis of the call proposed
//!   after that commit (the module doc: "the `InputCommitted` that names it later keeps that
//!   basis"). Nothing else drives that branch: the duplex mock always commits first.
//! * A response created on the first turn proposes its call after a second turn was committed
//!   while it was in progress, and after the first turn's late transcript arrived. The call
//!   answers the first turn, so it rests on that turn's transcript, not on the second turn's
//!   empty span (story: "each tool call based on the transcript of the turn the user spoke, also
//!   when a provider sends a transcript after a later turn was committed").
//!
//! Every wait is bounded by the case's wall-clock deadline.

#![cfg(all(feature = "run", feature = "duplex"))]

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::sync::mpsc;
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
    InputItemId, NativeSpeechModel, OpenError, ResponseFailed, ResponseId, SessionConfig,
    SessionError, SessionEvent as ModelEvent, SessionLedger, SpeechCallId, SpeechCapabilities,
    SpeechSessionClosed, SpeechSessionFailed, SpeechSessionHandle, SpeechSessionId, SpeechStarted,
    TurnDetection, TurnTuning,
};
use inference_pipeline::nodes::duplex::{DuplexNode, DuplexPayload, DuplexPorts};
use inference_pipeline::nodes::{RunLog, StreamEvent, ToolCallEvent, ToolCallUpdate, ToolPayload};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PortTraits, modality,
};
use inference_runtime::{RunController, RunError};
use inference_testkit::duplex::mock::{MockSpeechModel, pcm, utterance};
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

/// One provider event the scripted session records on its ledger.
#[derive(Debug, Clone)]
enum Step {
    /// `InputCommitted` naming the item.
    Commit(&'static str),
    /// `InputTranscribed` naming the item, with the text.
    Transcribe(&'static str, &'static str),
    /// `ResponseCreated` of a new response, the one later steps act on.
    Create,
    /// A call of `weather` proposed on the response created last, under the call id.
    Call(&'static str),
}

/// A provider that records `script` on the ledger, each step once the node appended at least
/// the frames the step names.
#[derive(Debug, Clone)]
struct Scripted {
    inner: MockSpeechModel,
    script: Vec<(usize, Step)>,
}

struct ScriptedSession {
    ledger: SessionLedger,
    script: VecDeque<(usize, Step)>,
    frames: usize,
    response: Option<ResponseId>,
}

impl NativeSpeechModel for Scripted {
    type Session = ScriptedSession;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        self.inner.capabilities()
    }

    fn open(&mut self, config: SessionConfig) -> Result<ScriptedSession, OpenError> {
        let (ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            self.inner.adapter(),
            self.inner.capabilities(),
            config,
            Instant::now(),
        )?;
        Ok(ScriptedSession {
            ledger,
            script: self.script.iter().cloned().collect(),
            frames: 0,
            response: None,
        })
    }
}

impl ScriptedSession {
    fn apply(&mut self, step: Step) {
        let now = Instant::now();
        match step {
            Step::Commit(item) => self
                .ledger
                .input_committed(InputItemId::new(item).expect("a non-empty id"), now)
                .expect("the commit is recorded"),
            Step::Transcribe(item, text) => self
                .ledger
                .input_transcribed(
                    InputItemId::new(item).expect("a non-empty id"),
                    text.to_owned(),
                    now,
                )
                .expect("the transcript is recorded"),
            Step::Create => {
                let response = ResponseId::generate();
                self.ledger
                    .response_created(response, now)
                    .expect("the response is created");
                self.response = Some(response);
            }
            Step::Call(call_id) => self
                .ledger
                .tool_call(
                    SpeechCallId::new(call_id),
                    self.response.expect("a response was created"),
                    "weather",
                    json!({"city": "Berlin"}),
                    now,
                )
                .expect("the call is proposed"),
        }
    }
}

impl SpeechSessionHandle for ScriptedSession {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        self.ledger.accept_input(frame)?;
        self.frames += 1;
        Ok(())
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        let item = self.ledger.assign_item();
        self.ledger.commit_input(item, Instant::now())
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.ledger.request_response(Instant::now())
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.ledger.cancel(response_id)
    }

    fn truncate_output(
        &mut self,
        _response_id: ResponseId,
        _heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        Ok(())
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        self.ledger.answer(call_id, result)
    }

    fn close(&mut self) -> Result<(), SessionError> {
        self.ledger.close()
    }

    fn next_event(&mut self, wait: Duration) -> Option<ModelEvent> {
        loop {
            if let Some(event) = self.ledger.next_event() {
                return Some(event);
            }
            if self.ledger.session().require_open().is_err() {
                return None;
            }
            match self.script.front() {
                Some((at, _)) if *at <= self.frames => {
                    let (_, step) = self.script.pop_front().expect("a step");
                    self.apply(step);
                }
                _ => {
                    thread::sleep(wait.min(Duration::from_millis(5)));
                    return None;
                }
            }
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
        auto_interrupt: false,
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

/// The frames of `turns` utterances back to back, each turn's frames in one entry.
fn turns(turns: usize) -> Vec<Vec<AudioFrame>> {
    let mut at = 0;
    (0..turns)
        .map(|_| {
            let spoken = utterance(pcm(16_000), at);
            at = spoken.next();
            spoken.frames()
        })
        .collect()
}

/// What the run put out: the proposal on `tool_call`, and every transcript committed on
/// `events`, by text.
struct Outcome {
    proposed: ToolCall,
    transcripts: Vec<(String, inference_text::SpanId)>,
}

impl Outcome {
    fn span_of(&self, text: &str) -> inference_text::SpanId {
        let spans: Vec<_> = self
            .transcripts
            .iter()
            .filter(|(said, _)| said == text)
            .map(|(_, span)| *span)
            .collect();
        assert_eq!(
            spans.len(),
            1,
            "{text:?} is committed once: {:#?}",
            self.transcripts
        );
        spans[0]
    }
}

/// Runs a `DuplexNode` over `model`, sends the frames of `spoken` one turn after another, and
/// returns once the first proposal left on `tool_call`, the run stopped and drained.
fn run(model: &Scripted, spoken: Vec<Vec<AudioFrame>>) -> Outcome {
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
    for frame in spoken.into_iter().flatten() {
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
    let mut transcripts = Vec::new();
    while let Some(envelope) = wait(events.recv()) {
        if let Voice::Stream(StreamEvent::TranscriptCommitted(committed)) = envelope.payload {
            transcripts.push((committed.text, committed.span_id));
        }
    }
    while wait(tool_call.recv()).is_some() {}
    driver
        .recv_timeout(remaining())
        .expect("the run ends within the deadline")
        .expect("the run completes");
    Outcome {
        proposed,
        transcripts,
    }
}

/// A transcript that arrives before the commit naming it is kept as the basis by that commit:
/// the call proposed after the commit rests on the transcript, not on an empty span the commit
/// committed over it.
#[test]
fn a_transcript_ahead_of_its_commit_stays_the_basis_of_the_call_after_the_commit() {
    start();
    let spoken = turns(1);
    let end = spoken[0].len();
    let model = Scripted {
        inner: MockSpeechModel::native(Vec::new()),
        script: vec![
            (end, Step::Transcribe("item-a", "how warm is it in Berlin")),
            (end, Step::Commit("item-a")),
            (end, Step::Create),
            (end, Step::Call("call-a")),
        ],
    };
    let outcome = run(&model, spoken);
    assert_eq!(
        outcome.proposed.basis_span_id(),
        outcome.span_of("how warm is it in Berlin"),
        "the call rests on the transcript that arrived before its turn's commit"
    );
}

/// Of two turns, the first one's transcript is missing at its commit and the second turn's
/// arrives before the second commit: the call of the second turn rests on the second turn's
/// transcript.
#[test]
fn a_second_turns_transcript_ahead_of_its_commit_is_its_calls_basis_after_an_untranscribed_turn() {
    start();
    let spoken = turns(2);
    let first = spoken[0].len();
    let second = first + spoken[1].len();
    let model = Scripted {
        inner: MockSpeechModel::native(Vec::new()),
        script: vec![
            (first, Step::Commit("item-a")),
            (second, Step::Transcribe("item-b", "and in Lisbon")),
            (second, Step::Commit("item-b")),
            (second, Step::Create),
            (second, Step::Call("call-b")),
        ],
    };
    let outcome = run(&model, spoken);
    assert_eq!(
        outcome.proposed.basis_span_id(),
        outcome.span_of("and in Lisbon"),
        "the call rests on its own turn's transcript"
    );
}

/// A response created on the first turn is still in progress when the second turn is
/// committed (input during output continues the response), and the first turn's transcript
/// arrives only after that commit. The response's call answers the first turn, so it rests on
/// the first turn's transcript, never on the second turn's empty span.
#[test]
fn a_call_of_the_first_turns_response_rests_on_its_late_transcript_after_a_second_commit() {
    start();
    let spoken = turns(2);
    let first = spoken[0].len();
    let second = first + spoken[1].len();
    let model = Scripted {
        inner: MockSpeechModel::native(Vec::new()),
        script: vec![
            (first, Step::Commit("item-a")),
            (first, Step::Create),
            (second, Step::Commit("item-b")),
            (
                second,
                Step::Transcribe("item-a", "how warm is it in Berlin"),
            ),
            (second, Step::Call("call-a")),
        ],
    };
    let outcome = run(&model, spoken);
    assert_eq!(
        outcome.proposed.basis_span_id(),
        outcome.span_of("how warm is it in Berlin"),
        "the call of the first turn's response rests on the first turn's transcript, not on the \
         empty span committed at the second turn's commit"
    );
}
