//! The mock source and sink nodes of a text-only pipeline, and what they share with the mock
//! model node of [`crate::llm`]: the payload the pipeline's edges carry, the run's event log, and
//! an exchange task per node. The source and the sink are generic over the payload, with the text
//! pipeline's [`PipelinePayload`] as the default, so that a pipeline of other payloads, such as
//! the Structured tool calls of `inference-pipeline`'s tool nodes, is driven by the same mocks.
//!
//! A started node exchanges events on a thread of its own, polling its endpoints with an
//! executor of its own, so the runtime that drives it commits to none. The thread holds the
//! node's task token, and a node's handle holds a weak reference to it, so a test sees the task
//! gone once the thread has ended and dropped its endpoints. Every `inference.stream` event a
//! node emits is wrapped in an envelope naming the run, the producer, its port, a stream of the
//! node's own and the next sequence on it, recorded in the run's [`RunLog`] and sent only once
//! the log took it; a control event is told from user content by its envelope's `kind` alone and
//! is not recorded, as the log records `inference.stream` events only.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::{self, Future};
use std::pin::pin;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::usage::UsageRecorded;
use inference_pipeline::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation};
use inference_text::{SegmentId, SpanId, TranscriptCommitted};

use crate::stream::{EnvelopeRefusal, EventLog, StreamEvent};

/// What the edges of the text pipeline carry: an `inference.stream` event, the usage record of a
/// model call, or a control event, which carries no payload of its own and whose envelope's
/// `kind` is the control kind's word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelinePayload {
    /// An `inference.stream` event; its envelope's `kind` is the event's declared name.
    Stream(StreamEvent),
    /// The usage of one model call, sent on a model node's `usage` port; its envelope's `kind`
    /// is [`UsageRecorded::KIND`].
    Usage(UsageRecorded),
    /// A control event; its envelope's `kind` is [`ControlKind::as_str`].
    Control(ControlKind),
}

