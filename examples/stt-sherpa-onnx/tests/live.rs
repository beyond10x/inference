//! The example recognises the checkpoint's LibriSpeech clip `test_wavs/0.wav` into committed
//! lines. Needs the unpacked pinned checkpoint in the directory `INFERENCE_SHERPA_ONNX_STT_DIR`;
//! ignored by default. A directory without it is refused, naming the first missing file, with no
//! weights.

use std::path::{Path, PathBuf};

use inference_sherpa_onnx::stt::{FILES, OpenRefused, REFERENCE_CLIP};
use stt_sherpa_onnx::{RunFailed, run};

const CHECKPOINT_DIR: &str = "INFERENCE_SHERPA_ONNX_STT_DIR";

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn the_reference_clip_is_printed_as_committed_lines() {
    let dir = PathBuf::from(std::env::var_os(CHECKPOINT_DIR).unwrap_or_else(|| {
        panic!("set {CHECKPOINT_DIR} to the directory of the unpacked pinned checkpoint")
    }));
    let lines = run(&dir, &dir.join(REFERENCE_CLIP.name)).expect("the clip is recognised");
    eprintln!("{lines:#?}");
    assert!(!lines.is_empty());
    assert!(lines.iter().all(|line| line.starts_with("committed: ")));
    let text = lines.join(" ").to_lowercase();
    for word in ["nightfall", "lamps", "brothels"] {
        assert!(text.contains(word), "{word:?} is committed: {lines:?}");
    }
}

#[test]
fn a_directory_without_the_checkpoint_is_refused_before_anything_is_recognised() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("stt-sherpa-onnx-empty");
    std::fs::create_dir_all(&dir).expect("the directory is created");
    let wav = dir.join("silence.wav");
    let mut file = b"RIFF\x24\x00\x00\x00WAVEfmt \x10\x00\x00\x00".to_vec();
    file.extend_from_slice(&[1, 0, 1, 0, 0x80, 0x3E, 0, 0, 0, 0x7D, 0, 0, 2, 0, 16, 0]);
    file.extend_from_slice(b"data\x00\x00\x00\x00");
    std::fs::write(&wav, file).expect("the file is written");

    match run(&dir, &wav) {
        Err(RunFailed::Open(OpenRefused::FileMissing { path, .. })) => {
            assert_eq!(path, dir.join(FILES[0].name));
        }
        other => panic!("refused as a missing checkpoint file: {other:?}"),
    }
}
