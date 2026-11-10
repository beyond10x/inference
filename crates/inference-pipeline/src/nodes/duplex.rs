//! The duplex node: a node of role `model` whose implementation is an `inference-duplex`
//! [`NativeSpeechModel`], so a native speech model (topology D of the design) runs under the
//! runtime and a session like any other model node.
//!
//! The node opens one session of its model when the run prepares it, from the [`SessionConfig`]
//! it was built with. Its ports ([`DuplexPorts`]) are the input `audio`, whose frames it appends
//! to the session, the input `control`, the input `tool_result` and the outputs `tool_call` and
//! `events`. Every payload an application reads leaves on `events`, in the order the node emits
//! it, so one egress delivers the model's events in the order the model produced them:
//!
//! | model event | what leaves on `events` |
//! |---|---|
//! | `SpeechStarted` | [`DuplexPayload::speech_started`] |
//! | `InputTranscribed` | `TranscriptCommitted` of a span of the turn it belongs to, committed at once |
//! | `UsageRecorded` | [`DuplexPayload::usage`], placed on the run and the node |
//! | `ResponseCreated` | `GenerationStarted` of the response's generation |
//! | `OutputTranscriptDelta` | `TextDelta`, then `TextSegmentCommitted` of a segment holding that text |
//! | `OutputAudio` | [`DuplexPayload::audio`] |
//! | `ResponseCompleted` | `GenerationCompleted` |
//! | `ResponseCancelled` | `GenerationCancelled` |
//! | `ResponseFailed` | [`DuplexPayload::response_failed`], then `GenerationCancelled` |
//! | `SpeechToolCallProposed` | `ToolCallProposed`, before the same update leaves on `tool_call` |
//! | `SpeechSessionClosed` | [`DuplexPayload::closed`], unless the node's own drain closed it |
//! | `SpeechSessionFailed` | [`DuplexPayload::failed`] |
//! | any other | nothing |
//!
//! The duplex's own `ResponseUsageRecorded` and `TranscriptionUsageRecorded` carry the counts its
//! `UsageRecorded` records carry, so only the records leave and nothing is counted twice. Every
//! `inference.stream` event the node emits is recorded in its envelope in the run's [`RunLog`].
//!
//! Each response is one generation of the node, started at its `ResponseCreated` with the epoch
//! counting the session's responses from 1, completed at its `ResponseCompleted` and cancelled at
//! its `ResponseCancelled` or `ResponseFailed`. A user turn is opened at the session's first
//! frame and at the first frame after each `InputCommitted`. Each `InputCommitted` makes its own
//! turn the basis of the calls proposed after it: an `InputCommitted` with no transcript of its
//! turn yet commits an empty span of that turn as the basis, so a call proposed on an
//! untranscribed turn still rests on committed input, and never on an earlier turn's words. Each
//! `InputCommitted` and `InputTranscribed` names its user turn by an `InputItemId`, so a
//! transcript is a span committed at once, of the committed turn that carries its id, whenever it
//! arrives: after the next turn's first frame or commit, or in any order among the turns. It
//! becomes the basis only while its turn is the last one committed, so a late transcript of an
//! earlier turn never becomes the basis of the next calls by itself. A call rests on the span of
//! the turn its response answers, the last turn committed when the response started, updated
//! when that turn's transcript arrives, whenever it does. A committed turn whose
//! transcript never arrives keeps its empty span and takes no other turn's transcript. A
//! transcript whose id no commit carries yet belongs to the open turn and becomes its basis, and
//! the `InputCommitted` that names it later keeps that basis. A provider that sends no commit
//! leaves every transcript in that state, so the node keeps the ids of the latest 64 of them.
//!
//! Tool calls: a proposal of a tool the node was given ([`DuplexNode::with_tools`]) becomes a
//! [`ToolCall`] of the response's generation resting on the basis, under a fresh
//! `ToolCallId`, sent on `events` and then on `tool_call` as a [`ToolCallUpdate`]; the node keeps
//! the model's own call id for it, which [`DuplexNode::call_ids`] reads. A proposal of any other
//! tool, or one that cannot rest on a generation and a committed span, never leaves the node: the
//! model is answered at once with an error result. A `Result`, `Failed` or `Rejected` update
//! arriving on `tool_result` for a call the node minted and has not answered is answered to the
//! model by its call id, a failure or rejection as an error result, and the call is marked
//! answered; any other update is dropped. A model that takes the caller's responses is asked for
//! one once no response is in progress, so it speaks the result.
//!
//! Controls, read off an envelope's `kind` on any input: a `cancel` cancels the response in
//! progress, at most once per response, and is ignored with none; a `commit` on `audio` commits
//! the input; a `stop`, or an input closing, drains the node. The node takes every envelope
//! already queued on its inputs before it asks the model for its next event, so a control sent
//! behind a frame acts before the model's next step.
//!
//! Draining, on a `stop` or the run's drain, the node admits no new audio and no tool result,
//! answers each call it minted and has not answered as rejected, so the model has an answer
//! under every call it made, closes the model's session and forwards every event the session still yields up to its
//! `SpeechSessionClosed`, which is the application's own close and is not emitted; then it ends
//! `events` and `tool_call`. It keeps reading its inputs until each has ended, so a `stop` sent
//! after the first is still taken; under the run's drain it lets go of `audio` and `control` and
//! ends once every edge into `tool_result` has ended, so that what a tool loop's other nodes send
//! at their end, after `tool_call` ended, still finds it. A session that closed itself was reported by
//! [`DuplexPayload::closed`]; the node then drops every frame and tool result that arrives and
//! fails nothing, and at the drain ends its outputs at once. A session that failed is reported by
//! [`DuplexPayload::failed`]; the node then lets go of its inputs, so the next envelope sent to
//! it is refused, ends its exchange and answers the run's drain with the failure. A run cancelled
//! closes the session if no drain did, forwards nothing, and lets go of the session.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use inference_audio::AudioFrame;
use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::usage::UsageRecorded;
use inference_duplex::{
    InputItemId, NativeSpeechModel, OutputTranscriptDelta, ResponseFailed, ResponseId,
    SessionConfig, SessionError, SessionEvent, SpeechCallId, SpeechSessionClosed,
    SpeechSessionFailed, SpeechSessionHandle, SpeechStarted, SpeechToolCallProposed,
};
use inference_text::{OpenTurn, OutputSegment, Span, Turn};
use inference_tools::{Json, Tool, ToolCall, ToolCallId};
use serde_json::json;

