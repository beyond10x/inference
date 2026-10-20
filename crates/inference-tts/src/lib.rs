//! The TTS contract of the inference framework.
//!
//! `inference-tts` is the synthesis contract over `inference-text`'s [`OutputSegment`] and
//! `inference-audio`'s [`AudioFrame`]. A [`Synthesiser`] is an `inference.model.Adapter`: it
//! carries the [`Adapter`] record it was declared with, and [`Synthesiser::start`] begins one
//! synthesis of one utterance. The returned [`SynthesisHandle`] takes committed segments and an
//! end-of-utterance marker, and yields the synthesis's [`SynthesisEvent`]s: audio frames, each
//! naming the segment it speaks, the `inference.stream.SynthesisCompleted` of each segment and,
//! last, the usage record of the synthesis.
//!
//! The synthesis is the one record of each segment offered to it, from Queued to Played or
//! Discarded. Offering a segment hands it over and runs `inference.stream.QueueForSynthesis` on
//! it: a Committed segment moves to Queued and its `SegmentQueued` is returned to the caller. A
//! Tentative segment is refused with [`SegmentStateConflict`], the command's `wrong-state`
//! outcome, since uncommitted text is not safe to speak (`docs/design/inference.md:390`); the
//! speculative lower-integrity mode is not declared. `inference.stream.CompleteSynthesis` moves a
//! Queued segment to Synthesized once its audio is produced. `inference.stream.AcknowledgePlayback`
//! moves a Synthesized segment to Played; it is the media sink's call, never the synthesiser's
//! (line 388), and after it the segment refuses `inference.stream.DiscardSegment`. It takes the
//! [`PlaybackReceipt`] the sink makes by consuming the frames it played, every frame emitted of
//! the segment; the frames are made only by the synthesis and are not `Clone`, so a synthesiser
//! that delivered them to a sink cannot make one. A synthesiser that withholds every frame from
//! every sink still can; no type prevents that in one process.
//!
//! [`Synthesiser::start`] refuses a start past the maximum concurrency the adapter record declares
//! with [`ConcurrencyLimitReached`]; a synthesis runs until its handle is dropped.
//!
//! A segment offered to one synthesis cannot be offered to another: it is moved into the first,
//! since `QueueForSynthesis` on a Queued segment is wrong-state, and [`OutputSegment`] is not
//! `Clone`.
//!
//! ```compile_fail,E0382
//! # use inference_tts::SynthesisOutput;
//! # use inference_tts::inference_core::model::Adapter;
//! # use inference_tts::inference_text::OutputSegment;
//! fn offer_twice(adapter: &Adapter, segment: OutputSegment) {
//!     let mut first = SynthesisOutput::start(adapter);
//!     let mut second = SynthesisOutput::start(adapter);
//!     let _ = first.offer(segment);
//!     let _ = second.offer(segment);
//! }
//! ```
//!
//! Nor can a second Committed value of the segment's identity be made to offer to the other: a
//! transition of [`OutputSegment`] consumes the segment it moves, so a Tentative segment commits
//! once.
//!
//! ```compile_fail,E0382
//! # use inference_tts::SynthesisOutput;
//! # use inference_tts::inference_core::model::Adapter;
//! # use inference_tts::inference_text::OutputSegment;
//! fn commit_twice_and_offer_both(adapter: &Adapter, tentative: OutputSegment) {
//!     let (segment, _) = tentative.commit().unwrap();
//!     let (again, _) = tentative.commit().unwrap();
//!     let _ = SynthesisOutput::start(adapter).offer(segment);
//!     let _ = SynthesisOutput::start(adapter).offer(again);
//! }
//! ```
//!
//! [`OutputSegment::emit_as`] takes the identity it is given, so a caller that names one identity
//! twice holds two segments of it, and a synthesis still refuses the second value as wrong-state;
//! two syntheses do not see each other.
//!
//! Incremental input and output streaming are distinct capabilities
//! (`inference.model.Capability`), read from the adapter record and never implied by the shape of
//! the API: every synthesiser takes segments and an end-of-utterance marker the same way. What the
//! contract can observe, it decides here:
//!
//! * A synthesiser takes input incrementally when it takes a segment before the utterance is
//!   complete. [`SynthesisOutput::take_input`] gives segments up before the end of the utterance
//!   only when the adapter claims [`Capability::IncrementalInput`], and refuses otherwise with
//!   [`CapabilityNotClaimed`] naming it; audio is produced only for a segment taken.
//! * A synthesiser streams audio when a frame of a segment is emitted while the segment is still
//!   Queued, before its synthesis completes. [`SynthesisOutput::audio`] emits a frame of a Queued
//!   segment only when the adapter claims [`Capability::OutputStreaming`], and refuses otherwise
//!   with [`CapabilityNotClaimed`] naming it. Without it, the whole audio of a segment is emitted
//!   by [`SynthesisOutput::complete`], in the call that moves the segment to Synthesized, after
//!   its `SynthesisCompleted`. No frame of a segment is emitted after that call, so its audio is
//!   final before it can be played.
//! * A frame's data is `sample_count * channels * width(encoding)` bytes
//!   ([`AudioFormat::data_len`](inference_audio::AudioFormat::data_len)); a frame that
//!   disagrees is refused with [`FrameLengthMismatch`]. `inference-audio` itself admits such a
//!   frame, as the specification does.
//!
//! Without output streaming, the call that completes a segment returns its `SynthesisCompleted`
//! and then its frames, and no frame follows:
//!
//! ```
//! # use std::num::{NonZeroU16, NonZeroU32};
//! # use inference_tts::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
//! # use inference_tts::inference_core::generation::{Generation, StartGeneration};
//! # use inference_tts::inference_core::id::{NodeId, RunId};
//! # use inference_tts::inference_core::model::{Adapter, DeclareAdapter, ExecutionForm, Locality, ModelId};
//! # use inference_tts::inference_text::{OutputSegment, SegmentState};
//! # use inference_tts::{AudioRefused, CONTRACT_REVISION, SynthesisEvent, SynthesisOutput};
//! # let (adapter, _) = Adapter::declare(DeclareAdapter {
//! #     model_id: ModelId::generate(),
//! #     backend: "doc".to_owned(),
//! #     contract_revision: CONTRACT_REVISION.to_owned(),
//! #     execution_form: ExecutionForm::InProcess,
//! #     locality: Locality::SameProcess,
//! #     capabilities: Vec::new(),
//! #     max_concurrency: 1,
//! # }).unwrap();
//! # let (generation, _) = Generation::start(StartGeneration {
//! #     run_id: RunId::generate(),
//! #     node_id: NodeId::generate(),
//! #     epoch: 1,
//! # });
//! # let (tentative, _) = OutputSegment::emit(&generation, 1, "hello".to_owned()).unwrap();
//! # let (segment, _) = tentative.commit().unwrap();
//! # let frame = || AudioFrame {
//! #     format: AudioFormat {
//! #         encoding: AudioEncoding::PcmS16le,
//! #         sample_rate: NonZeroU32::new(16_000).unwrap(),
//! #         channels: NonZeroU16::new(1).unwrap(),
//! #     },
//! #     position: SampleClockPosition::new(0).unwrap(),
//! #     sample_count: 2,
//! #     data: vec![0; 4],
//! # };
//! let id = segment.segment_id();
//! let mut output = SynthesisOutput::start(&adapter);
//! output.offer(segment).unwrap();
//! output.end_utterance().unwrap();
//! output.take_input().unwrap();
//! let events = output.complete(id, vec![frame(), frame()]).unwrap();
//! assert_eq!(output.segment(id).map(OutputSegment::state), Some(SegmentState::Synthesized));
//! assert!(matches!(events[0], SynthesisEvent::Completed(_)));
//! assert!(events[1..].iter().all(|event| matches!(event, SynthesisEvent::Audio(_))));
//! assert_eq!(events.len(), 3, "the frames come in the call that completes the segment");
//! assert!(matches!(
//!     output.audio(id, frame()),
//!     Err(AudioRefused::SegmentStateConflict(_))
//! ));
//! assert_eq!(output.frames_emitted(id), Some(2), "and none after it");
//! ```
//!
//! [`SynthesisOutput`] is the bookkeeping every implementation shares: the segments offered and
//! taken, moved only through the transitions of `inference-text`, the frames emitted of each and
//! the usage of the synthesis.
//!
//! A synthesis is one model call, and a synthesis whose utterance has ended yields one usage
//! record, [`SynthesisEvent::Usage`], the `inference.usage.UsageRecorded` that
//! [`SynthesisOutput::usage`] runs `inference.usage.RecordUsage` for. It is the last event of the
//! synthesis, and the contract holds it there: the record is refused before the end of the
//! utterance with [`UtteranceNotEnded`] and while a segment of the synthesis is still Queued,
//! taken or not, with [`SegmentNotSynthesized`], so the implementation records it right after
//! its last completion; once it is made, every further frame or completion is refused with
//! [`SynthesisEnded`], and a second record with [`UsageAlreadyRecorded`]. Playback and discard
//! are the sink's and the caller's moves, not the synthesis's, and stay open. Its quantities are
//! measured by the contract, so every synthesiser reports them without code of its own:
//! `input_characters`, the Unicode scalar values of the text of every segment taken
//! ([`SynthesisOutput::take_input`]); `output_audio_ms`, the audio emitted, the samples per
//! channel at each sample rate counted exactly, over that rate, summed and rounded down to the
//! millisecond once; and `requests`, 1. A segment discarded after it was taken still counts, its
//! text as taken and the audio emitted of it, since that audio was generated; a segment discarded
//! before it was taken counts nothing. The implementation adds the time its backend computed
//! through [`SynthesisOutput::add_local_compute`], reported as `local_compute_ms`, `measured`, and
//! `unknown` when it adds none. A backend call that failed is still recorded, with what was
//! measured, its counts not final (`final` false) once the implementation marks it with
//! [`SynthesisOutput::mark_failed`]; a failed call will not synthesize the segments it took and
//! left Queued, so the record no longer waits for them: their text counts as taken and nothing of
//! them is emitted after it. A segment the failed call never took is still waited for. The record
//! names the adapter and the model, starts when the
//! synthesis started and ends when it is made, on the wall clock, and is placed on no run, node or
//! session: the node that places it fills those. A synthesis dropped before its utterance ended
//! completed no call and yields no record.
//!
//! The crate carries no concrete model, backend or provider; the scripted mock synthesiser and a
//! sink double are `inference-testkit`'s, behind its `tts` feature. Voices, languages, codecs,
//! audio formats and speaker conditions as capabilities, and text-to-audio alignment, are not part
//! of the contract yet.
//!
//! The vocabulary crates the contract is written in are re-exported, so that a consumer of speech
//! synthesis alone depends on this crate and one adapter.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use inference_audio;
pub use inference_core;
pub use inference_text;

