//! The contract a native speech model implements: its capabilities, the configuration a session
//! is opened from, the operations of an open session and the events it yields.

use std::error::Error;
use std::fmt;
use std::time::Duration;

use inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_core::model::Adapter;
use inference_core::usage::UsageRecorded;
use inference_tools::{Json, Tool, ToolId};

use crate::{
    InputAccepted, InputCommitted, InputTranscribed, NonPositiveIdleLimit, OutputAudio,
    OutputTranscriptDelta, OutputTruncated, ResponseCancelled, ResponseCompleted, ResponseCreated,
    ResponseFailed, ResponseId, ResponseIdReused, ResponseNotFound, ResponseStateConflict,
    ResponseUsageRecorded, SessionFailure, SessionLedger, SpeechCallId, SpeechCallIdReused,
    SpeechSessionClosed, SpeechSessionFailed, SpeechSessionStateConflict, SpeechStarted,
    SpeechStopped, SpeechToolCallAnswered, SpeechToolCallProposed, SpeechToolCallStateConflict,
    TranscriptionUsageRecorded, TurnDetection, TurnSensitivity,
};

/// What a session is opened with.
///
/// Frames pass in `input_format` and `output_format` as they are: the session never resamples or
/// transcodes, so G.711 at 8 kHz in 20 ms frames reaches a model that takes it untouched, and a
/// model that does not take a format refuses it at open. `auto_response` on means every commit of
/// the input, the caller's or the provider's turn detection's, is a request for a response the
/// provider creates by itself; off, the caller creates each. `auto_interrupt` off means detected
/// speech does not cancel a response by itself, so barge-in is the caller's decision.
/// `turn_tuning` tunes the provider's turn detection. `idle_limit` is the liveness bound: a
/// provider that sends nothing for that long while the caller awaits it fails the session as
/// stalled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    pub voice: String,
    pub instructions: String,
    pub input_format: AudioFormat,
    pub output_format: AudioFormat,
    pub turn_detection: TurnDetection,
    pub turn_tuning: TurnTuning,
    pub auto_response: bool,
    pub auto_interrupt: bool,
    pub tools: Vec<Tool>,
    pub idle_limit: Duration,
}

impl SessionConfig {
    /// The tools of the configuration by identity, as `OpenSpeechSession` records them.
    #[must_use]
    pub fn tool_ids(&self) -> Vec<ToolId> {
        self.tools.iter().map(Tool::tool_id).collect()
    }
}

/// Tuning of the provider's turn detection, each `None` for the provider's default: the silence
/// that ends a turn, how readily a turn ends, and the audio kept before detected speech. Tuning
/// needs turn detection on; a model honours only the tuning its [`TuningSupport`] names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnTuning {
    pub silence: Option<Duration>,
    pub sensitivity: Option<TurnSensitivity>,
    pub prefix_padding: Option<Duration>,
}

/// Which [`TurnTuning`] a model honours.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TuningSupport {
    pub silence: bool,
    pub sensitivity: bool,
    pub prefix_padding: bool,
}

impl TuningSupport {
    /// Every tuning honoured.
    pub const ALL: Self = Self {
        silence: true,
        sensitivity: true,
        prefix_padding: true,
    };

    /// No tuning honoured.
    pub const NONE: Self = Self {
        silence: false,
        sensitivity: false,
        prefix_padding: false,
    };
}

/// One option of [`TurnTuning`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TuningOption {
    Silence,
    Sensitivity,
    PrefixPadding,
}

/// What a native speech model takes. Each telephony option is explicit, and a model that lacks
/// one refuses a configuration or an operation that needs it with [`Unsupported`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechCapabilities {
    /// The formats the model takes input audio in, unresampled.
    pub input_formats: Vec<AudioFormat>,
    /// The formats the model gives output audio in.
    pub output_formats: Vec<AudioFormat>,
    /// The turn detection modes the model offers.
    pub turn_detection: Vec<TurnDetection>,
    /// The turn tuning the model honours.
    pub turn_tuning: TuningSupport,
    /// Whether the caller may commit the input itself.
    pub manual_commit: bool,
    /// Whether the caller may create responses itself, and so turn `auto_response` off.
    pub manual_response: bool,
    /// Whether detected speech may leave a response running, and so `auto_interrupt` be off.
    pub manual_interrupt: bool,
    /// Whether the caller may cancel a response in progress itself.
    pub client_cancel: bool,
    /// Whether the caller may truncate output audio at the sample the listener heard.
    pub truncation: bool,
    /// Whether the model proposes tool calls and takes their results.
    pub tool_calling: bool,
}

