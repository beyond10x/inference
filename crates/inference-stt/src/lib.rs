//! The STT contract of the inference framework.
//!
//! `inference-stt` is the recognition contract over `inference-audio`'s [`AudioFrame`] and
//! `inference-text`'s [`Span`]. A [`Recogniser`] is an `inference.model.Adapter`: it carries the
//! [`Adapter`] record it was declared with, and [`Recogniser::start`] begins one recognition for a
//! [`RecognitionRequest`], which names the run and turn its spans belong to and the stream, node
//! and port its events are ordered on. The returned [`RecognitionHandle`] takes audio frames and
//! an end-of-input marker, and yields the recognition's events, the inbound commands of
//! `ess/domains/stream.yaml`: `TranscriptHypothesis`, `TranscriptRevision` and
//! `TranscriptCommitted`, each in the `inference.stream.EventEnvelope` that places it on the
//! request's stream with a per-stream sequence from the request's `first_sequence`, read on
//! [`Timebase::AudioFrameTime`]. The contract does not remember streams: the stream's owner
//! tracks the next sequence of each and hands it to the recognition that continues it. A
//! sequence is 1 or more, so a request continuing a stream below 1 is refused with
//! [`NonPositiveSequence`], and a recognition that has issued sequence `i64::MAX` refuses every
//! further event with [`SequenceExhausted`].
//!
//! [`Recogniser::start`] refuses a start past the maximum concurrency the adapter record declares
//! with [`ConcurrencyLimitReached`]; a recognition runs until its handle is dropped.
//!
//! Incremental input and output streaming are distinct capabilities
//! (`inference.model.Capability`), read from the adapter record and never implied by the shape of
//! the API: every recogniser takes frames and an end-of-input marker the same way, and batch
//! transcription is what remains when it claims neither. What the contract can observe, it
//! decides here:
//!
//! * A recogniser ingests incrementally when it takes offered frames before input has ended.
//!   [`TranscriptOutput::take_input`] gives frames up before the end of input only when the
//!   adapter claims [`Capability::IncrementalInput`], and refuses otherwise with
//!   [`CapabilityNotClaimed`] naming it. So an adapter that does not claim incremental input
//!   cannot take input early. The contract cannot see how an adapter that claims it processes
//!   what it takes: a chunked offline recogniser running complete chunks through independent
//!   calls must not claim incremental recognition (`docs/design/inference.md:362`), and catching
//!   one that does is the work of the conformance checks of a real adapter, not of this crate.
//! * A recogniser streams output when a span reaches the stream before input has ended.
//!   [`TranscriptOutput::hypothesis`] emits a span before the end of input only when the adapter
//!   claims [`Capability::OutputStreaming`], and refuses otherwise with [`CapabilityNotClaimed`]
//!   naming it.
//! * A Hypothesis span is provisional and a Committed span immutable: a revision of a committed
//!   span is refused with [`CommittedSpanImmutable`] and its text stays as it was.
//!
//! [`TranscriptOutput`] is the bookkeeping every implementation shares: the frames offered, the
//! spans emitted, moved only through the transitions of `inference-text`, and the sequence of the
//! stream. The crate carries no concrete model, backend or provider; the scripted mock recogniser
//! is `inference-testkit`'s, behind its `stt` feature. Languages, audio formats, sample rates and
//! channel limits as adapter descriptors, speech activity, endpointing, timing and diarization
//! metadata are not part of the contract yet.
//!
//! The vocabulary crates the contract is written in are re-exported, so that a consumer of speech
//! recognition alone depends on this crate and one adapter.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;

pub use inference_audio;
pub use inference_core;
pub use inference_text;

use inference_audio::AudioFrame;
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{Adapter, Capability};
use inference_text::{
    CommittedSpanImmutable, OpenTurn, ReviseSpan, Span, SpanId, SpanNotFound, SpanStateConflict,
    TranscriptCommitted, TranscriptHypothesis, TranscriptRevision, Turn, TurnId,
};