use inference_audio::AudioFrame;
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::usage::{Quantity, QuantityKind, RecordUsage, UsageRecord, UsageRecorded};
use inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentMoveRefused, SegmentQueued,
    SegmentSnapshot, SegmentState, SegmentStateConflict, SynthesisCompleted,
};

/// The revision of this contract, for the `contract_revision` an adapter implementing it
/// declares.
pub const CONTRACT_REVISION: &str = env!("CARGO_PKG_VERSION");

/// One frame of the audio of a segment, as the synthesis emitted it. Only [`SynthesisOutput`]
/// makes one, and it is not `Clone`: once delivered, the frame is the sink's, and a
/// [`PlaybackReceipt`] is made of it.
///
/// ```compile_fail
/// fn cloneable<T: Clone>() {}
/// cloneable::<inference_tts::SegmentAudio>();
/// ```
///
/// ```compile_fail,E0451
/// # use inference_tts::SegmentAudio;
/// # use inference_tts::inference_audio::AudioFrame;
/// # use inference_tts::inference_text::SegmentId;
/// fn forge(segment_id: SegmentId, frame: AudioFrame) -> SegmentAudio {
///     SegmentAudio { segment_id, frame }
/// }
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct SegmentAudio {
    segment_id: SegmentId,
    frame: AudioFrame,
}

