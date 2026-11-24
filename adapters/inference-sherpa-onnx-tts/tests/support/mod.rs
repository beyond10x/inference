//! What the tests share: the checkpoint directory `INFERENCE_SHERPA_ONNX_TTS_DIR` names, the
//! synthesiser opened on it, and scratch directories. Nothing here downloads weights.

use std::fs;
use std::path::{Path, PathBuf};

use inference_sherpa_onnx_tts::SherpaOnnxSynthesiser;

/// The variable naming the directory of the unpacked pinned checkpoint.
pub const CHECKPOINT_DIR: &str = "INFERENCE_SHERPA_ONNX_TTS_DIR";

/// The directory `INFERENCE_SHERPA_ONNX_TTS_DIR` names.
///
/// # Panics
///
/// When the variable is not set, naming it.
pub fn checkpoint_dir() -> PathBuf {
    std::env::var_os(CHECKPOINT_DIR)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!("set {CHECKPOINT_DIR} to the directory of the unpacked pinned checkpoint")
        })
}

/// Opens the pinned checkpoint of [`checkpoint_dir`] at most `max_concurrency` syntheses at
/// once.
pub fn open(max_concurrency: i64) -> SherpaOnnxSynthesiser {
    SherpaOnnxSynthesiser::open_with(checkpoint_dir(), max_concurrency)
        .unwrap_or_else(|refused| panic!("{CHECKPOINT_DIR} holds the pinned checkpoint: {refused}"))
}

/// A fresh, empty directory under Cargo's temporary directory for this test target.
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("inference-sherpa-onnx-tts")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("the previous run's directory is removed");
    }
    fs::create_dir_all(&dir).expect("the directory is created");
    dir
}
