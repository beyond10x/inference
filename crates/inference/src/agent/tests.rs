//! The pump's step, fed session events directly: the mapping, the barge-in, run epochs and the
//! Agent's end, with no session and no timing.

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_core::generation::{GenerationCancelled, GenerationStarted};
use inference_core::id::{GenerationId, ResponseId, RunId, SessionId, SpeechSessionId};
use inference_duplex::{
    ResponseFailed, SessionFailure, SessionFailureKind, SpeechSessionClosed, SpeechSessionFailed,
    SpeechStarted,
};
use inference_facade::events::SessionEvent;
use inference_facade::session::{
    PipelineSwitched, SessionClosed, SessionInterrupted, SessionReady,
};
use inference_pipeline::PipelineId;
use inference_runtime::RunError;
use inference_runtime::run::{FailureReason, RunCompleted, RunFailed};
use inference_text::{SegmentId, TextSegmentCommitted};
use tokio::sync::broadcast;

use super::{
    Act, CLOSED, CLOSING, Close, Core, OPEN, Playback, Route, SWITCHING, Step, SystemClock,
    barge_in,
};
use crate::Error;
use crate::event::{Event, Item};

/// A sink whose `pending()` is set by the test, counting cuts and frames played.
#[derive(Default)]
struct Sink {
    pending: AtomicBool,
    cuts: AtomicUsize,
    played: Mutex<Vec<AudioFrame>>,
}

impl Playback for Sink {
    fn play(&self, frame: AudioFrame) {
        self.played.lock().expect("unpoisoned").push(frame);
    }
    fn pending(&self) -> bool {
        self.pending.load(Ordering::SeqCst)
    }
    fn cut(&self) {
        self.cuts.fetch_add(1, Ordering::SeqCst);
    }
    fn played(&self) -> u64 {
        0
    }
}

struct Fixture {
    core: Core,
    receiver: broadcast::Receiver<Item>,
    generation: GenerationId,
}

fn fixture(auto_interrupt: bool) -> Fixture {
    let (sender, receiver) = broadcast::channel(64);
    let route = Arc::new(Route::new(sender));
    Fixture {
        core: Core::new(auto_interrupt, route, Arc::new(SystemClock)),
        receiver,
        generation: GenerationId::generate(),
    }
}

impl Fixture {
    fn attach(&self, pending: bool) -> Arc<Sink> {
        let sink = Arc::new(Sink::default());
        sink.pending.store(pending, Ordering::SeqCst);
        *self.core.route.sink.lock().expect("unpoisoned") =
            Some(Arc::clone(&sink) as Arc<dyn Playback>);
        sink
    }

    fn cut(&self) -> bool {
        self.core
            .reply
            .lock()
            .expect("unpoisoned")
            .cut
            .contains(&self.generation)
    }

    /// The events on the channel, by name, and whether it ended.
    fn drain(&mut self) -> (Vec<&'static str>, bool) {
        let mut names = Vec::new();
        loop {
            match self.receiver.try_recv() {
                Ok(Ok(event)) => names.push(name(&event)),
                Ok(Err(_)) => names.push("Err"),
                Err(broadcast::error::TryRecvError::Closed) => return (names, true),
                Err(_) => return (names, false),
            }
        }
    }
}

fn name(event: &Event) -> &'static str {
    match event {
        Event::Started { .. } => "Started",
        Event::Heard { .. } => "Heard",
        Event::Transcript { .. } => "Transcript",
        Event::Said { .. } => "Said",
        Event::Audio(_) => "Audio",
        Event::ToolCalled { .. } => "ToolCalled",
        Event::Usage(_) => "Usage",
        Event::Interrupted { .. } => "Interrupted",
        Event::Closed { .. } => "Closed",
        Event::Error { error, .. } => match **error {
            Error::Provider { .. } => "Error::Provider",
            Error::Layer { .. } => "Error::Layer",
            _ => "Error",
        },
        Event::Lagged(_) => "Lagged",
        Event::ConfirmationRequested { .. } => "ConfirmationRequested",
    }
}

fn started(generation: GenerationId) -> SessionEvent {
    SessionEvent::GenerationStarted(GenerationStarted {
        generation_id: generation,
        run_id: RunId::generate(),
        epoch: 0,
    })
}

