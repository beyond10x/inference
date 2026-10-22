//! The native speech contract of the inference framework.
//!
//! `inference-duplex` holds `ess/domains/duplex.yaml`: a native speech model, topology D of the
//! design, is one multiport node that takes continuous audio and control in and gives audio,
//! transcripts and tool calls out. It is never an STT, an LLM and a TTS node in disguise.
//!
//! A [`NativeSpeechModel`] opens a [`SpeechSessionHandle`] from a [`SessionConfig`]: voice,
//! instructions, input and output [`AudioFormat`]s, [`TurnDetection`], whether a detected turn end
//! creates a response by itself, whether detected speech interrupts a response by itself, the
//! [`Tool`]s the session may propose and a liveness bound. What a model takes is its
//! [`SpeechCapabilities`]; a configuration outside them is refused at open with the
//! [`Unsupported`] option named, never silently changed. The session takes audio frames, a commit
//! of the input, response creation, cancellation, truncation of output audio at the sample the
//! listener heard, and tool results by call id; it yields typed [`SessionEvent`]s.
//!
//! [`SessionLedger`] is the one record of a session that every implementation keeps: it decides
//! what the specification decides (an open session, a response in progress, a proposed call) and
//! what the contract adds (frames in the configured format, one response at a time, a tool the
//! session offers, truncation within the audio emitted, the liveness bound), and it holds the
//! events in the order they were recorded. An adapter records what its provider sends through it
//! and what its caller asks through it, so two adapters refuse the same things the same way.
//!
//! The ledger also records usage as `inference.usage` records ([`SessionEvent::UsageRecorded`]):
//! one per response, one per transcription the provider reports usage of, and usage of the whole
//! session where a provider bills per session, besides the duplex's own `ResponseUsageRecorded`
//! and `TranscriptionUsageRecorded`. An adapter builds the [`Quantity`]s from its provider's own
//! fields ([`reported`]), so a count the provider left out is `unknown`, never zero.
//!
//! The crate depends on `inference-core`, `inference-audio` and `inference-tools` and carries no
//! provider, transport or other concrete implementation. Serialized names are the ESS names.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

pub use inference_audio;
pub use inference_core;
pub use inference_tools;

use inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_core::model::AdapterId;
use inference_tools::{Json, ToolId};

pub mod conformance;
mod contract;
mod ledger;
mod record;

pub use contract::{
    NativeSpeechModel, OpenError, SessionConfig, SessionError, SessionEvent, SpeechCapabilities,
    SpeechSessionHandle, TuningOption, TuningSupport, TurnTuning, Unsupported,
};
pub use ledger::{Liveness, LivenessMark, SessionLedger};
pub use record::{ReceivedSessionEvent, Response, SpeechSession, SpeechToolCall};

/// The `inference.usage` vocabulary a session's usage records are written in, re-exported from
/// `inference_core::usage`.
pub use inference_core::usage::{
    Quantity, QuantityKind, QuantitySource, UsageRecord, UsageRecorded,
};

#[cfg(doc)]
use inference_tools::Tool;

/// `inference.duplex.SpeechSessionId` identifies a [`SpeechSession`] and
/// `inference.duplex.ResponseId` a [`Response`]; an adapter issues one response id per response
/// its provider starts, whatever identity the provider gives it. Both are declared in
/// `inference_core::id`, because a usage record of `inference.usage` names them too, and
/// re-exported here.
pub use inference_core::id::{ResponseId, SpeechSessionId};

/// `inference.duplex.SpeechCallId`: identifies a [`SpeechToolCall`]. It is the provider's own
/// string, carried unread, because the result goes back to the provider under it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpeechCallId(String);

impl SpeechCallId {
    /// The call id `id`.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The string the provider gave.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SpeechCallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `inference.duplex.TurnDetection`: who ends a user turn. Serialized as written in ESS
/// (`server_vad`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnDetection {
    /// The provider's voice activity detection.
    ServerVad,
    /// The provider's semantic turn detection.
    SemanticVad,
    /// Nobody: the caller commits the input and creates each response.
    Disabled,
}

impl TurnDetection {
    /// Every mode, in the order `ess/domains/duplex.yaml` lists them.
    pub const ALL: [Self; 3] = [Self::ServerVad, Self::SemanticVad, Self::Disabled];
}

