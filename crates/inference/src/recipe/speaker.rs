//! The speaker of `voice_cascade`: the reference cascade's [`AcknowledgingSink`] playing through
//! an [`AudioRoute`], the route native audio takes, instead of a device speaker opened before the
//! run. [`RoutedPlayout`] is the sink's [`Playout`] over that route.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use inference_audio::{AudioFrame, SampleClockPosition};
use inference_audio_device::PushError;
use inference_audio_device::acknowledging::{AcknowledgingSink, Playout};
use inference_core::id::RunId;
use inference_pipeline::PortId;

use super::{Instantiation, Recipe};
use crate::{Error, Stack};

/// Where the cascade's synthesised audio goes: the frames routed, how many of them were handed
/// out, a cut of what was not, and the application told of a barge-in.
pub(crate) trait AudioRoute: Send + Sync {
    /// Routes `frame`, after every frame routed before it.
    fn route(&self, frame: AudioFrame);
    /// How many of the frames routed so far have been handed out.
    fn played(&self) -> u64;
    /// Stops the audio routed and not yet handed out.
    fn cut(&self);
    /// Tells the application of a barge-in.
    fn interrupted(&self);
}

/// A frame routed and not yet seen handed out: its ordinal among the frames routed, its samples
/// and where its audio ends on the frames' clock.
#[derive(Debug, Clone, Copy)]
struct Routed {
    ordinal: u64,
    samples: usize,
    end: Option<SampleClockPosition>,
}

#[derive(Debug, Default)]
struct Routing {
    /// How many frames were routed.
    routed: u64,
    /// How many routed frames a cut dropped, which the route never hands out.
    dropped: u64,
    /// The frames routed since the last stop and not yet seen handed out, in order.
    unplayed: VecDeque<Routed>,
    /// The played position: the end of the last frame seen handed out, or the start of the first
    /// frame routed since the last stop; `None` when none was.
    played: Option<SampleClockPosition>,
}

/// The [`Playout`] over an [`AudioRoute`]: a frame pushed is routed at once and never answered
/// `Full`; the played position and the queue are read off the route's `played()`.
pub(crate) struct RoutedPlayout {
    route: Arc<dyn AudioRoute>,
    routing: Mutex<Routing>,
}

impl RoutedPlayout {
    pub(crate) fn new(route: Arc<dyn AudioRoute>) -> Self {
        Self {
            route,
            routing: Mutex::default(),
        }
    }

    /// The routing, with every frame the route has handed out since forgotten.
    fn routing(&self) -> std::sync::MutexGuard<'_, Routing> {
        let mut routing = self.routing.lock().unwrap_or_else(PoisonError::into_inner);
        let handed = self.route.played().saturating_add(routing.dropped);
        while let Some(front) = routing.unplayed.front().copied() {
            if front.ordinal >= handed {
                break;
            }
            routing.unplayed.pop_front();
            routing.played = front.end;
        }
        routing
    }
}

impl Playout for RoutedPlayout {
    fn try_push(&self, frame: &AudioFrame) -> Result<(), PushError> {
        let mut routing = self.routing();
        let ordinal = routing.routed;
        routing.routed += 1;
        let end = frame
            .position
            .get()
            .checked_add(u64::from(frame.sample_count))
            .and_then(SampleClockPosition::new);
        let samples = usize::try_from(frame.sample_count).unwrap_or(usize::MAX);
        if routing.unplayed.is_empty() && routing.played.is_none() {
            routing.played = Some(frame.position);
        }
        routing.unplayed.push_back(Routed {
            ordinal,
            samples,
            end,
        });
        self.route.route(frame.clone());
        drop(routing);
        Ok(())
    }

    fn finish(&self) {}

    fn stop(&self) -> Option<SampleClockPosition> {
        let mut routing = self.routing();
        self.route.cut();
        let position = routing.played;
        routing.dropped += u64::try_from(routing.unplayed.len()).unwrap_or(u64::MAX);
        routing.unplayed.clear();
        routing.played = None;
        position
    }

    fn played(&self) -> Option<SampleClockPosition> {
        self.routing().played
    }

    fn queued(&self) -> usize {
        self.routing()
            .unplayed
            .iter()
            .map(|routed| routed.samples)
            .sum()
    }
}

