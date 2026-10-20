//! Prints each span the local sherpa-onnx recogniser commits for a WAV file:
//!
//! ```text
//! INFERENCE_SHERPA_ONNX_STT_DIR=<checkpoint dir> cargo run -p stt-sherpa-onnx --example transcribe -- <file.wav>
//! ```
//!
//! The directory holds the unpacked pinned checkpoint of `inference-sherpa-onnx`; nothing is
//! downloaded.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// The variable naming the directory of the unpacked pinned checkpoint.
const CHECKPOINT_DIR: &str = "INFERENCE_SHERPA_ONNX_STT_DIR";

/// Transcribe a WAV file with the local sherpa-onnx recogniser.
#[derive(Debug, Parser)]
struct Args {
    /// The WAV file to transcribe: 16-bit PCM, 32-bit float, mu-law or A-law.
    wav: PathBuf,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let Some(dir) = std::env::var_os(CHECKPOINT_DIR) else {
        eprintln!("set {CHECKPOINT_DIR} to the directory of the unpacked pinned checkpoint");
        return ExitCode::FAILURE;
    };
    match stt_sherpa_onnx::run(&PathBuf::from(dir), &args.wav) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(failed) => {
            eprintln!("{failed}");
            ExitCode::FAILURE
        }
    }
}