/// The revision of this contract, for the `contract_revision` an adapter implementing it
/// declares.
pub const CONTRACT_REVISION: &str = env!("CARGO_PKG_VERSION");

/// The `schema_version` of every envelope this contract writes.
pub const ENVELOPE_SCHEMA_VERSION: i64 = 1;

/// A request for one recognition: the run and turn its spans belong to, the stream, producing
/// node and port its events are ordered on, and `first_sequence`, the sequence its first event
/// takes on that stream. No store of runs or turns is consulted: the turn is taken to be Open.
///
/// The contract does not remember streams. Several recognitions may share one stream, one turn
/// after another on one output port; the stream's owner, the graph runtime, tracks its next
/// sequence. Once the previous recognition has delivered its last event, the owner reads
/// [`TranscriptOutput::next_sequence`] from its handle's output, drops that handle, which ends
/// the recognition and frees its place under the adapter's maximum concurrency, and then starts
/// the next recognition with [`RecognitionRequest::continuing`] at that sequence. A recogniser
/// running at its maximum concurrency refuses the next start while the previous handle is still
/// held. `None` from `next_sequence` means the stream has no sequence left, and no recognition
/// continues it. [`RecognitionRequest::new`] starts a fresh stream at 1.
///
/// `first_sequence` is read through [`RecognitionRequest::first_sequence`] and set only by those
/// two constructors, so it is never below 1:
///
/// ```compile_fail,E0451
/// # use inference_stt::RecognitionRequest;
/// # use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
/// # use inference_stt::inference_text::TurnId;
/// let request = RecognitionRequest {
///     run_id: RunId::generate(),
///     turn_id: TurnId::generate(),
///     stream_id: StreamId::generate(),
///     first_sequence: 0,
///     node_id: NodeId::generate(),
///     port: "text_out".to_owned(),
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognitionRequest {
    pub run_id: RunId,
    pub turn_id: TurnId,
    pub stream_id: StreamId,
    first_sequence: i64,
    pub node_id: NodeId,
    pub port: String,
}

impl RecognitionRequest {
    /// A recognition on the fresh stream `stream_id`: its first event takes sequence 1.
    #[must_use]
    pub fn new(
        run_id: RunId,
        turn_id: TurnId,
        stream_id: StreamId,
        node_id: NodeId,
        port: impl Into<String>,
    ) -> Self {
        Self {
            run_id,
            turn_id,
            stream_id,
            first_sequence: 1,
            node_id,
            port: port.into(),
        }
    }

    /// A recognition continuing the stream `stream_id`, whose first event takes `next_sequence`,
    /// one past the last sequence the stream's owner issued on it.
    ///
    /// # Errors
    ///
    /// [`NonPositiveSequence`] carrying `next_sequence` when it is below 1, the first sequence of
    /// a stream; no request is made.
    pub fn continuing(
        run_id: RunId,
        turn_id: TurnId,
        stream_id: StreamId,
        next_sequence: i64,
        node_id: NodeId,
        port: impl Into<String>,
    ) -> Result<Self, NonPositiveSequence> {
        if next_sequence < 1 {
            return Err(NonPositiveSequence {
                submitted: next_sequence,
            });
        }
        Ok(Self {
            run_id,
            turn_id,
            stream_id,
            first_sequence: next_sequence,
            node_id,
            port: port.into(),
        })
    }

    /// The sequence the recognition's first event takes on the stream, 1 or more.
    #[must_use]
    pub const fn first_sequence(&self) -> i64 {
        self.first_sequence
    }
}

/// A stream's sequences start at 1, so a recognition continuing it at `submitted`, below 1, is
/// refused and no request is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonPositiveSequence {
    pub submitted: i64,
}

impl fmt::Display for NonPositiveSequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a stream's sequences start at 1, so a recognition cannot continue it at {}",
            self.submitted
        )
    }
}

impl Error for NonPositiveSequence {}