use super::log::{self, RunLog, StreamEvent};
use super::tool_authority::send_unless;
use super::{
    Exchange, ToolCallEvent, ToolCallUpdate, ToolPayload, Watched, block_on, unpark_waker,
};
use crate::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, PortId};

/// How many ids of transcripts that arrived before their commit the node keeps.
const AHEAD_KEPT: usize = 64;

/// How many committed turns the node keeps the transcript span of, the latest ones.
const TURNS_KEPT: usize = 64;

/// How long the node waits for the model's next event before it looks at its inputs again.
const POLL: Duration = Duration::from_millis(10);

/// How long the node rests when neither its inputs nor the model had anything for it.
const REST: Duration = Duration::from_millis(5);

/// The payload of the edges around a [`DuplexNode`]: the node reads frames and tool call updates
/// out of it and builds every event it emits into it. Every constructor is required, so a
/// payload cannot leave one of the node's events undeliverable by default.
pub trait DuplexPayload: ToolPayload {
    /// The payload carrying the `inference.stream` event `event`.
    fn stream(event: StreamEvent) -> Self;

    /// The payload carrying a frame of the model's output audio.
    fn audio(frame: AudioFrame) -> Self;

    /// The payload carrying the usage record `recorded`.
    fn usage(recorded: UsageRecorded) -> Self;

    /// The payload carrying `inference.duplex.SpeechStarted`: the user started speaking.
    fn speech_started(event: SpeechStarted) -> Self;

    /// The payload carrying `inference.duplex.ResponseFailed`, the one event that tells a failed
    /// response from a cancelled one.
    fn response_failed(event: ResponseFailed) -> Self;

    /// The payload carrying `inference.duplex.SpeechSessionClosed`: the model closed its session
    /// by itself.
    fn closed(event: SpeechSessionClosed) -> Self;

    /// The payload carrying `inference.duplex.SpeechSessionFailed`: the model's session failed.
    fn failed(event: SpeechSessionFailed) -> Self;

    /// The frame of audio the payload carries; the payload itself when it carries none.
    ///
    /// # Errors
    ///
    /// The payload, unchanged, when it carries no frame.
    fn into_audio(self) -> Result<AudioFrame, Self>;
}

/// The ports of a [`DuplexNode`]: its inputs `audio`, `control` and, optionally, `tool_result`,
/// and its outputs `events` and, optionally, `tool_call`. The node is handed the edges and
/// exposures its pipeline wires; `events` must be one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuplexPorts {
    pub audio: PortId,
    pub control: PortId,
    pub events: PortId,
    pub tool_call: Option<PortId>,
    pub tool_result: Option<PortId>,
}

/// A call the node minted, under the model's own call id.
#[derive(Debug, Clone)]
struct Minted {
    speech_call_id: SpeechCallId,
    answered: bool,
}

/// A read handle on the calls a [`DuplexNode`] proposed: each `ToolCallId` it minted and the
/// model's own [`SpeechCallId`] for it. An answered call stays.
#[derive(Debug, Clone, Default)]
pub struct CallIds(Arc<Mutex<HashMap<ToolCallId, Minted>>>);

