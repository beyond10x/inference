//! The acknowledging sink (feature `pipeline`): the speaker as a pipeline node of role `sink`
//! that confirms playback, the only node that may (`docs/design/inference.md` 388).
//!
//! An [`AcknowledgingSink`] has one input edge, carrying what a TTS node synthesises: the frames
//! of each segment as `inference-tts` [`SegmentAudio`], which only a synthesis makes and which is
//! not `Clone`, and each segment's `inference.stream.SynthesisCompleted`, sent once the segment's
//! last frame was sent. It has one output edge, carrying its receipts back to the TTS node over a
//! feedback edge.
//!
//! It pushes each frame to its [`Speaker`], waiting for room while the queue is full, and keeps
//! the [`SegmentAudio`] it was given; a frame is taken from the edge only once the one before it
//! is queued. Once a segment's `SynthesisCompleted` arrived and the
//! speaker's played position has passed the segment's last frame, it makes the segment's
//! [`PlaybackReceipt`] by consuming those frames, and sends it on, in an envelope of kind
//! [`ACKNOWLEDGE_PLAYBACK`] naming the generation of the segment's frames: the TTS node runs
//! `inference.stream.AcknowledgePlayback` with it. A segment completed without frames takes the
//! receipt of no frames at once. Receipts go out in the order the segments arrived. The frames of
//! one synthesis are taken to lie on one sample clock, in order, as the speaker's played position
//! is read on the clock of the frames pushed.
//!
//! Given a turns port ([`AcknowledgingSink::with_turns`]), it takes the turn events of the turn
//! policy on a second input edge, ahead of its audio and while a frame waits for room, and acts on
//! each `inference.stream.TurnOpened`, the barge-in of `docs/design/inference.md` 632-641: every
//! generation that began before it is stale from then on, and every segment it holds is
//! forgotten, never acknowledged. A generation began before the opening of a turn when the sink
//! took a frame of it before that opening, or when its `inference.stream.GenerationStarted`
//! ([`Playable::Started`]), which reaches the sink on the audio edge ahead of the generation's
//! frames, names an epoch below the turn's ordinal: the sink numbers the turns it saw open from 1,
//! and the epoch of a generation answering a turn is that turn's ordinal among the turns opened in
//! the run, as the cascade's model node numbers it. So a generation the user spoke over before
//! the first frame of it arrived is stale before that frame arrives. If audio was still to play,
//! a frame waiting for room or samples queued, it stops the speaker at once, clearing its queue,
//! and records the [`Interruption`]: the turn, the samples cleared, the played position the stop
//! answered and the one read after it, and the time from taking the `TurnOpened` to the stop
//! returning, the design's "interruption to playback stop" (1023). If none was, nothing is
//! stopped: the interruption of that turn, nothing cleared and the played position read at the
//! opening, is recorded once a generation that began before it is superseded, its output cut
//! before any of it played; a generation that began before it and is never superseded, its output
//! all played, makes no interruption. An `inference.stream.GenerationSuperseded` on the audio
//! edge makes its generation stale the same way, and stops the speaker if the sink holds a segment
//! of it. A frame of a stale generation that reaches the sink later is dropped and recorded
//! ([`StaleFrame`], with the played position when it was), never pushed, and the completion of a
//! segment whose frames were dropped is ignored, so no receipt is ever made of it (step 4: stale
//! results are rejected at the irreversible sink). A frame whose envelope names no generation is
//! never stale.
//!
//! At each `SynthesisCompleted` it [finishes](Speaker::finish) the audio pushed so far, so that
//! the segment's last sample plays and the wait for the next segment is no underrun. While a
//! segment waits for its playback the sink reads the played position every few milliseconds.
//!
//! On the end of input, a control event of kind `stop`, or once its input edge is closed, it
//! finishes the audio and goes on until every completed segment it holds was played and
//! acknowledged and, given a turns port, that edge ended too; it then sends the end of input on, a
//! control event of kind `stop`, and ends.
//! Cancelled, or stopped, it sends nothing more and stops the speaker at once. The speaker is
//! closed, its device stream released, when the exchange ends, or at the node's stop if it never
//! started. Content other than a frame, a completion or a control event is taken and ignored.
//!
//! [`AcknowledgingHandle`] reads what the sink has done: the frames it pushed, the first frame of
//! each segment handed to the speaker and when, each receipt it sent, each interruption and each
//! stale frame it dropped.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::future::{self, Future};
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use inference_audio::SampleClockPosition;
use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_pipeline::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, PortId};
use inference_tts::inference_text::{SegmentId, TurnId};
use inference_tts::{PlaybackReceipt, SegmentAudio};

