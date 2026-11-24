//! The synthesis over a scripted engine: what the handle does with the parts an engine hands
//! over, held without the native library's text-to-speech or a checkpoint.

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use inference_testkit::tts::conformance::{self, Utterance};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_core::model::Capability;
use inference_tts::inference_text::{OutputSegment, SegmentId, SegmentState};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

use super::engine::{Engine, Parts};
use super::*;

/// An engine that speaks every text as `parts` parts of `part_len` samples of 0.25, keeps the
/// texts it was given and counts the parts it handed over. With `permits`, it waits for one
/// before each part, from the first when `first_waits` and otherwise from the second; a closed
/// channel of permits lets it run on.
struct Scripted {
    parts: usize,
    part_len: usize,
    refuses: bool,
    first_waits: bool,
    permits: Option<Mutex<Receiver<()>>>,
    texts: Mutex<Vec<String>>,
    handed: AtomicUsize,
}

impl Scripted {
    fn speaking(parts: usize, part_len: usize) -> Self {
        Self {
            parts,
            part_len,
            refuses: false,
            first_waits: false,
            permits: None,
            texts: Mutex::new(Vec::new()),
            handed: AtomicUsize::new(0),
        }
    }

    /// The engine waiting for a permit before each part, and the sender of its permits.
    fn permitted(mut self, first_waits: bool) -> (Self, Sender<()>) {
        let (permit, permits) = mpsc::channel();
        self.permits = Some(Mutex::new(permits));
        self.first_waits = first_waits;
        (self, permit)
    }