impl CallIds {
    fn lock(&self) -> MutexGuard<'_, HashMap<ToolCallId, Minted>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The model's call id of the call `call_id`, if the node minted it.
    #[must_use]
    pub fn speech_call_id(&self, call_id: ToolCallId) -> Option<SpeechCallId> {
        self.lock()
            .get(&call_id)
            .map(|minted| minted.speech_call_id.clone())
    }

    fn mint(&self, call_id: ToolCallId, speech_call_id: SpeechCallId) {
        self.lock().insert(
            call_id,
            Minted {
                speech_call_id,
                answered: false,
            },
        );
    }

    /// Marks the call `call_id` answered and answers the model's call id; none when the node did
    /// not mint it or it was answered.
    fn answer(&self, call_id: ToolCallId) -> Option<SpeechCallId> {
        let mut calls = self.lock();
        let minted = calls.get_mut(&call_id)?;
        if minted.answered {
            return None;
        }
        minted.answered = true;
        Some(minted.speech_call_id.clone())
    }

    /// Marks every call not yet answered answered and answers the model's call ids, in order.
    fn unanswered(&self) -> Vec<SpeechCallId> {
        let mut calls = self.lock();
        let mut ids: Vec<SpeechCallId> = calls
            .values_mut()
            .filter(|minted| !minted.answered)
            .map(|minted| {
                minted.answered = true;
                minted.speech_call_id.clone()
            })
            .collect();
        ids.sort();
        ids
    }
}

/// What the runtime asked of the node, and how its exchange ended.
#[derive(Debug, Default)]
struct State {
    cancel: bool,
    drain: bool,
    failure: Option<String>,
}

/// The duplex node over the native speech model `M`; see the module documentation.
pub struct DuplexNode<M: NativeSpeechModel> {
    run_id: RunId,
    model: M,
    config: SessionConfig,
    ports: DuplexPorts,
    tools: Vec<Tool>,
    log: RunLog,
    node_id: Option<NodeId>,
    session: Option<M::Session>,
    call_ids: CallIds,
    state: Arc<Watched<State>>,
    exchange: Exchange,
}

impl<M: NativeSpeechModel> fmt::Debug for DuplexNode<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DuplexNode")
            .field("run_id", &self.run_id)
            .field("config", &self.config)
            .field("ports", &self.ports)
            .field("tools", &self.tools)
            .field("node_id", &self.node_id)
            .field("opened", &self.session.is_some())
            .field("exchange", &self.exchange)
            .finish_non_exhaustive()
    }
}

impl<M: NativeSpeechModel> DuplexNode<M> {
    /// A duplex node of the run `run_id` over `model`, opening its session from `config`, wired
    /// through `ports`, recording what it emits in `log`. It proposes no tool until it is given
    /// some ([`DuplexNode::with_tools`]).
    #[must_use]
    pub fn new(
        run_id: RunId,
        model: M,
        config: SessionConfig,
        ports: DuplexPorts,
        log: RunLog,
    ) -> Self {
        Self {
            run_id,
            model,
            config,
            ports,
            tools: Vec::new(),
            log,
            node_id: None,
            session: None,
            call_ids: CallIds::default(),
            state: Arc::new(Watched::new(State::default())),
            exchange: Exchange::new(),
        }
    }

    /// The same node proposing calls of `tools` only; a proposal of any other tool is answered
    /// to the model as an error and never leaves the node.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = tools;
        self
    }

    /// The model the node runs.
    #[must_use]
    pub const fn model(&self) -> &M {
        &self.model
    }

    /// A read handle on the calls the node proposes and the model's call id of each.
    #[must_use]
    pub fn call_ids(&self) -> CallIds {
        self.call_ids.clone()
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.state
            .read(|state| state.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

/// Which input of the node an endpoint delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Port {
    Audio,
    Control,
    ToolResult,
}

struct Input<P> {
    port: Port,
    endpoint: Box<dyn InputEndpoint<P>>,
    ended: bool,
}

/// One output of the node: every envelope on it is one stream of the node's own, numbered from 1.
struct Output<P> {
    endpoint: Box<dyn OutputEndpoint<P>>,
    run_id: RunId,
    producer: NodeId,
    port: String,
    stream_id: StreamId,
    sequence: i64,
}

impl<P> Output<P> {
    fn new(
        endpoint: Box<dyn OutputEndpoint<P>>,
        run_id: RunId,
        producer: NodeId,
        port: PortId,
    ) -> Self {
        Self {
            endpoint,
            run_id,
            producer,
            port: port.to_string(),
            stream_id: StreamId::generate(),
            sequence: 0,
        }
    }

    fn envelope<Q>(
        &mut self,
        kind: &str,
        generation_id: Option<GenerationId>,
        payload: Q,
    ) -> EventEnvelope<Q> {
        self.sequence += 1;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_micros()).unwrap_or(i64::MAX)
            });
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: self.port.clone(),
            kind: kind.to_owned(),
            generation_id,
            timestamp,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }
}

