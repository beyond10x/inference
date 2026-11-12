//! The conversation `talk --provider elevenlabs-cascade` holds, over the testkit's recogniser,
//! generator and synthesiser instead of ElevenLabs and a llama-server: a committed transcript is
//! a turn whose answer is spoken to the playout, and voiced input while the answer plays stops
//! the playout and drops the rest of the answer.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use inference_audio_device::convert::encode;
use inference_duplex::inference_audio::{AudioEncoding, AudioFrame, SampleClockPosition};
use inference_llm::Generator;
use inference_stt::inference_core::model::ModelId;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep,
};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use realtime_voice::compare::{Cascade, Playout};
use realtime_voice::pcm16;
use realtime_voice::talk::{CascadeNote, converse};

/// Samples in one 20 ms frame at 16 kHz.
const FRAME_SAMPLES: u32 = 320;

/// The wall-clock time a case may take before it fails as hanging.
const DEADLINE: Duration = Duration::from_secs(20);

/// The question the recogniser commits after its second frame.
const QUESTION: &str = "what time is it";

/// What the playout was handed and how often it was stopped.
#[derive(Debug, Default)]
struct Played {
    frames: usize,
    stops: usize,
}

struct Recorder(Rc<RefCell<Played>>);

impl Playout for Recorder {
    fn play(&mut self, _frame: &AudioFrame) {
        self.0.borrow_mut().frames += 1;
    }

    fn stop(&mut self) {
        self.0.borrow_mut().stops += 1;
    }
}

fn cascade(answer: &[&str]) -> Cascade<MockRecogniser, MockSynthesiser> {
    let cue = |after_frames, step| Cue { after_frames, step };
    let recogniser = MockRecogniser::new(
        SttConfiguration::Incremental,
        ModelId::generate(),
        vec![
            cue(1, ScriptStep::Hypothesis(QUESTION.to_owned())),
            cue(2, ScriptStep::Commit { span: 0 }),
        ],
    );
    let generator: Generator = MockGenerator::new(
        LlmConfiguration::StreamingOnly,
        ModelId::generate(),
        answer.iter().copied(),
    )
    .into();
    let synthesiser = MockSynthesiser::new(
        TtsConfiguration::IncrementalStreaming,
        ModelId::generate(),
        pcm16(16_000),
    );
    Cascade::new(recogniser, generator, synthesiser, pcm16(16_000))
}

/// The `index`th 20 ms frame of 16-bit PCM at 16 kHz, every sample `level`.
fn frame(index: u64, level: f32) -> AudioFrame {
    let samples = vec![level; FRAME_SAMPLES as usize];
    AudioFrame {
        format: pcm16(16_000),
        position: SampleClockPosition::new(index * u64::from(FRAME_SAMPLES)).expect("a position"),
        sample_count: FRAME_SAMPLES,
        data: encode(&samples, AudioEncoding::PcmS16le),
    }
}

/// Runs a conversation whose input is silence until the answer's audio reaches the playout, then
/// voiced when `barge` is set, until `done` says the notes are enough or the deadline passes.
fn run(
    answer: &[&str],
    barge: bool,
    done: fn(&[CascadeNote]) -> bool,
) -> (Vec<CascadeNote>, Played) {
    let mut cascade = cascade(answer);
    let played = Rc::new(RefCell::new(Played::default()));
    let notes = Rc::new(RefCell::new(Vec::new()));
    let mut playout = Recorder(Rc::clone(&played));
    let mut sent = 0;
    let started = Instant::now();
    let mut input = {
        let played = Rc::clone(&played);
        move || {
            let level = if barge && played.borrow().frames > 0 {
                0.5
            } else {
                0.0
            };
            std::thread::sleep(Duration::from_millis(1));
            sent += 1;
            Some(frame(sent, level))
        }
    };
    let open = {
        let notes = Rc::clone(&notes);
        move || !done(&notes.borrow()) && started.elapsed() < DEADLINE
    };
    let mut note = {
        let notes = Rc::clone(&notes);
        move |note| notes.borrow_mut().push(note)
    };
    converse(
        &mut cascade,
        &mut input,
        &mut playout,
        &open,
        |_| None,
        &mut note,
    );
    assert!(started.elapsed() < DEADLINE, "the conversation hangs");
    let notes = notes.borrow().clone();
    let played = std::mem::take(&mut *played.borrow_mut());
    (notes, played)
}

#[test]
fn a_committed_transcript_is_answered_and_the_answer_played() {
    let (notes, played) = run(&["It is noon. "], false, |notes| {
        notes.contains(&CascadeNote::Answered)
    });
    assert_eq!(
        notes.first(),
        Some(&CascadeNote::Heard(QUESTION.to_owned()))
    );
    assert!(
        notes
            .iter()
            .any(|note| matches!(note, CascadeNote::Said(text) if text.contains("noon"))),
        "{notes:?}"
    );
    assert!(notes.contains(&CascadeNote::Answered), "{notes:?}");
    assert!(!notes.contains(&CascadeNote::BargeIn), "{notes:?}");
    assert!(played.frames > 0, "{played:?}");
    assert_eq!(played.stops, 0, "{played:?}");
}

#[test]
fn voiced_input_while_the_answer_plays_stops_the_playout() {
    let long = "one two three four five six seven eight nine ten eleven twelve thirteen \
                fourteen fifteen sixteen seventeen eighteen nineteen twenty. ";
    let answer = [long; 10];
    let (notes, played) = run(&answer, true, |notes| notes.contains(&CascadeNote::BargeIn));
    assert!(notes.contains(&CascadeNote::BargeIn), "{notes:?}");
    assert_eq!(played.stops, 1, "{played:?}");
}
