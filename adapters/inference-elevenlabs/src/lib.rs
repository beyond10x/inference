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
//! message and its text ending in one space with `flush` set. So only the first segment of an
//! utterance pays a handshake. The audio chunks that come back for the context are emitted as
//! frames of that segment while it is still Queued, on one sample clock across the synthesis,
//! and the context's final message completes it. The context stays open while its audio comes:
//! once it has gone without a message for the endpoint's context quiet
//! ([`Endpoint::context_quiet`], [`DEFAULT_CONTEXT_QUIET`], 250 ms), counted from its last
//! message, the binding sends its close, and the endpoint, which ends a flushed context only when
//! it is closed, sends the final message; a final message that comes first needs no close. A
//! segment is sent as soon as the handle is iterated after it was offered, before the utterance
//! ends. So the binding claims `incremental_input` and `output_streaming` ([`claims`]), and
//! `records/claims-tts.json` holds the same claims. Once the utterance has ended and its last
//! segment is spoken, the binding sends `close_socket` and closes the connection.
//!
//! The time to first audio of each segment, from its offer to its first frame, is the
//! contract's: `inference_tts::SynthesisOutput::first_audio`, read through
//! [`inference_tts::SynthesisHandle::output`]. A segment offered while another is spoken waits
//! for it, and that wait counts.
//!
//! Output formats ([`OutputFormat`]): `ulaw_8000` gives G.711 μ-law frames at 8 kHz, `alaw_8000`
//! A-law at 8 kHz, and `pcm_16000`, `pcm_22050`, `pcm_24000` and `pcm_44100` signed 16-bit
//! little-endian mono PCM at that rate. The documentation states neither bit depth, byte order
//! nor channels for `pcm_*`; 16-bit little-endian mono is this crate's assumption, and the live
//! example checks it. MP3, Opus and WAV are refused when the configuration is parsed.
//!
//! A cancel is a discard of the segment being spoken, as a barge-in makes it: the binding sends
//! the close of that segment's context, as ElevenLabs' multi-context guide handles a barge-in,
//! writing it within [`CLOSE_BOUND`] and waiting for no answer, drops every later message of the
//! context and emits nothing more of it; the connection stays open and the next segment speaks
//! on it. The close is the documented way to end a context, and the guide closes the interrupted
//! context so that the endpoint stops speaking it; the AsyncAPI page also says the close of a
//! context already set to flush "will continue flushing", which leaves open whether the endpoint
//! still generates, and bills, what the flush released ([`protocol`] says more). A context the
//! binding already closed on its quiet is sent no second close: its audio had stopped coming for
//! the quiet, so the endpoint had most likely sent it all. Dropping the handle closes the
//! connection.
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
//! The key is copied only where the handshake needs it, and no field of the binding holds it past
//! the handshake. Every copy:
//!
//! - the [`Credential`] the source returns, zeroized on drop. The `xi-api-key` header value is the
//!   credential's own buffer, not a copy of it, and the buffer is zeroized when tungstenite has
//!   turned the request into bytes, or when the handshake fails or times out first;
//! - those bytes: tungstenite, the WebSocket library, writes the request, header included, into a
//!   buffer of its own that it frees without zeroizing once the request is sent, and the smaller
//!   buffers that one outgrew while being written may hold part of it too. The binding cannot
//!   reach them short of a WebSocket client of its own. They are freed memory of this process
//!   until the allocator reuses it, which matters only to someone who can already read this
//!   process's memory or a core dump of it, and the binding accepts that;
//! - for `wss://`, the TLS record rustls copies the request into and encrypts in place, so what it
//!   keeps is ciphertext;
//! - the trace-level log record named above, when a logger lets it through.
//!
//! The Agents Platform binding takes no key: a private agent's signed URL carries a token of its
//! own, held as `agent::SignedUrl` (feature `duplex`) describes.
//!
//! The live check is an example, never a test: with a key with text-to-speech access in
//! `ELEVENLABS_API_KEY` (or the keychain) and a voice in `ELEVENLABS_VOICE_ID`,
//! `cargo run -p inference-elevenlabs --features live --example live` speaks one short utterance
//! in `ulaw_8000` and one in `pcm_24000`, checks the encoding, rate and frame lengths of what
//! comes back and prints the time to first audio. It is billed.

