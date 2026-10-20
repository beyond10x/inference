//! Local adapters of the inference framework over the sherpa-onnx runtime.
//!
//! `inference-sherpa-onnx` is an adapter crate: it runs models through sherpa-onnx, a C++ speech
//! runtime over onnxruntime, reached through the official `sherpa-onnx` crate at 1.13.8 as native
//! FFI in the calling process. Each model kind sits behind a feature of its own and is enabled
//! explicitly by the caller, never by default:
//!
//! * `stt`: [`stt`], a streaming speech recogniser implementing the `inference-stt` contract over
//!   one pinned, hash-checked checkpoint.
//!
//! The native library is the release archive built without text-to-speech, so no GPL-licensed
//! phonemiser is linked. It is fetched and checked against its pinned sha256 by the repository's
//! `sherpa-onnx-archive` task, never by the build itself.

#[cfg(feature = "stt")]
pub mod stt;
