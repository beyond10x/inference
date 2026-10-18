//! The shared vocabulary of the inference framework.
//!
//! `inference-core` holds the minimal primitives every other crate of the workspace builds on: the
//! `inference.model` vocabulary of `ess/domains/model.yaml` and the identifiers shared across the
//! graph, stream and session layers. It carries no concrete model, backend or provider
//! implementation.

pub mod id;
pub mod model;
