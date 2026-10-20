//! Measures the resource profile of the recogniser for its capability record: the load time of
//! [`SherpaOnnxRecogniser::open`] (checking the four files against their pins and creating the
//! online recogniser), the real-time factor of streaming the reference clip in 20 ms frames and
//! decoding it on CPU, and the peak resident memory of the process (`VmHWM`). Each figure is the
//! median of three runs and is printed as the `resource_profile` of the record; the test asserts
//! only that recognition keeps up with real time. It is alone in its test binary, so the peak
//! memory is the recogniser's and the harness's alone.
//!
//! Needs the unpacked pinned checkpoint in the directory `INFERENCE_SHERPA_ONNX_STT_DIR`; ignored
//! by default. The record's figures come from the release profile, run on the machine the record
//! names with
//! `INFERENCE_SHERPA_ONNX_STT_DIR=<dir> cargo test --release -p inference-sherpa-onnx --all-features --test stt_resources -- --ignored --nocapture`;
//! in the unoptimised test profile the sha256 check of the 250 MB encoder makes loading several
//! times slower.
#![cfg(all(feature = "stt", target_os = "linux"))]

mod support;

use std::time::{Duration, Instant};

use inference_sherpa_onnx::stt::SherpaOnnxRecogniser;
use inference_stt::Recogniser;

use support::{CLIP_RATE, PCM16_16K_MONO, checkpoint_dir, frames, reference_clip, request, stream};

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
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn measure_the_resource_profile_on_cpu() {
    let dir = checkpoint_dir();
    let clip = reference_clip();
    let audio = Duration::from_secs_f64(clip.len() as f64 / f64::from(CLIP_RATE));

    let mut loads = Vec::new();
    let mut recogniser = None;
    for _ in 0..RUNS {
        drop(recogniser.take());
        let started = Instant::now();
        let opened = SherpaOnnxRecogniser::open(&dir).expect("the pinned checkpoint opens");
        loads.push(started.elapsed());
        recogniser = Some(opened);
    }
    let mut recogniser = recogniser.expect("a recogniser was opened");

    let mut recognitions = Vec::new();
    for _ in 0..RUNS {
        let frames = frames(&clip, PCM16_16K_MONO);
        let mut handle = recogniser.start(request()).expect("nothing else runs");
        let started = Instant::now();
        let feed = stream(&mut handle, frames);
        recognitions.push(started.elapsed());
        assert!(
            feed.committed().contains("nightfall"),
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
