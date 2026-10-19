//! Test utilities of the inference framework.
//!
//! `inference-testkit` holds one in-memory store module per domain of `ess/system.yaml` and,
//! behind the `stt`, `llm` and `tts` features, the mocks of the contract crates. Its test tree
//! carries the conformance target that answers the synthesized suite of `ess/`
//! (`tests/conform.rs`); the library itself pulls no ESS crate, so a crate depending on the stores
//! or the mocks carries no test tooling.
//!
//! Every module exists from the start, empty until the story that declares its domain fills it, so
//! that no later story adds a line here.

/// `inference.model`: the in-memory stores of the declared models and adapters, and their views.
pub mod model;

/// `inference.policy`: the in-memory store of the declared deployment profiles, and its view.
pub mod policy;

/// `inference.graph`: the in-memory pipeline declaration, its nodes, ports, edges and bindings,
/// and their views; empty of runs until the run stores land.
pub mod graph;

/// `inference.stream`: the in-memory stores of the turns, spans, generations and output segments,
/// their views, and the log that records their events in envelopes.
pub mod stream;

/// `inference.tools`: empty until the tool-call stores land.
pub mod tools;

/// `inference.session`: empty until the session stores land.
pub mod session;

/// The scripted mock recogniser of the STT contract, in two capability configurations.
#[cfg(feature = "stt")]
pub mod stt;

/// The scripted mock generator of the LLM contract, in three capability configurations.
#[cfg(feature = "llm")]
pub mod llm;

/// The mock synthesiser of the TTS contract; empty until that contract lands.
#[cfg(feature = "tts")]
pub mod tts;