/// Where the model's session stands, as the node sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The session takes input and yields events.
    Open,
    /// The node closed the session and forwards what it yields up to its close.
    Closing,
    /// The session closed by itself; the node drops what arrives.
    SelfClosed,
    /// The node ended its outputs.
    Ended,
}

/// One started exchange of a [`DuplexNode`].
struct Exchanging<P, S> {
    run_id: RunId,
    node_id: NodeId,
    session: S,
    tools: Vec<Tool>,
    log: RunLog,
    state: Arc<Watched<State>>,
    call_ids: CallIds,
    inputs: Vec<Input<P>>,
    events: Output<P>,
    tool_call: Option<Output<P>>,
    phase: Phase,
    draining: bool,
    /// The responses of the session so far, the epoch of the next generation less one.
    epoch: i64,
    generations: HashMap<ResponseId, Generation>,
    /// The sequence of the next output segment.
    segment: i64,
    /// The open user turn, none before the first frame and after an `InputCommitted` until the
    /// next frame.
    turn: Option<Turn>,
    /// The last turn was committed; the next frame opens another.
    committed: bool,
    /// The committed turns by the id their `InputCommitted` carried, which a transcript naming
    /// the id is committed on, whenever it arrives; at most the latest `TURNS_KEPT` of them.
    turns: HashMap<InputItemId, Turn>,
    /// The ids in `turns`, oldest first, to evict the oldest past `TURNS_KEPT`.
    order: VecDeque<InputItemId>,
    /// The id of the last committed turn, the one whose transcript becomes the basis.
    latest: Option<InputItemId>,
    /// The ids of transcripts that arrived before their `InputCommitted`, oldest first, at most
    /// `AHEAD_KEPT`.
    ahead: VecDeque<InputItemId>,
    /// The span of each turn named by an id: the empty span committed at its commit until its
    /// transcript arrives, then the transcript's span; a transcript that arrived before its
    /// commit is kept here for it. Committed turns keep a span for the latest `TURNS_KEPT` only.
    spans: HashMap<InputItemId, Span>,
    /// The turn each response answers: the last turn committed when the response started, until
    /// the response ends.
    answers: HashMap<ResponseId, InputItemId>,
    /// The span of the last committed turn, which a call of a response that answers no committed
    /// turn rests on.
    basis: Option<Span>,
    /// A tool result was answered and the model is to be asked for a response.
    respond: bool,
}

impl<P: DuplexPayload, S: SpeechSessionHandle> Exchanging<P, S> {
    fn admits(&self) -> bool {
        self.phase == Phase::Open && !self.draining
    }

    /// Sends `payload` of `kind` on `events`, unless the node is cancelled first.
    fn emit(
        &mut self,
        kind: &str,
        generation_id: Option<GenerationId>,
        payload: P,
    ) -> Result<(), String> {
        let envelope = self.events.envelope(kind, generation_id, payload);
        block_on(send_unless(
            &self.state,
            |state| state.cancel,
            self.events.endpoint.as_ref(),
            envelope,
        ))
    }

    /// Records `event` in the run's log and sends it on `events`, its envelope naming
    /// `generation_id`, or the generation the event names.
    fn stream(
        &mut self,
        event: StreamEvent,
        generation_id: Option<GenerationId>,
    ) -> Result<(), String> {
        let generation_id = generation_id.or_else(|| event.generation_id());
        let envelope = self.events.envelope(event.name(), generation_id, event);
        self.log
            .record(envelope.clone())
            .map_err(|refusal| refusal.to_string())?;
        let envelope = log::carry(envelope, P::stream);
        block_on(send_unless(
            &self.state,
            |state| state.cancel,
            self.events.endpoint.as_ref(),
            envelope,
        ))
    }

    /// Sends the end of input, a `stop`, on every output, and ends them.
    fn end_outputs(&mut self) -> Result<(), String> {
        self.phase = Phase::Ended;
        let stop = self.events.envelope(
            ControlKind::Stop.as_str(),
            None,
            P::control(ControlKind::Stop),
        );
        block_on(send_unless(
            &self.state,
            |state| state.cancel,
            self.events.endpoint.as_ref(),
            stop,
        ))?;
        if let Some(tool_call) = self.tool_call.as_mut() {
            let stop = tool_call.envelope(
                ControlKind::Stop.as_str(),
                None,
                P::control(ControlKind::Stop),
            );
            block_on(send_unless(
                &self.state,
                |state| state.cancel,
                tool_call.endpoint.as_ref(),
                stop,
            ))?;
        }
        Ok(())
    }