use crate::playout::{PushError, Speaker};

/// The `kind` of the envelope carrying a receipt: the command the receiver runs with it.
pub const ACKNOWLEDGE_PLAYBACK: &str = "inference.stream.AcknowledgePlayback";

/// How long the sink waits for room in the speaker's queue, or for input while a segment waits
/// for its playback, before looking again.
const POLL: Duration = Duration::from_millis(2);

/// What a payload of the sink's input carries, as the sink reads it.
#[derive(Debug)]
pub enum Playable {
    /// A frame of a segment's audio, as the synthesis emitted it.
    Audio(SegmentAudio),
    /// `inference.stream.SynthesisCompleted` of the segment: every frame of it was sent.
    Completed(SegmentId),
    /// `inference.stream.TurnOpened` of the turn, on the turns port.
    TurnOpened(TurnId),
    /// `inference.stream.GenerationStarted` of the generation, and its epoch.
    Started {
        generation_id: GenerationId,
        epoch: i64,
    },
    /// `inference.stream.GenerationSuperseded` of the generation.
    Superseded(GenerationId),
    /// Anything else.
    Other,
}

/// An interruption: the speaker stopped at the opening of the turn `turn_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interruption {
    pub turn_id: TurnId,
    /// When the sink took the `TurnOpened`.
    pub at: Instant,
    /// The time from taking the `TurnOpened` to the speaker's stop returning, the design's
    /// "interruption to playback stop" (1023).
    pub stop: Duration,
    /// The samples queued and not yet played that the stop cleared.
    pub cleared: usize,
    /// The played position the stop answered.
    pub position: Option<SampleClockPosition>,
    /// The played position read right after the stop.
    pub played_after: Option<SampleClockPosition>,
}

/// A frame of a stale generation the sink dropped, and the played position when it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleFrame {
    pub segment_id: SegmentId,
    pub generation_id: Option<GenerationId>,
    pub played: Option<SampleClockPosition>,
}

/// The payload of the edges around an [`AcknowledgingSink`]: the sink reads frames and
/// completions out of it and builds a receipt, or a control event, into it.
pub trait PlaybackPayload: Send + Sized + 'static {
    /// What the payload carries, taken out of it.
    fn playable(self) -> Playable;

    /// The payload carrying `receipt`, the playback of the segment `segment_id`.
    fn receipt(segment_id: SegmentId, receipt: PlaybackReceipt) -> Self;

    /// The payload of the control event `kind`; its envelope's `kind` is [`ControlKind::as_str`].
    fn control(kind: ControlKind) -> Self;
}

/// A receipt the sink sent: the segment, how many frames it was made of, and when it was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SentReceipt {
    pub segment_id: SegmentId,
    pub frames: usize,
    pub at: Instant,
}

/// The first frame of a segment the sink handed to its speaker, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstFrame {
    pub segment_id: SegmentId,
    pub at: Instant,
}

/// What the sink was asked and what it has done.
#[derive(Debug, Default)]
struct State {
    cancelled: bool,
    ending: bool,
    done: bool,
    pushed: u64,
    first_frames: Vec<FirstFrame>,
    receipts: Vec<SentReceipt>,
    interruptions: Vec<Interruption>,
    stale: Vec<StaleFrame>,
    failure: Option<String>,
    finished: Option<Waker>,
}