//!
//! With feature `duplex`, `ElevenLabsAgent` is a conversation with an ElevenLabs agent as a
//! native speech model of the `inference-duplex` contract, over the agent WebSocket
//! `/v1/convai/conversation`: `agent` is its wire protocol, with the documentation it was taken
//! from and what that leaves open, and `agent_duplex` says how the conversation maps onto the
//! contract. It reads messages and frames of at most `agent::MAX_MESSAGE_BYTES` (1 MiB). Its
//! claims are `agent_claims`, and `records/claims-agent.json` holds the same; `native_duplex`
//! is not among them, since the agent answers speech over a response by interrupting it.
//!
//! # Speech to text
//!
//! With feature `stt`, `ElevenLabsRecogniser` implements `inference_stt::Recogniser` at locality
//! `external_provider` over the realtime speech-to-text WebSocket `/v1/speech-to-text/realtime`:
//! the audio of every recognition leaves the machine. `stt_protocol` says what it sends and
//! reads, the documentation it was taken from with its date, and what that documentation leaves
//! open.
//!
//! A recognition opens its connection when its first frame is taken, with the configured input
//! format (`InputFormat`): `ulaw_8000`, G.711 μ-law at 8 kHz as telephony carries it, or 16-bit
//! PCM at 8, 16, 22.05, 24, 44.1 or 48 kHz. Any other format is refused when the configuration
//! is parsed, and a frame in another format than the configured one fails the recognition with
//! `ElevenLabsError::UnsupportedAudio` before it is sent. Every frame is one `input_audio_chunk`,
//! sent as soon as the handle is iterated after it was offered, and partial transcripts reach the
//! caller before input ends, so the recogniser claims `incremental_input` and `output_streaming`
//! (`stt_claims`), and `records/claims-stt.json` holds the same claims. The configuration
//! (`TranscriptionConfig`) names the model (`scribe_v2_realtime` unless configured), the
//! language, the key terms and the commit strategy: `manual`, where the end of input and
//! `ElevenLabsRecognition::commit` commit, or `vad` with its thresholds, where the endpoint
//! commits after silence and the end of input commits what is left.
//!
//! - A `partial_transcript` opens a Hypothesis span with its text, or revises the open one to
//!   read it if that differs; a blank one emits nothing.
//! - A `committed_transcript_with_timestamps` brings the open span to its text and commits it;
//!   its words' first start and last end are the span's timing, read through
//!   `ElevenLabsRecognition::span_timing`, since the contract's events carry none. A
//!   `committed_transcript` only brings the open span to its text.
//! - The end of input sends a commit; its committed transcript ends the recognition, with `vad`
//!   once the endpoint has then been silent for `VAD_END_QUIET` with no span open, since a commit
//!   the endpoint made on its own may answer for it. The usage record is then its last event. The protocol reports no usage, so the record is the
//!   contract's: `input_audio_ms`, the audio sent (on a failed call, the audio sent before the
//!   failure), `requests` 1 and `local_compute_ms` unknown.
//!
//! Each of the fourteen error types the endpoint documents fails the recognition with an
//! `ElevenLabsError` variant of its own (`ElevenLabsError::scribe_error_type` names it), and so
//! do a refused handshake or one not done within the connect timeout (`ElevenLabsError::Connect`,
//! also when input ended while it was pending), a close, a message outside the protocol or larger
//! than `MAX_MESSAGE_SIZE` (4 MiB, the bound of every connection of the TTS binding and the
//! recogniser), and a server silent for the read timeout once input has ended while a commit is
//! unanswered or a span is open (`ElevenLabsError::Stalled`); never a panic or a hang. A server
//! silent for it once every commit is answered and no span is open ends the recognition cleanly.
//! The opening, the credential included, is bounded by the connect timeout (once input has ended
//! the handle waits for its outcome at most the connect timeout and `OPEN_SLACK`, 500 ms, from
//! the start of the opening, then fails it as `Connect` itself, so a credential source that
//! blocks its thread cannot hold the caller) and every write by the read timeout. After the end
//! of input the read timeout runs from the latest of the end of input, the opening of the
//! connection and the last server message that delivered an event or answered a commit, and does
//! not run while the handshake is pending; a `warning` or a partial transcript repeating the open
//! span's text moves nothing, so a server repeating them cannot hold the caller. A timeout too
//! large for an instant is no deadline.
//! `ElevenLabsRecognition::cancel` and dropping the handle close the connection and deliver
//! nothing more. The connection is driven on a thread of its own, so the handle may be iterated
//! from any thread; at most 32 server messages wait for the caller. The
//! key travels as the TTS binding sends it: only in the `xi-api-key` header, from the injected
//! source, zeroized, and `ws://` only to a loopback host.
//!
//! The live check is `tests/stt_live.rs`, behind features `live` and `stt` and ignored: with a
//! key with speech-to-text access in `ELEVENLABS_API_KEY` (or the keychain) and a German G.711
//! μ-law clip of about 30 s in `INFERENCE_ELEVENLABS_STT_CLIP`,
//! `cargo test -p inference-elevenlabs --features live,stt --test stt_live -- --ignored`
//! transcribes it. It is billed.

#[cfg(feature = "duplex")]
pub mod agent;
#[cfg(feature = "duplex")]
mod agent_duplex;
mod config;
mod connection;
mod credential;
mod error;
pub mod protocol;
#[cfg(feature = "stt")]
mod recogniser;
#[cfg(feature = "stt")]
mod stt_config;
#[cfg(feature = "stt")]
pub mod stt_protocol;
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
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_CONTEXT_QUIET, DEFAULT_MODEL, DEFAULT_READ_TIMEOUT,
    DEFAULT_URL, Endpoint, MAX_INACTIVITY_TIMEOUT, OutputFormat, VoiceConfig, VoiceSettings,
};
pub use connection::{CLOSE_BOUND, MAX_MESSAGE_SIZE};
#[cfg(feature = "keychain")]
pub use credential::KeychainSource;
pub use credential::{
    Credential, CredentialError, CredentialFuture, CredentialSource, ELEVENLABS_KEY_NAME,
    ELEVENLABS_KEY_VARIABLE, EnvironmentSource,
};
pub use error::{ConfigError, ConnectError, ElevenLabsError};
#[cfg(feature = "stt")]
pub use recogniser::{
    ElevenLabsRecogniser, ElevenLabsRecognition, OPEN_SLACK, STT_BACKEND, STT_INBOUND_DEPTH,
    VAD_END_QUIET, WARNINGS_KEPT_STT, stt_claims,
};
#[cfg(feature = "stt")]
pub use stt_config::{
    CommitStrategy, DEFAULT_STT_MODEL, InputFormat, TranscriptionConfig, VadSettings,
};
#[cfg(feature = "stt")]
pub use stt_protocol::SpanTiming;
pub use synthesiser::{BACKEND, ElevenLabsSynthesis, ElevenLabsSynthesiser, claims};