/// `inference.duplex.TurnSensitivity`: how readily the provider's turn detection ends a turn.
/// Serialized as written in ESS (`low`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSensitivity {
    /// Waits longer before it ends a turn.
    Low,
    /// The provider's middle setting.
    Medium,
    /// Ends a turn sooner.
    High,
}

impl TurnSensitivity {
    /// Every sensitivity, in the order `ess/domains/duplex.yaml` lists them.
    pub const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::High];
}

/// `inference.duplex.SessionFailureKind`: how a session or a response failed. Serialized as
/// written in ESS (`transport_closed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionFailureKind {
    /// The provider sent nothing within the liveness bound.
    Stalled,
    /// The transport to the provider closed.
    TransportClosed,
    /// The provider refused a request.
    Rejected,
    /// The provider reported an error of its own.
    ProviderError,
}

impl SessionFailureKind {
    /// Every kind, in the order `ess/domains/duplex.yaml` lists them.
    pub const ALL: [Self; 4] = [
        Self::Stalled,
        Self::TransportClosed,
        Self::Rejected,
        Self::ProviderError,
    ];
}

/// `inference.duplex.SessionFailure`: the typed failure of a session or a response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionFailure {
    pub kind: SessionFailureKind,
    pub detail: String,
}

impl fmt::Display for SessionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the session failed ({:?}): {}", self.kind, self.detail)
    }
}

impl Error for SessionFailure {}

/// `inference.duplex.TokenCounts`: the tokens of one direction of one response, by kind.
/// `cached_tokens` counts input tokens the provider served from its cache, inside the other two.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenCounts {
    pub text_tokens: u64,
    pub audio_tokens: u64,
    pub cached_tokens: u64,
}

/// `inference.duplex.ResponseUsage`: the usage of one response, input and output apart. It is the
/// running total of the response so far: a later record replaces an earlier one, and
/// [`SessionLedger::usage`] holds the latest. An adapter whose provider reports deltas sums them
/// per response before it records; one that reports totals records them as they are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseUsage {
    pub input: TokenCounts,
    pub output: TokenCounts,
}

impl ResponseUsage {
    /// Every count as a quantity the provider reported, mapped as `ess/domains/usage.yaml` maps
    /// them: input text, audio and cached tokens, output text and audio tokens (output cached
    /// tokens have no kind). It is for a provider that reports every count; one that leaves a
    /// count out builds its quantities from its own fields, so that count stays unknown. A count
    /// above `i64::MAX` is unknown ([`reported`]).
    #[must_use]
    pub fn quantities(&self) -> Vec<Quantity> {
        vec![
            reported(QuantityKind::InputTextTokens, Some(self.input.text_tokens)),
            reported(
                QuantityKind::InputAudioTokens,
                Some(self.input.audio_tokens),
            ),
            reported(
                QuantityKind::CachedInputTokens,
                Some(self.input.cached_tokens),
            ),
            reported(
                QuantityKind::OutputTextTokens,
                Some(self.output.text_tokens),
            ),
            reported(
                QuantityKind::OutputAudioTokens,
                Some(self.output.audio_tokens),
            ),
        ]
    }
}

/// The quantity `kind` as a provider reports it: `count` as `provider_reported`, or `unknown`
/// when the provider left it out or the count lies beyond the ESS `Integer` (`i64::MAX`), since
/// no number recorded in its place would be the count.
#[must_use]
pub fn reported(kind: QuantityKind, count: Option<u64>) -> Quantity {
    count
        .and_then(|count| i64::try_from(count).ok())
        .map_or(Quantity::unknown(kind), |count| {
            Quantity::provider_reported(kind, count)
        })
}

/// `inference.duplex.TranscriptionUsage`: the usage of transcribing the input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl TranscriptionUsage {
    /// Both counts as quantities the provider reported: the input tokens as
    /// `input_audio_tokens`, the output tokens as `output_text_tokens`. A count above
    /// `i64::MAX` is unknown ([`reported`]).
    #[must_use]
    pub fn quantities(&self) -> Vec<Quantity> {
        vec![
            reported(QuantityKind::InputAudioTokens, Some(self.input_tokens)),
            reported(QuantityKind::OutputTextTokens, Some(self.output_tokens)),
        ]
    }
}

/// Whether a provider's report of a response's usage is the response's final total, which no
/// later report changes ([`UsageTotal::Final`]), or a running total that a later report may still
/// replace, also after the response ended ([`UsageTotal::Running`]). It decides when
/// [`SessionLedger`] records the response's usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageTotal {
    /// A later report may still replace it, also after the response ended.
    Running,
    /// No later report changes it.
    Final,
}