/// The recognition has issued sequence `i64::MAX` on the stream `stream_id`, so no further event
/// has a sequence; the move is refused and nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceExhausted {
    pub stream_id: StreamId,
}

impl fmt::Display for SequenceExhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "stream {} has issued its last sequence, so no further event is enveloped",
            self.stream_id
        )
    }
}

impl Error for SequenceExhausted {}

/// The recogniser already runs `max_concurrency` recognitions, the maximum concurrency its
/// adapter declares, so a further start is refused and nothing started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConcurrencyLimitReached {
    pub max_concurrency: i64,
}

impl fmt::Display for ConcurrencyLimitReached {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the recogniser already runs {} recognition(s), its declared maximum concurrency",
            self.max_concurrency
        )
    }
}

impl Error for ConcurrencyLimitReached {}

/// An event of one recognition, the payload of the envelope a [`RecognitionHandle`] yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEvent {
    /// `inference.stream.TranscriptHypothesis`: a provisional span exists.
    Hypothesis(TranscriptHypothesis),
    /// `inference.stream.TranscriptRevision`: a hypothesis now reads the revised text.
    Revision(TranscriptRevision),
    /// `inference.stream.TranscriptCommitted`: a span is immutable.
    Committed(TranscriptCommitted),
}

impl TranscriptEvent {
    /// The name `ess/domains/stream.yaml` declares the event under: the `kind` of its envelope.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Hypothesis(_) => "inference.stream.TranscriptHypothesis",
            Self::Revision(_) => "inference.stream.TranscriptRevision",
            Self::Committed(_) => "inference.stream.TranscriptCommitted",
        }
    }

    /// The span the event is about.
    #[must_use]
    pub const fn span_id(&self) -> SpanId {
        match self {
            Self::Hypothesis(event) => event.span_id,
            Self::Revision(event) => event.span_id,
            Self::Committed(event) => event.span_id,
        }
    }
}

/// The adapter does not claim `capability`, so the move that needs it is refused and nothing
/// changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityNotClaimed {
    pub capability: Capability,
}

impl fmt::Display for CapabilityNotClaimed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the adapter does not claim {:?}, so the move is refused before the end of input",
            self.capability
        )
    }
}

impl Error for CapabilityNotClaimed {}

/// Input has ended, so no further frame is taken and the end is not marked again; nothing
/// changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEnded;

impl fmt::Display for InputEnded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("input has ended, so nothing is taken")
    }
}

impl Error for InputEnded {}

/// Declares a refusal enum whose variants each wrap one error type of the same name, with its
/// `Display`, `Error::source` and a `From` for each variant.
macro_rules! refusal {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $( $(#[$variant_meta])* $variant($variant), )+
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    $( Self::$variant(error) => fmt::Display::fmt(error, f), )+
                }
            }
        }

        impl Error for $name {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                match self {
                    $( Self::$variant(error) => Some(error), )+
                }
            }
        }

        $(
            impl From<$variant> for $name {
                fn from(error: $variant) -> Self {
                    Self::$variant(error)
                }
            }
        )+
    };
}

refusal! {
    /// Why a hypothesis was refused; nothing changed either way.
    pub enum HypothesisRefused {
        /// Input has not ended and the adapter does not claim [`Capability::OutputStreaming`].
        CapabilityNotClaimed,
        /// The stream has issued its last sequence.
        SequenceExhausted,
    }
}

refusal! {
    /// Why a revision was refused; nothing changed either way.
    pub enum RevisionRefused {
        /// No span of the recognition carries the identity the revision named.
        SpanNotFound,
        /// The span is Committed, so its text is not rewritten.
        CommittedSpanImmutable,
        /// The stream has issued its last sequence.
        SequenceExhausted,
    }
}

refusal! {
    /// Why a commit was refused; nothing changed either way.
    pub enum CommitRefused {
        /// No span of the recognition carries the identity the commit named.
        SpanNotFound,
        /// The span is not a Hypothesis.
        SpanStateConflict,
        /// The stream has issued its last sequence.
        SequenceExhausted,
    }
}

