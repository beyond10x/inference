//! Everything an application names, in one import: the handles, the stack, the recipes, the
//! events, usage, [`Error`], [`ErrorKind`], [`Result`] and, with the feature `audio-host`,
//! `AudioHost`.
//!
//! ```
//! use inference::prelude::*;
//!
//! fn reach(
//!     _: inference::core::model::Locality,
//!     _: inference::text::TextDelta,
//!     _: inference::audio::AudioFrame,
//!     _: inference::stt::RecognitionRequest,
//!     _: inference::llm::GenerationRequest,
//!     _: inference::tts::SegmentAudio,
//!     _: inference::duplex::SessionLedger,
//!     _: inference::tools::SideEffectClass,
//!     _: inference::pipeline::PipelineId,
//!     _: inference::runtime::run::RunState,
//!     _: inference::facade::events::SessionEvent,
//! ) {
//! }
//! #[cfg(feature = "audio-host")]
//! fn host(_: &dyn AudioHost, _: inference::audio_device::fake::FakeHost) {}
//!
//! let error = Error::Layer { message: "beneath".to_owned(), source: None };
//! assert_eq!(error.kind(), ErrorKind::Layer);
//! let done: Result<()> = Err(error);
//! assert!(done.is_err());
//! ```

#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use crate::agent::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use crate::chat::*;
pub use crate::error::{Error, ErrorKind, Result};
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use crate::event::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use crate::recipe::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use crate::stack::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use crate::transcriber::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use crate::usage::*;
#[allow(
    unused_imports,
    reason = "a stub module re-exports nothing until its story fills it"
)]
pub use crate::voice::*;
#[cfg(feature = "audio-host")]
pub use inference_audio_device::AudioHost;