impl SegmentAudio {
    /// The segment the frame speaks.
    #[must_use]
    pub const fn segment_id(&self) -> SegmentId {
        self.segment_id
    }

    /// The frame.
    #[must_use]
    pub const fn frame(&self) -> &AudioFrame {
        &self.frame
    }
}

/// What a media sink makes once it has played the audio of one segment, by consuming the
/// [`SegmentAudio`] values of it that were delivered to it; a segment synthesized without frames
/// takes the receipt of no frames.
///
/// A [`SegmentAudio`] is made only by the synthesis and is not `Clone`, so a synthesiser that
/// handed its frames to a sink holds nothing to make a receipt from, and cannot confirm their
/// playback:
///
/// ```compile_fail,E0382
/// # use inference_tts::{PlaybackReceipt, SegmentAudio};
/// fn deliver_then_confirm(frames: Vec<SegmentAudio>, sink: &mut Vec<SegmentAudio>) {
///     sink.extend(frames);
///     let _ = PlaybackReceipt::from_frames(frames);
/// }
/// ```
///
/// That is the whole of what the type keeps from the synthesiser. One that withholds every frame
/// from every sink still holds them, and can make a receipt of them and confirm playback of audio
/// nobody heard; in one process no type prevents that, and it is a fault of the implementation
/// for the conformance checks of a real adapter to catch.
#[derive(Debug, PartialEq, Eq)]
pub struct PlaybackReceipt {
    segment_id: Option<SegmentId>,
    frames: usize,
}

impl PlaybackReceipt {
    /// The receipt of playback of `frames`, every one a frame of one segment, consuming them. No
    /// frames make the receipt of no frames, which names no segment.
    ///
    /// # Errors
    ///
    /// [`FrameOfAnotherSegment`] when the frames speak more than one segment. No receipt is
    /// made.
    pub fn from_frames(frames: Vec<SegmentAudio>) -> Result<Self, FrameOfAnotherSegment> {
        let segment_id = frames.first().map(|audio| audio.segment_id);
        if let Some(segment_id) = segment_id
            && let Some(other) = frames.iter().find(|audio| audio.segment_id != segment_id)
        {
            return Err(FrameOfAnotherSegment {
                segment_id,
                other: other.segment_id,
            });
        }
        Ok(Self {
            segment_id,
            frames: frames.len(),
        })
    }

    /// The segment whose frames were played, or `None` for the receipt of no frames.
    #[must_use]
    pub const fn segment_id(&self) -> Option<SegmentId> {
        self.segment_id
    }

    /// How many frames of it were played.
    #[must_use]
    pub const fn frames(&self) -> usize {
        self.frames
    }
}

/// An event of one synthesis, as a [`SynthesisHandle`] yields it, in the order the implementation
/// produced it.
#[derive(Debug, PartialEq, Eq)]
pub enum SynthesisEvent {
    /// A frame of a segment's audio.
    Audio(SegmentAudio),
    /// `inference.stream.SynthesisCompleted`: the segment's audio is generated and not yet played.
    Completed(SynthesisCompleted),
    /// `inference.usage.UsageRecorded`: the usage of the synthesis, the last event its
    /// implementation delivers once the utterance has ended; see [`SynthesisOutput::usage`].
    Usage(UsageRecorded),
}

impl SynthesisEvent {
    /// The name the event is declared under, in `ess/domains/audio.yaml` for a frame,
    /// `ess/domains/stream.yaml` for a completion and `ess/domains/usage.yaml` for a usage
    /// record: the `kind` of its envelope.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Audio(_) => "inference.audio.AudioFrame",
            Self::Completed(_) => "inference.stream.SynthesisCompleted",
            Self::Usage(_) => UsageRecorded::KIND,
        }
    }

    /// The segment the event is about; none for a usage record, which is about the synthesis.
    #[must_use]
    pub const fn segment_id(&self) -> Option<SegmentId> {
        match self {
            Self::Audio(audio) => Some(audio.segment_id),
            Self::Completed(event) => Some(event.segment_id),
            Self::Usage(_) => None,
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
            "the adapter does not claim {:?}, so the move is refused",
            self.capability
        )
    }
}

impl Error for CapabilityNotClaimed {}

/// The utterance is complete, so no further segment is offered and the end is not marked again;
/// nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtteranceEnded;

impl fmt::Display for UtteranceEnded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the utterance is complete, so nothing more is offered")
    }
}

impl Error for UtteranceEnded {}

/// The implementation has not taken the segment `segment_id`, so it has no audio of it and its
/// synthesis does not complete; nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentNotTaken {
    pub segment_id: SegmentId,
}

impl fmt::Display for SegmentNotTaken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "segment {} has not been taken by the synthesiser",
            self.segment_id
        )
    }
}

impl Error for SegmentNotTaken {}

/// No segment offered to the synthesis carries `segment_id`; nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentNotHeld {
    pub segment_id: SegmentId,
}

impl fmt::Display for SegmentNotHeld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "segment {} was not offered to this synthesis",
            self.segment_id
        )
    }
}

impl Error for SegmentNotHeld {}

/// The frame's data is `actual` bytes where its sample count, channels and encoding width make
/// `expected`, so it is not emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLengthMismatch {
    pub expected: u64,
    pub actual: u64,
}

impl fmt::Display for FrameLengthMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the frame carries {} bytes of data where its samples make {}",
            self.actual, self.expected
        )
    }
}

impl Error for FrameLengthMismatch {}