    /// Drains the node: answers every call it minted and has not answered, held or running,
    /// with the session's close, as no tool result is admitted from then on, then closes an open session, whose events up to its
    /// close are then forwarded, or ends the outputs at once when the session is no longer open.
    fn drain(&mut self) -> Result<(), String> {
        if self.draining {
            return Ok(());
        }
        if self.phase == Phase::Open {
            for speech_call_id in self.call_ids.unanswered() {
                self.submit(
                    &speech_call_id,
                    json!({"error": "the session closed before the call's result"}),
                );
            }
        }
        self.draining = true;
        match self.phase {
            Phase::Open => match self.session.close() {
                Ok(()) => {
                    self.phase = Phase::Closing;
                    Ok(())
                }
                Err(_) => self.end_outputs(),
            },
            Phase::SelfClosed => self.end_outputs(),
            Phase::Closing | Phase::Ended => Ok(()),
        }
    }

    /// The run was cancelled: an open session the node did not close yet is closed, and
    /// nothing is forwarded.
    fn abandon(&mut self) {
        if self.phase == Phase::Open && !self.draining {
            let _ = self.session.close();
        }
    }

    /// Takes every envelope already queued on the inputs.
    fn take_inputs(&mut self, waker: &Waker) -> Result<(), String> {
        let mut cx = Context::from_waker(waker);
        loop {
            let mut progressed = false;
            let mut index = 0;
            while index < self.inputs.len() {
                if self.inputs[index].ended {
                    index += 1;
                    continue;
                }
                let polled = self.inputs[index].endpoint.recv().as_mut().poll(&mut cx);
                match polled {
                    Poll::Ready(Some(envelope)) => {
                        progressed = true;
                        let port = self.inputs[index].port;
                        self.take(index, port, envelope)?;
                    }
                    Poll::Ready(None) => {
                        progressed = true;
                        self.inputs[index].ended = true;
                        self.drain()?;
                    }
                    Poll::Pending => {}
                }
                index += 1;
            }
            if !progressed {
                return Ok(());
            }
        }
    }

    /// Acts on `envelope`, taken from the input at `index`, its port `port`.
    fn take(&mut self, index: usize, port: Port, envelope: EventEnvelope<P>) -> Result<(), String> {
        if let Some(control) = ControlKind::of(&envelope.kind) {
            match control {
                ControlKind::Stop => {
                    self.inputs[index].ended = true;
                    return self.drain();
                }
                ControlKind::Cancel => self.cancel_response(),
                ControlKind::Commit if port == Port::Audio && self.admits() => {
                    let _ = self.session.commit_input();
                }
                ControlKind::Start
                | ControlKind::Commit
                | ControlKind::Revise
                | ControlKind::Error => {}
            }
            return Ok(());
        }
        if !self.admits() {
            return Ok(());
        }
        match port {
            Port::Audio => match envelope.payload.into_audio() {
                Ok(frame) => self.append(frame),
                Err(_) => Err(format!(
                    "the node takes audio frames on its audio port and was handed `{}`",
                    envelope.kind
                )),
            },
            Port::ToolResult => {
                if let Ok(update) = envelope.payload.into_tool_call() {
                    self.answer(&update);
                }
                Ok(())
            }
            Port::Control => Ok(()),
        }
    }

    /// Appends `frame` to the session, opening a user turn first when none is open.
    fn append(&mut self, frame: AudioFrame) -> Result<(), String> {
        if self.turn.is_none() || self.committed {
            self.open_turn();
        }
        match self.session.append_audio(frame) {
            Ok(()) | Err(SessionError::NotOpen(_)) => Ok(()),
            Err(refused) => Err(refused.to_string()),
        }
    }

    fn open_turn(&mut self) {
        self.turn = Some(
            Turn::open(OpenTurn {
                run_id: self.run_id,
            })
            .0,
        );
        self.committed = false;
    }

    /// Cancels the response in progress. The ledger clears the response in progress when it
    /// takes the cancel, so a second cancel finds none and a response is cancelled at most once.
    fn cancel_response(&mut self) {
        if !self.admits() {
            return;
        }
        let Some(response_id) = self.session.ledger().in_progress() else {
            return;
        };
        let _ = self.session.cancel_response(response_id);
    }

    /// Answers the model's call of `update`, when it is a call the node minted and has not
    /// answered, and the update is its result, failure or rejection.
    fn answer(&mut self, update: &ToolCallUpdate) {
        let result = match &update.event {
            ToolCallEvent::Result(result) => result.result.clone(),
            ToolCallEvent::Failed(failed) => json!({"error": failed.failure.detail}),
            ToolCallEvent::Rejected(rejected) => {
                json!({"error": format!("rejected: {:?}", rejected.reason)})
            }
            ToolCallEvent::Proposed(_)
            | ToolCallEvent::Authorized(_)
            | ToolCallEvent::Started(_) => {
                return;
            }
        };
        let Some(speech_call_id) = self.call_ids.answer(update.call.call_id()) else {
            return;
        };
        self.submit(&speech_call_id, result);
    }

