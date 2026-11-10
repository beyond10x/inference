//! Test utilities of the inference framework.
//!
//! `inference-testkit` holds one in-memory store module per domain of `ess/system.yaml`; behind
//! the `run` feature, the runs and node instances of the graph store, the recording mock node
//! and the mock source and sink nodes the runtime's run controller drives, generic over the
//! payload of the pipeline, and the run refusals of the stream store, which take
//! `inference-runtime`; and behind the `stt`, `llm`, `tts` and `duplex` features, the mocks of
//! the contract crates, with the graph nodes over them behind each of those features and `run`
//! together: the mock model node, the STT node and the fan-out of a voice pipeline, and the TTS
//! node, the STT and TTS nodes generic over the recogniser and the synthesiser, the mock by
//! default, so that a local adapter can take the mock's place. Its test tree carries the
//! conformance target that answers the synthesized suite of `ess/` (`tests/conform.rs`), which
//! needs the `run` feature; the library itself pulls no ESS crate, so a crate depending on the
//! stores or the mocks carries no test tooling, and one that leaves `run` off carries no runtime.
//!
//! Every module of a domain `ess/system.yaml` lists exists, empty until the story that declares its
//! domain fills it; a domain added to the system adds its module line here.

pub mod model;

pub mod policy;

pub mod graph;

pub mod stream;

pub mod tools;

pub mod session;

pub mod duplex;

pub mod mark;

pub mod usage;

#[cfg(feature = "stt")]
pub mod stt;

#[cfg(feature = "llm")]
pub mod llm;

#[cfg(feature = "tts")]
pub mod tts;

#[cfg(all(feature = "stt", feature = "llm", feature = "tts", feature = "run"))]
pub mod cascade;