/// The cascade's speaker sink of the run `run_id` over `route`, given the turns port `turns`:
/// it cuts at a turn's opening only when `auto_interrupt` is on, and tells the route of each
/// interruption it records.
pub(crate) fn sink(
    run_id: RunId,
    route: &Arc<dyn AudioRoute>,
    turns: PortId,
    auto_interrupt: bool,
) -> AcknowledgingSink {
    let told = Arc::clone(route);
    AcknowledgingSink::over(run_id, Box::new(RoutedPlayout::new(Arc::clone(route))))
        .with_turns(turns)
        .cutting(auto_interrupt)
        .on_interruption(move |_| told.interrupted())
}

/// Instantiates `voice_cascade`, or any recipe, over `stack`, the cascade's audio taking `route`
/// and its barge-in following `auto_interrupt`.
///
/// # Errors
///
/// Those of [`Recipe::instantiate`].
pub(crate) fn instantiate(
    recipe: &Recipe,
    stack: &Stack,
    route: Arc<dyn AudioRoute>,
    auto_interrupt: bool,
) -> Result<Instantiation, Error> {
    let mut instantiation = recipe.instantiate(stack)?;
    instantiation.speaker = Some(Speaker {
        route,
        auto_interrupt,
    });
    Ok(instantiation)
}

/// What an instantiation's cascade plays through: the route and whether a turn's opening cuts.
#[derive(Clone)]
pub(crate) struct Speaker {
    pub(crate) route: Arc<dyn AudioRoute>,
    pub(crate) auto_interrupt: bool,
}

impl std::fmt::Debug for Speaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Speaker")
            .field("auto_interrupt", &self.auto_interrupt)
            .finish_non_exhaustive()
    }
}