    fn texts(&self) -> Vec<String> {
        self.texts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn handed(&self) -> usize {
        self.handed.load(Ordering::Acquire)
    }
}

impl Engine for Scripted {
    fn generate(&self, text: &str, _speaker: i32, mut parts: Parts) -> bool {
        self.texts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(text.to_owned());
        if self.refuses {
            return false;
        }
        let samples = vec![0.25; self.part_len];
        for part in 0..self.parts {
            if let Some(permits) = &self.permits
                && (part > 0 || self.first_waits)
            {
                let _ = permits
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .recv();
            }
            self.handed.fetch_add(1, Ordering::AcqRel);
            if !parts(&samples) {
                break;
            }
        }
        true
    }
}

/// How long a case waits for a thread of the scripted engine to end before it fails.
const PATIENCE: Duration = Duration::from_secs(10);

fn rate(sample_rate: u32) -> NonZeroU32 {
    NonZeroU32::new(sample_rate).expect("the rate is not zero")
}

/// The synthesiser of the pinned checkpoint's records over `engine`.
fn synthesiser(engine: &Arc<Scripted>, max_concurrency: i64) -> SherpaOnnxSynthesiser {
    let (model, _) = declare_model();
    let (adapter, _) =
        declare_adapter(model.model_id(), max_concurrency).expect("a positive concurrency");
    let engine: Arc<dyn Engine> = engine.clone();
    SherpaOnnxSynthesiser::over(
        model,
        adapter,
        Created {
            engine,
            sample_rate: rate(SAMPLE_RATE),
            speakers: 11,
        },
    )
}

fn generation() -> Generation {
    Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0
}

/// A Committed segment of `text`.
fn committed(generation: &Generation, sequence: i64, text: &str) -> OutputSegment {
    let (tentative, _) = OutputSegment::emit(generation, sequence, text.to_owned())
        .expect("a fresh generation is Active");
    tentative.commit().expect("a Tentative segment commits").0
}

/// Offers `texts` to `synthesis`, one Committed segment each, and returns their ids.
fn offer(synthesis: &mut SherpaOnnxSynthesis, texts: &[&str]) -> Vec<SegmentId> {
    let generation = generation();
    texts
        .iter()
        .zip(1..)
        .map(|(text, sequence)| {
            let segment = committed(&generation, sequence, text);
            let segment_id = segment.segment_id();
            synthesis
                .offer_segment(segment)
                .expect("a Committed segment is queued");
            segment_id
        })
        .collect()
}

fn state(synthesis: &SherpaOnnxSynthesis, segment_id: SegmentId) -> Option<SegmentState> {
    synthesis
        .output()
        .segment(segment_id)
        .map(OutputSegment::state)
}

#[test]
fn a_text_without_a_nul_byte_is_speakable_and_one_with_it_is_refused_at_its_byte() {
    assert_eq!(speakable("plain text, and an é"), Ok(()));
    assert_eq!(speakable(""), Ok(()));
    assert_eq!(speakable("a\0b"), Err(NulInText { at: 1 }));
    assert_eq!(speakable("é\0"), Err(NulInText { at: 2 }));
    assert!(
        NulInText { at: 1 }
            .to_string()
            .contains("NUL byte at byte 1"),
        "{}",
        NulInText { at: 1 }
    );
}

#[test]
fn a_text_with_a_nul_byte_fails_the_synthesis_before_the_engine_is_given_it() {
    let engine = Arc::new(Scripted::speaking(1, 480));
    let mut synthesiser = synthesiser(&engine, 1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    let ids = offer(&mut synthesis, &["Hello.", "be\0fore", "After."]);
    synthesis.end_utterance().expect("the utterance ends once");

    let events: Vec<SynthesisEvent> = synthesis.by_ref().collect();

    assert_eq!(
        synthesis.failure(),
        Some(&SynthesisFailed::NulInText {
            segment_id: ids[1],
            at: 2
        })
    );
    assert_eq!(
        engine.texts(),
        ["Hello."],
        "the engine is given neither the text with the NUL byte nor the one after it"
    );
    assert_eq!(state(&synthesis, ids[0]), Some(SegmentState::Synthesized));
    assert_eq!(state(&synthesis, ids[1]), Some(SegmentState::Queued));
    assert_eq!(state(&synthesis, ids[2]), Some(SegmentState::Queued));
    let Some(SynthesisEvent::Usage(usage)) = events.last() else {
        panic!("the usage record is the last event: {events:?}");
    };
    assert!(!usage.r#final, "a failed synthesis's usage is not final");
    assert!(
        synthesis
            .failure()
            .is_some_and(|failure| failure.to_string().contains("NUL byte at byte 2")),
        "{:?}",
        synthesis.failure()
    );
}

#[test]
fn a_segment_is_framed_in_20_ms_frames_at_consecutive_positions_while_it_is_queued() {
    let engine = Arc::new(Scripted::speaking(2, 1_000));
    let mut synthesiser = synthesiser(&engine, 1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    let ids = offer(&mut synthesis, &["Two parts."]);

    let mut frames = Vec::new();
    let mut completions = 0;
    while let Some(event) = synthesis.next() {
        match event {
            SynthesisEvent::Audio(audio) => {
                let state = state(&synthesis, audio.segment_id());
                frames.push((audio, state));
            }
            SynthesisEvent::Completed(completed) => {
                assert_eq!(completed.segment_id, ids[0]);
                completions += 1;
            }
            SynthesisEvent::Usage(_) => panic!("the utterance has not ended"),
        }
    }

    assert_eq!(completions, 1);
    assert_eq!(
        frames
            .iter()
            .map(|(audio, _)| audio.frame().sample_count)
            .collect::<Vec<_>>(),
        [480, 480, 480, 480, 80],
        "2000 samples at 24 kHz are four frames of 20 ms and what is left"
    );
    let mut position = 0;
    for (audio, _) in &frames {
        let frame = audio.frame();
        assert_eq!(frame.format, synthesiser.format());
        assert_eq!(frame.position.get(), position);
        assert_eq!(frame.check_length(), Ok(()));
        assert!(
            frame
                .data
                .as_chunks::<2>()
                .0
                .iter()
                .all(|sample| i16::from_le_bytes(*sample) == 8_192),
            "0.25 is 8192 of 32767"
        );
        position += u64::from(frame.sample_count);
    }
    assert!(
        frames[..4]
            .iter()
            .all(|(_, state)| *state == Some(SegmentState::Queued)),
        "the frames of the first part are delivered while the segment is Queued"
    );
    assert_eq!(state(&synthesis, ids[0]), Some(SegmentState::Synthesized));
    assert_eq!(synthesis.failure(), None);
}

#[test]
fn a_sample_outside_the_range_is_clipped() {
    assert_eq!(synthesis::pcm16(0.0), 0_i16.to_le_bytes());
    assert_eq!(synthesis::pcm16(1.0), 32_767_i16.to_le_bytes());
    assert_eq!(synthesis::pcm16(1.5), i16::MAX.to_le_bytes());
    assert_eq!(synthesis::pcm16(-1.5), i16::MIN.to_le_bytes());
    assert_eq!(synthesis::pcm16(f32::NAN), 0_i16.to_le_bytes());
}

#[test]
fn a_voice_at_another_rate_is_reported_and_framed_at_its_own_rate() {
    let engine = Arc::new(Scripted::speaking(1, 500));
    let format = format_at(rate(22_050));
    let (model, _) = declare_model();
    let (adapter, _) = declare(
        model.model_id(),
        "a voice at 22.05 kHz".to_owned(),
        1,
        vec!["0".to_owned()],
        format,
    )
    .expect("a positive concurrency");
    let scripted: Arc<dyn Engine> = engine.clone();
    let mut synthesiser = SherpaOnnxSynthesiser::over(
        model,
        adapter,
        Created {
            engine: scripted,
            sample_rate: rate(22_050),
            speakers: 1,
        },
    );

    assert_eq!(synthesiser.format(), format);
    assert_eq!(
        synthesiser.adapter().descriptor().codecs,
        ["pcm_s16le/22050/1"]
    );
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    offer(&mut synthesis, &["One part."]);
    let frames: Vec<_> = synthesis
        .by_ref()
        .filter_map(|event| match event {
            SynthesisEvent::Audio(audio) => Some(audio),
            _ => None,
        })
        .collect();
    assert!(frames.iter().all(|audio| audio.frame().format == format));
    assert_eq!(
        frames
            .iter()
            .map(|audio| audio.frame().sample_count)
            .collect::<Vec<_>>(),
        [441, 59],
        "20 ms at 22.05 kHz are 441 samples"
    );
}

#[test]
fn a_segment_with_nothing_to_pronounce_completes_without_audio_and_without_the_engine() {
    let engine = Arc::new(Scripted::speaking(1, 480));
    let mut synthesiser = synthesiser(&engine, 1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    let ids = offer(&mut synthesis, &[" \n", "...", " , "]);
    synthesis.end_utterance().expect("the utterance ends once");

    let events: Vec<SynthesisEvent> = synthesis.by_ref().collect();

    assert_eq!(
        events.iter().map(SynthesisEvent::name).collect::<Vec<_>>(),
        [
            "inference.stream.SynthesisCompleted",
            "inference.stream.SynthesisCompleted",
            "inference.stream.SynthesisCompleted",
            "inference.usage.UsageRecorded"
        ],
        "{events:?}"
    );
    for id in ids {
        assert_eq!(state(&synthesis, id), Some(SegmentState::Synthesized));
    }
    assert!(engine.texts().is_empty(), "{:?}", engine.texts());
}

#[test]
fn an_engine_that_refuses_a_text_fails_the_synthesis_naming_the_segment() {
    let mut engine = Scripted::speaking(1, 480);
    engine.refuses = true;
    let engine = Arc::new(engine);
    let mut synthesiser = synthesiser(&engine, 1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    let ids = offer(&mut synthesis, &["Refused."]);
    synthesis.end_utterance().expect("the utterance ends once");

    let events: Vec<SynthesisEvent> = synthesis.by_ref().collect();

    assert_eq!(
        synthesis.failure(),
        Some(&SynthesisFailed::BackendRefused { segment_id: ids[0] })
    );
    let [SynthesisEvent::Usage(usage)] = events.as_slice() else {
        panic!("only the usage record is delivered: {events:?}");
    };
    assert!(!usage.r#final);
}

#[test]
fn discarding_the_segment_being_generated_stops_the_engine_at_its_next_part() {
    let (engine, permit) = Scripted::speaking(1_000, 480).permitted(false);
    let engine = Arc::new(engine);
    let mut synthesiser = synthesiser(&engine, 1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    let ids = offer(&mut synthesis, &["A long text.", "The next."]);
    synthesis.end_utterance().expect("the utterance ends once");

    let first = synthesis.next().expect("the first part is delivered");
    assert_eq!(first.segment_id(), Some(ids[0]));
    synthesis
        .discard(ids[0])
        .expect("a Queued segment is discarded");
    drop(permit);
    let rest: Vec<SynthesisEvent> = synthesis.by_ref().collect();
    let workers = Arc::clone(&synthesiser.voice.workers);
    drop(synthesis);
    drop(synthesiser);
    assert!(workers.wait_idle(PATIENCE), "every generation ends");

    assert!(
        rest.iter().all(|event| event.segment_id() != Some(ids[0])),
        "no event of the discarded segment follows its discard: {rest:?}"
    );
    assert!(
        rest.iter()
            .any(|event| event.segment_id() == Some(ids[1])
                && matches!(event, SynthesisEvent::Audio(_))),
        "the segment after it is spoken: {rest:?}"
    );
    assert_eq!(
        engine.handed(),
        2 + 1_000,
        "the engine handed over the part delivered and the part it was stopped at of the \
         discarded text, then every part of the next"
    );
}

#[test]
fn dropping_the_synthesis_stops_the_engine_at_its_next_part() {
    let (engine, permit) = Scripted::speaking(1_000, 480).permitted(false);
    let engine = Arc::new(engine);
    let mut synthesiser = synthesiser(&engine, 1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    offer(&mut synthesis, &["A long text."]);

    assert!(synthesis.next().is_some(), "the first part is delivered");
    drop(synthesis);
    assert!(
        synthesiser.start().is_ok(),
        "the dropped synthesis gave its slot back at once"
    );
    drop(permit);
    let workers = Arc::clone(&synthesiser.voice.workers);
    drop(synthesiser);
    assert!(workers.wait_idle(PATIENCE), "the generation ends");

    assert_eq!(
        engine.handed(),
        2,
        "the part delivered and the part the engine was stopped at"
    );
}

#[test]
fn dropping_the_last_handle_does_not_wait_for_the_part_being_generated() {
    let (engine, permit) = Scripted::speaking(1_000, 480).permitted(false);
    let engine = Arc::new(engine);
    let mut synthesiser = synthesiser(&engine, 1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    offer(&mut synthesis, &["A long text."]);
    assert!(synthesis.next().is_some(), "the first part is delivered");
    // The engine now waits for its permit inside the second part. From here on the handles and
    // the thread generating are all that hold it.
    let workers = Arc::clone(&synthesiser.voice.workers);
    let held = Arc::downgrade(&engine);
    drop(engine);

    let (dropped, done) = mpsc::channel();
    let dropping = std::thread::spawn(move || {
        let before = Instant::now();
        drop(synthesis);
        drop(synthesiser);
        let _ = dropped.send(before.elapsed());
    });
    let within = done.recv_timeout(Duration::from_millis(50));
    let (running, handed) = (
        workers.running(),
        held.upgrade().map(|engine| engine.handed()),
    );
    // The permit lets the engine end its part and ask whether to go on.
    drop(permit);
    dropping.join().expect("the dropping thread ends");

    assert!(
        within.is_ok(),
        "dropping the synthesis and the synthesiser while the engine generates a part returns \
         within 50 ms, without waiting for the part"
    );
    assert_eq!(
        (running, handed),
        (1, Some(1)),
        "after the drop the thread still generates the second part and keeps the engine alive"
    );
    assert!(
        workers.wait_idle(PATIENCE),
        "the thread ends once the part in flight has ended"
    );
    assert!(
        held.upgrade().is_none(),
        "the thread released the engine before it ended, and no handle holds it"
    );
}

/// Counts how often the task was woken.
#[derive(Default)]
struct Woken(AtomicUsize);

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

#[test]
fn the_async_form_is_pending_while_the_engine_generates_and_is_woken_by_its_part() {
    let (engine, permit) = Scripted::speaking(1, 480).permitted(true);
    let engine = Arc::new(engine);
    let mut synthesiser = synthesiser(&engine, 1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    offer(&mut synthesis, &["One part."]);
    let woken = Arc::new(Woken::default());
    let waker = Waker::from(Arc::clone(&woken));
    let mut cx = Context::from_waker(&waker);

    assert!(
        synthesis.poll_event(&mut cx).is_pending(),
        "the engine has handed over nothing yet"
    );
    assert_eq!(woken.0.load(Ordering::Acquire), 0);
    permit.send(()).expect("the engine waits for its permit");
    let deadline = Instant::now() + Duration::from_secs(10);
    while woken.0.load(Ordering::Acquire) == 0 {
        assert!(Instant::now() < deadline, "the part wakes the task");
        std::thread::sleep(Duration::from_millis(1));
    }
    let Poll::Ready(Some(SynthesisEvent::Audio(_))) = synthesis.poll_event(&mut cx) else {
        panic!("the part's first frame is ready once the task is woken");
    };
}

fn utterance() -> Utterance {
    Utterance::default()
}

fn conforming() -> SherpaOnnxSynthesiser {
    synthesiser(&Arc::new(Scripted::speaking(2, 700)), 2)
}

#[test]
fn the_handle_passes_tentative_refused_over_a_scripted_engine() {
    let observed = conformance::tentative_refused(&mut conforming(), &utterance());
    assert!(observed.is_ok(), "{observed:?}");
}

#[test]
fn the_handle_passes_played_only_by_sink_over_a_scripted_engine() {
    let observed = conformance::played_only_by_sink(&mut conforming(), &utterance());
    assert!(observed.is_ok(), "{observed:?}");
}

#[test]
fn the_handle_passes_claims_observed_over_a_scripted_engine() {
    let mut synthesiser = conforming();
    assert_eq!(
        synthesiser.adapter().capabilities(),
        [Capability::IncrementalInput, Capability::OutputStreaming]
    );
    let observed = conformance::claims_observed(&mut synthesiser, &utterance());
    assert_eq!(
        observed,
        Ok(conformance::ObservedClaims {
            incremental_input: true,
            output_streaming: true
        })
    );
}

#[test]
fn the_handle_passes_audio_valid_over_a_scripted_engine() {
    let mut synthesiser = conforming();
    let format = synthesiser.format();
    let observed = conformance::audio_valid(&mut synthesiser, &utterance(), Some(format));
    assert_eq!(
        observed.map(|observed| observed.samples),
        Ok(2 * 2 * 700),
        "two sentences of two parts of 700 samples"
    );
}

#[test]
fn the_handle_passes_cancellation_over_a_scripted_engine() {
    let observed = conformance::cancellation(&mut conforming(), &utterance());
    assert_eq!(
        observed.map(|observed| (observed.discarded.len(), observed.max_concurrency)),
        Ok((2, 2))
    );
}

#[test]
fn the_handle_passes_usage_recorded_over_a_scripted_engine() {
    let observed = conformance::usage_recorded(&mut conforming(), &utterance());
    assert!(
        observed
            .as_ref()
            .is_ok_and(|observed| observed.local_compute.value().is_some()),
        "the time in the engine is measured: {observed:?}"
    );
}
