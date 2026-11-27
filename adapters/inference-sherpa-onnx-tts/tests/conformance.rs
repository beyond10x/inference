//! Adapter conformance against the pinned checkpoint: the six checks of
//! `inference_testkit::tts::conformance`, and what the claim of output streaming rests on,
//! that sherpa-onnx hands over new samples at each call of its callback and stops when the
//! callback says so. Every test speaks, so every test is ignored: it needs the checkpoint
//! directory `INFERENCE_SHERPA_ONNX_TTS_DIR` and a process started on the sherpa-onnx library
//! built with text-to-speech.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use inference_sherpa_onnx_tts::inference_tts::Synthesiser;
use inference_sherpa_onnx_tts::inference_tts::inference_core::model::Capability;
use inference_sherpa_onnx_tts::{ESPEAK_NG_DATA, FILES, SAMPLE_RATE, THREADS};
use inference_testkit::tts::conformance::{self, ObservedClaims, Utterance};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig,
    OfflineTtsModelConfig,
};

use crate::support::{checkpoint_dir, open};

fn utterance() -> Utterance {
    Utterance::default()
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn a_tentative_segment_is_refused_and_only_the_committed_one_is_spoken() {
    let observed = conformance::tentative_refused(&mut open(1), &utterance());
    eprintln!("tentative_refused: {observed:?}");
    assert!(observed.is_ok(), "{observed:?}");
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn a_segment_is_played_only_on_the_sinks_acknowledgement() {
    let observed = conformance::played_only_by_sink(&mut open(1), &utterance());
    eprintln!("played_only_by_sink: {observed:?}");
    assert!(observed.is_ok(), "{observed:?}");
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn incremental_input_and_output_streaming_are_observed_and_are_exactly_the_claims() {
    let mut synthesiser = open(1);
    assert_eq!(
        synthesiser.adapter().capabilities(),
        [Capability::IncrementalInput, Capability::OutputStreaming]
    );
    let observed = conformance::claims_observed(&mut synthesiser, &utterance());
    eprintln!("claims_observed: {observed:?}");
    assert_eq!(
        observed,
        Ok(ObservedClaims {
            incremental_input: true,
            output_streaming: true
        })
    );
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn every_frame_is_mono_pcm_at_the_checkpoints_rate_at_consecutive_positions() {
    let mut synthesiser = open(1);
    let format = synthesiser.format();
    assert_eq!(format.sample_rate.get(), SAMPLE_RATE);
    let observed = conformance::audio_valid(&mut synthesiser, &utterance(), Some(format));
    eprintln!("audio_valid: {observed:?}");
    assert!(
        observed.is_ok_and(|observed| observed.samples > u64::from(SAMPLE_RATE)),
        "two sentences are more than a second of audio"
    );
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn a_discarded_segment_and_a_dropped_synthesis_yield_nothing_more() {
    let observed = conformance::cancellation(&mut open(2), &utterance());
    eprintln!("cancellation: {observed:?}");
    assert_eq!(
        observed.map(|observed| (observed.discarded.len(), observed.max_concurrency)),
        Ok((2, 2))
    );
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn a_completed_utterance_yields_one_final_usage_record_with_its_compute_measured() {
    let observed = conformance::usage_recorded(&mut open(1), &utterance());
    eprintln!("usage_recorded: {observed:?}");
    assert!(
        observed.is_ok_and(|observed| observed.local_compute.value().is_some_and(|ms| ms > 0)),
        "the time sherpa-onnx spent is measured"
    );
}

/// Each call of the callback: the samples it was handed, the progress, and the milliseconds since
/// the generation started.
type Calls = Rc<RefCell<Vec<(Vec<f32>, f32, u128)>>>;

/// sherpa-onnx's offline text-to-speech on the pinned checkpoint, as the adapter configures it,
/// reached without the adapter.
fn native() -> OfflineTts {
    let dir = checkpoint_dir();
    let path = |name: &str| Some(dir.join(name).to_string_lossy().into_owned());
    OfflineTts::create(&OfflineTtsConfig {
        model: OfflineTtsModelConfig {
            kokoro: OfflineTtsKokoroModelConfig {
                model: path(FILES[0].name),
                voices: path(FILES[1].name),
                tokens: path(FILES[2].name),
                data_dir: path(ESPEAK_NG_DATA.name),
                ..OfflineTtsKokoroModelConfig::default()
            },
            num_threads: THREADS,
            ..OfflineTtsModelConfig::default()
        },
        max_num_sentences: 1,
        silence_scale: 0.2,
        ..OfflineTtsConfig::default()
    })
    .expect("the library built with text-to-speech creates the synthesiser")
}

const THREE_SENTENCES: &str =
    "The first sentence is short. The second one follows it at once. And the third ends it.";

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn sherpa_onnx_hands_over_new_samples_at_each_call_of_its_callback() {
    let tts = native();
    let started = Instant::now();
    let calls: Calls = Rc::default();
    let seen = Rc::clone(&calls);
    let audio = tts
        .generate_with_config(
            THREE_SENTENCES,
            &GenerationConfig::default(),
            Some(move |samples: &[f32], progress: f32| {
                seen.borrow_mut()
                    .push((samples.to_vec(), progress, started.elapsed().as_millis()));
                true
            }),
        )
        .expect("the text is spoken");
    let ended = started.elapsed().as_millis();
    let calls = calls.borrow();
    for (index, (samples, progress, at)) in calls.iter().enumerate() {
        eprintln!(
            "call {index}: {} samples, progress {progress}, {at} ms after the start",
            samples.len()
        );
    }
    eprintln!(
        "returned: {} samples at {} Hz, {ended} ms after the start",
        audio.samples().len(),
        audio.sample_rate()
    );

    assert_eq!(calls.len(), 3, "one call per sentence");
    assert!(calls.iter().all(|(samples, _, _)| !samples.is_empty()));
    let handed: Vec<f32> = calls
        .iter()
        .flat_map(|(samples, _, _)| samples.iter().copied())
        .collect();
    assert_eq!(
        handed.len(),
        audio.samples().len(),
        "the calls together hand over the samples returned, each sample once"
    );
    assert!(
        handed == audio.samples(),
        "each call hands over the samples that follow those of the call before"
    );
    assert!(
        calls[0].2 < ended,
        "the first sentence is handed over before the generation ends"
    );
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn sherpa_onnx_stops_generating_when_its_callback_answers_false() {
    let tts = native();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&calls);
    let audio = tts.generate_with_config(
        THREE_SENTENCES,
        &GenerationConfig::default(),
        Some(move |samples: &[f32], _progress: f32| {
            seen.borrow_mut().push(samples.len());
            false
        }),
    );
    eprintln!(
        "stopped after {:?}; returned {:?} samples",
        calls.borrow(),
        audio.as_ref().map(|audio| audio.samples().len())
    );
    assert_eq!(
        calls.borrow().len(),
        1,
        "no sentence is generated after the callback answered false"
    );
}