/// A playback receipt speaks one segment, `segment_id`, and a frame of `other` was among its
/// frames, or was offered as the receipt of `segment_id`; no receipt is made and nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameOfAnotherSegment {
    pub segment_id: SegmentId,
    pub other: SegmentId,
}

impl fmt::Display for FrameOfAnotherSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a receipt for segment {} was given a frame of segment {}",
            self.segment_id, self.other
        )
    }
}

impl Error for FrameOfAnotherSegment {}

/// The receipts given for the segment `segment_id` confirm `played` frames of it, this one
/// included, where the synthesis emitted `emitted`, so its audio was not played exactly; the
/// segment did not move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayedFramesMismatch {
    pub segment_id: SegmentId,
    pub played: usize,
    pub emitted: usize,
}

impl fmt::Display for PlayedFramesMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the receipts confirm {} frame(s) of segment {} where {} were emitted",
            self.played, self.segment_id, self.emitted
        )
    }
}

impl Error for PlayedFramesMismatch {}

/// The synthesiser already runs `max_concurrency` syntheses, the maximum concurrency its adapter
/// declares, so a further start is refused and nothing started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConcurrencyLimitReached {
    pub max_concurrency: i64,
}

impl fmt::Display for ConcurrencyLimitReached {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the synthesiser already runs {} synthesis(es), its declared maximum concurrency",
            self.max_concurrency
        )
    }
}

impl Error for ConcurrencyLimitReached {}

/// The utterance has not ended, so the synthesis has not completed and its usage is not
/// recorded; nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtteranceNotEnded;

impl fmt::Display for UtteranceNotEnded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the utterance has not ended, so the synthesis's usage is not recorded yet")
    }
}

impl Error for UtteranceNotEnded {}

/// The usage of the synthesis is recorded already, once per synthesis; nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageAlreadyRecorded;

impl fmt::Display for UsageAlreadyRecorded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the synthesis's usage is recorded already, so it is not recorded again")
    }
}

impl Error for UsageAlreadyRecorded {}

/// The segment `segment_id` of the synthesis is still Queued, taken or not, so the synthesis's
/// last segment is not synthesized and its usage is not recorded; nothing changed. Once the
/// backend call is marked failed, a taken segment no longer refuses the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentNotSynthesized {
    pub segment_id: SegmentId,
}

impl fmt::Display for SegmentNotSynthesized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "segment {} is still Queued, so the synthesis's usage is not recorded yet",
            self.segment_id
        )
    }
}

impl Error for SegmentNotSynthesized {}

/// The synthesis's usage is recorded, its last event, so the synthesis has ended and emits
/// nothing more; nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SynthesisEnded;

impl fmt::Display for SynthesisEnded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the synthesis's usage is recorded, its last event, so nothing more is emitted")
    }
}

impl Error for SynthesisEnded {}

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
    /// Why an offered segment was refused; nothing changed either way.
    pub enum OfferRefused {
        /// The `wrong-state` outcome of `inference.stream.QueueForSynthesis`: the segment is not
        /// Committed, or the synthesis already holds it, in the state carried.
        SegmentStateConflict,
        /// The utterance is complete.
        UtteranceEnded,
    }
}

refusal! {
    /// Why a frame was not emitted; nothing changed either way.
    pub enum AudioRefused {
        /// The synthesis's usage is recorded, so it emits nothing more.
        SynthesisEnded,
        /// The implementation has not taken the segment.
        SegmentNotTaken,
        /// The segment is Queued and the adapter does not claim [`Capability::OutputStreaming`].
        CapabilityNotClaimed,
        /// The segment is not Queued: its audio is final.
        SegmentStateConflict,
        /// The frame's data length disagrees with its samples.
        FrameLengthMismatch,
    }
}

refusal! {
    /// Why a segment's synthesis did not complete; nothing changed either way.
    pub enum CompletionRefused {
        /// The synthesis's usage is recorded, so it emits nothing more.
        SynthesisEnded,
        /// The implementation has not taken the segment.
        SegmentNotTaken,
        /// The `wrong-state` outcome of `inference.stream.CompleteSynthesis`: the segment is not
        /// Queued.
        SegmentStateConflict,
        /// A frame's data length disagrees with its samples.
        FrameLengthMismatch,
    }
}

refusal! {
    /// Why playback was not acknowledged. The segment did not move; the frames of the receipt
    /// stay counted as played of the segment they speak, so the receipt is not lost and a later
    /// acknowledgement confirms them (see [`SynthesisOutput::acknowledge_playback`]).
    pub enum PlaybackRefused {
        /// The synthesis holds no such segment.
        SegmentNotHeld,
        /// The `wrong-state` outcome of `inference.stream.AcknowledgePlayback`: the segment is not
        /// Synthesized.
        SegmentStateConflict,
        /// The receipt's frames are of another segment.
        FrameOfAnotherSegment,
        /// The receipts confirm fewer or more frames than the synthesis emitted of the segment.
        PlayedFramesMismatch,
    }
}

refusal! {
    /// Why a segment was not discarded; nothing changed either way.
    pub enum DiscardRefused {
        /// The synthesis holds no such segment.
        SegmentNotHeld,
        /// The `wrong-state` outcome of `inference.stream.DiscardSegment`: the segment is Played,
        /// which cannot be unsaid, or already Discarded.
        SegmentStateConflict,
    }
}

refusal! {
    /// Why the usage record was refused; nothing changed either way.
    pub enum UsageRefused {
        /// The utterance has not ended.
        UtteranceNotEnded,
        /// The usage of the synthesis is recorded already.
        UsageAlreadyRecorded,
        /// A segment of the synthesis is still Queued.
        SegmentNotSynthesized,
    }
}