/// The state a sink, its exchange thread and its handle share.
#[derive(Debug, Default)]
struct Shared(Mutex<State>);

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn update<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.lock();
        let answer = change(&mut state);
        let waker = state.finished.take();
        drop(state);
        if let Some(waker) = waker {
            waker.wake();
        }
        answer
    }

    fn read<T>(&self, read: impl FnOnce(&State) -> T) -> T {
        read(&self.lock())
    }

    fn over(&self) -> bool {
        self.read(|state| state.cancelled || state.ending)
    }

    /// Waits until the exchange is done.
    async fn done(&self) {
        future::poll_fn(|cx| {
            let mut state = self.lock();
            if state.done {
                Poll::Ready(())
            } else {
                state.finished = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await;
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.read(|state| state.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Marks the exchange done when its thread leaves its work, returning or unwinding.
struct Done(Arc<Shared>);

impl Drop for Done {
    fn drop(&mut self) {
        self.0.update(|state| state.done = true);
    }
}

/// The frames of one segment the sink holds, and where its audio ends on the frames' clock.
#[derive(Debug, Default)]
struct Held {
    frames: Vec<SegmentAudio>,
    end: u64,
    completed: bool,
    generation_id: Option<GenerationId>,
}

/// One started exchange of an [`AcknowledgingSink`].
struct Playing<P> {
    run_id: RunId,
    producer: NodeId,
    port: String,
    stream_id: StreamId,
    sequence: i64,
    speaker: Speaker,
    input: Box<dyn InputEndpoint<P>>,
    /// The edge at the turns port, if the sink was given one, until it ended.
    turns: Option<Box<dyn InputEndpoint<P>>>,
    output: Box<dyn OutputEndpoint<P>>,
    shared: Arc<Shared>,
    /// The segments not yet acknowledged, in arrival order, and what the sink holds of each.
    order: VecDeque<SegmentId>,
    held: HashMap<SegmentId, Held>,
    /// The frame taken and waiting for room in the speaker's queue, with its generation.
    pending: Option<(SegmentAudio, Option<GenerationId>)>,
    /// The generations of the frames taken, and those whose frames are stale.
    seen: HashSet<GenerationId>,
    stale: HashSet<GenerationId>,
    /// The epoch of each generation whose `GenerationStarted` the sink took.
    epochs: HashMap<GenerationId, i64>,
    /// How many turns the sink saw open.
    opened: i64,
    /// The interruption of the turn opened last when no audio was left to stop, recorded once a
    /// generation that began before it is superseded.
    awaiting: Option<Interruption>,
    /// The segments whose frames the sink dropped, which it never acknowledges.
    dropped: HashSet<SegmentId>,
    input_ended: bool,
}

impl<P: PlaybackPayload> Playing<P> {
    fn envelope(
        &mut self,
        kind: &str,
        generation_id: Option<GenerationId>,
        payload: P,
    ) -> EventEnvelope<P> {
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

    /// Sends `envelope`, unless the sink is cancelled or ended first.
    fn send(&self, envelope: EventEnvelope<P>) -> Result<(), String> {
        let shared = Arc::clone(&self.shared);
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut sending = pin!(self.output.send(envelope));
        loop {
            if shared.over() {
                return Ok(());
            }
            if let Poll::Ready(sent) = sending.as_mut().poll(&mut context) {
                return sent.map_err(|refused| refused.to_string());
            }
            thread::park_timeout(POLL);
        }
    }

    /// Pushes the frame waiting for room, if the queue has room for it now, and keeps it; true
    /// while it still waits.
    fn push_pending(&mut self) -> Result<bool, String> {
        let Some((audio, _)) = &self.pending else {
            return Ok(false);
        };
        match self.speaker.try_push(audio.frame()) {
            Ok(()) => {}
            Err(PushError::Full) => return Ok(true),
            Err(PushError::Stopped) => {
                self.pending = None;
                return Ok(false);
            }
            Err(refused) => return Err(format!("the speaker refused a frame: {refused}")),
        }
        let Some((audio, generation_id)) = self.pending.take() else {
            return Ok(false);
        };
        let segment_id = audio.segment_id();
        let frame = audio.frame();
        let end = frame.position.get() + u64::from(frame.sample_count);
        let first = !self.held.contains_key(&segment_id);
        if first {
            self.order.push_back(segment_id);
        }
        let held = self.held.entry(segment_id).or_default();
        held.end = held.end.max(end);
        held.generation_id = held.generation_id.or(generation_id);
        held.frames.push(audio);
        let at = Instant::now();
        self.shared.update(|state| {
            state.pushed += 1;
            if first {
                state.first_frames.push(FirstFrame { segment_id, at });
            }
        });
        Ok(false)
    }

    /// Drops `audio`, a frame of a stale generation: no receipt is ever made of its segment.
    fn drop_stale(&mut self, audio: &SegmentAudio, generation_id: Option<GenerationId>) {
        let segment_id = audio.segment_id();
        self.dropped.insert(segment_id);
        let played = self.speaker.played();
        self.shared.update(|state| {
            state.stale.push(StaleFrame {
                segment_id,
                generation_id,
                played,
            });
        });
    }

    /// Forgets every segment the sink holds, none of which it acknowledges.
    fn forget_held(&mut self) {
        self.dropped.extend(self.order.drain(..));
        self.held.clear();
    }

    /// Whether the generation `generation_id` began before the opening of the turn opened last:
    /// its `GenerationStarted` named an epoch below that turn's ordinal.
    fn began_before_turn(&self, generation_id: GenerationId) -> bool {
        self.epochs
            .get(&generation_id)
            .is_some_and(|epoch| *epoch < self.opened)
    }

    /// Whether a frame of the generation `generation_id` is stale.
    fn is_stale(&self, generation_id: Option<GenerationId>) -> bool {
        generation_id.is_some_and(|generation| {
            self.stale.contains(&generation) || self.began_before_turn(generation)
        })
    }

    /// The opening of the turn `turn_id`: every generation that began before it is stale; audio
    /// still to play is stopped at once and its segments forgotten.
    fn turn_opened(&mut self, turn_id: TurnId) {
        let at = Instant::now();
        self.opened += 1;
        self.stale.extend(self.seen.iter().copied());
        let opened = self.opened;
        self.stale.extend(
            self.epochs
                .iter()
                .filter(|(_, epoch)| **epoch < opened)
                .map(|(generation_id, _)| *generation_id),
        );
        let unplayed = self.pending.is_some() || self.speaker.queued() > 0;
        if let Some((audio, generation_id)) = self.pending.take() {
            if self.is_stale(generation_id) {
                self.drop_stale(&audio, generation_id);
            } else {
                self.pending = Some((audio, generation_id));
            }
        }
        self.forget_held();
        if !unplayed {
            let played = self.speaker.played();
            self.awaiting = Some(Interruption {
                turn_id,
                at,
                stop: at.elapsed(),
                cleared: 0,
                position: played,
                played_after: played,
            });
            return;
        }
        self.awaiting = None;
        let cleared = self.speaker.queued();
        let position = self.speaker.stop();
        let stop = at.elapsed();
        let played_after = self.speaker.played();
        self.shared.update(|state| {
            state.interruptions.push(Interruption {
                turn_id,
                at,
                stop,
                cleared,
                position,
                played_after,
            });
        });
    }

    /// The generation `generation_id` was superseded: its frames are stale, and audio of it still
    /// to play is stopped at once. A generation that began before the turn opened last, which
    /// found no audio to stop, was cut before it played: the interruption of that turn is
    /// recorded.
    fn superseded(&mut self, generation_id: GenerationId) {
        let by_turn = self.is_stale(Some(generation_id));
        self.stale.insert(generation_id);
        let holds = self
            .held
            .values()
            .any(|held| held.generation_id == Some(generation_id));
        if holds {
            let _unplayed_is_dropped = self.speaker.stop();
            self.forget_held();
        }
        if by_turn && let Some(interruption) = self.awaiting.take() {
            self.shared
                .update(|state| state.interruptions.push(interruption));
        }
    }

    /// The segment `segment_id` is complete: every frame of it was given.
    fn completed(&mut self, segment_id: SegmentId) {
        if !self.held.contains_key(&segment_id) {
            self.order.push_back(segment_id);
        }
        self.held.entry(segment_id).or_default().completed = true;
        self.speaker.finish();
    }

    /// Sends the receipt of every completed segment, in arrival order, whose audio the speaker
    /// played past.
    fn acknowledge(&mut self) -> Result<(), String> {
        while let Some(&segment_id) = self.order.front() {
            let Some(held) = self.held.get(&segment_id) else {
                self.order.pop_front();
                continue;
            };
            let played = held.frames.is_empty()
                || self
                    .speaker
                    .played()
                    .is_some_and(|played| played.get() >= held.end);
            if !held.completed || !played {
                return Ok(());
            }
            self.order.pop_front();
            let held = self.held.remove(&segment_id).unwrap_or_default();
            let frames = held.frames.len();
            let receipt = PlaybackReceipt::from_frames(held.frames)
                .map_err(|refused| format!("the sink holds frames of two segments: {refused}"))?;
            let envelope = self.envelope(
                ACKNOWLEDGE_PLAYBACK,
                held.generation_id,
                P::receipt(segment_id, receipt),
            );
            self.send(envelope)?;
            let at = Instant::now();
            self.shared.update(|state| {
                state.receipts.push(SentReceipt {
                    segment_id,
                    frames,
                    at,
                });
            });
        }
        Ok(())
    }

    /// Acts on `envelope`, from the audio input.
    fn take(&mut self, envelope: EventEnvelope<P>) {
        match ControlKind::of(&envelope.kind) {
            Some(ControlKind::Stop) => {
                self.input_ended = true;
                self.speaker.finish();
            }
            Some(_) => {}
            None => {
                let generation_id = envelope.generation_id;
                let stale = self.is_stale(generation_id);
                match envelope.payload.playable() {
                    Playable::Audio(audio) if stale => self.drop_stale(&audio, generation_id),
                    Playable::Audio(audio) => {
                        if let Some(generation) = generation_id {
                            self.seen.insert(generation);
                        }
                        self.pending = Some((audio, generation_id));
                    }
                    Playable::Completed(segment_id) => {
                        if !stale && !self.dropped.contains(&segment_id) {
                            self.completed(segment_id);
                        }
                    }
                    Playable::Superseded(generation) => self.superseded(generation),
                    Playable::Started {
                        generation_id,
                        epoch,
                    } => {
                        self.epochs.insert(generation_id, epoch);
                    }
                    Playable::TurnOpened(_) | Playable::Other => {}
                }
            }
        }
    }

    /// Acts on `envelope`, from the turns input.
    fn take_turn(&mut self, envelope: EventEnvelope<P>) {
        if ControlKind::of(&envelope.kind) == Some(ControlKind::Stop) {
            self.turns = None;
            return;
        }
        if let Playable::TurnOpened(turn_id) = envelope.payload.playable() {
            self.turn_opened(turn_id);
        }
    }

    fn exchange(&mut self) -> Result<(), String> {
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut context = Context::from_waker(&waker);
        loop {
            if self.shared.over() {
                let _unplayed_is_dropped = self.speaker.stop();
                return Ok(());
            }
            let turn = self
                .turns
                .as_mut()
                .map(|turns| turns.recv().as_mut().poll(&mut context));
            match turn {
                Some(Poll::Ready(Some(envelope))) => {
                    self.take_turn(envelope);
                    continue;
                }
                Some(Poll::Ready(None)) => self.turns = None,
                Some(Poll::Pending) | None => {}
            }
            let waiting = self.push_pending()?;
            let received = if self.input_ended || waiting {
                Poll::Pending
            } else {
                self.input.recv().as_mut().poll(&mut context)
            };
            let idle = received.is_pending();
            match received {
                Poll::Ready(Some(envelope)) => self.take(envelope),
                Poll::Ready(None) => {
                    self.input_ended = true;
                    self.speaker.finish();
                }
                Poll::Pending => {}
            }
            self.push_pending()?;
            self.acknowledge()?;
            if self.input_ended
                && self.order.is_empty()
                && self.pending.is_none()
                && self.turns.is_none()
            {
                let stop = ControlKind::Stop;
                let end = self.envelope(stop.as_str(), None, P::control(stop));
                return self.send(end);
            }
            if idle {
                thread::park_timeout(POLL);
            }
        }
    }
}

/// The speaker as a sink node that acknowledges playback; see the module documentation.
pub struct AcknowledgingSink {
    run_id: RunId,
    turns: Option<PortId>,
    node_id: Option<NodeId>,
    speaker: Option<Speaker>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for AcknowledgingSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcknowledgingSink")
            .field("run_id", &self.run_id)
            .field("node_id", &self.node_id)
            .field("speaker", &self.speaker)
            .field("shared", &self.shared)
            .finish_non_exhaustive()
    }
}

impl AcknowledgingSink {
    /// The sink of the run `run_id` playing to `speaker`, which it owns and closes once it
    /// ended.
    #[must_use]
    pub fn new(run_id: RunId, speaker: Speaker) -> Self {
        Self {
            run_id,
            turns: None,
            node_id: None,
            speaker: Some(speaker),
            shared: Arc::default(),
            thread: None,
        }
    }

    /// The same sink given a turns port, `turns`: an edge arriving there carries the turn
    /// events of the turn policy; see the module documentation.
    #[must_use]
    pub const fn with_turns(mut self, turns: PortId) -> Self {
        self.turns = Some(turns);
        self
    }

    /// The caller's side of the sink.
    #[must_use]
    pub fn handle(&self) -> AcknowledgingHandle {
        AcknowledgingHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Waits until the exchange is done, then joins its thread; a panic of the thread becomes
    /// the sink's failure.
    async fn finished(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.shared.done().await;
            if thread.join().is_err() {
                self.shared.update(|state| {
                    state.failure = Some("the sink's exchange panicked".to_owned());
                });
            }
        }
    }
}

/// The caller's side of an [`AcknowledgingSink`].
#[derive(Debug, Clone)]
pub struct AcknowledgingHandle {
    shared: Arc<Shared>,
}

impl AcknowledgingHandle {
    /// How many frames the sink pushed to its speaker.
    #[must_use]
    pub fn pushed(&self) -> u64 {
        self.shared.read(|state| state.pushed)
    }

    /// The first frame of each segment the sink handed to its speaker, in the order handed.
    #[must_use]
    pub fn first_frames(&self) -> Vec<FirstFrame> {
        self.shared.read(|state| state.first_frames.clone())
    }

    /// Every receipt the sink sent, in the order sent.
    #[must_use]
    pub fn receipts(&self) -> Vec<SentReceipt> {
        self.shared.read(|state| state.receipts.clone())
    }

    /// Every interruption, in the order the turns opened.
    #[must_use]
    pub fn interruptions(&self) -> Vec<Interruption> {
        self.shared.read(|state| state.interruptions.clone())
    }

    /// Every frame of a stale generation the sink dropped, in the order dropped.
    #[must_use]
    pub fn stale_frames(&self) -> Vec<StaleFrame> {
        self.shared.read(|state| state.stale.clone())
    }

    /// Whether the sink's exchange ended.
    #[must_use]
    pub fn ended(&self) -> bool {
        self.shared.read(|state| state.done)
    }

    /// The failure that ended the sink's exchange, if one did.
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        self.shared.read(|state| state.failure.clone())
    }
}

impl<P: PlaybackPayload> NodeImplementation<P> for AcknowledgingSink {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let prepared = if self.speaker.is_some() {
            Ok(())
        } else {
            Err(NodeError::new(
                "the sink's speaker was already used by a run",
            ))
        };
        Box::pin(future::ready(prepared))
    }

    fn start(&mut self, mut endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let turns_port = self.turns;
        let at_turns = endpoints
            .inputs
            .iter()
            .position(|input| Some(input.port_id) == turns_port);
        let turns = at_turns.map(|index| endpoints.inputs.remove(index).endpoint);
        let shaped = if endpoints.inputs.len() == 1
            && endpoints.outputs.len() == 1
            && turns.is_some() == turns_port.is_some()
        {
            Ok(())
        } else {
            Err(NodeError::new(format!(
                "the sink takes 1 input edge, 1 more at its turns port if it was given one, and 1 \
                 output edge, and was handed {} input edge(s) besides {} at its turns port and {} \
                 output edge(s)",
                endpoints.inputs.len(),
                usize::from(turns.is_some()),
                endpoints.outputs.len()
            )))
        };
        let started = shaped.and_then(|()| {
            let producer = self
                .node_id
                .ok_or_else(|| NodeError::new("the node was started before it was described"))?;
            let speaker = self
                .speaker
                .take()
                .ok_or_else(|| NodeError::new("the sink's speaker was already used"))?;
            let output = endpoints.outputs.remove(0);
            let mut playing = Playing {
                run_id: self.run_id,
                producer,
                port: output.port_id.to_string(),
                stream_id: StreamId::generate(),
                sequence: 0,
                speaker,
                input: endpoints.inputs.remove(0).endpoint,
                turns,
                output: output.endpoint,
                shared: Arc::clone(&self.shared),
                order: VecDeque::new(),
                held: HashMap::new(),
                pending: None,
                seen: HashSet::new(),
                stale: HashSet::new(),
                epochs: HashMap::new(),
                opened: 0,
                awaiting: None,
                dropped: HashSet::new(),
                input_ended: false,
            };
            let done = Done(Arc::clone(&self.shared));
            self.thread = Some(thread::spawn(move || {
                let ended = playing.exchange();
                drop(playing);
                if let Err(failure) = ended {
                    done.0.update(|state| state.failure = Some(failure));
                }
                drop(done);
            }));
            Ok(())
        });
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(async move {
            self.finished().await;
            self.shared.failure()
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.shared.update(|state| state.cancelled = true);
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.shared.update(|state| state.ending = true);
        self.speaker = None;
        Box::pin(self.finished())
    }
}