fn speech() -> SessionEvent {
    SessionEvent::SpeechStarted(SpeechStarted {
        session_id: SpeechSessionId::generate(),
        position: SampleClockPosition::new(0).expect("in range"),
    })
}

fn said() -> SessionEvent {
    SessionEvent::TextSegmentCommitted(TextSegmentCommitted {
        segment_id: SegmentId::generate(),
        text: "hi".to_owned(),
    })
}

fn chunk() -> SessionEvent {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).expect("non-zero"),
        channels: NonZeroU16::MIN,
    };
    SessionEvent::AudioChunk(AudioFrame {
        format,
        position: SampleClockPosition::new(0).expect("in range"),
        sample_count: 1,
        data: vec![0, 1],
    })
}

fn interrupted() -> SessionEvent {
    SessionEvent::SessionInterrupted(SessionInterrupted {
        session_id: SessionId::generate(),
    })
}

fn failure() -> SessionFailure {
    SessionFailure {
        kind: SessionFailureKind::TransportClosed,
        detail: "gone".to_owned(),
    }
}

fn run_failed() -> SessionEvent {
    SessionEvent::RunFailed(RunFailed {
        run_id: RunId::generate(),
        failure: FailureReason {
            node_id: None,
            detail: "failed".to_owned(),
        },
    })
}

fn session_closed() -> SessionEvent {
    SessionEvent::SessionClosed(SessionClosed {
        session_id: SessionId::generate(),
    })
}

fn model_closed() -> SessionEvent {
    SessionEvent::SpeechSessionClosed(SpeechSessionClosed {
        session_id: SpeechSessionId::generate(),
    })
}

fn model_failed() -> SessionEvent {
    SessionEvent::SpeechSessionFailed(SpeechSessionFailed {
        session_id: SpeechSessionId::generate(),
        failure: failure(),
    })
}

fn interrupts(step: &mut Step, core: &Core, events: Vec<SessionEvent>) -> usize {
    events
        .into_iter()
        .filter(|event| step.on_event(core, event.clone()).interrupt)
        .count()
}

#[test]
fn speech_with_no_reply_and_no_sink_requests_nothing() {
    let mut fixture = fixture(true);
    let mut step = Step::new(None);
    assert_eq!(interrupts(&mut step, &fixture.core, vec![speech()]), 0);
    assert_eq!(fixture.drain().0, Vec::<&str>::new());
}

#[test]
fn speech_before_the_reply_starts_lets_its_audio_pass() {
    let mut fixture = fixture(true);
    let mut step = Step::new(None);
    let events = vec![speech(), started(fixture.generation), chunk()];
    assert_eq!(interrupts(&mut step, &fixture.core, events), 0);
    assert_eq!(fixture.drain().0, ["Audio"]);
}

#[test]
fn a_barge_in_cuts_the_reply_and_its_interrupt_yields_one_interrupted() {
    let mut fixture = fixture(true);
    let mut step = Step::new(None);
    let events = vec![started(fixture.generation), said(), speech(), chunk()];
    assert_eq!(interrupts(&mut step, &fixture.core, events), 1);
    assert!(fixture.cut());
    step.on_event(&fixture.core, interrupted());
    assert_eq!(fixture.drain().0, ["Said", "Interrupted"]);
}

#[test]
fn speech_with_audio_pending_at_the_sink_is_a_barge_in() {
    let mut fixture = fixture(true);
    let sink = fixture.attach(true);
    let mut step = Step::new(None);
    assert_eq!(interrupts(&mut step, &fixture.core, vec![speech()]), 1);
    assert_eq!(sink.cuts.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.drain().0, Vec::<&str>::new());
}

#[test]
fn a_failed_response_is_an_error_and_no_interruption() {
    let mut fixture = fixture(true);
    let mut step = Step::new(None);
    let failed = SessionEvent::SpeechResponseFailed(ResponseFailed {
        response_id: ResponseId::generate(),
        failure: failure(),
    });
    let cancelled = SessionEvent::GenerationCancelled(GenerationCancelled {
        generation_id: fixture.generation,
    });
    let events = vec![started(fixture.generation), failed, cancelled];
    assert_eq!(interrupts(&mut step, &fixture.core, events), 0);
    assert_eq!(fixture.drain().0, ["Error::Provider"]);
}