impl From<ControlKind> for PipelinePayload {
    fn from(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

/// The event log of one run, shared by its nodes and the test: every `inference.stream` event a
/// node emits, in its envelope, in the order the nodes recorded them. A clone reads and records
/// into the same log.
#[derive(Debug, Clone, Default)]
pub struct RunLog(Arc<Mutex<EventLog>>);

impl RunLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn log(&self) -> MutexGuard<'_, EventLog> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records `envelope` as [`EventLog::record`] does.
    ///
    /// # Errors
    ///
    /// The [`EnvelopeRefusal`] of [`EventLog::record`]; nothing is recorded.
    pub fn record(&self, envelope: EventEnvelope<StreamEvent>) -> Result<(), EnvelopeRefusal> {
        self.log().record(envelope)
    }

    /// Every recorded envelope, in the order it was recorded.
    #[must_use]
    pub fn envelopes(&self) -> Vec<EventEnvelope<StreamEvent>> {
        self.log().envelopes().to_vec()
    }
}

/// A state shared between a node, its exchange thread and the test: a change wakes the task
/// waiting on it and every thread waiting on it.
pub(crate) struct Watched<S> {
    state: Mutex<(S, Option<Waker>)>,
    changed: Condvar,
}

impl<S> Watched<S> {
    pub(crate) fn new(state: S) -> Self {
        Self {
            state: Mutex::new((state, None)),
            changed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, (S, Option<Waker>)> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes the state by `change` and wakes whoever waits on it.
    pub(crate) fn update<T>(&self, change: impl FnOnce(&mut S) -> T) -> T {
        let mut guard = self.lock();
        let answer = change(&mut guard.0);
        let waker = guard.1.take();
        drop(guard);
        self.changed.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
        answer
    }

    /// Reads the state.
    pub(crate) fn read<T>(&self, read: impl FnOnce(&S) -> T) -> T {
        read(&self.lock().0)
    }

    /// Waits, as a task, until `ready` answers.
    pub(crate) async fn until<T>(&self, mut ready: impl FnMut(&mut S) -> Option<T>) -> T {
        future::poll_fn(|cx| {
            let mut guard = self.lock();
            match ready(&mut guard.0) {
                Some(answer) => Poll::Ready(answer),
                None => {
                    guard.1 = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await
    }

    /// Waits, blocking the thread for at most `patience`, until `ready` holds.
    fn wait(&self, patience: Duration, ready: impl Fn(&S) -> bool) -> bool {
        let deadline = Instant::now() + patience;
        let mut guard = self.lock();
        loop {
            if ready(&guard.0) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            guard = self
                .changed
                .wait_timeout(guard, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// Receives the next envelope of `input`, unless `stop` holds of `watched` first: `None` then,
/// and otherwise what the edge answered, `None` inside once the edge is closed.
pub(crate) async fn recv_unless<P, S>(
    input: &mut dyn InputEndpoint<P>,
    watched: &Watched<S>,
    stop: impl Fn(&S) -> bool,
) -> Option<Option<EventEnvelope<P>>> {
    let mut recv = input.recv();
    future::poll_fn(|cx| {
        {
            let mut guard = watched.lock();
            if stop(&guard.0) {
                return Poll::Ready(None);
            }
            guard.1 = Some(cx.waker().clone());
        }
        recv.as_mut().poll(cx).map(Some)
    })
    .await
}

/// Sends `envelope` along `output`, unless `stop` holds of `watched` first or while the send
/// waits for room: the send is then abandoned, the envelope not sent, and the answer is `Ok`, as
/// a node cancelled sends nothing more. Otherwise what the edge answered, a refusal described.
pub(crate) async fn send_unless<P, S>(
    output: &dyn OutputEndpoint<P>,
    watched: &Watched<S>,
    stop: impl Fn(&S) -> bool,
    envelope: EventEnvelope<P>,
) -> Result<(), String> {
    let mut send = output.send(envelope);
    future::poll_fn(|cx| {
        let mut guard = watched.lock();
        if stop(&guard.0) {
            return Poll::Ready(Ok(()));
        }
        guard.1 = Some(cx.waker().clone());
        send.as_mut()
            .poll(cx)
            .map(|sent| sent.map_err(|refused| refused.to_string()))
    })
    .await
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park();
    }
}

/// The exchange task of one node: the thread it runs on once started, and the token that thread
/// holds while it runs.
pub(crate) struct Exchange {
    token: Option<Arc<()>>,
    probe: Weak<()>,
    done: Arc<Watched<bool>>,
    thread: Option<JoinHandle<()>>,
    panicked: Option<String>,
}

/// Marks the task done when its thread leaves its work, returning or unwinding.
struct Done(Arc<Watched<bool>>);

impl Drop for Done {
    fn drop(&mut self) {
        self.0.update(|done| *done = true);
    }
}

impl Exchange {
    pub(crate) fn new() -> Self {
        let token = Arc::new(());
        Self {
            probe: Arc::downgrade(&token),
            token: Some(token),
            done: Arc::new(Watched::new(false)),
            thread: None,
            panicked: None,
        }
    }

    /// Nothing, unless the task's thread panicked: then the error naming the panic.
    pub(crate) fn panic(&self) -> Result<(), NodeError> {
        self.panicked
            .clone()
            .map_or(Ok(()), |panic| Err(NodeError::new(panic)))
    }

    /// A weak reference to the task's token: alive until the task ended.
    pub(crate) fn probe(&self) -> Weak<()> {
        Weak::clone(&self.probe)
    }

    /// Runs `work` on a thread of its own, which holds the token until `work` returned and
    /// dropped what it took.
    pub(crate) fn spawn(&mut self, work: impl FnOnce() + Send + 'static) {
        let token = self.token.take();
        let done = Done(Arc::clone(&self.done));
        self.thread = Some(thread::spawn(move || {
            let _done = done;
            work();
            drop(token);
        }));
    }

    /// Waits until the task ended, then joins its thread; a task never spawned has ended. A
    /// panic of the thread is kept for [`Exchange::panic`].
    pub(crate) async fn finished(&mut self) {
        self.token = None;
        if let Some(thread) = self.thread.take() {
            self.done.until(|done| done.then_some(())).await;
            if let Err(panic) = thread.join() {
                let message = panic
                    .downcast_ref::<&str>()
                    .map(|message| (*message).to_owned())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "a panic without a message".to_owned());
                self.panicked = Some(format!("the node's exchange panicked: {message}"));
            }
        }
    }
}

impl fmt::Debug for Exchange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Exchange")
            .field("running", &(self.probe.strong_count() > 0))
            .finish_non_exhaustive()
    }
}

/// Wraps what one node emits on one output port in envelopes: one stream of its own, numbered
/// from 1. An envelope names the generation its event names; an event naming only its segment,
/// such as `TextSegmentCommitted`, is enveloped under the generation of the deltas this emitter
/// sent of that segment, and under none when it sent none.
pub(crate) struct Emitter {
    run_id: RunId,
    producer: NodeId,
    port: String,
    stream_id: StreamId,
    sequence: i64,
    log: RunLog,
    /// The generation of each segment a delta this emitter sent named.
    segments: HashMap<SegmentId, GenerationId>,
}

impl Emitter {
    pub(crate) fn new(run_id: RunId, producer: NodeId, port: String, log: RunLog) -> Self {
        Self {
            run_id,
            producer,
            port,
            stream_id: StreamId::generate(),
            sequence: 0,
            log,
            segments: HashMap::new(),
        }
    }

    /// The generation the envelope of `event` names: the one the event names, or the one of the
    /// deltas sent of the segment the event names.
    fn generation(&mut self, event: &StreamEvent) -> Option<GenerationId> {
        let segment_id = match event {
            StreamEvent::TextDelta(delta) => {
                self.segments.insert(delta.segment_id, delta.generation_id);
                return Some(delta.generation_id);
            }
            StreamEvent::TextSegmentCommitted(event) => event.segment_id,
            StreamEvent::SegmentQueued(event) => event.segment_id,
            StreamEvent::SynthesisCompleted(event) => event.segment_id,
            StreamEvent::AudioPlayed(event) => event.segment_id,
            StreamEvent::SegmentDiscarded(event) => event.segment_id,
            event => return event.generation_id(),
        };
        self.segments.get(&segment_id).copied()
    }

    fn envelope<Q>(&mut self, kind: String, payload: Q) -> EventEnvelope<Q> {
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
            kind,
            generation_id: None,
            timestamp,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }

    /// The envelope of `event`, recorded in the run's log.
    ///
    /// # Errors
    ///
    /// The log's [`EnvelopeRefusal`]; the event is then neither recorded nor to be sent.
    pub(crate) fn stream(
        &mut self,
        event: StreamEvent,
    ) -> Result<EventEnvelope<PipelinePayload>, EnvelopeRefusal> {
        let generation_id = self.generation(&event);
        let mut envelope = self.envelope(event.name().to_owned(), event);
        envelope.generation_id = generation_id;
        self.log.record(envelope.clone())?;
        let EventEnvelope {
            schema_version,
            event_id,
            run_id,
            session_id,
            stream_id,
            sequence,
            producer,
            port,
            kind,
            generation_id,
            timestamp,
            timebase,
            causation_id,
            payload,
        } = envelope;
        Ok(EventEnvelope {
            schema_version,
            event_id,
            run_id,
            session_id,
            stream_id,
            sequence,
            producer,
            port,
            kind,
            generation_id,
            timestamp,
            timebase,
            causation_id,
            payload: PipelinePayload::Stream(payload),
        })
    }

    /// The envelope of `held`, naming its generation, on the same stream.
    fn held<P>(&mut self, held: Held<P>) -> EventEnvelope<P> {
        let mut envelope = self.envelope(held.kind, held.payload);
        envelope.generation_id = held.generation_id;
        envelope
    }

    /// The envelope of the control event `kind`, on the same stream.
    pub(crate) fn control<P: From<ControlKind>>(&mut self, kind: ControlKind) -> EventEnvelope<P> {
        self.envelope(kind.as_str().to_owned(), P::from(kind))
    }
}

/// Sends `event` along `output` in its envelope, once the run's log recorded it.
///
/// # Errors
///
/// The log's refusal or the edge's, described.
pub(crate) async fn emit(
    output: &dyn OutputEndpoint<PipelinePayload>,
    emitter: &mut Emitter,
    event: StreamEvent,
) -> Result<(), String> {
    let envelope = emitter
        .stream(event)
        .map_err(|refusal| refusal.to_string())?;
    output
        .send(envelope)
        .await
        .map_err(|refused| refused.to_string())
}

/// Nothing, when `endpoints` holds `inputs` input edges and `outputs` output edges; otherwise
/// the error naming what the node was handed instead.
fn shape<P>(endpoints: &NodeEndpoints<P>, inputs: usize, outputs: usize) -> Result<(), NodeError> {
    if endpoints.inputs.len() == inputs && endpoints.outputs.len() == outputs {
        Ok(())
    } else {
        Err(NodeError::new(format!(
            "the node takes {inputs} input and {outputs} output edge(s) and was handed {} and {}",
            endpoints.inputs.len(),
            endpoints.outputs.len()
        )))
    }
}

/// The node's identity, once it was described and handed `inputs` input edges and `outputs`
/// output edges; otherwise the error saying which of the two it was not.
pub(crate) fn started<P>(
    node_id: Option<NodeId>,
    endpoints: &NodeEndpoints<P>,
    inputs: usize,
    outputs: usize,
) -> Result<NodeId, NodeError> {
    shape(endpoints, inputs, outputs)?;
    node_id.ok_or_else(|| NodeError::new("the node was started before it was described"))
}

/// How a [`TextSource`] records an envelope it emits before sending it.
type Record<P> = fn(&RunLog, &EventEnvelope<P>) -> Result<(), String>;

/// Records the `inference.stream` event of `envelope` in `log`, as [`Emitter::stream`] does.
fn record_stream(log: &RunLog, envelope: &EventEnvelope<PipelinePayload>) -> Result<(), String> {
    let PipelinePayload::Stream(event) = &envelope.payload else {
        return Ok(());
    };
    log.record(EventEnvelope {
        schema_version: envelope.schema_version,
        event_id: envelope.event_id,
        run_id: envelope.run_id,
        session_id: envelope.session_id,
        stream_id: envelope.stream_id,
        sequence: envelope.sequence,
        producer: envelope.producer,
        port: envelope.port.clone(),
        kind: envelope.kind.clone(),
        generation_id: envelope.generation_id,
        timestamp: envelope.timestamp,
        timebase: envelope.timebase,
        causation_id: envelope.causation_id,
        payload: event.clone(),
    })
    .map_err(|refusal| refusal.to_string())
}

/// Records nothing: the log records `inference.stream` events only.
const fn record_nothing<P>(_: &RunLog, _: &EventEnvelope<P>) -> Result<(), String> {
    Ok(())
}

/// One event a [`TextSource`] holds: the `kind` and the generation of its envelope, and its
/// payload.
#[derive(Debug)]
struct Held<P> {
    kind: String,
    generation_id: Option<GenerationId>,
    payload: P,
}

/// What a [`TextSource`] holds and has done.
#[derive(Debug)]
struct SourceState<P> {
    held: VecDeque<Held<P>>,
    released: VecDeque<Held<P>>,
    end_released: bool,
    end_sent: bool,
    closed: bool,
    cancelled: bool,
    emitted: usize,
    failure: Option<String>,
}

impl<P> SourceState<P> {
    fn holding(held: impl IntoIterator<Item = Held<P>>) -> Self {
        Self {
            held: held.into_iter().collect(),
            released: VecDeque::new(),
            end_released: false,
            end_sent: false,
            closed: false,
            cancelled: false,
            emitted: 0,
            failure: None,
        }
    }
}

/// What the source's exchange does next.
enum SourceStep<P> {
    Event(Held<P>),
    End,
    Done,
}

/// A mock source node: it holds a fixed list of events and, once started, emits the next one
/// each time the test releases it on its one output edge, then the end of input, a control event
/// of kind `stop`, when the test releases it. Once drained it admits no further release and emits
/// the events released before the drain; once cancelled it emits nothing more.
///
/// A source of a text-only pipeline ([`TextSource::new`]) holds texts, each emitted as an
/// `inference.stream.TranscriptCommitted` recorded in the run's [`RunLog`]; a scripted source
/// ([`TextSource::scripted`]) holds events of any payload, a Structured tool call among them,
/// and records none.
#[derive(Debug)]
pub struct TextSource<P = PipelinePayload> {
    run_id: RunId,
    log: RunLog,
    record: Record<P>,
    node_id: Option<NodeId>,
    state: Arc<Watched<SourceState<P>>>,
    exchange: Exchange,
}

impl<P: fmt::Debug> fmt::Debug for Watched<SourceState<P>> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

impl TextSource {
    /// A source of the run `run_id` holding `texts`, recording what it emits in `log`.
    #[must_use]
    pub fn new<I, S>(run_id: RunId, texts: I, log: RunLog) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let held = texts.into_iter().map(|text| {
            let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
                span_id: SpanId::generate(),
                text: text.into(),
            });
            Held {
                kind: event.name().to_owned(),
                generation_id: event.generation_id(),
                payload: PipelinePayload::Stream(event),
            }
        });
        Self {
            run_id,
            log,
            record: record_stream,
            node_id: None,
            state: Arc::new(Watched::new(SourceState::holding(held))),
            exchange: Exchange::new(),
        }
    }
}

impl<P: Send + 'static> TextSource<P> {
    /// A source of the run `run_id` holding `events`, each the `kind` of its envelope, the
    /// generation the envelope names and its payload. It records nothing.
    #[must_use]
    pub fn scripted<I>(run_id: RunId, events: I) -> Self
    where
        I: IntoIterator<Item = (String, Option<GenerationId>, P)>,
    {
        let held = events
            .into_iter()
            .map(|(kind, generation_id, payload)| Held {
                kind,
                generation_id,
                payload,
            });
        Self {
            run_id,
            log: RunLog::new(),
            record: record_nothing,
            node_id: None,
            state: Arc::new(Watched::new(SourceState::holding(held))),
            exchange: Exchange::new(),
        }
    }

    /// The test's side of the source.
    #[must_use]
    pub fn handle(&self) -> SourceHandle<P> {
        SourceHandle {
            state: Arc::clone(&self.state),
            task: self.exchange.probe(),
        }
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.state
            .read(|state| state.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

impl<P: From<ControlKind> + Send + 'static> TextSource<P> {
    /// Sends the end of input along `output`, again while the edge is full, until the edge takes
    /// it, refuses it otherwise, or the source is cancelled.
    async fn end(
        state: &Watched<SourceState<P>>,
        output: &dyn OutputEndpoint<P>,
        emitter: &mut Emitter,
    ) -> Result<(), String> {
        loop {
            match output.send(emitter.control(ControlKind::Stop)).await {
                Ok(()) => return Ok(()),
                Err(SendError::Full(_)) if !state.read(|state| state.cancelled) => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(refused) => return Err(refused.to_string()),
            }
        }
    }

    /// Emits what the test releases until the source is done; once a send failed, the end of
    /// input is still sent, so that the run drains and the drain reports the failure.
    async fn exchange(
        state: Arc<Watched<SourceState<P>>>,
        output: Box<dyn OutputEndpoint<P>>,
        mut emitter: Emitter,
        log: RunLog,
        record: Record<P>,
    ) {
        let end_taken = Self::emit(&state, output.as_ref(), &mut emitter, &log, record).await;
        let failed = state.read(|state| state.failure.is_some() && !state.cancelled);
        if failed && !end_taken {
            let _first_failure_is_kept = Self::end(&state, output.as_ref(), &mut emitter).await;
        }
    }

    /// Emits what the test releases until the source is done, recording the first failure;
    /// whether the edge took the end of input.
    async fn emit(
        state: &Watched<SourceState<P>>,
        output: &dyn OutputEndpoint<P>,
        emitter: &mut Emitter,
        log: &RunLog,
        record: Record<P>,
    ) -> bool {
        let mut end_taken = false;
        loop {
            let step = state
                .until(|state| {
                    if state.cancelled || state.failure.is_some() {
                        Some(SourceStep::Done)
                    } else if let Some(held) = state.released.pop_front() {
                        Some(SourceStep::Event(held))
                    } else if state.end_released && !state.end_sent {
                        state.end_sent = true;
                        Some(SourceStep::End)
                    } else if state.closed || state.end_sent {
                        Some(SourceStep::Done)
                    } else {
                        None
                    }
                })
                .await;
            let sent = match step {
                SourceStep::Event(held) => {
                    let envelope = emitter.held(held);
                    match record(log, &envelope) {
                        Ok(()) => output
                            .send(envelope)
                            .await
                            .map(|()| state.update(|state| state.emitted += 1))
                            .map_err(|refused| refused.to_string()),
                        Err(refusal) => Err(refusal),
                    }
                }
                SourceStep::End => {
                    let ended = Self::end(state, output, emitter).await;
                    end_taken = ended.is_ok();
                    ended
                }
                SourceStep::Done => return end_taken,
            };
            if let Err(failure) = sent {
                state.update(|state| state.failure = Some(failure));
            }
        }
    }
}

/// The test's side of a [`TextSource`]: releases its events and its end of input, and reads what
/// it emitted.
#[derive(Debug)]
pub struct SourceHandle<P = PipelinePayload> {
    state: Arc<Watched<SourceState<P>>>,
    task: Weak<()>,
}

impl<P> Clone for SourceHandle<P> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            task: Weak::clone(&self.task),
        }
    }
}

impl<P> SourceHandle<P> {
    /// Releases the next held event; false, admitting nothing, once every event was released or
    /// the end of input was, or once the source was drained or cancelled.
    #[must_use]
    pub fn release(&self) -> bool {
        self.state.update(|state| {
            if state.closed || state.end_released {
                return false;
            }
            state
                .held
                .pop_front()
                .map(|held| state.released.push_back(held))
                .is_some()
        })
    }

