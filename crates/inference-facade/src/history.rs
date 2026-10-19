//! A session's history: the ordered committed facts of its conversation
//! (`inference.session.HistoryEntry` of `ess/domains/session.yaml`).
//!
//! Conversation state is kept apart from model inference state and media state
//! (`docs/design/inference.md` 735-741): an entry is one committed fact the session accepted or
//! delivered, the committed input text of a user turn, the text of a committed output segment or
//! the outcome of a tool call, and never a hypothesis, a tentative segment, a delta or audio. Each
//! entry names the run the session held when it was recorded and its position in the session,
//! from 1, and it lives as long as the session row does, past the run that produced it, so that
//! committed history can be reconstructed or replayed (990, 1008). History is held in memory and
//! read through the view `inference.session.History` ([`crate::session::Sessions::history_view`]);
//! it is not persisted and not fed into a run's model context.

use std::error::Error;
use std::fmt;

use inference_core::id::{RunId, SessionId};
use inference_tools::ToolCallId;

use crate::session::{SessionNotFound, SessionStateConflict};

inference_core::uuid_newtype!(
    /// `inference.session.HistoryEntryId`: identifies a [`HistoryEntry`], written and read as the
    /// identifiers of `inference_core::id` are.
    HistoryEntryId
);

/// `inference.session.HistoryKind`: what one committed fact of the conversation is. Serialized as
/// written in ESS (`user_turn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoryKind {
    /// The committed input text of one user turn.
    UserTurn,
    /// The text of one committed output segment.
    AssistantSegment,
    /// The outcome of one tool call.
    ToolResult,
}

impl HistoryKind {
    /// Every kind, in the order `ess/domains/session.yaml` lists them.
    pub const ALL: [Self; 3] = [Self::UserTurn, Self::AssistantSegment, Self::ToolResult];

    /// The kind as ESS writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserTurn => "user_turn",
            Self::AssistantSegment => "assistant_segment",
            Self::ToolResult => "tool_result",
        }
    }
}

impl fmt::Display for HistoryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `inference.session.HistoryEntry`: one committed fact of a session's conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub(crate) entry_id: HistoryEntryId,
    pub(crate) session_id: SessionId,
    pub(crate) run_id: RunId,
    pub(crate) position: i64,
    pub(crate) kind: HistoryKind,
    pub(crate) text: String,
    pub(crate) call_id: Option<ToolCallId>,
}

impl HistoryEntry {
    #[must_use]
    pub const fn entry_id(&self) -> HistoryEntryId {
        self.entry_id
    }

    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// The run the session held when the entry was recorded.
    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    /// The entry's place in its session's history, from 1.
    #[must_use]
    pub const fn position(&self) -> i64 {
        self.position
    }

    #[must_use]
    pub const fn kind(&self) -> HistoryKind {
        self.kind
    }

    /// The committed text of a user turn or an assistant segment; the outcome of a tool call.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The tool call a [`HistoryKind::ToolResult`] entry records the outcome of.
    #[must_use]
    pub const fn call_id(&self) -> Option<ToolCallId> {
        self.call_id
    }
}

/// `inference.session.RecordHistory`: the input that records one committed fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordHistory {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub position: i64,
    pub kind: HistoryKind,
    pub text: String,
    pub call_id: Option<ToolCallId>,
}

/// `inference.session.HistoryRecorded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryRecorded {
    pub entry_id: HistoryEntryId,
    pub session_id: SessionId,
    pub run_id: RunId,
    pub position: i64,
    pub kind: HistoryKind,
}

/// Why `inference.session.RecordHistory` recorded nothing, in the order
/// [`crate::session::Sessions::record_history`] decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordHistoryError {
    /// The `no-such-session` outcome.
    NoSuchSession(SessionNotFound),
    /// The `closed` outcome: the session is Closed.
    Closed(SessionStateConflict),
}

impl fmt::Display for RecordHistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchSession(error) => fmt::Display::fmt(error, f),
            Self::Closed(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for RecordHistoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NoSuchSession(error) => Some(error),
            Self::Closed(error) => Some(error),
        }
    }
}
