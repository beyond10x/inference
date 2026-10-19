//! The devices as pipeline nodes (feature `pipeline`): a [`MicrophoneSource`] of role `source`
//! with one output port of audio frames, and a [`SpeakerSink`] of role `sink` with one input port
//! of them, each implementing `inference-pipeline`'s [`NodeImplementation`] over the plain handle
//! it wraps.
//!
//! A started node exchanges on a thread of its own, polling its endpoint with an executor of its
//! own, so the runtime that drives it commits to none.
//!
//! The source sends each frame its [`Microphone`] produces in an envelope of kind
//! [`AUDIO_FRAME`], on [`Timebase::AudioFrameTime`] with the frame's position as its timestamp, on
//! one stream of its own numbered from 1. An edge that is full under the overload policy `fail`
//! drops the frame, which [`SourceHandle::dropped`] counts: a microphone does not wait. Drained,
//! it sends the frames already captured, then the end of input, a control event of kind `stop`
//! timed just past the last frame, and ends; cancelled, it sends nothing more. An edge whose
//! receiving side is gone ends it. The microphone is closed when the exchange ends.
//!
//! The sink pushes each audio frame it receives to its [`Speaker`], waiting for room while the
//! queue is full. A `stop` control event [finishes](Speaker::finish) the audio so far; a `cancel`
//! control event [stops](Speaker::stop) the speaker at once, clearing the queue, and
//! [`SinkHandle::stopped_at`] answers the played position reached. Once its input edge is closed
//! it finishes the audio and ends, leaving what is queued to play out. Cancelled, it stops the
//! speaker. Content other than an audio frame or a control event fails the node.

use std::fmt;
use std::future::{self, Future};
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::Duration;

use inference_audio::{AudioFrame, SampleClockPosition};
use inference_core::endpoint::{InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_pipeline::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation};

use crate::capture::Microphone;
use crate::playout::{PushError, Speaker};

/// The `kind` of an envelope carrying an audio frame.
pub const AUDIO_FRAME: &str = "inference.audio.AudioFrame";

/// How long the source waits for a frame, and the sink for room, before looking at its flags
/// again.
const POLL: Duration = Duration::from_millis(5);

/// The payload of a run whose edges carry audio frames: the source builds one of a frame or a
/// control event, and the sink reads a frame out of it.
pub trait DevicePayload: Send + Sized + 'static {
    /// The payload carrying `frame`.
    fn from_audio(frame: AudioFrame) -> Self;

    /// The audio frame the payload carries.
    ///
    /// # Errors
    ///
    /// The payload, unchanged, when it carries none.
    fn into_audio(self) -> Result<AudioFrame, Self>;

    /// The payload of the control event `kind`; its envelope's `kind` is [`ControlKind::as_str`].
    fn control(kind: ControlKind) -> Self;
}

/// What a node was asked to do and what its exchange came to, and the wakers of the tasks waiting
/// on a change.
#[derive(Debug, Default)]
struct Flags {
    draining: bool,
    cancelled: bool,
    ending: bool,
    done: bool,
    failure: Option<String>,
    exchange: Option<Waker>,
    finished: Option<Waker>,
}

