//! A local adapter of the inference framework over whisper.cpp.
//!
//! `inference-whispercpp` is an adapter crate: it runs one pinned Whisper checkpoint in ggml
//! format through whisper.cpp, reached through the `whisper-rs` crate at 0.16.0 as native FFI in
//! the calling process. [`stt`] is a batch speech recogniser implementing the `inference-stt`
//! contract: whisper.cpp transcribes one complete buffer per call, so the recogniser claims
//! neither incremental input nor output streaming.
//!
//! whisper.cpp is compiled from the sources `whisper-rs-sys` ships, with `cmake`, a C++ compiler
//! and `libclang` (for `bindgen`) on the builder; nothing is downloaded at build time. The crate
//! builds whisper.cpp for the CPU only and exposes no GPU feature.

pub mod stt;