    fn submit(&mut self, speech_call_id: &SpeechCallId, result: Json) {
        if self
            .session
            .submit_tool_result(speech_call_id, result)
            .is_ok()
        {
            self.respond = true;
        }
    }

    /// Asks a model that takes the caller's responses for one once a tool result was answered
    /// and no response is in progress.
    fn respond(&mut self) {
        if !self.respond || !self.admits() {
            return;
        }
        if !self.session.ledger().capabilities().manual_response {
            self.respond = false;
            return;
        }
        if self.session.ledger().in_progress().is_none() {
            self.respond = false;
            let _ = self.session.create_response();
        }
    }

    /// The open turn, opened first when none is.
    fn open(&mut self) -> Result<Turn, String> {
        if self.turn.is_none() {
            self.open_turn();
        }
        self.turn
            .clone()
            .ok_or_else(|| "no turn is open".to_owned())
    }

    /// A committed span of `turn` holding `text`.
    fn committed_span(
        &self,
        turn: &Turn,
        text: String,
    ) -> Result<(Span, inference_text::TranscriptCommitted), String> {
        let (span, _) = Span::emit(
            turn,
            self.run_id,
            self.events.stream_id,
            self.events.sequence + 1,
            text,
        )
        .map_err(|refused| refused.to_string())?;
        span.commit().map_err(|refused| refused.to_string())
    }

    fn generation_id(&self, response_id: ResponseId) -> Option<GenerationId> {
        self.generations
            .get(&response_id)
            .map(Generation::generation_id)
    }

    /// Emits the end of the response's generation that `end` moves it to; none for a response
    /// with no generation or one no longer Active.
    fn end_generation(
        &mut self,
        response_id: ResponseId,
        end: impl FnOnce(&Generation) -> Option<(Generation, StreamEvent)>,
    ) -> Result<(), String> {
        self.answers.remove(&response_id);
        let Some(generation) = self.generations.get(&response_id) else {
            return Ok(());
        };
        let Some((moved, event)) = end(generation) else {
            return Ok(());
        };
        self.generations.insert(response_id, moved);
        self.stream(event, None)
    }

    fn cancel_generation(&mut self, response_id: ResponseId) -> Result<(), String> {
        self.end_generation(response_id, |generation| {
            generation
                .cancel()
                .ok()
                .map(|(moved, event)| (moved, StreamEvent::GenerationCancelled(event)))
        })
    }

    fn text(&mut self, delta: OutputTranscriptDelta) -> Result<(), String> {
        let Some(generation) = self.generations.get(&delta.response_id) else {
            return Ok(());
        };
        self.segment += 1;
        let Ok((segment, emitted)) = OutputSegment::emit(generation, self.segment, delta.text)
        else {
            return Ok(());
        };
        let generation_id = Some(emitted.generation_id);
        let (_, committed) = segment
            .commit()
            .map_err(|refused| refused.conflict().to_string())?;
        self.stream(StreamEvent::TextDelta(emitted), generation_id)?;
        self.stream(StreamEvent::TextSegmentCommitted(committed), generation_id)
    }

    /// The model proposed a call: sent on as a [`ToolCall`] of the node, or answered at once as
    /// an error.
    fn propose(&mut self, proposed: SpeechToolCallProposed) -> Result<(), String> {
        let Some(tool) = self
            .tools
            .iter()
            .find(|tool| tool.tool_id() == proposed.tool_id)
            .cloned()
        else {
            self.submit(
                &proposed.call_id,
                json!({"error": "denied: the tool was not given to this node"}),
            );
            return Ok(());
        };
        let basis = self
            .answers
            .get(&proposed.response_id)
            .and_then(|item| self.spans.get(item))
            .or(self.basis.as_ref());
        let call = match (self.generations.get(&proposed.response_id), basis) {
            (Some(generation), Some(basis)) => {
                ToolCall::propose(&tool, generation, basis, proposed.arguments)
                    .map_err(|refused| refused.to_string())
            }
            (None, _) => Err("the call names no response of the session".to_owned()),
            (Some(_), None) => Err("the call rests on no committed input".to_owned()),
        };
        let (call, event) = match call {
            Ok(call) => call,
            Err(reason) => {
                self.submit(&proposed.call_id, json!({"error": reason}));
                return Ok(());
            }
        };
        self.call_ids.mint(call.call_id(), proposed.call_id);
        let generation_id = Some(call.generation_id());
        let update = ToolCallUpdate {
            call,
            event: ToolCallEvent::Proposed(event),
        };
        let kind = update.event.name();
        self.emit(kind, generation_id, P::from_tool_call(update.clone()))?;
        if let Some(tool_call) = self.tool_call.as_mut() {
            let envelope = tool_call.envelope(kind, generation_id, P::from_tool_call(update));
            block_on(send_unless(
                &self.state,
                |state| state.cancel,
                tool_call.endpoint.as_ref(),
                envelope,
            ))?;
        }
        Ok(())
    }