/// `inference.duplex.SpeechSession.State`. Serialized as written in ESS (`Open`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpeechSessionState {
    /// The session takes input and yields output.
    Open,
    /// The session was closed and takes nothing more.
    Closed,
    /// The session failed with a typed failure and takes nothing more.
    Failed,
}

impl SpeechSessionState {
    /// Every state, in the order `ess/domains/duplex.yaml` lists them.
    pub const ALL: [Self; 3] = [Self::Open, Self::Closed, Self::Failed];
}

/// `inference.duplex.Response.State`. Serialized as written in ESS (`InProgress`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResponseState {
    /// The response streams output.
    InProgress,
    /// The response is done.
    Completed,
    /// The response was cancelled; nothing more of it is delivered.
    Cancelled,
    /// The response failed with a typed failure.
    Failed,
}

impl ResponseState {
    /// Every state, in the order `ess/domains/duplex.yaml` lists them.
    pub const ALL: [Self; 4] = [
        Self::InProgress,
        Self::Completed,
        Self::Cancelled,
        Self::Failed,
    ];
}

/// `inference.duplex.SpeechToolCall.State`. Serialized as written in ESS (`Proposed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpeechToolCallState {
    /// Proposed by a response; it awaits its result.
    Proposed,
    /// Its result is on the session.
    Answered,
}

impl SpeechToolCallState {
    /// Every state, in the order `ess/domains/duplex.yaml` lists them.
    pub const ALL: [Self; 2] = [Self::Proposed, Self::Answered];
}

/// `inference.duplex.OpenSpeechSession`: the record of a session's configuration, its tools by
/// identity, its turn tuning and liveness bound in milliseconds, each tuning absent for the
/// provider's default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSpeechSession {
    pub adapter_id: AdapterId,
    pub voice: String,
    pub instructions: String,
    pub input_format: AudioFormat,
    pub output_format: AudioFormat,
    pub turn_detection: TurnDetection,
    pub auto_response: bool,
    pub turn_silence_ms: Option<i64>,
    pub turn_sensitivity: Option<TurnSensitivity>,
    pub turn_prefix_padding_ms: Option<i64>,
    pub auto_interrupt: bool,
    pub tools: Vec<ToolId>,
    pub idle_limit_ms: i64,
}

/// `inference.duplex.ProposeSpeechToolCall`: a response proposes a call of a tool with JSON
/// arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposeSpeechToolCall {
    pub response_id: ResponseId,
    pub tool_id: ToolId,
    pub arguments: Json,
}

/// `inference.duplex.SpeechSessionOpened`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechSessionOpened {
    pub session_id: SpeechSessionId,
    pub adapter_id: AdapterId,
    pub turn_detection: TurnDetection,
}

/// `inference.duplex.InputAccepted`: a frame of input audio, accepted as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputAccepted {
    pub session_id: SpeechSessionId,
    pub frame: AudioFrame,
}

/// `inference.duplex.InputCommitted`: the input accepted so far is one user turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputCommitted {
    pub session_id: SpeechSessionId,
}

/// `inference.duplex.SpeechStarted`: speech started at `position` of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechStarted {
    pub session_id: SpeechSessionId,
    pub position: SampleClockPosition,
}

/// `inference.duplex.SpeechStopped`: speech stopped at `position` of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechStopped {
    pub session_id: SpeechSessionId,
    pub position: SampleClockPosition,
}

/// `inference.duplex.InputTranscribed`: the provider's transcript of the user's input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputTranscribed {
    pub session_id: SpeechSessionId,
    pub text: String,
}

/// `inference.duplex.TranscriptionUsageRecorded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionUsageRecorded {
    pub session_id: SpeechSessionId,
    pub usage: TranscriptionUsage,
}

/// `inference.duplex.ResponseCreated`: a response is in progress under a new id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseCreated {
    pub response_id: ResponseId,
    pub session_id: SpeechSessionId,
}

/// `inference.duplex.OutputAudio`: audio of a response in the session's output format. The
/// frame's position counts samples from the start of the response's audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputAudio {
    pub response_id: ResponseId,
    pub frame: AudioFrame,
}

/// `inference.duplex.OutputTranscriptDelta`: transcript of a response's audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputTranscriptDelta {
    pub response_id: ResponseId,
    pub text: String,
}

/// `inference.duplex.ResponseCompleted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseCompleted {
    pub response_id: ResponseId,
}