impl SpeechCapabilities {
    /// Whether the model takes `config`, the first option it does not take named.
    ///
    /// # Errors
    ///
    /// [`Unsupported`] naming, in this order, an input format, an output format, a turn detection
    /// mode, a turn tuning (any tuning with turn detection off), `auto_response` off,
    /// `auto_interrupt` off (which needs both [`Unsupported::ManualInterrupt`] and
    /// [`Unsupported::ClientCancel`], since barge-in is then the caller's to do), or tools, that
    /// the model does not take.
    pub fn admit(&self, config: &SessionConfig) -> Result<(), Unsupported> {
        if !self.input_formats.contains(&config.input_format) {
            return Err(Unsupported::InputFormat(config.input_format));
        }
        if !self.output_formats.contains(&config.output_format) {
            return Err(Unsupported::OutputFormat(config.output_format));
        }
        if !self.turn_detection.contains(&config.turn_detection) {
            return Err(Unsupported::TurnDetection(config.turn_detection));
        }
        let tuning = config.turn_tuning;
        let detected = config.turn_detection != TurnDetection::Disabled;
        for (set, honoured, option) in [
            (
                tuning.silence.is_some(),
                self.turn_tuning.silence,
                TuningOption::Silence,
            ),
            (
                tuning.sensitivity.is_some(),
                self.turn_tuning.sensitivity,
                TuningOption::Sensitivity,
            ),
            (
                tuning.prefix_padding.is_some(),
                self.turn_tuning.prefix_padding,
                TuningOption::PrefixPadding,
            ),
        ] {
            if set && !(honoured && detected) {
                return Err(Unsupported::TurnTuning(option));
            }
        }
        if !config.auto_response && !self.manual_response {
            return Err(Unsupported::ManualResponse);
        }
        if !config.auto_interrupt && !self.manual_interrupt {
            return Err(Unsupported::ManualInterrupt);
        }
        if !config.auto_interrupt && !self.client_cancel {
            return Err(Unsupported::ClientCancel);
        }
        if !config.tools.is_empty() && !self.tool_calling {
            return Err(Unsupported::ToolCalling);
        }
        Ok(())
    }
}

/// An option or operation a model does not take; nothing was opened or done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    InputFormat(AudioFormat),
    OutputFormat(AudioFormat),
    TurnDetection(TurnDetection),
    TurnTuning(TuningOption),
    ManualCommit,
    ManualResponse,
    ManualInterrupt,
    ClientCancel,
    Truncation,
    ToolCalling,
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputFormat(format) => write!(f, "the model takes no input audio in {format:?}"),
            Self::OutputFormat(format) => {
                write!(f, "the model gives no output audio in {format:?}")
            }
            Self::TurnDetection(mode) => write!(f, "the model offers no {mode:?} turn detection"),
            Self::TurnTuning(option) => write!(
                f,
                "the model honours no {option:?} tuning of the turn detection configured"
            ),
            Self::ManualCommit => f.write_str("the model takes no commit of the input"),
            Self::ManualResponse => f.write_str("the model creates every response itself"),
            Self::ManualInterrupt => f.write_str("detected speech always interrupts the model"),
            Self::ClientCancel => f.write_str("the model takes no cancellation from the caller"),
            Self::Truncation => f.write_str("the model truncates no output audio"),
            Self::ToolCalling => f.write_str("the model proposes no tool calls"),
        }
    }
}

impl Error for Unsupported {}