    /// Releases the end of input; false, admitting nothing, once it was released or the source
    /// was drained or cancelled.
    #[must_use]
    pub fn release_end(&self) -> bool {
        self.state.update(|state| {
            if state.closed || state.end_released {
                return false;
            }
            state.end_released = true;
            true
        })
    }

    /// How many events the source emitted: the ones its edge took.
    #[must_use]
    pub fn emitted(&self) -> usize {
        self.state.read(|state| state.emitted)
    }

    /// Waits at most `patience` until the source emitted `count` events.
    #[must_use]
    pub fn wait_emitted(&self, count: usize, patience: Duration) -> bool {
        self.state.wait(patience, |state| state.emitted >= count)
    }

    /// A weak reference to the source's exchange task, alive until the task ended.
    #[must_use]
    pub fn task(&self) -> Weak<()> {
        Weak::clone(&self.task)
    }
}

impl<P: From<ControlKind> + Send + 'static> NodeImplementation<P> for TextSource<P> {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(&mut self, mut endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = started(self.node_id, &endpoints, 0, 1).map(|node_id| {
            let output = endpoints.outputs.remove(0);
            let emitter = Emitter::new(
                self.run_id,
                node_id,
                output.port_id.to_string(),
                self.log.clone(),
            );
            let state = Arc::clone(&self.state);
            let (log, record) = (self.log.clone(), self.record);
            self.exchange.spawn(move || {
                block_on(Self::exchange(state, output.endpoint, emitter, log, record));
            });
        });
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.state.update(|state| state.closed = true);
        Box::pin(async move {
            self.exchange.finished().await;
            self.exchange.panic()?;
            self.failure()
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.state.update(|state| {
            state.closed = true;
            state.cancelled = true;
        });
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(self.exchange.finished())
    }
}