/// One synthesis of one utterance: the segments offered, in the order they were, the first
/// `taken` of them taken by the implementation, and the frames emitted of each and confirmed as
/// played of each, with the capabilities, identity and model of the adapter producing it, and the
/// usage of the synthesis. It is the one record of each segment offered to it:
/// every segment moves through a transition of [`OutputSegment`], so a refused move changes
/// nothing.
///
/// It is not `Clone`: a copy would be a second record of the same segments.
///
/// ```compile_fail
/// fn cloneable<T: Clone>() {}
/// cloneable::<inference_tts::SynthesisOutput>();
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct SynthesisOutput {
    capabilities: Vec<Capability>,
    adapter_id: AdapterId,
    model_id: ModelId,
    segments: Vec<OutputSegment>,
    frames: Vec<usize>,
    played: Vec<usize>,
    taken: usize,
    utterance_ended: bool,
    /// The Unicode scalar values of the text of every segment taken.
    characters_taken: u128,
    /// The samples per channel emitted so far, by sample rate.
    emitted_samples: BTreeMap<u32, u128>,
    started_at_ms: i64,
    local_compute: Option<Duration>,
    failed: bool,
    usage_recorded: bool,
}

impl SynthesisOutput {
    /// Starts a synthesis on behalf of `adapter`, whose claims, identity and model it keeps, now
    /// on the wall clock.
    #[must_use]
    pub fn start(adapter: &Adapter) -> Self {
        Self {
            capabilities: adapter.capabilities().to_vec(),
            adapter_id: adapter.adapter_id(),
            model_id: adapter.model_id(),
            segments: Vec::new(),
            frames: Vec::new(),
            played: Vec::new(),
            taken: 0,
            utterance_ended: false,
            characters_taken: 0,
            emitted_samples: BTreeMap::new(),
            started_at_ms: wall_clock_ms(),
            local_compute: None,
            failed: false,
            usage_recorded: false,
        }
    }

    /// Takes `segment` over and runs the `queued` outcome of `inference.stream.QueueForSynthesis`
    /// on it, after the segments offered before it.
    ///
    /// # Errors
    ///
    /// [`OfferRefused::UtteranceEnded`] when the utterance is complete;
    /// [`OfferRefused::SegmentStateConflict`] carrying the state the synthesis holds the segment
    /// in when it already holds it, and otherwise the segment's own state when it is not
    /// Committed, a Tentative one among them. Nothing is queued.
    pub fn offer(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        if self.utterance_ended {
            return Err(UtteranceEnded.into());
        }
        if let Some(held) = self.segment(segment.segment_id()) {
            return Err(SegmentStateConflict {
                current: held.state(),
            }
            .into());
        }
        let (queued, event) = segment.queue().map_err(SegmentStateConflict::from)?;
        self.segments.push(queued);
        self.frames.push(0);
        self.played.push(0);
        Ok(event)
    }