/// One recognition: the frames offered and not yet taken, the spans emitted, and the sequence of
/// the request's stream, with the capabilities of the adapter producing it. Every span moves
/// through a transition of [`Span`], so a refused move changes nothing.
///
/// Each event it emits is enveloped on the request's stream at the next sequence, the request's
/// `first_sequence` first, with
/// the request's run, node and port, a fresh [`EventId`], no session, generation or cause, and a
/// timestamp on [`Timebase::AudioFrameTime`]: the sample clock position just past the last
/// sample taken so far, 0 before any. A span's own `sequence` is the sequence of the envelope of
/// its `TranscriptHypothesis`.
///
/// It is not `Clone`: a copy would issue the same sequences again on the same stream.
///
/// ```compile_fail
/// fn cloneable<T: Clone>() {}
/// cloneable::<inference_stt::TranscriptOutput>();
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct TranscriptOutput {
    capabilities: Vec<Capability>,
    request: RecognitionRequest,
    turn: Turn,
    offered: VecDeque<AudioFrame>,
    taken: usize,
    taken_through: i64,
    input_ended: bool,
    next_sequence: Option<i64>,
    spans: Vec<Span>,
}

impl TranscriptOutput {
    /// Starts the recognition `request` asks for on behalf of `adapter`, whose claims it keeps.
    #[must_use]
    pub fn start(adapter: &Adapter, request: RecognitionRequest) -> Self {
        let (turn, _) = Turn::open_as(
            request.turn_id,
            OpenTurn {
                run_id: request.run_id,
            },
        );
        Self {
            capabilities: adapter.capabilities().to_vec(),
            next_sequence: Some(request.first_sequence),
            request,
            turn,
            offered: VecDeque::new(),
            taken: 0,
            taken_through: 0,
            input_ended: false,
            spans: Vec::new(),
        }
    }

    /// Offers `frame` to the recognition, after the frames offered before it.
    ///
    /// # Errors
    ///
    /// [`InputEnded`] when input has ended; the frame is not offered.
    pub fn offer(&mut self, frame: AudioFrame) -> Result<(), InputEnded> {
        if self.input_ended {
            return Err(InputEnded);
        }
        self.offered.push_back(frame);
        Ok(())
    }

    /// Marks the end of input: every frame of the utterance has been offered.
    ///
    /// # Errors
    ///
    /// [`InputEnded`] when input has already ended.
    pub fn end_input(&mut self) -> Result<(), InputEnded> {
        if self.input_ended {
            return Err(InputEnded);
        }
        self.input_ended = true;
        Ok(())
    }

    /// Gives the frames offered and not yet taken to the implementation, in the order they were
    /// offered.
    ///
    /// # Errors
    ///
    /// [`CapabilityNotClaimed`] naming [`Capability::IncrementalInput`] when input has not ended
    /// and the adapter does not claim it; no frame is taken.
    pub fn take_input(&mut self) -> Result<Vec<AudioFrame>, CapabilityNotClaimed> {
        self.require_before_end(Capability::IncrementalInput)?;
        let frames: Vec<AudioFrame> = self.offered.drain(..).collect();
        for frame in &frames {
            let end = frame
                .position
                .get()
                .saturating_add(u64::from(frame.sample_count));
            self.taken_through = self
                .taken_through
                .max(i64::try_from(end).unwrap_or(i64::MAX));
        }
        self.taken += frames.len();
        Ok(frames)
    }

