//! ElevenLabs streaming text-to-speech as a synthesiser of the `inference-tts` contract.
//!
//! `inference-elevenlabs` is a hosted adapter of the inference framework, at locality
//! `external_provider`: the text of every segment leaves the machine. [`ElevenLabsSynthesiser`]
//! implements [`inference_tts::Synthesiser`] over the multi-stream WebSocket
//! `/v1/text-to-speech/{voice_id}/multi-stream-input`; [`protocol`] says what it sends and reads,
//! the documentation it was taken from with its date, and what that documentation leaves open.
//!
//! A synthesis takes the segments offered to it in order and speaks them on one connection,
//! opened for its first segment, each segment a context of its own: the context's opening
//! message, its text ending in one space with `flush` set, and the close of the context. So only
//! the first segment of an utterance pays a handshake. The audio chunks that come back for the
//! context are emitted as frames of that segment while it is still Queued, on one sample clock
//! across the synthesis, and the context's final message completes it. A segment is sent as soon
//! as the handle is iterated after it was offered, before the utterance ends. So the binding
//! claims `incremental_input` and `output_streaming` ([`claims`]), and `records/claims-tts.json`
//! holds the same claims. Once the utterance has ended and its last segment is spoken, the
//! binding sends `close_socket` and closes the connection.
//!
//! Output formats ([`OutputFormat`]): `ulaw_8000` gives G.711 μ-law frames at 8 kHz, `alaw_8000`
//! A-law at 8 kHz, and `pcm_16000`, `pcm_22050`, `pcm_24000` and `pcm_44100` signed 16-bit
//! little-endian mono PCM at that rate. The documentation states neither bit depth, byte order
//! nor channels for `pcm_*`; 16-bit little-endian mono is this crate's assumption, and the live
//! example checks it. MP3, Opus and WAV are refused when the configuration is parsed.
//!
//! A cancel is a discard of the segment being spoken, as a barge-in makes it: the binding ends
//! that segment's context, drops every later message of it and emits nothing more of it, without
//! waiting for the server; the connection stays open and the next segment speaks on it. A cancel
//! stops nothing at the server: the context was flushed and closed when its text was sent, so
//! the endpoint still generates and sends the whole cancelled segment, and a barge-in still pays
//! for it. It saves playback, not cost. Dropping the handle closes the connection.
//!
//! A connection that ends between segments, as the endpoint's inactivity timeout may end it, is
//! reopened once for the next segment: when sending its context fails because the connection
//! ended, or the connection ends before any message of that context arrives, the binding opens a
//! new connection and sends the context again; a second failure fails the synthesis. Any other
//! close, stall or message outside the protocol fails the synthesis with an [`ElevenLabsError`]
//! ([`ElevenLabsSynthesis::failure`]), never a panic or a hang: the handshake is bounded by the
//! endpoint's connect timeout, every write by its read timeout, and every wait for the segment
//! being spoken by its read timeout from that segment's last message.
//!
//! The handle is a blocking iterator, as the contract defines it. It drives its connection on a
//! runtime of its own. On a thread where an async runtime is current, a `spawn_blocking` thread
//! or an async worker alike, that runtime is driven on a scoped thread of its own while the
//! caller waits, so no thread panics or is refused; an async worker is still blocked meanwhile,
//! so iterate the handle from `tokio::task::spawn_blocking` or a plain thread.
//!
//! The key is never read by the binding itself: the caller injects a [`CredentialSource`] and
//! names the secret ([`EnvironmentSource::elevenlabs`] reads `ELEVENLABS_API_KEY`; with feature
//! `keychain`, `KeychainSource` reads the OS keychain through the `secrets` library). It is sent
//! in the `xi-api-key` header of each handshake, marked sensitive, and never kept, printed,
//! logged or put in an error. The WebSocket library below does log the raw handshake request at
//! trace level under targets beginning with `tungstenite`; an embedder that installs a logger
//! keeps those targets below trace.
//!
//! The live check is an example, never a test: with a key with text-to-speech access in
//! `ELEVENLABS_API_KEY` (or the keychain) and a voice in `ELEVENLABS_VOICE_ID`,
//! `cargo run -p inference-elevenlabs --features live --example live` speaks one short utterance
//! in `ulaw_8000` and one in `pcm_24000`, checks the encoding, rate and frame lengths of what
//! comes back and prints the time to first audio. It is billed.

//!
//! With feature `duplex`, [`ElevenLabsAgent`] is a conversation with an ElevenLabs agent as a
//! native speech model of the `inference-duplex` contract, over the agent WebSocket
//! `/v1/convai/conversation`: [`agent`] is its wire protocol, with the documentation it was taken
//! from and what that leaves open, and `agent_duplex` says how the conversation maps onto the
//! contract. It reads messages and frames of at most [`agent::MAX_MESSAGE_BYTES`] (1 MiB). Its
//! claims are [`agent_claims`], and `records/claims-agent.json` holds the same; `native_duplex`
//! is not among them, since the agent answers speech over a response by interrupting it.

#[cfg(feature = "duplex")]
pub mod agent;
#[cfg(feature = "duplex")]
mod agent_duplex;
mod config;
mod connection;
mod credential;
mod error;
pub mod protocol;
mod synthesiser;

#[cfg(feature = "duplex")]
pub use agent_duplex::{
    AGENT_BACKEND, AGENT_MODEL, ElevenLabsAgent, ElevenLabsAgentSession, INBOUND_QUEUE,
    MESSAGE_QUIET, OUTBOUND_QUEUE, OpenCanceller, agent_capabilities, agent_claims,
    agent_native_duplex_verdict,
};
#[cfg(feature = "duplex")]
pub use inference_duplex;

pub use config::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_MODEL, DEFAULT_READ_TIMEOUT, DEFAULT_URL, Endpoint,
    MAX_INACTIVITY_TIMEOUT, OutputFormat, VoiceConfig, VoiceSettings,
};
pub use connection::CLOSE_BOUND;
#[cfg(feature = "keychain")]
pub use credential::KeychainSource;
pub use credential::{
    Credential, CredentialError, CredentialFuture, CredentialSource, ELEVENLABS_KEY_NAME,
    ELEVENLABS_KEY_VARIABLE, EnvironmentSource,
};
pub use error::{ConfigError, ConnectError, ElevenLabsError};
pub use synthesiser::{BACKEND, ElevenLabsSynthesis, ElevenLabsSynthesiser, claims};