/// What a [`TextSink`] received.
#[derive(Debug)]
struct SinkState<P> {
    received: Vec<EventEnvelope<P>>,
    cancelled: bool,
}

/// A mock sink node: once started it collects every envelope its one input edge delivers, of
/// whatever payload, until the edge is closed or the sink is cancelled.
#[derive(Debug)]
pub struct TextSink<P = PipelinePayload> {
    state: Arc<Watched<SinkState<P>>>,
    exchange: Exchange,
}

impl<P: fmt::Debug> fmt::Debug for Watched<SinkState<P>> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

impl<P> Default for TextSink<P> {
    fn default() -> Self {
        Self {
            state: Arc::new(Watched::new(SinkState {
                received: Vec::new(),
                cancelled: false,
            })),
            exchange: Exchange::new(),
        }
    }
}

impl TextSink {
    /// A sink of a text-only pipeline that has received nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl<P: Send + 'static> TextSink<P> {
    /// The test's side of the sink.
    #[must_use]
    pub fn handle(&self) -> SinkHandle<P> {
        SinkHandle {
            state: Arc::clone(&self.state),
            task: self.exchange.probe(),
        }
    }

    async fn exchange(state: Arc<Watched<SinkState<P>>>, mut input: Box<dyn InputEndpoint<P>>) {
        while let Some(Some(envelope)) =
            recv_unless(input.as_mut(), &state, |state| state.cancelled).await
        {
            state.update(|state| state.received.push(envelope));
        }
    }
}

