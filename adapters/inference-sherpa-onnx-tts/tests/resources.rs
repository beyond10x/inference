//! The measurement behind the `resource_profile` of `records/tts-xeon-8375c-cpu.json`: one short
//! utterance, spoken once, on the pinned checkpoint. It prints the load time, the CPU real-time
//! factor and the peak resident memory of its process; the test runner gives each test a process
//! of its own. Ignored: it needs the checkpoint directory `INFERENCE_SHERPA_ONNX_TTS_DIR` and a
//! process started on the sherpa-onnx library built with text-to-speech.

use std::fs;
use std::time::Instant;

use inference_sherpa_onnx_tts::SherpaOnnxSynthesiser;
use inference_sherpa_onnx_tts::inference_tts::inference_core::generation::{
    Generation, StartGeneration,
};
use inference_sherpa_onnx_tts::inference_tts::inference_core::id::{NodeId, RunId};
use inference_sherpa_onnx_tts::inference_tts::inference_text::OutputSegment;
use inference_sherpa_onnx_tts::inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use inference_testkit::tts::conformance::DEFAULT_SENTENCES;

use crate::support::checkpoint_dir;

/// The peak resident memory of this process in MiB, `VmHWM` of `/proc/self/status`.
fn peak_resident_mib() -> u64 {
    let status = fs::read_to_string("/proc/self/status").expect("the process status is read");
    let kib: u64 = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("the status names VmHWM in kB");
    kib / 1_024
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn one_short_utterance_measures_load_time_real_time_factor_and_peak_memory() {
    let started = Instant::now();
    let mut synthesiser =
        SherpaOnnxSynthesiser::open(checkpoint_dir()).expect("the pinned checkpoint opens");
    let load_ms = started.elapsed().as_millis();

    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    for (sentence, sequence) in DEFAULT_SENTENCES.iter().zip(1..) {
        let (tentative, _) = OutputSegment::emit(&generation, sequence, (*sentence).to_owned())
            .expect("a fresh generation is Active");
        let (segment, _) = tentative.commit().expect("a Tentative segment commits");
        synthesis
            .offer_segment(segment)
            .expect("a Committed segment is queued");
    }
    synthesis.end_utterance().expect("the utterance ends once");
    let started = Instant::now();
    let samples: u64 = synthesis
        .by_ref()
        .filter_map(|event| match event {
            SynthesisEvent::Audio(audio) => Some(u64::from(audio.frame().sample_count)),
            _ => None,
        })
        .sum();
    let spent_ms = started.elapsed().as_millis();
    assert_eq!(synthesis.failure(), None);
    assert!(samples > 0, "the utterance is spoken");

    let rate = u128::from(synthesiser.format().sample_rate.get());
    let audio_ms = u128::from(samples) * 1_000 / rate;
    eprintln!(
        "load_time_ms {load_ms}, {samples} samples at {rate} Hz = {audio_ms} ms of audio in \
         {spent_ms} ms, cpu_real_time_factor_milli {}, peak_resident_memory_mib {}",
        spent_ms * 1_000 / audio_ms.max(1),
        peak_resident_mib()
    );
}
