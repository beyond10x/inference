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

/// `inference.model`: the in-memory stores of the declared models, adapters and capability
/// records, their views, and the loader that checks an adapter's record file against its claims.
pub mod model;

/// `inference.policy`: the in-memory store of the declared deployment profiles, and its view.
pub mod policy;

/// `inference.graph`: the in-memory pipeline declaration, its nodes, ports, edges and bindings,
/// their validation, their planning, behind the `run` feature the runs of planned pipelines with
/// their node instances and the recording mock node, and their views.
pub mod graph;

/// `inference.stream`: the in-memory stores of the turns, spans, generations and output segments,
/// their views, and the log that records their events in envelopes.
pub mod stream;

/// `inference.tools`: the in-memory stores of the declared tools and of the tool calls through
/// their lifecycle, their views, and the scripted mock of the executor contract.
pub mod tools;

/// `inference.session`, behind the `run` feature: the in-memory store of the sessions, each holding
/// its current run, and its view, over the session state machine of `inference-facade`.
pub mod session;

/// `inference.duplex`: the in-memory stores of the native speech sessions, their responses and
/// tool calls, their views, and behind `duplex` the scripted mock native speech model.
pub mod duplex;

/// `inference.usage`: the in-memory store of the usage records of model calls, each of a
/// declared adapter and model.
pub mod usage;

/// The scripted mock recogniser of the STT contract, in two capability configurations, and
/// synthetic audio of tone and silence.
#[cfg(feature = "stt")]
pub mod stt;

/// The scripted mock `llm` model of the LLM contract, wrapped by its generator, in three
/// capability configurations, which may also be scripted to propose tool calls and to report the
/// usage of each turn.
#[cfg(feature = "llm")]
pub mod llm;

/// The scripted mock synthesiser of the TTS contract, in four capability configurations, a sink
/// double that acknowledges playback, and the backend-neutral checks of the TTS adapter exit
/// criteria that an adapter crate runs against its own synthesiser.
#[cfg(feature = "tts")]
pub mod tts;

/// The voice cascade on the mocks: topology B declared with exposed ends, its payload and the
/// node implementations of one run, the TTS node of a cascade whose sink acknowledges playback,
/// the model node of a cascade that can be interrupted, one generation per turn, and a
/// synthesiser scripted to deliver frames late, behind the `stt`, `llm`, `tts` and `run` features
/// together.
#[cfg(all(feature = "stt", feature = "llm", feature = "tts", feature = "run"))]
pub mod cascade;