/// The state a node, its exchange thread and its handle share.
#[derive(Debug, Default)]
struct Shared {
    flags: Mutex<Flags>,
    sent: AtomicU64,
    dropped: AtomicU64,
    pushed: AtomicU64,
    stopped_at: Mutex<Option<SampleClockPosition>>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Flags> {
        self.flags.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes the flags by `change` and wakes every task waiting on them.
    fn update(&self, change: impl FnOnce(&mut Flags)) {
        let mut flags = self.lock();
        change(&mut flags);
        let wakers = [flags.exchange.take(), flags.finished.take()];
        drop(flags);
        for waker in wakers.into_iter().flatten() {
            waker.wake();
        }
    }

    fn read<T>(&self, read: impl FnOnce(&Flags) -> T) -> T {
        read(&self.lock())
    }

    /// What `operation` answers, unless the node is cancelled or ended first: `None` then.
    async fn unless_cancelled<T>(&self, operation: impl Future<Output = T>) -> Option<T> {
        let mut operation = pin!(operation);
        future::poll_fn(|cx| {
            {
                let mut flags = self.lock();
                if flags.cancelled || flags.ending {
                    return Poll::Ready(None);
                }
                flags.exchange = Some(cx.waker().clone());
            }
            operation.as_mut().poll(cx).map(Some)
        })
        .await
    }

    /// Waits until the exchange is done.
    async fn done(&self) {
        future::poll_fn(|cx| {
            let mut flags = self.lock();
            if flags.done {
                Poll::Ready(())
            } else {
                flags.finished = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await;
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.read(|flags| flags.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes.
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park();
    }
}

/// Marks the exchange done when its thread leaves its work, returning or unwinding.
struct Done(Arc<Shared>);

impl Drop for Done {
    fn drop(&mut self) {
        self.0.update(|flags| flags.done = true);
    }
}

/// The exchange thread of one node, once started.
#[derive(Debug, Default)]
struct Task {
    thread: Option<JoinHandle<()>>,
}

impl Task {
    /// Runs `work` on a thread of its own, recording the failure it answers.
    fn spawn(
        &mut self,
        shared: &Arc<Shared>,
        work: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) {
        let done = Done(Arc::clone(shared));
        self.thread = Some(thread::spawn(move || {
            let ended = work();
            if let Err(failure) = ended {
                done.0.update(|flags| flags.failure = Some(failure));
            }
            drop(done);
        }));
    }

    /// Waits until the exchange is done, then joins its thread; a panic of the thread becomes the
    /// node's failure.
    async fn finished(&mut self, shared: &Shared) {
        if let Some(thread) = self.thread.take() {
            shared.done().await;
            if thread.join().is_err() {
                shared.update(|flags| {
                    flags.failure = Some("the node's exchange panicked".to_owned());
                });
            }
        }
    }
}

/// The error of a node handed other endpoints than its one input or one output edge.
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

/// Wraps what the source emits in envelopes on one stream of its own, numbered from 1.
struct Emitter {
    run_id: RunId,
    producer: NodeId,
    port: String,
    stream_id: StreamId,
    sequence: i64,
}

impl Emitter {
    fn envelope<P>(&mut self, kind: &str, at: u64, payload: P) -> EventEnvelope<P> {
        self.sequence += 1;
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
            generation_id: None,
            timestamp: i64::try_from(at).unwrap_or(i64::MAX),
            timebase: Timebase::AudioFrameTime,
            causation_id: None,
            payload,
        }
    }
}

/// The microphone as a source node; see the module documentation.
pub struct MicrophoneSource {
    run_id: RunId,
    node_id: Option<NodeId>,
    microphone: Option<Microphone>,
    shared: Arc<Shared>,
    task: Task,
}

impl fmt::Debug for MicrophoneSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MicrophoneSource")
            .field("run_id", &self.run_id)
            .field("node_id", &self.node_id)
            .field("shared", &self.shared)
            .finish_non_exhaustive()
    }
}

impl MicrophoneSource {
    /// The source of the run `run_id` sending what `microphone` captures.
    #[must_use]
    pub fn new(run_id: RunId, microphone: Microphone) -> Self {
        Self {
            run_id,
            node_id: None,
            microphone: Some(microphone),
            shared: Arc::default(),
            task: Task::default(),
        }
    }

    /// The caller's side of the source.
    #[must_use]
    pub fn handle(&self) -> SourceHandle {
        SourceHandle {
            shared: Arc::clone(&self.shared),
        }
    }
}

/// One started exchange of a [`MicrophoneSource`].
struct Sending<P> {
    microphone: Microphone,
    output: Box<dyn OutputEndpoint<P>>,
    emitter: Emitter,
    shared: Arc<Shared>,
    next: u64,
}

impl<P: DevicePayload> Sending<P> {
    /// Sends `frame`; whether the exchange goes on.
    async fn frame(&mut self, frame: AudioFrame) -> Result<bool, String> {
        self.next = frame.position.get() + u64::from(frame.sample_count);
        let envelope =
            self.emitter
                .envelope(AUDIO_FRAME, frame.position.get(), P::from_audio(frame));
        match self
            .shared
            .unless_cancelled(self.output.send(envelope))
            .await
        {
            None | Some(Err(SendError::Closed(_))) => Ok(false),
            Some(Ok(())) => {
                self.shared.sent.fetch_add(1, Ordering::Relaxed);
                Ok(true)
            }
            Some(Err(SendError::Full(_))) => {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                Ok(true)
            }
            Some(Err(refused)) => Err(refused.to_string()),
        }
    }

    /// Sends the frames already captured, then the end of input.
    async fn end(&mut self) -> Result<(), String> {
        while let Some(frame) = self.microphone.try_frame() {
            if !self.frame(frame).await? {
                return Ok(());
            }
        }
        let stop = ControlKind::Stop;
        let envelope = self
            .emitter
            .envelope(stop.as_str(), self.next, P::control(stop));
        match self
            .shared
            .unless_cancelled(self.output.send(envelope))
            .await
        {
            None | Some(Ok(()) | Err(SendError::Closed(_))) => Ok(()),
            Some(Err(refused)) => Err(refused.to_string()),
        }
    }

    async fn exchange(&mut self) -> Result<(), String> {
        loop {
            let (draining, over) = self
                .shared
                .read(|flags| (flags.draining, flags.cancelled || flags.ending));
            if over {
                return Ok(());
            }
            if draining {
                return self.end().await;
            }
            if let Some(frame) = self.microphone.next_frame(POLL)
                && !self.frame(frame).await?
            {
                return Ok(());
            }
        }
    }
}

/// The caller's side of a [`MicrophoneSource`].
#[derive(Debug, Clone)]
pub struct SourceHandle {
    shared: Arc<Shared>,
}

impl SourceHandle {
    /// How many frames the source sent along its edge.
    #[must_use]
    pub fn sent(&self) -> u64 {
        self.shared.sent.load(Ordering::Relaxed)
    }

    /// How many frames the source dropped because its edge was full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }
}

impl<P: DevicePayload> NodeImplementation<P> for MicrophoneSource {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let prepared = if self.microphone.is_some() {
            Ok(())
        } else {
            Err(NodeError::new(
                "the source's microphone was already used by a run",
            ))
        };
        Box::pin(future::ready(prepared))
    }

    fn start(&mut self, mut endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = shape(&endpoints, 0, 1).and_then(|()| {
            let producer = self
                .node_id
                .ok_or_else(|| NodeError::new("the node was started before it was described"))?;
            let microphone = self
                .microphone
                .take()
                .ok_or_else(|| NodeError::new("the source's microphone was already used"))?;
            let output = endpoints.outputs.remove(0);
            let mut sending = Sending {
                microphone,
                emitter: Emitter {
                    run_id: self.run_id,
                    producer,
                    port: output.port_id.to_string(),
                    stream_id: StreamId::generate(),
                    sequence: 0,
                },
                output: output.endpoint,
                shared: Arc::clone(&self.shared),
                next: 0,
            };
            self.task
                .spawn(&self.shared, move || block_on(sending.exchange()));
            Ok(())
        });
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.shared.update(|flags| flags.draining = true);
        Box::pin(async move {
            self.task.finished(&self.shared).await;
            self.shared.failure()
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.shared.update(|flags| flags.cancelled = true);
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.shared.update(|flags| flags.ending = true);
        self.microphone = None;
        Box::pin(self.task.finished(&self.shared))
    }
}

/// The speaker as a sink node; see the module documentation.
#[derive(Debug)]
pub struct SpeakerSink {
    speaker: Arc<Speaker>,
    shared: Arc<Shared>,
    task: Task,
}

impl SpeakerSink {
    /// The sink playing to `speaker`; the caller keeps its own reference for the played position
    /// and for a stop.
    #[must_use]
    pub fn new(speaker: Arc<Speaker>) -> Self {
        Self {
            speaker,
            shared: Arc::default(),
            task: Task::default(),
        }
    }

    /// The caller's side of the sink.
    #[must_use]
    pub fn handle(&self) -> SinkHandle {
        SinkHandle {
            shared: Arc::clone(&self.shared),
        }
    }
}

/// One started exchange of a [`SpeakerSink`].
struct Playing<P> {
    speaker: Arc<Speaker>,
    input: Box<dyn InputEndpoint<P>>,
    shared: Arc<Shared>,
}

impl<P: DevicePayload> Playing<P> {
    /// Pushes `frame`, waiting for room unless the sink is cancelled or ended.
    fn push(&self, frame: &AudioFrame) -> Result<(), String> {
        loop {
            if self.shared.read(|flags| flags.cancelled || flags.ending) {
                return Ok(());
            }
            match self.speaker.push(frame, POLL) {
                Ok(()) => {
                    self.shared.pushed.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
                Err(PushError::Full) => {}
                Err(PushError::Stopped) => return Ok(()),
                Err(refused) => return Err(format!("the speaker refused a frame: {refused}")),
            }
        }
    }

    async fn exchange(&mut self) -> Result<(), String> {
        loop {
            let Some(next) = self.shared.unless_cancelled(self.input.recv()).await else {
                return Ok(());
            };
            let Some(envelope) = next else {
                self.speaker.finish();
                return Ok(());
            };
            match ControlKind::of(&envelope.kind) {
                Some(ControlKind::Stop) => self.speaker.finish(),
                Some(ControlKind::Cancel) => {
                    let reached = self.speaker.stop();
                    *self
                        .shared
                        .stopped_at
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = reached;
                }
                Some(_) => {}
                None => {
                    let kind = envelope.kind;
                    let frame = envelope.payload.into_audio().map_err(|_| {
                        format!(
                            "the speaker sink takes audio frames; an event of kind {kind} arrived"
                        )
                    })?;
                    self.push(&frame)?;
                }
            }
        }
    }
}

/// The caller's side of a [`SpeakerSink`].
#[derive(Debug, Clone)]
pub struct SinkHandle {
    shared: Arc<Shared>,
}

impl SinkHandle {
    /// How many frames the sink pushed to its speaker.
    #[must_use]
    pub fn pushed(&self) -> u64 {
        self.shared.pushed.load(Ordering::Relaxed)
    }

    /// The played position the last `cancel` control event stopped the speaker at, if one did
    /// and frames had been pushed before it.
    #[must_use]
    pub fn stopped_at(&self) -> Option<SampleClockPosition> {
        *self
            .shared
            .stopped_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl<P: DevicePayload> NodeImplementation<P> for SpeakerSink {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(&mut self, mut endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = shape(&endpoints, 1, 0).map(|()| {
            let mut playing = Playing {
                speaker: Arc::clone(&self.speaker),
                input: endpoints.inputs.remove(0).endpoint,
                shared: Arc::clone(&self.shared),
            };
            self.task
                .spawn(&self.shared, move || block_on(playing.exchange()));
        });
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(async move {
            self.task.finished(&self.shared).await;
            self.shared.failure()
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.shared.update(|flags| flags.cancelled = true);
        let _unplayed_is_dropped = self.speaker.stop();
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.shared.update(|flags| flags.ending = true);
        Box::pin(self.task.finished(&self.shared))
    }
}