    /// Marks the end of the utterance: every segment of it has been offered.
    ///
    /// # Errors
    ///
    /// [`UtteranceEnded`] when the utterance is already complete.
    pub fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        if self.utterance_ended {
            return Err(UtteranceEnded);
        }
        self.utterance_ended = true;
        Ok(())
    }

    /// Gives a snapshot of each segment offered and not yet taken to the implementation, in the
    /// order they were offered, leaving out those discarded before they were taken. The segments
    /// themselves stay in the synthesis's record, and the text of each taken counts towards the
    /// synthesis's `input_characters`.
    ///
    /// # Errors
    ///
    /// [`CapabilityNotClaimed`] naming [`Capability::IncrementalInput`] when the utterance is not
    /// complete and the adapter does not claim it; no segment is taken.
    pub fn take_input(&mut self) -> Result<Vec<SegmentSnapshot>, CapabilityNotClaimed> {
        if !self.utterance_ended && !self.capabilities.contains(&Capability::IncrementalInput) {
            return Err(CapabilityNotClaimed {
                capability: Capability::IncrementalInput,
            });
        }
        let taken: Vec<SegmentSnapshot> = self.segments[self.taken..]
            .iter()
            .filter(|segment| segment.state() == SegmentState::Queued)
            .map(OutputSegment::snapshot)
            .collect();
        self.taken = self.segments.len();
        for segment in &taken {
            self.characters_taken = self
                .characters_taken
                .saturating_add(segment.text().chars().count() as u128);
        }
        Ok(taken)
    }

    /// Emits `frame` as audio of the Queued segment `segment_id`, before its synthesis completes,
    /// and returns its [`SynthesisEvent::Audio`].
    ///
    /// # Errors
    ///
    /// [`AudioRefused::SynthesisEnded`] once the synthesis's usage is recorded, whatever the
    /// segment; [`AudioRefused::SegmentNotTaken`] when the implementation has not taken the
    /// segment; [`AudioRefused::SegmentStateConflict`] carrying its state when it is not Queued;
    /// [`AudioRefused::CapabilityNotClaimed`] naming [`Capability::OutputStreaming`] when the
    /// adapter does not claim it; and [`AudioRefused::FrameLengthMismatch`] when the frame's data
    /// is not [`AudioFormat::data_len`](inference_audio::AudioFormat::data_len) of its sample
    /// count. Nothing is emitted.
    pub fn audio(
        &mut self,
        segment_id: SegmentId,
        frame: AudioFrame,
    ) -> Result<SynthesisEvent, AudioRefused> {
        self.require_running()?;
        let index = self.taken_index(segment_id)?;
        let current = self.segments[index].state();
        if current != SegmentState::Queued {
            return Err(SegmentStateConflict { current }.into());
        }
        if !self.capabilities.contains(&Capability::OutputStreaming) {
            return Err(CapabilityNotClaimed {
                capability: Capability::OutputStreaming,
            }
            .into());
        }
        check_length(&frame)?;
        self.frames[index] += 1;
        self.count_emitted(&frame);
        Ok(SynthesisEvent::Audio(SegmentAudio { segment_id, frame }))
    }

    /// Runs the `synthesized` outcome of `inference.stream.CompleteSynthesis` on the segment
    /// `segment_id`, Queued to Synthesized, and emits `frames`, the rest of its audio. Returns its
    /// [`SynthesisEvent::Completed`] followed by a [`SynthesisEvent::Audio`] per frame. A
    /// synthesiser that does not stream emits the whole audio of a segment here; one that does
    /// may pass none.
    ///
    /// # Errors
    ///
    /// [`CompletionRefused::SynthesisEnded`] once the synthesis's usage is recorded, whatever the
    /// segment, [`CompletionRefused::SegmentNotTaken`] when the implementation has not taken the
    /// segment, [`CompletionRefused::SegmentStateConflict`] carrying the held state when it is not
    /// Queued, and [`CompletionRefused::FrameLengthMismatch`] for the first frame whose data
    /// length disagrees with its samples. Nothing moves and nothing is emitted.
    pub fn complete(
        &mut self,
        segment_id: SegmentId,
        frames: Vec<AudioFrame>,
    ) -> Result<Vec<SynthesisEvent>, CompletionRefused> {
        self.require_running()?;
        let index = self.taken_index(segment_id)?;
        self.require(index, SegmentState::Queued)?;
        for frame in &frames {
            check_length(frame)?;
        }
        let event = self.move_segment(index, OutputSegment::synthesize)?;
        self.frames[index] += frames.len();
        for frame in &frames {
            self.count_emitted(frame);
        }
        let mut events = Vec::with_capacity(frames.len() + 1);
        events.push(SynthesisEvent::Completed(event));
        events.extend(
            frames
                .into_iter()
                .map(|frame| SynthesisEvent::Audio(SegmentAudio { segment_id, frame })),
        );
        Ok(events)
    }

    /// Runs the `played` outcome of `inference.stream.AcknowledgePlayback` on the segment
    /// `segment_id`: Synthesized to Played. It takes the [`PlaybackReceipt`] the media sink made
    /// by consuming the frames of the segment delivered to it, every frame the synthesis emitted
    /// of it; the receipt of no frames confirms a segment the synthesis emitted no frame of. A
    /// segment identity is not a receipt:
    ///
    /// ```compile_fail,E0308
    /// # use inference_tts::SynthesisOutput;
    /// # use inference_tts::inference_text::SegmentId;
    /// fn confirm_own_playback(output: &mut SynthesisOutput, segment_id: SegmentId) {
    ///     let _ = output.acknowledge_playback(segment_id, segment_id);
    /// }
    /// ```
    ///
    /// So a synthesiser that delivered the frames to a sink cannot confirm their playback. One
    /// that withholds every frame from every sink can make the receipt of them itself; no type
    /// prevents that in one process (see [`PlaybackReceipt`]).
    ///
    /// A refusal loses no playback. The frames of the receipt are counted as played of the
    /// segment they speak, if the synthesis holds it, whatever the outcome
    /// ([`SynthesisOutput::frames_played`]); the segment moves only once the frames counted as
    /// played of it are every frame it emitted. So a sink whose receipt was refused, because the
    /// segment was still Queued or because it named the wrong segment, acknowledges again once
    /// the segment is Synthesized, with the receipt of the frames it has played since, the
    /// receipt of no frames if none.
    ///
    /// # Errors
    ///
    /// [`PlaybackRefused::SegmentNotHeld`] when the synthesis holds no such segment,
    /// [`PlaybackRefused::SegmentStateConflict`] carrying its state when it is not Synthesized, a
    /// Queued one among them, [`PlaybackRefused::FrameOfAnotherSegment`] when the receipt's frames
    /// are of another segment, and [`PlaybackRefused::PlayedFramesMismatch`] carrying the frames
    /// counted as played when they are not as many as the synthesis emitted of the segment, the
    /// receipt of no frames for a segment with frames none of which were counted among them. The
    /// segment does not move; the receipt's frames stay counted.
    pub fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        if let Some(spoken) = receipt.segment_id
            && let Ok(index) = self.held_index(spoken)
        {
            self.played[index] = self.played[index].saturating_add(receipt.frames);
        }
        let index = self.held_index(segment_id)?;
        self.require(index, SegmentState::Synthesized)?;
        if let Some(other) = receipt.segment_id
            && other != segment_id
        {
            return Err(FrameOfAnotherSegment { segment_id, other }.into());
        }
        if self.played[index] != self.frames[index] {
            return Err(PlayedFramesMismatch {
                segment_id,
                played: self.played[index],
                emitted: self.frames[index],
            }
            .into());
        }
        Ok(self.move_segment(index, OutputSegment::play)?)
    }

    /// Runs the `discarded` outcome of `inference.stream.DiscardSegment` on the segment
    /// `segment_id`: it is gone and its audio, if any, is not to be spoken.
    ///
    /// # Errors
    ///
    /// [`DiscardRefused::SegmentNotHeld`] when the synthesis holds no such segment, and
    /// [`DiscardRefused::SegmentStateConflict`] carrying its state when it is Played or already
    /// Discarded. Nothing moves.
    pub fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        let index = self.held_index(segment_id)?;
        Ok(self.move_segment(index, OutputSegment::discard)?)
    }

    /// Adds `spent`, time the implementation's backend computed for this synthesis, to the
    /// synthesis's local compute, which its usage record then reports as `local_compute_ms`,
    /// `measured`. A synthesis to which nothing is added reports it `unknown`.
    pub fn add_local_compute(&mut self, spent: Duration) {
        self.local_compute = Some(self.local_compute.unwrap_or_default().saturating_add(spent));
    }

    /// The local compute added so far, none when nothing was.
    #[must_use]
    pub const fn local_compute(&self) -> Option<Duration> {
        self.local_compute
    }

    /// Marks the implementation's backend call for this synthesis failed: its usage record is
    /// still made, with what was measured, and its counts are not final (`final` false), as
    /// `inference-llm` records a failed turn, without waiting for the segments the call took and
    /// left Queued.
    pub const fn mark_failed(&mut self) {
        self.failed = true;
    }

    /// Whether the implementation marked its backend call failed.
    #[must_use]
    pub const fn failed(&self) -> bool {
        self.failed
    }

    /// The Unicode scalar values of the text of every segment the implementation has taken, a
    /// segment discarded after it was taken among them.
    #[must_use]
    pub fn input_characters(&self) -> i64 {
        i64::try_from(self.characters_taken).unwrap_or(i64::MAX)
    }

    /// Milliseconds of audio emitted by the synthesis so far, of a segment discarded since among
    /// them: the samples per channel emitted at each sample rate, counted exactly, over that
    /// rate, summed as an exact fraction and rounded down to the millisecond once, at the end.
    /// Should that fraction not fit `u128`, which takes several sample rates with no common
    /// divisor in one synthesis, each rate's milliseconds are rounded down on their own and
    /// summed.
    #[must_use]
    pub fn output_audio_ms(&self) -> i64 {
        let exact = self.emitted_samples.iter().try_fold(
            (0_u128, 1_u128),
            |(numerator, denominator), (&rate, &samples)| {
                add_fraction(
                    (numerator, denominator),
                    (samples.checked_mul(1_000)?, u128::from(rate)),
                )
            },
        );
        let ms = exact.map_or_else(
            || {
                self.emitted_samples
                    .iter()
                    .map(|(&rate, &samples)| samples.saturating_mul(1_000) / u128::from(rate))
                    .fold(0_u128, u128::saturating_add)
            },
            |(numerator, denominator)| numerator / denominator,
        );
        i64::try_from(ms).unwrap_or(i64::MAX)
    }

    /// Runs `inference.usage.RecordUsage` for the synthesis and returns its
    /// `inference.usage.UsageRecorded` as [`SynthesisEvent::Usage`]; see the crate documentation.
    ///
    /// # Errors
    ///
    /// [`UsageRefused::UtteranceNotEnded`] before the end of the utterance,
    /// [`UsageRefused::UsageAlreadyRecorded`] once the record was made, and
    /// [`UsageRefused::SegmentNotSynthesized`] naming the first segment still Queued, taken or
    /// not, or, once the call is marked failed, the first not taken; nothing is recorded.
    pub fn usage(&mut self) -> Result<SynthesisEvent, UsageRefused> {
        if !self.utterance_ended {
            return Err(UtteranceNotEnded.into());
        }
        if self.usage_recorded {
            return Err(UsageAlreadyRecorded.into());
        }
        let failed_taken = if self.failed { self.taken } else { 0 };
        if let Some(queued) = self.segments[failed_taken..]
            .iter()
            .find(|segment| segment.state() == SegmentState::Queued)
        {
            return Err(SegmentNotSynthesized {
                segment_id: queued.segment_id(),
            }
            .into());
        }
        let local_compute =
            self.local_compute
                .map_or(Quantity::unknown(QuantityKind::LocalComputeMs), |spent| {
                    Quantity::measured(
                        QuantityKind::LocalComputeMs,
                        i64::try_from(spent.as_millis()).unwrap_or(i64::MAX),
                    )
                });
        let mut input = RecordUsage::new(
            self.adapter_id,
            self.model_id,
            self.started_at_ms,
            wall_clock_ms().max(self.started_at_ms),
            vec![
                Quantity::measured(QuantityKind::InputCharacters, self.input_characters()),
                Quantity::measured(QuantityKind::OutputAudioMs, self.output_audio_ms()),
                Quantity::measured(QuantityKind::Requests, 1),
                local_compute,
            ],
        );
        input.r#final = !self.failed;
        let (_, recorded) = UsageRecord::record(input);
        self.usage_recorded = true;
        Ok(SynthesisEvent::Usage(recorded))
    }

    /// Whether the usage of the synthesis is recorded.
    #[must_use]
    pub const fn usage_recorded(&self) -> bool {
        self.usage_recorded
    }

    /// The capabilities the producing adapter claimed when the synthesis started.
    #[must_use]
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// Whether the utterance is complete.
    #[must_use]
    pub const fn utterance_ended(&self) -> bool {
        self.utterance_ended
    }

    /// How many segments the implementation has taken, or has passed over as discarded.
    #[must_use]
    pub const fn segments_taken(&self) -> usize {
        self.taken
    }

    /// The segments offered and queued, in the order they were, as they stand now.
    #[must_use]
    pub fn segments(&self) -> &[OutputSegment] {
        &self.segments
    }

    /// The segment of this synthesis carrying `segment_id`.
    #[must_use]
    pub fn segment(&self, segment_id: SegmentId) -> Option<&OutputSegment> {
        self.segments
            .iter()
            .find(|segment| segment.segment_id() == segment_id)
    }

    /// How many frames of the segment `segment_id` have been emitted, or `None` when the
    /// synthesis holds no such segment. Once the segment is Synthesized the count is final.
    #[must_use]
    pub fn frames_emitted(&self, segment_id: SegmentId) -> Option<usize> {
        self.held_index(segment_id)
            .ok()
            .map(|index| self.frames[index])
    }

    /// How many frames of the segment `segment_id` the receipts given to
    /// [`SynthesisOutput::acknowledge_playback`] have confirmed as played, refused ones among
    /// them, or `None` when the synthesis holds no such segment.
    #[must_use]
    pub fn frames_played(&self, segment_id: SegmentId) -> Option<usize> {
        self.held_index(segment_id)
            .ok()
            .map(|index| self.played[index])
    }

    fn held_index(&self, segment_id: SegmentId) -> Result<usize, SegmentNotHeld> {
        self.segments
            .iter()
            .position(|segment| segment.segment_id() == segment_id)
            .ok_or(SegmentNotHeld { segment_id })
    }

    /// Passes until the synthesis's usage is recorded, its last event.
    const fn require_running(&self) -> Result<(), SynthesisEnded> {
        if self.usage_recorded {
            Err(SynthesisEnded)
        } else {
            Ok(())
        }
    }

    /// Counts the samples per channel of `frame`, emitted, at its sample rate.
    fn count_emitted(&mut self, frame: &AudioFrame) {
        let samples = self
            .emitted_samples
            .entry(frame.format.sample_rate.get())
            .or_insert(0);
        *samples = samples.saturating_add(u128::from(frame.sample_count));
    }

    /// Passes when the segment at `index` holds `state`, the one state the transition about to
    /// run moves from; it is read first so that a later check refuses before anything moves.
    fn require(&self, index: usize, state: SegmentState) -> Result<(), SegmentStateConflict> {
        let current = self.segments[index].state();
        if current == state {
            Ok(())
        } else {
            Err(SegmentStateConflict { current })
        }
    }

    /// Moves the segment at `index` by `transition`, which consumes it, and keeps the moved
    /// segment in its place, or the segment as it was when the transition is refused.
    fn move_segment<E>(
        &mut self,
        index: usize,
        transition: fn(OutputSegment) -> Result<(OutputSegment, E), SegmentMoveRefused>,
    ) -> Result<E, SegmentStateConflict> {
        let segment = self.segments.remove(index);
        let (kept, answer) = match transition(segment) {
            Ok((moved, event)) => (moved, Ok(event)),
            Err(refused) => {
                let conflict = refused.conflict();
                (refused.into_segment(), Err(conflict))
            }
        };
        self.segments.insert(index, kept);
        answer
    }

    /// The index of the segment `segment_id` among those the implementation has taken.
    fn taken_index(&self, segment_id: SegmentId) -> Result<usize, SegmentNotTaken> {
        self.segments[..self.taken]
            .iter()
            .position(|segment| segment.segment_id() == segment_id)
            .ok_or(SegmentNotTaken { segment_id })
    }
}

