//! The shared vocabulary of the inference framework.
//!
//! `inference-core` holds the minimal primitives every other crate of the workspace builds on: the
//! `inference.model` vocabulary of `ess/domains/model.yaml` ([`model`]), the
//! `inference.stream.Generation` of `ess/domains/stream.yaml`, one output epoch of a node within a
//! run ([`generation`]), and the identifiers shared across the graph, stream and session layers
//! ([`id`]). It carries no concrete model, backend or provider implementation.

pub mod envelope;
pub mod generation;
pub mod id;
pub mod model;

/// The crates [`uuid_newtype!`] expands to, so that a crate declaring an identifier with it needs
/// neither as a dependency of its own. Not part of the API.
#[doc(hidden)]
pub mod __private {
    pub use serde;
    pub use uuid;
}