    /// Acts on an event the model's session yielded; see the module documentation.
    fn forward(&mut self, event: SessionEvent) -> Result<(), String> {
        match event {
            SessionEvent::SpeechStarted(started) => self.emit(
                "inference.duplex.SpeechStarted",
                None,
                P::speech_started(started),
            ),
            SessionEvent::InputCommitted(committed) => {
                let turn = self.open()?;
                if let Some(at) = self.ahead.iter().position(|item| *item == committed.item) {
                    self.ahead.remove(at);
                } else {
                    let empty = self.committed_span(&turn, String::new())?.0;
                    self.spans.insert(committed.item.clone(), empty);
                }
                self.basis = self.spans.get(&committed.item).cloned();
                self.turns.insert(committed.item.clone(), turn);
                self.order.push_back(committed.item.clone());
                if self.order.len() > TURNS_KEPT
                    && let Some(oldest) = self.order.pop_front()
                {
                    self.turns.remove(&oldest);
                    self.spans.remove(&oldest);
                }
                self.latest = Some(committed.item);
                self.committed = true;
                Ok(())
            }
            SessionEvent::InputTranscribed(transcribed) => {
                let (turn, last) = match self.turns.get(&transcribed.item) {
                    Some(turn) => (
                        turn.clone(),
                        self.latest.as_ref() == Some(&transcribed.item),
                    ),
                    None => {
                        if self.committed {
                            self.open_turn();
                        }
                        if !self.ahead.contains(&transcribed.item) {
                            if self.ahead.len() == AHEAD_KEPT
                                && let Some(evicted) = self.ahead.pop_front()
                            {
                                self.spans.remove(&evicted);
                            }
                            self.ahead.push_back(transcribed.item.clone());
                        }
                        (self.open()?, true)
                    }
                };
                let (span, committed) = self.committed_span(&turn, transcribed.text)?;
                self.spans.insert(transcribed.item, span.clone());
                if last {
                    self.basis = Some(span);
                }
                self.stream(StreamEvent::TranscriptCommitted(committed), None)
            }
            SessionEvent::UsageRecorded(mut recorded) => {
                recorded.run_id = Some(self.run_id);
                recorded.node_id = Some(self.node_id);
                self.emit(UsageRecorded::KIND, None, P::usage(recorded))
            }
            SessionEvent::ResponseCreated(created) => {
                self.epoch += 1;
                let (generation, started) = Generation::start(StartGeneration {
                    run_id: self.run_id,
                    node_id: self.node_id,
                    epoch: self.epoch,
                });
                self.generations.insert(created.response_id, generation);
                if let Some(item) = self.latest.clone() {
                    self.answers.insert(created.response_id, item);
                }
                self.stream(StreamEvent::GenerationStarted(started), None)
            }
            SessionEvent::OutputTranscriptDelta(delta) => self.text(delta),
            SessionEvent::OutputAudio(audio) => {
                let generation_id = self.generation_id(audio.response_id);
                self.emit(
                    "inference.duplex.OutputAudio",
                    generation_id,
                    P::audio(audio.frame),
                )
            }
            SessionEvent::ResponseCompleted(completed) => {
                self.end_generation(completed.response_id, |generation| {
                    generation
                        .complete(None)
                        .ok()
                        .map(|(moved, event)| (moved, StreamEvent::GenerationCompleted(event)))
                })
            }
            SessionEvent::ResponseCancelled(cancelled) => {
                self.cancel_generation(cancelled.response_id)
            }
            SessionEvent::ResponseFailed(failed) => {
                let response_id = failed.response_id;
                let generation_id = self.generation_id(response_id);
                self.emit(
                    "inference.duplex.ResponseFailed",
                    generation_id,
                    P::response_failed(failed),
                )?;
                self.cancel_generation(response_id)
            }
            SessionEvent::SpeechToolCallProposed(proposed) => self.propose(proposed),
            SessionEvent::SpeechSessionClosed(closed) => {
                if self.phase == Phase::Closing {
                    return self.end_outputs();
                }
                self.phase = Phase::SelfClosed;
                self.emit(
                    "inference.duplex.SpeechSessionClosed",
                    None,
                    P::closed(closed),
                )
            }
            SessionEvent::SpeechSessionFailed(failed) => {
                self.inputs.clear();
                let detail = format!("the native speech session failed: {}", failed.failure);
                let _ = self.emit(
                    "inference.duplex.SpeechSessionFailed",
                    None,
                    P::failed(failed),
                );
                Err(detail)
            }
            SessionEvent::InputAccepted(_)
            | SessionEvent::SpeechStopped(_)
            | SessionEvent::TranscriptionUsageRecorded(_)
            | SessionEvent::OutputTruncated(_)
            | SessionEvent::ResponseUsageRecorded(_)
            | SessionEvent::SpeechToolCallAnswered(_) => Ok(()),
        }
    }