impl Instantiation {
    /// The route and barge-in the cascade was instantiated with, through [`instantiate`].
    pub(crate) const fn speaker(&self) -> Option<&Speaker> {
        self.speaker.as_ref()
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::future::{self, Future};
    use std::num::{NonZeroU16, NonZeroU32};
    use std::pin::pin;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::task::{Context, Poll, Wake, Waker};
    use std::thread::{self, Thread};
    use std::time::{Duration, Instant};

    use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
    use inference_audio_device::acknowledging::AcknowledgingHandle;
    use inference_audio_device::acknowledging::{ACKNOWLEDGE_PLAYBACK, Playable, PlaybackPayload};
    use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
    use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
    use inference_core::generation::{Generation, StartGeneration};
    use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
    use inference_core::model::{
        Adapter, AdapterDescriptor, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
    };
    use inference_facade::live::{LiveSession, LiveSessions};
    use inference_pipeline::{NodeEndpoints, NodeImplementation, NodeInput, NodeOutput};
    use inference_runtime::RunError;
    use inference_runtime::run::{Run, RunState};
    use inference_testkit::stt::synthetic_audio::{FIXTURE, Stretch, synthesize};
    use inference_text::{OutputSegment, SegmentId, TurnId};
    use inference_tts::{CONTRACT_REVISION, PlaybackReceipt, SynthesisEvent, SynthesisOutput};

    use super::{AudioRoute, sink};
    use crate::{Binding, Env, ErrorKind, Payload, Recipe, Role, Stack};

    const FRAME: u32 = 320;

    #[test]
    fn instantiating_the_cascade_without_tts_is_role_unbound() {
        let stack = Stack::new()
            .bind(Binding::named(Role::Stt, "mock", BTreeMap::new()))
            .bind(Binding::named(Role::Llm, "mock", BTreeMap::new()))
            .resolve_with(&Env::new())
            .expect("the stack resolves");
        let route: Arc<dyn AudioRoute> = Arc::new(Route::default());
        let error = super::instantiate(&Recipe::voice_cascade(), &stack, route, true)
            .expect_err("no tts is bound");
        assert_eq!(error.kind(), ErrorKind::RoleUnbound);
        assert!(error.to_string().contains("tts"), "{error}");
    }

    #[test]
    fn the_cascade_keeps_the_route_and_barge_in_it_was_instantiated_with() {
        let stack = [Role::Stt, Role::Llm, Role::Tts]
            .into_iter()
            .fold(Stack::new(), |stack, role| {
                stack.bind(Binding::named(role, "mock", BTreeMap::new()))
            })
            .resolve_with(&Env::new())
            .expect("the stack resolves");
        let route: Arc<dyn AudioRoute> = Arc::new(Route::default());
        let instantiation =
            super::instantiate(&Recipe::voice_cascade(), &stack, Arc::clone(&route), false)
                .expect("the cascade plans over the mocks");
        let speaker = instantiation
            .speaker()
            .expect("the entry keeps the speaker");
        assert!(Arc::ptr_eq(&speaker.route, &route));
        assert!(!speaker.auto_interrupt);
        assert!(
            Recipe::voice_cascade()
                .instantiate(&stack)
                .expect("plans")
                .speaker()
                .is_none()
        );
    }

    #[test]
    fn the_opened_cascade_answers_its_speakers_handle() {
        let stack = [Role::Stt, Role::Llm, Role::Tts]
            .into_iter()
            .fold(Stack::new(), |stack, role| {
                stack.bind(Binding::named(role, "mock", BTreeMap::new()))
            })
            .resolve_with(&Env::new())
            .expect("the stack resolves");
        let route: Arc<dyn AudioRoute> = Arc::new(Route::default());
        let mut instantiation = super::instantiate(&Recipe::voice_cascade(), &stack, route, true)
            .expect("the cascade plans over the mocks");
        assert!(instantiation.sink().is_none(), "no run is open");
        let sessions = LiveSessions::new();
        let (mut session, driver) =
            block_on(instantiation.open(&stack, &sessions)).expect("the cascade's run opens");
        let sink = instantiation
            .sink()
            .cloned()
            .expect("the open run's speaker answers");
        assert_eq!(sink.pushed(), 0);
        assert!(sink.interruptions().is_empty());
        assert!(!sink.ended(), "the speaker runs");
        let (done, ended) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = done.send(block_on(driver.drive()));
        });
        block_on(session.close()).expect("a Ready session drains");
        while block_on(session.receive()).is_some() {}
        let run = ended
            .recv_timeout(Duration::from_secs(20))
            .expect("the run ends within 20 s")
            .expect("the run ends");
        assert_eq!(run.state(), RunState::Completed);
        assert!(
            sink.ended(),
            "the handle is the run's speaker, which ended with it"
        );
    }

    /// A route of the test: records each frame routed with its ordinal, the count of frames
    /// routed before it, counts its cuts and interruptions, drops the frames not yet handed out
    /// at a cut and keeps the ordinal the last cut came at; the test sets `played()`.
    #[derive(Default)]
    struct Route {
        frames: Mutex<Vec<AudioFrame>>,
        ordinals: Mutex<Vec<usize>>,
        next: AtomicUsize,
        cut_at: AtomicUsize,
        played: AtomicU64,
        cuts: AtomicUsize,
        interruptions: AtomicUsize,
    }

    impl Route {
        fn routed(&self) -> usize {
            self.frames
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len()
        }

        /// The ordinals of the frames handed out: the first `played()` of those routed.
        fn handed_out(&self) -> Vec<usize> {
            let ordinals = self.ordinals.lock().unwrap_or_else(PoisonError::into_inner);
            let played = usize::try_from(self.played()).unwrap_or(usize::MAX);
            ordinals[..played.min(ordinals.len())].to_vec()
        }
    }

    impl AudioRoute for Route {
        fn route(&self, frame: AudioFrame) {
            let mut frames = self.frames.lock().unwrap_or_else(PoisonError::into_inner);
            frames.push(frame);
            self.ordinals
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(self.next.fetch_add(1, Ordering::SeqCst));
        }

        fn played(&self) -> u64 {
            self.played.load(Ordering::SeqCst)
        }

        fn cut(&self) {
            let mut frames = self.frames.lock().unwrap_or_else(PoisonError::into_inner);
            self.cuts.fetch_add(1, Ordering::SeqCst);
            self.cut_at
                .store(self.next.load(Ordering::SeqCst), Ordering::SeqCst);
            let handed = usize::try_from(self.played()).unwrap_or(usize::MAX);
            frames.truncate(handed);
            self.ordinals
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .truncate(handed);
        }

        fn interrupted(&self) {
            self.interruptions.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// What the edges of the test carry.
    #[derive(Debug)]
    enum Carried {
        Audio(inference_tts::SegmentAudio),
        Completed(SegmentId),
        Receipt(SegmentId),
        TurnOpened(TurnId),
        Other,
    }

    impl PlaybackPayload for Carried {
        fn playable(self) -> Playable {
            match self {
                Self::Audio(audio) => Playable::Audio(audio),
                Self::Completed(segment_id) => Playable::Completed(segment_id),
                Self::TurnOpened(turn_id) => Playable::TurnOpened(turn_id),
                Self::Receipt(_) | Self::Other => Playable::Other,
            }
        }

        fn receipt(segment_id: SegmentId, _: PlaybackReceipt) -> Self {
            Self::Receipt(segment_id)
        }

        fn control(_: ControlKind) -> Self {
            Self::Other
        }
    }

    struct Unpark(Thread);

    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut future = pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
                return output;
            }
            thread::park_timeout(Duration::from_millis(10));
        }
    }

    fn eventually(ready: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if ready() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        ready()
    }

    #[derive(Default)]
    struct Queue {
        envelopes: VecDeque<EventEnvelope<Carried>>,
        closed: bool,
        waker: Option<Waker>,
    }

    /// An unbounded edge the test reads and writes.
    #[derive(Clone, Default)]
    struct Edge(Arc<Mutex<Queue>>);

    impl Edge {
        fn with<T>(&self, change: impl FnOnce(&mut Queue) -> T) -> T {
            let mut queue = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            let answer = change(&mut queue);
            if let Some(waker) = queue.waker.take() {
                waker.wake();
            }
            answer
        }

        fn put(&self, kind: &str, generation_id: Option<GenerationId>, payload: Carried) {
            let envelope = EventEnvelope {
                schema_version: 1,
                event_id: EventId::generate(),
                run_id: RunId::generate(),
                session_id: None,
                stream_id: StreamId::generate(),
                sequence: 1,
                producer: NodeId::generate(),
                port: "test".to_owned(),
                kind: kind.to_owned(),
                generation_id,
                timestamp: 0,
                timebase: Timebase::WallTime,
                causation_id: None,
                payload,
            };
            self.with(|queue| queue.envelopes.push_back(envelope));
        }

        fn close(&self) {
            self.with(|queue| queue.closed = true);
        }

        fn receipts(&self) -> Vec<SegmentId> {
            self.with(|queue| {
                queue
                    .envelopes
                    .iter()
                    .filter(|envelope| envelope.kind == ACKNOWLEDGE_PLAYBACK)
                    .filter_map(|envelope| match envelope.payload {
                        Carried::Receipt(segment_id) => Some(segment_id),
                        _ => None,
                    })
                    .collect()
            })
        }
    }

    impl OutputEndpoint<Carried> for Edge {
        fn send(
            &self,
            envelope: EventEnvelope<Carried>,
        ) -> EndpointFuture<'_, Result<(), SendError<Carried>>> {
            self.with(|queue| queue.envelopes.push_back(envelope));
            Box::pin(future::ready(Ok(())))
        }
    }

    impl InputEndpoint<Carried> for Edge {
        fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Carried>>> {
            let queue = Arc::clone(&self.0);
            Box::pin(future::poll_fn(move |cx| {
                let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(envelope) = queue.envelopes.pop_front() {
                    Poll::Ready(Some(envelope))
                } else if queue.closed {
                    Poll::Ready(None)
                } else {
                    queue.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }))
        }
    }

    /// One generation's synthesis of one segment.
    struct Synthesis {
        output: SynthesisOutput,
        generation: Generation,
        next_sample: u64,
    }

    impl Synthesis {
        fn new() -> Self {
            let (adapter, _) = Adapter::declare(DeclareAdapter {
                model_id: ModelId::generate(),
                backend: "cascade speaker test".to_owned(),
                contract_revision: CONTRACT_REVISION.to_owned(),
                execution_form: ExecutionForm::InProcess,
                locality: Locality::SameProcess,
                capabilities: vec![Capability::IncrementalInput, Capability::OutputStreaming],
                max_concurrency: 1,
                descriptor: AdapterDescriptor::default(),
            })
            .expect("a positive concurrency");
            let (generation, _) = Generation::start(StartGeneration {
                run_id: RunId::generate(),
                node_id: NodeId::generate(),
                epoch: 1,
            });
            Self {
                output: SynthesisOutput::start(&adapter),
                generation,
                next_sample: 0,
            }
        }

        fn segment(&mut self) -> SegmentId {
            let (tentative, _) = OutputSegment::emit(&self.generation, 1, "hello".to_owned())
                .expect("an Active generation");
            let (committed, _) = tentative.commit().expect("a Tentative segment commits");
            let segment_id = committed.segment_id();
            self.output
                .offer(committed)
                .expect("a Committed segment queues");
            self.output
                .take_input()
                .expect("incremental input is claimed");
            segment_id
        }

        fn frame(&mut self, segment_id: SegmentId) -> Carried {
            let frame = AudioFrame {
                format: AudioFormat {
                    encoding: AudioEncoding::PcmS16le,
                    sample_rate: NonZeroU32::new(16_000).expect("a rate"),
                    channels: NonZeroU16::MIN,
                },
                position: SampleClockPosition::new(self.next_sample).expect("a position"),
                sample_count: FRAME,
                data: inference_audio_device::convert::encode(
                    &[0.25; FRAME as usize],
                    AudioEncoding::PcmS16le,
                ),
            };
            self.next_sample += u64::from(FRAME);
            match self.output.audio(segment_id, frame) {
                Ok(SynthesisEvent::Audio(audio)) => Carried::Audio(audio),
                other => panic!("the synthesis emits a frame: {other:?}"),
            }
        }

        fn complete(&mut self, segment_id: SegmentId) -> Carried {
            self.output
                .complete(segment_id, Vec::new())
                .expect("a Queued segment completes");
            Carried::Completed(segment_id)
        }
    }

    /// The cascade's speaker sink over `route`, started on test edges: the speech edge, the
    /// turns edge and the receipts edge.
    struct Started {
        sink: inference_audio_device::acknowledging::AcknowledgingSink,
        speech: Edge,
        turns: Edge,
        receipts: Edge,
        synthesis: Synthesis,
    }

    impl Started {
        fn new(route: &Arc<Route>, auto_interrupt: bool) -> Self {
            let declaration = Recipe::voice_cascade().declaration();
            let node = declaration
                .nodes()
                .find(|node| node.name() == "speaker")
                .expect("the cascade has a speaker")
                .clone();
            let port = |name: &str| {
                declaration
                    .ports()
                    .find(|port| port.node_id() == node.node_id() && port.name() == name)
                    .map(inference_pipeline::Port::port_id)
                    .expect("the speaker has the port")
            };
            let route: Arc<dyn AudioRoute> = Arc::clone(route) as Arc<dyn AudioRoute>;
            let mut sink = sink(RunId::generate(), &route, port("turns"), auto_interrupt);
            let (speech, turns, receipts) = (Edge::default(), Edge::default(), Edge::default());
            let implementation: &mut dyn NodeImplementation<Carried> = &mut sink;
            block_on(implementation.describe(&node));
            block_on(implementation.prepare()).expect("the sink prepares");
            block_on(implementation.start(NodeEndpoints {
                inputs: vec![
                    NodeInput {
                        port_id: port("turns"),
                        endpoint: Box::new(turns.clone()),
                    },
                    NodeInput {
                        port_id: port("audio"),
                        endpoint: Box::new(speech.clone()),
                    },
                ],
                outputs: vec![NodeOutput {
                    port_id: port("played"),
                    endpoint: Box::new(receipts.clone()),
                }],
            }))
            .expect("the sink starts");
            Self {
                sink,
                speech,
                turns,
                receipts,
                synthesis: Synthesis::new(),
            }
        }

        fn send(&self, payload: Carried) {
            let kind = match payload {
                Carried::Completed(_) => "inference.stream.SynthesisCompleted",
                _ => "inference.audio.AudioChunk",
            };
            self.speech.put(
                kind,
                Some(self.synthesis.generation.generation_id()),
                payload,
            );
        }

        fn open_turn(&self) {
            self.turns.put(
                "inference.stream.TurnOpened",
                None,
                Carried::TurnOpened(TurnId::generate()),
            );
        }

        fn end(mut self) {
            self.speech.close();
            self.turns.close();
            let implementation: &mut dyn NodeImplementation<Carried> = &mut self.sink;
            block_on(implementation.cancel());
            block_on(implementation.stop());
        }
    }

    #[test]
    fn a_segment_is_acknowledged_once_the_route_handed_out_its_last_frame() {
        let route = Arc::new(Route::default());
        let mut run = Started::new(&route, true);
        let segment = run.synthesis.segment();
        let first = run.synthesis.frame(segment);
        run.send(first);
        let second = run.synthesis.frame(segment);
        run.send(second);
        let completed = run.synthesis.complete(segment);
        run.send(completed);
        assert!(eventually(|| route.routed() == 2), "both frames are routed");
        let positions: Vec<u64> = route
            .frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|frame| frame.position.get())
            .collect();
        assert_eq!(positions, [0, u64::from(FRAME)], "in order");

        route.played.store(1, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(100));
        assert!(run.receipts.receipts().is_empty(), "one frame is unplayed");

        route.played.store(2, Ordering::SeqCst);
        assert!(eventually(|| run.receipts.receipts() == [segment]));
        run.end();
    }

    /// Two frames of one segment, `played()` held at 0, a turn opened, then a third frame.
    fn barge_in(auto_interrupt: bool) -> (Arc<Route>, Started, SegmentId) {
        let route = Arc::new(Route::default());
        let mut run = Started::new(&route, auto_interrupt);
        let segment = run.synthesis.segment();
        for _ in 0..2 {
            let frame = run.synthesis.frame(segment);
            run.send(frame);
        }
        assert!(eventually(|| route.routed() == 2));
        run.open_turn();
        thread::sleep(Duration::from_millis(100));
        let third = run.synthesis.frame(segment);
        run.send(third);
        let completed = run.synthesis.complete(segment);
        run.send(completed);
        thread::sleep(Duration::from_millis(100));
        (route, run, segment)
    }

    #[test]
    fn a_turn_opened_cuts_the_routed_audio_and_drops_the_later_frame() {
        let (route, run, _) = barge_in(true);
        let handle = run.sink.handle();
        assert!(eventually(|| handle.stale_frames().len() == 1));
        assert_eq!(route.cuts.load(Ordering::SeqCst), 1);
        assert_eq!(route.interruptions.load(Ordering::SeqCst), 1);
        assert_eq!(
            route.routed(),
            0,
            "the cut dropped both, and the third never came"
        );
        assert_eq!(handle.interruptions().len(), 1);
        route.played.store(3, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(100));
        assert!(
            run.receipts.receipts().is_empty(),
            "no receipt for the segment"
        );
        run.end();
    }

    #[test]
    fn a_sink_not_cutting_takes_the_turn_and_acts_on_none() {
        let (route, run, segment) = barge_in(false);
        let handle = run.sink.handle();
        assert!(
            eventually(|| route.routed() == 3),
            "all three frames are routed"
        );
        assert_eq!(route.cuts.load(Ordering::SeqCst), 0);
        assert_eq!(route.interruptions.load(Ordering::SeqCst), 0);
        assert!(handle.interruptions().is_empty());
        route.played.store(3, Ordering::SeqCst);
        assert!(eventually(|| run.receipts.receipts() == [segment]));
        run.end();
    }

    /// The microphone frames of `stretches`, their positions moved on by `from` samples, as the
    /// envelopes of the cascade's `audio` ingress.
    fn microphone(stretches: &[Stretch], from: u64) -> (Vec<EventEnvelope<Payload>>, u64) {
        let mut next = from;
        let envelopes = synthesize(stretches)
            .into_iter()
            .map(|mut frame| {
                let position = from + frame.position.get();
                frame.position = SampleClockPosition::new(position).expect("a position");
                next = position + u64::from(frame.sample_count);
                EventEnvelope {
                    schema_version: 1,
                    event_id: EventId::generate(),
                    run_id: RunId::generate(),
                    session_id: None,
                    stream_id: StreamId::generate(),
                    sequence: 1,
                    producer: NodeId::generate(),
                    port: "audio".to_owned(),
                    kind: "inference.audio.AudioChunk".to_owned(),
                    generation_id: None,
                    timestamp: i64::try_from(position).expect("within i64"),
                    timebase: Timebase::AudioFrameTime,
                    causation_id: None,
                    payload: Payload::Audio(frame),
                }
            })
            .collect();
        (envelopes, next)
    }

    /// The opened cascade of a barge-in: its route, its speaker's handle, the frames the route
    /// had recorded when the second tone began, and the session to close.
    struct Barged {
        route: Arc<Route>,
        sink: AcknowledgingHandle,
        before: usize,
        session: LiveSession<Payload>,
        ended: std::sync::mpsc::Receiver<Result<Run, RunError>>,
    }

    /// `voice_cascade` instantiated through the crate's entry over the mocks and a test route,
    /// opened, with `played()` held at 0: the frames of one turn, then, once the route recorded a
    /// frame of reply 0, the second tone, which opens turn 2 while that reply is unplayed.
    fn cascade_barge_in(auto_interrupt: bool) -> Barged {
        let stack = [Role::Stt, Role::Llm, Role::Tts]
            .into_iter()
            .fold(Stack::new(), |stack, role| {
                stack.bind(Binding::named(role, "mock", BTreeMap::new()))
            })
            .resolve_with(&Env::new())
            .expect("the stack resolves");
        let route = Arc::new(Route::default());
        let routed: Arc<dyn AudioRoute> = Arc::clone(&route) as Arc<dyn AudioRoute>;
        let mut instantiation =
            super::instantiate(&Recipe::voice_cascade(), &stack, routed, auto_interrupt)
                .expect("the cascade plans over the mocks");
        let sessions = LiveSessions::new();
        let (session, driver) =
            block_on(instantiation.open(&stack, &sessions)).expect("the cascade's run opens");
        let sink = instantiation
            .sink()
            .cloned()
            .expect("the open run's speaker answers");
        let (done, ended) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = done.send(block_on(driver.drive()));
        });
        let (first, next) = microphone(&FIXTURE[..2], 0);
        for envelope in first {
            block_on(session.send(envelope)).expect("the ingress takes every frame");
        }
        assert!(
            eventually(|| route.routed() > 0),
            "reply 0 reaches the route"
        );
        let before = route.routed();
        let (second, _) = microphone(&FIXTURE[2..3], next);
        for envelope in second {
            block_on(session.send(envelope)).expect("the ingress takes every frame");
        }
        Barged {
            route,
            sink,
            before,
            session,
            ended,
        }
    }

    impl Barged {
        /// Hands out every frame routed from now on, so the speaker drains, then closes the
        /// session and waits for the run to complete.
        fn end(mut self) {
            self.route.played.store(u64::MAX >> 1, Ordering::SeqCst);
            block_on(self.session.close()).expect("the session drains");
            while block_on(self.session.receive()).is_some() {}
            let run = self
                .ended
                .recv_timeout(Duration::from_secs(20))
                .expect("the run ends within 20 s")
                .expect("the run ends");
            assert_eq!(run.state(), RunState::Completed);
        }
    }

    #[test]
    fn a_second_turn_cuts_the_cascades_unplayed_reply() {
        let barged = cascade_barge_in(true);
        let route = &barged.route;
        assert!(
            eventually(|| route.cuts.load(Ordering::SeqCst) > 0),
            "turn 2 cuts the route"
        );
        assert!(eventually(|| barged.sink.interruptions().len() == 1));
        let cut_at = route.cut_at.load(Ordering::SeqCst);
        assert!(
            cut_at >= barged.before,
            "the cut came after reply 0 was routed"
        );
        route.played.store(route.routed() as u64, Ordering::SeqCst);
        assert!(
            route.handed_out().iter().all(|ordinal| *ordinal >= cut_at),
            "no frame routed before the cut is handed out: {:?}",
            route.handed_out()
        );
        barged.end();
    }

    #[test]
    fn a_cascade_without_auto_interrupt_keeps_its_unplayed_reply() {
        let barged = cascade_barge_in(false);
        let route = &barged.route;
        thread::sleep(Duration::from_millis(500));
        assert_eq!(route.cuts.load(Ordering::SeqCst), 0);
        assert!(barged.sink.interruptions().is_empty());
        route.played.store(route.routed() as u64, Ordering::SeqCst);
        let handed = route.handed_out();
        assert!(
            (0..barged.before).all(|ordinal| handed.contains(&ordinal)),
            "every frame routed before turn 2 is handed out: {handed:?}"
        );
        barged.end();
    }
}
