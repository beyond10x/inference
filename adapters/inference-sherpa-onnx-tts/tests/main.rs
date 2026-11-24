//! The integration tests of the adapter, one binary.
//!
//! The tests that are not ignored need no weights and run on the sherpa-onnx native library built
//! without text-to-speech, the one the workspace builds against. The ignored ones speak: they
//! need the pinned checkpoint, its directory named by `INFERENCE_SHERPA_ONNX_TTS_DIR`, and a
//! process started on the native library built with text-to-speech. Nothing here downloads
//! either.

mod conformance;
mod licence;
mod open;
mod support;