/// The test's side of a [`TextSink`]: what it received.
#[derive(Debug)]
pub struct SinkHandle<P = PipelinePayload> {
    state: Arc<Watched<SinkState<P>>>,
    task: Weak<()>,
}

impl<P> Clone for SinkHandle<P> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            task: Weak::clone(&self.task),
        }
    }
}

impl<P: Clone> SinkHandle<P> {
    /// Every envelope the sink received, in the order it received them.
    #[must_use]
    pub fn received(&self) -> Vec<EventEnvelope<P>> {
        self.state.read(|state| state.received.clone())
    }

    /// Waits at most `patience` until the sink received `count` envelopes.
    #[must_use]
    pub fn wait_received(&self, count: usize, patience: Duration) -> bool {
        self.state
            .wait(patience, |state| state.received.len() >= count)
    }

    /// A weak reference to the sink's exchange task, alive until the task ended.
    #[must_use]
    pub fn task(&self) -> Weak<()> {
        Weak::clone(&self.task)
    }
}

impl SinkHandle {
    /// The text of every `inference.stream.TextSegmentCommitted` the sink received, in order.
    #[must_use]
    pub fn committed(&self) -> Vec<String> {
        self.state.read(|state| committed(&state.received))
    }

    /// Waits at most `patience` until the sink received `count` committed segments.
    #[must_use]
    pub fn wait_committed(&self, count: usize, patience: Duration) -> bool {
        self.state
            .wait(patience, |state| committed(&state.received).len() >= count)
    }
}

fn committed(received: &[EventEnvelope<PipelinePayload>]) -> Vec<String> {
    received
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            PipelinePayload::Stream(StreamEvent::TextSegmentCommitted(committed)) => {
                Some(committed.text.clone())
            }
            _ => None,
        })
        .collect()
}

impl<P: Send + 'static> NodeImplementation<P> for TextSink<P> {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(&mut self, mut endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = shape(&endpoints, 1, 0).map(|()| {
            let input = endpoints.inputs.remove(0);
            let state = Arc::clone(&self.state);
            self.exchange.spawn(move || {
                block_on(Self::exchange(state, input.endpoint));
            });
        });
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(async move {
            self.exchange.finished().await;
            self.exchange.panic()?;
            Ok(())
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.state.update(|state| state.cancelled = true);
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(self.exchange.finished())
    }
}