#[test]
fn a_cancelled_generation_alone_is_no_interruption() {
    let mut fixture = fixture(true);
    let mut step = Step::new(None);
    let cancelled = SessionEvent::GenerationCancelled(GenerationCancelled {
        generation_id: fixture.generation,
    });
    assert_eq!(interrupts(&mut step, &fixture.core, vec![cancelled]), 0);
    assert_eq!(fixture.drain().0, Vec::<&str>::new());
}

#[test]
fn with_auto_interrupt_off_nothing_is_cut() {
    let mut fixture = fixture(false);
    let sink = fixture.attach(true);
    let mut step = Step::new(None);
    let events = vec![started(fixture.generation), said(), speech(), chunk()];
    assert_eq!(interrupts(&mut step, &fixture.core, events), 0);
    assert!(!fixture.cut());
    assert_eq!(sink.cuts.load(Ordering::SeqCst), 0);
    assert_eq!(sink.played.lock().expect("unpoisoned").len(), 1);
    assert_eq!(fixture.drain().0, ["Said"]);
}

#[test]
fn the_cut_ends_the_reply() {
    let mut fixture = fixture(true);
    let mut step = Step::new(None);
    let events = vec![
        started(fixture.generation),
        speech(),
        interrupted(),
        speech(),
        interrupted(),
    ];
    assert_eq!(interrupts(&mut step, &fixture.core, events), 1);
    assert_eq!(fixture.drain().0, ["Interrupted"]);
}

#[test]
fn a_frame_is_attributed_to_the_last_started_generation() {
    let mut fixture = fixture(true);
    let mut step = Step::new(None);
    interrupts(
        &mut step,
        &fixture.core,
        vec![started(fixture.generation), speech(), chunk()],
    );
    assert_eq!(fixture.drain().0, Vec::<&str>::new());
    interrupts(
        &mut step,
        &fixture.core,
        vec![started(GenerationId::generate()), chunk()],
    );
    assert_eq!(fixture.drain().0, ["Audio"]);
}

#[test]
fn the_applications_cut_drops_the_replys_frames() {
    let mut fixture = fixture(true);
    let mut step = Step::new(None);
    step.on_event(&fixture.core, started(fixture.generation));
    {
        let mut reply = fixture.core.reply.lock().expect("unpoisoned");
        let generation = reply.in_progress.take().expect("in progress");
        reply.cut.insert(generation);
    }
    assert_eq!(
        interrupts(&mut step, &fixture.core, vec![chunk(), speech()]),
        0
    );
    assert!(fixture.cut());
    assert_eq!(fixture.drain().0, Vec::<&str>::new());
}

#[test]
fn a_restarted_pump_reads_the_same_tracking() {
    let mut fixture = fixture(true);
    Step::new(None).on_event(&fixture.core, started(fixture.generation));
    let mut step = Step::new(None);
    assert_eq!(
        interrupts(&mut step, &fixture.core, vec![speech(), chunk()]),
        1
    );
    assert_eq!(fixture.drain().0, Vec::<&str>::new());
}

#[test]
fn an_old_epochs_close_and_run_end_are_dropped() {
    let mut fixture = fixture(true);
    let r0 = RunId::generate();
    let mut step = Step::new(Some(r0));
    let switched = SessionEvent::PipelineSwitched(PipelineSwitched {
        session_id: SessionId::generate(),
        pipeline_id: PipelineId::generate(),
    });
    for event in [
        switched,
        model_closed(),
        SessionEvent::RunCompleted(RunCompleted { run_id: r0 }),
    ] {
        assert_eq!(step.on_event(&fixture.core, event), Act::default());
    }
    assert_eq!(fixture.core.state(), OPEN);
    assert!(step.left.is_none());
    step.on_event(&fixture.core, said());
    assert_eq!(fixture.drain().0, ["Said"]);
}

#[test]
fn a_model_close_while_switching_leaves_the_switch_to_win() {
    let fixture = fixture(true);
    fixture.core.state.store(SWITCHING, Ordering::SeqCst);
    let act = Step::new(None).on_event(&fixture.core, model_closed());
    assert_eq!(act, Act::default());
    assert_eq!(fixture.core.state(), SWITCHING);
}