/// Passes when the data of `frame` is as long as its samples make it.
fn check_length(frame: &AudioFrame) -> Result<(), FrameLengthMismatch> {
    let expected = frame.format.data_len(frame.sample_count);
    let actual = frame.data.len() as u64;
    if actual == expected {
        Ok(())
    } else {
        Err(FrameLengthMismatch { expected, actual })
    }
}

/// The greatest common divisor of `a` and `b`; `a` when `b` is 0.
const fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// The sum of the fractions `left` and `right`, each a numerator over a denominator above 0, in
/// lowest terms; none when it does not fit `u128`.
fn add_fraction(left: (u128, u128), right: (u128, u128)) -> Option<(u128, u128)> {
    let ((a, b), (c, d)) = (left, right);
    let denominator = b.checked_mul(d / gcd(b, d))?;
    let numerator = a
        .checked_mul(denominator / b)?
        .checked_add(c.checked_mul(denominator / d)?)?;
    let common = gcd(numerator, denominator);
    Some((numerator / common, denominator / common))
}

/// Now, in Unix epoch milliseconds on the wall clock; 0 before the epoch.
fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}

/// An implementation of the TTS contract: an `inference.model.Adapter` that synthesises speech.
pub trait Synthesiser {
    /// The handle of one running synthesis.
    type Handle: SynthesisHandle;

