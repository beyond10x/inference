//! The inference SDK: one dependency and one import (`docs/design/sdk.md`).
//!
//! Five layers, each one method away from the next: one call ([`transcribe`]-style functions),
//! handles (`Agent`, `Chat`, `Transcriber`, `Voice`), recipes, the graph over one [`payload`],
//! and the contract crates beneath, re-exported as modules ([`core`], [`text`], [`audio`],
//! [`stt`], [`llm`], [`tts`], [`duplex`], [`tools`], [`pipeline`], [`runtime`], [`facade`]).
//! A stack binds each role to an adapter; an adapter is compiled in only with its feature, and the
//! default build carries none. Every failure is one [`Error`] whose message names its fix.
//!
//! `use inference::prelude::*;` brings the handles, the stack, the recipes, the events, usage,
//! [`Error`], [`ErrorKind`] and [`Result`].

extern crate self as inference;

pub use inference_audio as audio;
pub use inference_core as core;
pub use inference_duplex as duplex;
pub use inference_facade as facade;
pub use inference_llm as llm;
pub use inference_pipeline as pipeline;
pub use inference_runtime as runtime;
pub use inference_stt as stt;
pub use inference_text as text;
pub use inference_tts as tts;

/// The audio devices: `AudioHost`, the fake host of the tests and, with the feature
/// `audio-device`, `CpalHost`.
#[cfg(feature = "audio-host")]
pub use inference_audio_device as audio_device;

pub use inference_macros::{main, tool};

/// The tool contract: `Tool`, `ToolExecutor`, `SideEffectClass` and the rest of `inference-tools`,
/// at the paths `#[inference::tool]` emits.
pub mod tools {
    pub use inference_tools::*;
}

mod error;
mod macro_selftest;

pub mod agent;
pub mod chat;
#[cfg(feature = "audio-host")]
pub mod devices;
pub mod event;
pub mod graph;
pub mod oneshot;
pub mod payload;
pub mod prelude;
pub mod recipe;
pub mod registry;
pub mod stack;
#[cfg(feature = "testing")]
pub mod testing;
pub mod transcriber;
pub mod usage;
pub mod voice;

#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use agent::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use chat::*;
#[cfg(feature = "audio-host")]
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use devices::*;
pub use error::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use event::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use oneshot::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use payload::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use recipe::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use registry::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use stack::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use transcriber::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use usage::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use voice::*;

/// What the macros' expansions reach, at paths only they use.
#[doc(hidden)]
pub mod __private {
    pub use schemars;
    pub use serde;
    pub use serde_json;
    pub use tokio;

    /// The end of the program `#[inference::main]` emits: [`report`](crate::report) over stderr.
    #[must_use]
    pub fn exit(result: crate::Result<()>) -> std::process::ExitCode {
        crate::report(result, &mut std::io::stderr().lock())
    }
}
