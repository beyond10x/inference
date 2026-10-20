//! The shared vocabulary of the inference framework.
//!
//! `inference-core` holds the minimal primitives every other crate of the workspace builds on: the
//! `inference.model` vocabulary of `ess/domains/model.yaml` ([`model`]), the
//! `inference.stream.Generation` of `ess/domains/stream.yaml`, one output epoch of a node within a
//! run ([`generation`]), the `inference.usage` vocabulary of `ess/domains/usage.yaml`, the usage
//! of one model call ([`usage`]), and the identifiers shared across the graph, stream, session
//! and duplex layers ([`id`]). It carries no concrete model, backend or provider implementation.

pub mod endpoint;
pub mod envelope;
pub mod generation;
pub mod id;
pub mod model;
pub mod policy;
pub mod usage;

/// The audio vocabulary a [`model::CapabilityRecord`] names its audio formats in, re-exported so
/// that a crate reading records needs no dependency of its own on `inference-audio`.
pub use inference_audio;

/// The crates [`uuid_newtype!`] expands to, so that a crate declaring an identifier with it needs
/// neither as a dependency of its own. Not part of the API.
#[doc(hidden)]
pub mod __private {
    pub use serde;
    pub use uuid;
}