    /// The adapter record this synthesiser was declared with, the capabilities it claims among
    /// it.
    fn adapter(&self) -> &Adapter;

    /// Starts one synthesis of one utterance. A synthesis runs until its handle is dropped.
    ///
    /// # Errors
    ///
    /// [`ConcurrencyLimitReached`] carrying the adapter's `max_concurrency` when that many
    /// syntheses of this synthesiser already run; nothing starts.
    fn start(&mut self) -> Result<Self::Handle, ConcurrencyLimitReached>;
}

/// One running synthesis. A caller offers committed segments with
/// [`SynthesisHandle::offer_segment`] and marks the end of the utterance with
/// [`SynthesisHandle::end_utterance`]; iterating the handle yields the events the implementation
/// has delivered so far. `None` means nothing more is delivered until more input is offered or the
/// utterance ends, and, once it has ended, that nothing more is delivered at all. Once the
/// utterance has ended and its last segment is synthesized, the last event the implementation
/// delivers is the synthesis's usage record, [`SynthesisEvent::Usage`], which
/// [`SynthesisOutput::usage`] makes. [`SynthesisHandle::output`] shows where the synthesis stands.
///
/// The media sink acknowledges playback through [`SynthesisHandle::acknowledge_playback`] with the
/// [`PlaybackReceipt`] it made by consuming the frames it played, and a caller discards a segment
/// through
/// [`SynthesisHandle::discard`]; both run on the synthesis's own record. An implementation passes
/// both through and never calls them itself.
pub trait SynthesisHandle: Iterator<Item = SynthesisEvent> {
    /// The synthesis and its segments, as they stand now.
    fn output(&self) -> &SynthesisOutput;

    /// Offers `segment` through [`SynthesisOutput::offer`], returning its `SegmentQueued`.
    ///
    /// # Errors
    ///
    /// The [`OfferRefused`] of [`SynthesisOutput::offer`]; nothing is queued.
    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused>;

    /// Marks the end of the utterance through [`SynthesisOutput::end_utterance`].
    ///
    /// # Errors
    ///
    /// [`UtteranceEnded`] when the utterance is already complete.
    fn end_utterance(&mut self) -> Result<(), UtteranceEnded>;

    /// Runs `inference.stream.AcknowledgePlayback` on the segment `segment_id` through
    /// [`SynthesisOutput::acknowledge_playback`] with the media sink's `receipt`.
    ///
    /// # Errors
    ///
    /// The [`PlaybackRefused`] of [`SynthesisOutput::acknowledge_playback`]; nothing moves.
    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused>;

    /// Runs `inference.stream.DiscardSegment` through [`SynthesisOutput::discard`]; nothing of
    /// the segment is delivered afterwards.
    ///
    /// # Errors
    ///
    /// The [`DiscardRefused`] of [`SynthesisOutput::discard`]; nothing moves.
    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused>;
}