    /// Runs the `emitted` outcome of `inference.stream.EmitHypothesis` in the request's turn: adds
    /// a Hypothesis span carrying `text` and returns its enveloped `TranscriptHypothesis`.
    ///
    /// # Errors
    ///
    /// [`HypothesisRefused::CapabilityNotClaimed`] naming [`Capability::OutputStreaming`] when
    /// input has not ended and the adapter does not claim it, and
    /// [`HypothesisRefused::SequenceExhausted`] when the stream has issued its last sequence; no
    /// span is added and no sequence is used.
    pub fn hypothesis(
        &mut self,
        text: String,
    ) -> Result<EventEnvelope<TranscriptEvent>, HypothesisRefused> {
        self.require_before_end(Capability::OutputStreaming)?;
        let sequence = self.sequence()?;
        let (span, event) = Span::emit(
            &self.turn,
            self.request.run_id,
            self.request.stream_id,
            sequence,
            text,
        )
        .expect("the turn of a recognition is never completed by it, so it is Open");
        self.spans.push(span);
        Ok(self.envelope(TranscriptEvent::Hypothesis(event)))
    }

    /// Runs the `revised` outcome of `inference.stream.ReviseSpan` on a span of this recognition
    /// and returns its enveloped `TranscriptRevision`.
    ///
    /// # Errors
    ///
    /// [`RevisionRefused::SpanNotFound`] when no span of the recognition carries `span_id`,
    /// [`RevisionRefused::SequenceExhausted`] when the stream has issued its last sequence, and
    /// [`RevisionRefused::CommittedSpanImmutable`] when the span is Committed; its text is
    /// unchanged and no sequence is used.
    pub fn revise(
        &mut self,
        input: ReviseSpan,
    ) -> Result<EventEnvelope<TranscriptEvent>, RevisionRefused> {
        let index = self.index_of(input.span_id)?;
        self.sequence()?;
        let (span, event) = self.spans[index].revise(input.text)?;
        self.spans[index] = span;
        Ok(self.envelope(TranscriptEvent::Revision(event)))
    }

    /// Runs the `committed` outcome of `inference.stream.CommitSpan` on a span of this
    /// recognition and returns its enveloped `TranscriptCommitted`.
    ///
    /// # Errors
    ///
    /// [`CommitRefused::SpanNotFound`] when no span of the recognition carries `span_id`,
    /// [`CommitRefused::SequenceExhausted`] when the stream has issued its last sequence, and
    /// [`CommitRefused::SpanStateConflict`] when the span is not a Hypothesis; nothing moves and
    /// no sequence is used.
    pub fn commit(
        &mut self,
        span_id: SpanId,
    ) -> Result<EventEnvelope<TranscriptEvent>, CommitRefused> {
        let index = self.index_of(span_id)?;
        self.sequence()?;
        let (span, event) = self.spans[index].commit()?;
        self.spans[index] = span;
        Ok(self.envelope(TranscriptEvent::Committed(event)))
    }

    /// The request the recognition was started for.
    #[must_use]
    pub const fn request(&self) -> &RecognitionRequest {
        &self.request
    }

    /// The sequence the next event of the recognition takes on the request's stream: where a
    /// recognition continuing the stream after this one starts. `None` once the recognition has
    /// issued sequence `i64::MAX`, the last a stream has.
    #[must_use]
    pub const fn next_sequence(&self) -> Option<i64> {
        self.next_sequence
    }

    /// The capabilities the producing adapter claimed when the recognition started.
    #[must_use]
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// Whether input has ended.
    #[must_use]
    pub const fn input_ended(&self) -> bool {
        self.input_ended
    }

    /// How many frames the implementation has taken.
    #[must_use]
    pub const fn frames_taken(&self) -> usize {
        self.taken
    }

    /// The spans the recognition emitted, in emission order.
    #[must_use]
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// The span of this recognition carrying `span_id`.
    #[must_use]
    pub fn span(&self, span_id: SpanId) -> Option<&Span> {
        self.spans.iter().find(|span| span.span_id() == span_id)
    }

    fn index_of(&self, span_id: SpanId) -> Result<usize, SpanNotFound> {
        self.spans
            .iter()
            .position(|span| span.span_id() == span_id)
            .ok_or(SpanNotFound { span_id })
    }

