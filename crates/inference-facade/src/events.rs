//! The session's one event vocabulary, [`SessionEvent`], and the trait through which a run's
//! payload converts into it, [`SessionPayload`].
//!
//! An application receives every event of a session as a [`SessionEvent`], whatever topology the
//! session runs: the variants group the event families of `docs/design/inference.md` §6.4
//! (330-345), each variant an event `ess/` declares or a §6.4 name, carrying the contract type
//! that declares it:
//!
//! | family | variants |
//! |---|---|
//! | session lifecycle | `SessionOpened`, `RunAttached`, `SessionReady`, `InputSent`, `InputCommitted`, `SessionInterrupted`, `PipelineSwitched`, `SessionDraining`, `SessionClosed`, `HistoryRecorded` |
//! | transcript | `TranscriptHypothesis`, `TranscriptRevision`, `TranscriptCommitted` |
//! | text generation | `GenerationStarted`, `TextDelta`, `TextSegmentCommitted`, `GenerationCompleted`, `GenerationSuperseded`, `GenerationCancelled` |
//! | synthesis audio | `AudioChunk`, `SynthesisCompleted`, `AudioPlayed` |
//! | tools | `ToolCallProposed`, `ToolCallAuthorized`, `ToolCallRejected`, `ToolCallStarted`, `ToolCallResult`, `ToolCallFailed` |
//! | usage | `UsageRecorded` |
//! | run | `RunCompleted`, `RunCancelled`, `RunFailed` |
//!
//! A topology emits the subset it produces. The session lifecycle events are the session state
//! machine's ([`crate::session`]); the run events are the session's run ending; every other
//! variant comes from the payload of an envelope the run sends on one of its egresses, converted
//! by [`SessionPayload::session_event`]. A payload event no variant carries is not delivered.

use inference_audio::AudioFrame;
use inference_core::envelope::ControlKind;
use inference_core::generation::{
    GenerationCancelled, GenerationCompleted, GenerationStarted, GenerationSuperseded,
};
use inference_core::usage::UsageRecorded;
use inference_runtime::run::{RunCancelled, RunCompleted, RunFailed};
use inference_text::{
    AudioPlayed, SynthesisCompleted, TextDelta, TextSegmentCommitted, TranscriptCommitted,
    TranscriptHypothesis, TranscriptRevision,
};
use inference_tools::{
    ToolCallAuthorized, ToolCallFailed, ToolCallProposed, ToolCallRejected, ToolCallResult,
    ToolCallStarted,
};

use crate::history::HistoryRecorded;
use crate::session::{
    InputCommitted, InputSent, PipelineSwitched, RunAttached, SessionClosed, SessionDraining,
    SessionInterrupted, SessionOpened, SessionReady,
};

/// An event of a session, as the application receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// `inference.session.SessionOpened`.
    SessionOpened(SessionOpened),
    /// `inference.session.RunAttached`.
    RunAttached(RunAttached),
    /// `inference.session.SessionReady`.
    SessionReady(SessionReady),
    /// `inference.session.InputSent`.
    InputSent(InputSent),
    /// `inference.session.InputCommitted`.
    InputCommitted(InputCommitted),
    /// `inference.session.SessionInterrupted`: the session cut its active generation.
    SessionInterrupted(SessionInterrupted),
    /// `inference.session.PipelineSwitched`: the session moved over to another pipeline; its next
    /// run is announced by `RunAttached`.
    PipelineSwitched(PipelineSwitched),
    /// `inference.session.SessionDraining`.
    SessionDraining(SessionDraining),
    /// `inference.session.SessionClosed`.
    SessionClosed(SessionClosed),
    /// `inference.session.HistoryRecorded`: an entry joined the session's history. The state
    /// machine records it; a live session does not deliver it, its history is read through the
    /// view.
    HistoryRecorded(HistoryRecorded),
    /// `inference.stream.TranscriptHypothesis`.
    TranscriptHypothesis(TranscriptHypothesis),
    /// `inference.stream.TranscriptRevision`.
    TranscriptRevision(TranscriptRevision),
    /// `inference.stream.TranscriptCommitted`.
    TranscriptCommitted(TranscriptCommitted),
    /// `inference.stream.GenerationStarted`.
    GenerationStarted(GenerationStarted),
    /// `inference.stream.TextDelta`.
    TextDelta(TextDelta),
    /// `inference.stream.TextSegmentCommitted`.
    TextSegmentCommitted(TextSegmentCommitted),
    /// `inference.stream.GenerationCompleted`.
    GenerationCompleted(GenerationCompleted),
    /// `inference.stream.GenerationSuperseded`.
    GenerationSuperseded(GenerationSuperseded),
    /// `inference.stream.GenerationCancelled`.
    GenerationCancelled(GenerationCancelled),
    /// §6.4 `AudioChunk`: a frame of synthesized audio.
    AudioChunk(AudioFrame),
    /// `inference.stream.SynthesisCompleted`.
    SynthesisCompleted(SynthesisCompleted),
    /// `inference.stream.AudioPlayed`.
    AudioPlayed(AudioPlayed),
    /// `inference.tools.ToolCallProposed`.
    ToolCallProposed(ToolCallProposed),
    /// `inference.tools.ToolCallAuthorized`.
    ToolCallAuthorized(ToolCallAuthorized),
    /// `inference.tools.ToolCallRejected`.
    ToolCallRejected(ToolCallRejected),
    /// `inference.tools.ToolCallStarted`.
    ToolCallStarted(ToolCallStarted),
    /// `inference.tools.ToolCallResult`.
    ToolCallResult(ToolCallResult),
    /// `inference.tools.ToolCallFailed`.
    ToolCallFailed(ToolCallFailed),
    /// `inference.usage.UsageRecorded`: the usage of one model call of the session's run, sent on
    /// an egress the pipeline exposes a model node's `usage` port on. An interrupt does not cut
    /// it, and the session adds it to its usage view ([`crate::usage`]).
    UsageRecorded(UsageRecorded),
    /// `inference.graph.RunCompleted`: the session's run drained and completed.
    RunCompleted(RunCompleted),
    /// `inference.graph.RunCancelled`: the session's run was cancelled.
    RunCancelled(RunCancelled),
    /// `inference.graph.RunFailed`: the session's run failed.
    RunFailed(RunFailed),
}

/// The payload of the envelopes a session's run carries, as the session reads and writes it.
///
/// A session writes control events into the run's ingresses (`commit` for `commit_input`, `stop`
/// when it closes) and converts what the run sends on its egresses into [`SessionEvent`]s.
pub trait SessionPayload: Send + 'static {
    /// The payload of a control event of `kind`.
    fn control(kind: ControlKind) -> Self;

    /// The session event this payload is delivered as; none when no variant of [`SessionEvent`]
    /// carries it, a control event among them.
    fn session_event(&self) -> Option<SessionEvent>;
}