/// `inference.duplex.ResponseCancelled`: barge-in; nothing more of the response is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseCancelled {
    pub response_id: ResponseId,
}

/// `inference.duplex.ResponseFailed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseFailed {
    pub response_id: ResponseId,
    pub failure: SessionFailure,
}

/// `inference.duplex.OutputTruncated`: the response's audio ends at `heard`, a sample of its own
/// audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputTruncated {
    pub response_id: ResponseId,
    pub heard: SampleClockPosition,
}

/// `inference.duplex.ResponseUsageRecorded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseUsageRecorded {
    pub response_id: ResponseId,
    pub usage: ResponseUsage,
}

/// `inference.duplex.SpeechToolCallProposed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechToolCallProposed {
    pub call_id: SpeechCallId,
    pub response_id: ResponseId,
    pub tool_id: ToolId,
    pub arguments: Json,
}

/// `inference.duplex.SpeechToolCallAnswered`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechToolCallAnswered {
    pub call_id: SpeechCallId,
    pub result: Json,
}

/// `inference.duplex.SpeechSessionClosed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechSessionClosed {
    pub session_id: SpeechSessionId,
}

/// `inference.duplex.SpeechSessionFailed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechSessionFailed {
    pub session_id: SpeechSessionId,
    pub failure: SessionFailure,
}

/// `inference.duplex.AdapterNotFound`: no adapter carries the identity the session named. Only a
/// store holding the adapters decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterNotFound {
    pub adapter_id: AdapterId,
}

impl fmt::Display for AdapterNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no adapter carries the identity {}", self.adapter_id)
    }
}

impl Error for AdapterNotFound {}

/// `inference.duplex.NonPositiveIdleLimit`: the liveness bound must be at least one millisecond.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonPositiveIdleLimit {
    pub submitted: i64,
}

impl fmt::Display for NonPositiveIdleLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the liveness bound must be at least one millisecond, got {}",
            self.submitted
        )
    }
}

impl Error for NonPositiveIdleLimit {}

/// `inference.duplex.FormatOutOfRange`: a format or a frame lies outside the range
/// `inference-audio` declares (sample rate 1 to `u32::MAX`, channels 1 to `u16::MAX`, sample count
/// within `u32`, position within the ESS `Integer`). An [`AudioFormat`] or [`AudioFrame`] cannot
/// hold such a value, so only a reader of the wire meets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatOutOfRange;

impl fmt::Display for FormatOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the format or frame lies outside the audio vocabulary's range")
    }
}

impl Error for FormatOutOfRange {}

/// `inference.duplex.SpeechSessionNotFound`: no session carries the identity. Only a store
/// holding the sessions decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechSessionNotFound {
    pub session_id: SpeechSessionId,
}

impl fmt::Display for SpeechSessionNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no session carries the identity {}", self.session_id)
    }
}

impl Error for SpeechSessionNotFound {}

/// `inference.duplex.SpeechSessionStateConflict`: the session is not in a state the command acts
/// in. `current` is the state it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechSessionStateConflict {
    pub current: SpeechSessionState,
}

impl fmt::Display for SpeechSessionStateConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the session is {:?}, which this command does not act in",
            self.current
        )
    }
}

impl Error for SpeechSessionStateConflict {}

/// `inference.duplex.ResponseNotFound`: no response carries the identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseNotFound {
    pub response_id: ResponseId,
}

impl fmt::Display for ResponseNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no response carries the identity {}", self.response_id)
    }
}

impl Error for ResponseNotFound {}

/// `inference.duplex.ResponseStateConflict`: the response is not in a state the command acts in.
/// `current` is the state it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseStateConflict {
    pub current: ResponseState,
}

impl fmt::Display for ResponseStateConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the response is {:?}, which this command does not act in",
            self.current
        )
    }
}

impl Error for ResponseStateConflict {}

/// `inference.duplex.ResponseIdReused`: a response of the session already carries the identity
/// an adapter issued; nothing was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseIdReused {
    pub response_id: ResponseId,
}

impl fmt::Display for ResponseIdReused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a response already carries the identity {}",
            self.response_id
        )
    }
}

impl Error for ResponseIdReused {}

/// `inference.duplex.SpeechCallIdReused`: a call of the session already carries the identity the
/// provider gave; nothing was proposed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechCallIdReused {
    pub call_id: SpeechCallId,
}

impl fmt::Display for SpeechCallIdReused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a call already carries the identity {}", self.call_id)
    }
}