    /// Passes once input has ended, and before that only when the adapter claims `capability`.
    fn require_before_end(&self, capability: Capability) -> Result<(), CapabilityNotClaimed> {
        if self.input_ended || self.capabilities.contains(&capability) {
            Ok(())
        } else {
            Err(CapabilityNotClaimed { capability })
        }
    }

    /// The next sequence of the request's stream, if it has one left.
    const fn sequence(&self) -> Result<i64, SequenceExhausted> {
        match self.next_sequence {
            Some(sequence) => Ok(sequence),
            None => Err(SequenceExhausted {
                stream_id: self.request.stream_id,
            }),
        }
    }

    /// Envelopes `payload` at the next sequence of the request's stream. Every caller has
    /// checked that one is left.
    fn envelope(&mut self, payload: TranscriptEvent) -> EventEnvelope<TranscriptEvent> {
        let sequence = self
            .next_sequence
            .expect("the caller checked that a sequence is left");
        self.next_sequence = sequence.checked_add(1);
        EventEnvelope {
            schema_version: ENVELOPE_SCHEMA_VERSION,
            event_id: EventId::generate(),
            run_id: self.request.run_id,
            session_id: None,
            stream_id: self.request.stream_id,
            sequence,
            producer: self.request.node_id,
            port: self.request.port.clone(),
            kind: payload.name().to_owned(),
            generation_id: None,
            timestamp: self.taken_through,
            timebase: Timebase::AudioFrameTime,
            causation_id: None,
            payload,
        }
    }
}

/// An implementation of the STT contract: an `inference.model.Adapter` that recognises speech.
pub trait Recogniser {
    /// The handle of one running recognition.
    type Handle: RecognitionHandle;

    /// The adapter record this recogniser was declared with, the capabilities it claims among it.
    fn adapter(&self) -> &Adapter;

    /// Starts one recognition for `request`. A recognition runs until its handle is dropped.
    ///
    /// # Errors
    ///
    /// [`ConcurrencyLimitReached`] carrying the adapter's `max_concurrency` when that many
    /// recognitions of this recogniser already run; nothing starts.
    fn start(
        &mut self,
        request: RecognitionRequest,
    ) -> Result<Self::Handle, ConcurrencyLimitReached>;
}

/// One running recognition. A caller offers frames with [`RecognitionHandle::offer_frame`] and
/// marks the end of input with [`RecognitionHandle::end_input`]; iterating the handle yields the
/// enveloped events the implementation has delivered so far, in sequence order. `None` means
/// nothing more is delivered until more input is offered or input ends, and, once input has
/// ended, that nothing more is delivered at all. [`RecognitionHandle::output`] shows where the
/// recognition stands.
pub trait RecognitionHandle: Iterator<Item = EventEnvelope<TranscriptEvent>> {
    /// The recognition, its frames and its spans, as they stand now.
    fn output(&self) -> &TranscriptOutput;

    /// Offers `frame` through [`TranscriptOutput::offer`].
    ///
    /// # Errors
    ///
    /// [`InputEnded`] when input has ended; the frame is not offered.
    fn offer_frame(&mut self, frame: AudioFrame) -> Result<(), InputEnded>;

    /// Marks the end of input through [`TranscriptOutput::end_input`].
    ///
    /// # Errors
    ///
    /// [`InputEnded`] when input has already ended.
    fn end_input(&mut self) -> Result<(), InputEnded>;

    /// Runs `inference.stream.ReviseSpan` on a span of this recognition through
    /// [`TranscriptOutput::revise`], for a revision issued from outside the recogniser, such as a
    /// correction; the `TranscriptRevision` is delivered in sequence with the other events.
    ///
    /// # Errors
    ///
    /// The [`RevisionRefused`] of [`TranscriptOutput::revise`]: no such span, or a Committed one
    /// whose text stays as it was.
    fn revise(&mut self, input: ReviseSpan) -> Result<(), RevisionRefused>;
}