/// Why a session was not opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// The model does not take an option of the configuration.
    Unsupported(Unsupported),
    /// The liveness bound is below one millisecond.
    NoLivenessBound(NonPositiveIdleLimit),
    /// The provider refused the session or could not be reached.
    Failed(SessionFailure),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(error) => fmt::Display::fmt(error, f),
            Self::NoLivenessBound(error) => fmt::Display::fmt(error, f),
            Self::Failed(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for OpenError {}

/// Why a session operation, or an event an adapter recorded, changed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The session is closed or failed.
    NotOpen(SpeechSessionStateConflict),
    /// The frame is not in the session's format for its direction; the session never converts.
    FormatMismatch {
        expected: AudioFormat,
        found: AudioFormat,
    },
    /// A response is already in progress; one response runs at a time.
    ResponseInProgress { response_id: ResponseId },
    /// A response already carries the identity the adapter issued.
    ResponseIdReused(ResponseIdReused),
    /// No response of the session carries the identity.
    UnknownResponse(ResponseNotFound),
    /// The response is no longer in progress; its late output is dropped.
    ResponseNotInProgress(ResponseStateConflict),
    /// Truncation beyond the audio the response emitted, in samples of its own audio.
    BeyondOutput {
        heard: SampleClockPosition,
        emitted: u64,
    },
    /// A call already carries the identity the provider gave.
    CallIdReused(SpeechCallIdReused),
    /// No call of the session carries the identity.
    UnknownCall { call_id: SpeechCallId },
    /// The call was already answered.
    CallConflict(SpeechToolCallStateConflict),
    /// The session offers no tool of that name.
    ToolNotOffered { name: String },
    /// The model does not take the operation.
    Unsupported(Unsupported),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOpen(error) => fmt::Display::fmt(error, f),
            Self::FormatMismatch { expected, found } => write!(
                f,
                "the frame is in {found:?}, and the session takes {expected:?}"
            ),
            Self::ResponseInProgress { response_id } => {
                write!(f, "the response {response_id} is still in progress")
            }
            Self::ResponseIdReused(error) => fmt::Display::fmt(error, f),
            Self::UnknownResponse(error) => fmt::Display::fmt(error, f),
            Self::ResponseNotInProgress(error) => fmt::Display::fmt(error, f),
            Self::BeyondOutput { heard, emitted } => write!(
                f,
                "sample {} was not emitted; the response emitted {emitted}",
                heard.get()
            ),
            Self::CallIdReused(error) => fmt::Display::fmt(error, f),
            Self::UnknownCall { call_id } => write!(f, "no call carries the identity {call_id}"),
            Self::CallConflict(error) => fmt::Display::fmt(error, f),
            Self::ToolNotOffered { name } => write!(f, "the session offers no tool `{name}`"),
            Self::Unsupported(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for SessionError {}

/// An event of an open session, in the order its [`SessionLedger`] recorded it: what the
/// provider sent and what the caller did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    InputAccepted(InputAccepted),
    InputCommitted(InputCommitted),
    SpeechStarted(SpeechStarted),
    SpeechStopped(SpeechStopped),
    InputTranscribed(InputTranscribed),
    TranscriptionUsageRecorded(TranscriptionUsageRecorded),
    ResponseCreated(ResponseCreated),
    OutputAudio(OutputAudio),
    OutputTranscriptDelta(OutputTranscriptDelta),
    ResponseCompleted(ResponseCompleted),
    ResponseCancelled(ResponseCancelled),
    ResponseFailed(ResponseFailed),
    OutputTruncated(OutputTruncated),
    ResponseUsageRecorded(ResponseUsageRecorded),
    SpeechToolCallProposed(SpeechToolCallProposed),
    SpeechToolCallAnswered(SpeechToolCallAnswered),
    SpeechSessionClosed(SpeechSessionClosed),
    SpeechSessionFailed(SpeechSessionFailed),
    /// `inference.usage.UsageRecorded`: the usage record of a response, of a transcription or of
    /// the session as a whole, as [`SessionLedger`] decides when.
    UsageRecorded(UsageRecorded),
}

/// A native speech model: one multiport node that opens sessions.
pub trait NativeSpeechModel {
    /// The handle of one open session.
    type Session: SpeechSessionHandle;

    /// The adapter record this model was declared with, the capabilities it claims among it.
    fn adapter(&self) -> &Adapter;

    /// The options and operations the model takes.
    fn capabilities(&self) -> &SpeechCapabilities;

    /// Opens a session from `config`.
    ///
    /// # Errors
    ///
    /// [`OpenError::Unsupported`] for an option outside [`NativeSpeechModel::capabilities`],
    /// [`OpenError::NoLivenessBound`] for a bound below one millisecond, [`OpenError::Failed`]
    /// when the provider refused or could not be reached; nothing is opened.
    fn open(&mut self, config: SessionConfig) -> Result<Self::Session, OpenError>;
}

/// One open session. Every operation is checked by the session's [`SessionLedger`] before it
/// reaches the provider, and its event is delivered by [`SpeechSessionHandle::next_event`] in
/// order with the provider's.
pub trait SpeechSessionHandle {
    /// The session, its responses and calls, as they stand now.
    fn ledger(&self) -> &SessionLedger;

    /// Sends a frame of input audio in the session's input format.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`] or [`SessionError::FormatMismatch`]; nothing is sent.
    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError>;

    /// Ends the user turn with the input sent so far.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], or [`SessionError::Unsupported`] when the model takes no commit.
    fn commit_input(&mut self) -> Result<(), SessionError>;

    /// Asks for a response; its [`ResponseCreated`] carries the new id.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::ResponseInProgress`], or
    /// [`SessionError::Unsupported`] when the model creates every response itself.
    fn create_response(&mut self) -> Result<(), SessionError>;

    /// Cancels the response in progress: barge-in decided by the caller. Nothing more of it is
    /// delivered from the moment this returns.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::Unsupported`] with
    /// [`Unsupported::ClientCancel`] when the model takes no cancellation from the caller,
    /// [`SessionError::UnknownResponse`] or [`SessionError::ResponseNotInProgress`].
    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError>;

    /// Cuts a response's audio at `heard`, a sample of its own audio, the position the listener
    /// actually heard, so the provider keeps nothing the listener did not hear.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::UnknownResponse`],
    /// [`SessionError::BeyondOutput`], or [`SessionError::Unsupported`] when the model truncates
    /// nothing.
    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError>;

    /// Answers a proposed call with its JSON result, by call id.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::UnknownCall`] or [`SessionError::CallConflict`].
    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError>;

    /// Closes the session.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`] when it is already closed or failed.
    fn close(&mut self) -> Result<(), SessionError>;

    /// The next event, waiting at most `wait` for the provider. `None` means nothing arrived in
    /// `wait` and the session stands as it was. A provider silent beyond the liveness bound
    /// yields [`SpeechSessionFailed`] with [`SessionFailureKind::Stalled`], and a closed transport
    /// yields it with [`SessionFailureKind::TransportClosed`] at once, never silence.
    ///
    /// [`SessionFailureKind::Stalled`]: crate::SessionFailureKind::Stalled
    /// [`SessionFailureKind::TransportClosed`]: crate::SessionFailureKind::TransportClosed
    fn next_event(&mut self, wait: Duration) -> Option<SessionEvent>;
}