    fn exchanged(&mut self) -> Result<(), String> {
        let waker = unpark_waker();
        loop {
            let (cancel, drain) = self.state.read(|state| (state.cancel, state.drain));
            if cancel {
                self.abandon();
                return Ok(());
            }
            if drain {
                self.drain()?;
            }
            self.take_inputs(&waker)?;
            if self.phase == Phase::Ended {
                let done = self
                    .inputs
                    .iter()
                    .all(|input| input.ended || (drain && input.port != Port::ToolResult));
                if done {
                    return Ok(());
                }
                thread::park_timeout(REST);
                continue;
            }
            self.respond();
            if matches!(self.phase, Phase::Open | Phase::Closing) {
                match self.session.next_event(POLL) {
                    Some(event) => self.forward(event)?,
                    None => thread::park_timeout(REST),
                }
            } else {
                thread::park_timeout(REST);
            }
        }
    }

    fn run(mut self) {
        let failure = self.exchanged().err();
        self.state.update(|state| state.failure = failure);
    }
}

impl<M: NativeSpeechModel> DuplexNode<M> {
    /// The node's exchange over `endpoints` and `session`, once it was handed only edges at its
    /// own ports and one at `events`; otherwise the error saying what it was handed.
    fn exchanging<P: DuplexPayload>(
        &self,
        endpoints: NodeEndpoints<P>,
        session: M::Session,
    ) -> Result<Exchanging<P, M::Session>, NodeError> {
        let node_id = self
            .node_id
            .ok_or_else(|| NodeError::new("the node was started before it was described"))?;
        let ports = self.ports;
        let NodeEndpoints { inputs, outputs } = endpoints;
        let mut taken = Vec::with_capacity(inputs.len());
        for input in inputs {
            let port = match input.port_id {
                id if id == ports.audio => Port::Audio,
                id if id == ports.control => Port::Control,
                id if Some(id) == ports.tool_result => Port::ToolResult,
                id => {
                    return Err(NodeError::new(format!("the node has no input port {id}")));
                }
            };
            taken.push(Input {
                port,
                endpoint: input.endpoint,
                ended: false,
            });
        }
        let (mut events, mut tool_call) = (None, None);
        for output in outputs {
            let port = output.port_id;
            let slot = if port == ports.events {
                &mut events
            } else if Some(port) == ports.tool_call {
                &mut tool_call
            } else {
                return Err(NodeError::new(format!(
                    "the node has no output port {port}"
                )));
            };
            if slot.is_some() {
                return Err(NodeError::new(format!(
                    "the node takes one edge at its output port {port} and was handed more"
                )));
            }
            *slot = Some(Output::new(output.endpoint, self.run_id, node_id, port));
        }
        let events = events.ok_or_else(|| {
            NodeError::new(format!(
                "the node was handed no edge at its output port {}",
                ports.events
            ))
        })?;
        Ok(Exchanging {
            run_id: self.run_id,
            node_id,
            session,
            tools: self.tools.clone(),
            log: self.log.clone(),
            state: Arc::clone(&self.state),
            call_ids: self.call_ids.clone(),
            inputs: taken,
            events,
            tool_call,
            phase: Phase::Open,
            draining: false,
            epoch: 0,
            generations: HashMap::new(),
            segment: 0,
            turn: None,
            committed: false,
            turns: HashMap::new(),
            order: VecDeque::new(),
            latest: None,
            ahead: VecDeque::new(),
            spans: HashMap::new(),
            answers: HashMap::new(),
            basis: None,
            respond: false,
        })
    }
}

/// The session is opened when the run prepares the node and runs on the exchange's thread from
/// the start; a session never started is closed and let go of at the stop.
impl<P, M> NodeImplementation<P> for DuplexNode<M>
where
    P: DuplexPayload,
    M: NativeSpeechModel + Send + 'static,
    M::Session: Send + 'static,
{
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let opened = self
            .model
            .open(self.config.clone())
            .map(|session| self.session = Some(session))
            .map_err(|refused| {
                NodeError::new(format!(
                    "the native speech model opened no session: {refused}"
                ))
            });
        Box::pin(future::ready(opened))
    }

    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = self
            .session
            .take()
            .ok_or_else(|| NodeError::new("the node was started before it was prepared"))
            .and_then(|session| self.exchanging(endpoints, session))
            .map(|exchanging| self.exchange.spawn(move || exchanging.run()));
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.state.update(|state| state.drain = true);
        Box::pin(async move {
            self.exchange.finished().await;
            self.exchange.panic()?;
            self.failure()
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.state.update(|state| state.cancel = true);
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        if let Some(mut session) = self.session.take() {
            let _ = session.close();
        }
        Box::pin(self.exchange.finished())
    }
}
