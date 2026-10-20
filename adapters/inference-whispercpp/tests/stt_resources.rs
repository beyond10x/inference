//! Measures the resource profile of the recogniser for its capability record: the load time of
//! [`WhisperCppRecogniser::open`] (checking the file against its pin and loading it into a
//! whisper.cpp context), the real-time factor of offering the recorded speech clip in 20 ms
//! frames and transcribing it on CPU once input has ended, and the peak resident memory of the
//! process (`VmHWM`). Each figure is the median of three runs and is printed as the
//! `resource_profile` of the record; the test asserts only that recognition keeps up with real
//! time. It is alone in its test binary, so the peak memory is the recogniser's and the
//! harness's alone.
//!
//! Needs the pinned checkpoint file at the path `INFERENCE_WHISPERCPP_STT_MODEL` and the speech
//! clip at the path `INFERENCE_WHISPERCPP_STT_CLIP`; ignored by default. The record's figures come
//! from the release profile, run on the machine the record names with
//! `INFERENCE_WHISPERCPP_STT_MODEL=<ggml-base.en.bin> INFERENCE_WHISPERCPP_STT_CLIP=<0.wav> cargo test --release -p inference-whispercpp --test stt_resources -- --ignored --nocapture`.
#![cfg(target_os = "linux")]

mod support;

use std::time::{Duration, Instant};

use inference_stt::Recogniser;
use inference_whispercpp::stt::WhisperCppRecogniser;

use support::{CLIP_RATE, PCM16_16K_MONO, frames, model_path, request, speech_clip, stream};

const RUNS: usize = 3;

fn median(mut values: Vec<Duration>) -> Duration {
    values.sort();
    values[values.len() / 2]
}

/// The peak resident memory of this process in MiB, `VmHWM` of `/proc/self/status`.
fn peak_resident_mib() -> i64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("Linux reports the status");
    let kib: u64 = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|kib| kib.parse().ok())
        .expect("the status carries VmHWM in kB");
    i64::try_from(kib.div_ceil(1024)).expect("the peak fits i64")
}

#[test]
#[ignore = "needs ggml-base.en.bin at INFERENCE_WHISPERCPP_STT_MODEL and the speech clip at INFERENCE_WHISPERCPP_STT_CLIP"]
fn measure_the_resource_profile_on_cpu() {
    let path = model_path();
    let clip = speech_clip();
    let audio = Duration::from_secs_f64(clip.samples.len() as f64 / f64::from(CLIP_RATE));

    let mut loads = Vec::new();
    let mut recogniser = None;
    for _ in 0..RUNS {
        drop(recogniser.take());
        let started = Instant::now();
        let opened = WhisperCppRecogniser::open(&path).expect("the pinned checkpoint opens");
        loads.push(started.elapsed());
        recogniser = Some(opened);
    }
    let mut recogniser = recogniser.expect("a recogniser was opened");

    let mut recognitions = Vec::new();
    for _ in 0..RUNS {
        let frames = frames(&clip.samples, PCM16_16K_MONO);
        let mut handle = recogniser.start(request()).expect("nothing else runs");
        let started = Instant::now();
        let feed = stream(&mut handle, frames);
        recognitions.push(started.elapsed());
        assert!(
            feed.committed().contains(clip.words[0]),
            "the clip is recognised: {:?}",
            feed.committed()
        );
    }

    let load = median(loads.clone());
    let recognition = median(recognitions.clone());
    let real_time_factor_milli =
        (recognition.as_secs_f64() / audio.as_secs_f64() * 1_000.0).ceil() as i64;
    let peak = peak_resident_mib();
    eprintln!("audio {audio:?}; loads {loads:?}; recognitions {recognitions:?}");
    eprintln!(
        "\"resource_profile\": {{\"peak_resident_memory_mib\": {peak}, \"load_time_ms\": {}, \
         \"cpu_real_time_factor_milli\": {real_time_factor_milli}, \"gpu_memory_mib\": null}}",
        load.as_millis()
    );
    assert!(
        real_time_factor_milli < 1_000,
        "recognition on CPU keeps up with real time: {real_time_factor_milli}"
    );
}