#[test]
fn a_model_close_while_open_spawns_the_close() {
    let fixture = fixture(true);
    let act = Step::new(None).on_event(&fixture.core, model_closed());
    assert_eq!(act.close, Some(Close::Drain));
    assert_eq!(fixture.core.state(), CLOSING);
}

#[test]
fn an_old_epochs_barge_in_is_dropped() {
    let mut fixture = fixture(true);
    let r0 = RunId::generate();
    let mut step = Step::new(Some(r0));
    let events = vec![
        started(fixture.generation),
        speech(),
        SessionEvent::RunCompleted(RunCompleted { run_id: r0 }),
    ];
    assert_eq!(interrupts(&mut step, &fixture.core, events), 0);
    assert!(!fixture.cut());
    let events = vec![started(GenerationId::generate()), speech()];
    assert_eq!(interrupts(&mut step, &fixture.core, events), 1);
    assert_eq!(fixture.drain().0, Vec::<&str>::new());
}

#[test]
fn a_failed_run_after_the_drivers_end_closes_once() {
    let mut fixture = fixture(true);
    let mut step = Step::new(None);
    let mut closes = usize::from(step.on_driver(&fixture.core, &Ok(())).close.is_some());
    closes += usize::from(step.on_event(&fixture.core, run_failed()).close.is_some());
    step.on_end(&fixture.core, Some(&Ok(())));
    assert_eq!(closes, 1);
    assert_eq!(fixture.drain(), (vec!["Error::Layer", "Closed"], true));
    assert_eq!(fixture.core.state(), CLOSED);
}

#[test]
fn the_drivers_end_then_the_session_closed_closes_once() {
    let mut fixture = fixture(true);
    let mut step = Step::new(None);
    let act = step.on_driver(&fixture.core, &Ok(()));
    assert_eq!(act.close, Some(Close::Cancel));
    assert!(step.on_event(&fixture.core, session_closed()).end);
    step.on_end(&fixture.core, Some(&Ok(())));
    assert_eq!(fixture.drain(), (vec!["Closed"], true));
}

#[test]
fn a_failed_model_session_then_a_failed_run_yields_one_error() {
    let mut fixture = fixture(true);
    let mut step = Step::new(None);
    let mut closes = 0;
    for event in [model_failed(), run_failed()] {
        closes += usize::from(step.on_event(&fixture.core, event).close.is_some());
    }
    assert!(step.on_event(&fixture.core, session_closed()).end);
    step.on_end(&fixture.core, None);
    assert_eq!(closes, 1);
    assert_eq!(fixture.drain(), (vec!["Error::Layer", "Closed"], true));
}

#[test]
fn a_failed_run_while_closing_spawns_no_close() {
    let mut fixture = fixture(true);
    fixture.core.state.store(CLOSING, Ordering::SeqCst);
    let mut step = Step::new(None);
    assert_eq!(step.on_event(&fixture.core, run_failed()).close, None);
    step.on_event(&fixture.core, session_closed());
    step.on_end(&fixture.core, None);
    let (names, ended) = fixture.drain();
    assert_eq!(names.iter().filter(|name| **name == "Closed").count(), 1);
    assert!(ended);
}

#[test]
fn the_drivers_err_yields_one_layer_error() {
    let mut fixture = fixture(true);
    let mut step = Step::new(None);
    let error = Arc::new(RunError::NodeFailed(FailureReason {
        node_id: None,
        detail: "failed".to_owned(),
    }));
    let act = step.on_driver(&fixture.core, &Err(Arc::clone(&error)));
    assert_eq!(act.close, Some(Close::Cancel));
    step.on_end(&fixture.core, Some(&Err(error)));
    assert_eq!(fixture.drain(), (vec!["Error::Layer", "Closed"], true));
    assert_eq!(fixture.core.state(), CLOSED);
}

#[test]
fn the_first_ready_is_started() {
    let mut fixture = fixture(true);
    let ready = || {
        SessionEvent::SessionReady(SessionReady {
            session_id: SessionId::generate(),
        })
    };
    let mut step = Step::new(None);
    step.on_event(&fixture.core, ready());
    step.on_event(&fixture.core, ready());
    assert_eq!(fixture.drain().0, ["Started"]);
    assert!(!barge_in(&fixture.core));
}