impl Error for SpeechCallIdReused {}

/// `inference.duplex.SpeechToolCallStateConflict`: the call is not in a state the command moves
/// it from. `current` is the state it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechToolCallStateConflict {
    pub current: SpeechToolCallState,
}

impl fmt::Display for SpeechToolCallStateConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the call is {:?}, which this command does not move it from",
            self.current
        )
    }
}

impl Error for SpeechToolCallStateConflict {}

inference_core::uuid_newtype!(
    /// `inference.duplex.ReceivedEventId`: identifies a [`ReceivedSessionEvent`].
    ReceivedEventId
);

/// `inference.duplex.ReceivedEventKind`: what a session received from its provider. Serialized as
/// written in ESS (`input_transcript`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceivedEventKind {
    /// Usage of a response, a transcription or the session.
    Usage,
    /// The transcript of the input.
    InputTranscript,
    /// Output audio of a response.
    OutputAudio,
    /// Output transcript of a response.
    OutputTranscript,
    /// A tool call a response proposed.
    ToolCall,
    /// The end of a response.
    ResponseEnd,
}

impl ReceivedEventKind {
    /// Every kind, in the order `ess/domains/duplex.yaml` lists them.
    pub const ALL: [Self; 6] = [
        Self::Usage,
        Self::InputTranscript,
        Self::OutputAudio,
        Self::OutputTranscript,
        Self::ToolCall,
        Self::ResponseEnd,
    ];
}

/// `inference.duplex.SessionEnd`: the end event a received event was delivered before. Serialized
/// as written in ESS (`closed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEnd {
    /// `SpeechSessionClosed`.
    Closed,
    /// `SpeechSessionFailed`.
    Failed,
}

impl SessionEnd {
    /// Every end, in the order `ess/domains/duplex.yaml` lists them.
    pub const ALL: [Self; 2] = [Self::Closed, Self::Failed];
}

/// `inference.duplex.ReceivedSessionEvent.State`. Serialized as written in ESS (`Received`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReceivedSessionEventState {
    /// Received from the provider; the caller does not have it yet.
    Received,
    /// The caller has it.
    Delivered,
}

impl ReceivedSessionEventState {
    /// Every state, in the order `ess/domains/duplex.yaml` lists them.
    pub const ALL: [Self; 2] = [Self::Received, Self::Delivered];
}

/// `inference.duplex.SessionEventReceived`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEventReceived {
    pub received_id: ReceivedEventId,
    pub session_id: SpeechSessionId,
    pub kind: ReceivedEventKind,
    pub sequence: i64,
}

/// `inference.duplex.SessionEventDelivered`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEventDelivered {
    pub received_id: ReceivedEventId,
}

/// `inference.duplex.ReceivedSessionEventStateConflict`: the received event is not held for the
/// caller. `current` is the state it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceivedSessionEventStateConflict {
    pub current: ReceivedSessionEventState,
}

impl fmt::Display for ReceivedSessionEventStateConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the received event is {:?}, which this command does not deliver it from",
            self.current
        )
    }
}

impl Error for ReceivedSessionEventStateConflict {}

/// Why `inference.duplex.DeliverSessionEvent` moved nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliverSessionEventError {
    /// No received event carries the identity: the `wrong-state` outcome with no state.
    Unknown { received_id: ReceivedEventId },
    /// The `wrong-state` outcome.
    Conflict(ReceivedSessionEventStateConflict),
}

impl fmt::Display for DeliverSessionEventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { received_id } => {
                write!(f, "no received event carries the identity {received_id}")
            }
            Self::Conflict(conflict) => conflict.fmt(f),
        }
    }
}

impl Error for DeliverSessionEventError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A count up to `i64::MAX` is recorded as sent; one above it is unknown, never a clamped
    /// number.
    #[test]
    fn a_count_above_i64_max_is_unknown() {
        let kind = QuantityKind::InputTextTokens;
        let largest = u64::try_from(i64::MAX).expect("i64::MAX fits a u64");
        assert_eq!(
            reported(kind, Some(largest)),
            Quantity::provider_reported(kind, i64::MAX)
        );
        assert_eq!(reported(kind, Some(largest + 1)), Quantity::unknown(kind));
        assert_eq!(reported(kind, Some(u64::MAX)), Quantity::unknown(kind));
        assert_eq!(reported(kind, None), Quantity::unknown(kind));
    }
}
