//! Adversary pass 1: one recognition whose frames change sample rate. The STT contract takes any
//! `AudioFrame` and leaves its format to the frame, and the whisper.cpp adapter resamples each
//! run of one rate (`samples_16k`); the sherpa-onnx recogniser hands every frame's rate to one
//! online stream. The recognition runs in a child process of this test binary so that a
//! process exit inside the native library is observed as a status, not as a lost test run.
#![cfg(feature = "stt")]

mod support;

use std::process::Command;

use inference_stt::inference_audio::{AudioFrame, SampleClockPosition};
use inference_stt::{Recogniser, RecognitionHandle};

use support::{
    Feed, PCM16_8K_MONO, PCM16_16K_MONO, SECOND_PART_WORDS, SPLIT, frames, open, reference_clip,
    request,
};

const CHILD: &str = "ADVERSARY1_SAMPLE_RATE_CHILD";

/// `frames` with positions moved on by `offset`.
fn shifted(frames: Vec<AudioFrame>, offset: u64) -> Vec<AudioFrame> {
    frames
        .into_iter()
        .map(|mut frame| {
            frame.position =
                SampleClockPosition::new(frame.position.get() + offset).expect("the clip is short");
            frame
        })
        .collect()
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn child_recognises_frames_at_8_khz_then_16_khz() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let mut recogniser = open(1);
    let mut handle = recogniser.start(request()).expect("nothing else runs");
    let clip = reference_clip();
    let (first_part, second_part) = clip.split_at(SPLIT);
    let first = frames(first_part, PCM16_8K_MONO);
    let offset: u64 = first
        .iter()
        .map(|frame| u64::from(frame.sample_count))
        .sum();
    let second = shifted(frames(second_part, PCM16_16K_MONO), offset);
    let mut feed = Feed::default();
    for frame in first.into_iter().chain(second) {
        feed.offer(&mut handle, frame);
    }
    feed.end(&mut handle);
    feed.print("mixed");
    let committed = feed.committed();
    for word in SECOND_PART_WORDS {
        assert!(
            committed.contains(word),
            "the 16 kHz part is recognised after the 8 kHz part: {committed:?}"
        );
    }
    assert!(handle.output().input_ended());
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn a_recognition_whose_frames_change_sample_rate_does_not_end_the_process() {
    let exe = std::env::current_exe().expect("the test binary has a path");
    let output = Command::new(exe)
        .args([
            "--exact",
            "child_recognises_frames_at_8_khz_then_16_khz",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .output()
        .expect("the test binary runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let tail: String = stderr
        .lines()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "the recognition of 8 kHz then 16 kHz frames ran to its end in-process: status {:?}\n\
         stderr tail:\n{tail}\nstdout:\n{stdout}",
        output.status.code()
    );
}
