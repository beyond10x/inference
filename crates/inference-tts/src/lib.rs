//! The TTS contract of the inference framework.
//!
//! `inference-tts` is the synthesis contract over `inference-text`'s [`OutputSegment`] and
//! `inference-audio`'s [`AudioFrame`]. A [`Synthesiser`] is an `inference.model.Adapter`: it
//! carries the [`Adapter`] record it was declared with, and [`Synthesiser::start`] begins one
//! synthesis of one utterance. The returned [`SynthesisHandle`] takes committed segments and an
//! end-of-utterance marker, and yields the synthesis's [`SynthesisEvent`]s: audio frames, each
//! naming the segment it speaks, and the `inference.stream.SynthesisCompleted` of each segment.
//!
//! The synthesis is the one record of each segment offered to it, from Queued to Played or
//! Discarded. Offering a segment hands it over and runs `inference.stream.QueueForSynthesis` on
//! it: a Committed segment moves to Queued and its `SegmentQueued` is returned to the caller. A
//! Tentative segment is refused with [`SegmentStateConflict`], the command's `wrong-state`
//! outcome, since uncommitted text is not safe to speak (`docs/design/inference.md:390`); the
//! speculative lower-integrity mode is not declared. `inference.stream.CompleteSynthesis` moves a
//! Queued segment to Synthesized once its audio is produced. `inference.stream.AcknowledgePlayback`
//! moves a Synthesized segment to Played; it is the media sink's call, never the synthesiser's
//! (line 388), and after it the segment refuses `inference.stream.DiscardSegment`.
//!
//! A segment offered to one synthesis cannot be offered to another: it is moved into the first,
//! since `QueueForSynthesis` on a Queued segment is wrong-state.
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
//!   by [`SynthesisOutput::complete`], after its `SynthesisCompleted`; no frame of a segment is
//!   emitted once it is Synthesized, so its audio is final before it can be played.
//! * A frame's data is `sample_count * channels * width(encoding)` bytes
//!   ([`AudioFormat::data_len`](inference_audio::AudioFormat::data_len)); a frame that
//!   disagrees is refused with [`FrameLengthMismatch`]. `inference-audio` itself admits such a
//!   frame, as the specification does.
//!
//! [`SynthesisOutput`] is the bookkeeping every implementation shares: the segments offered and
//! taken, moved only through the transitions of `inference-text`, and the frames emitted of each.
//! The crate carries no concrete model, backend or provider; the scripted mock synthesiser and a
//! sink double are `inference-testkit`'s, behind its `tts` feature. Voices, languages, codecs,
//! audio formats and speaker conditions as capabilities, and text-to-audio alignment, are not part
//! of the contract yet.
//!
//! The vocabulary crates the contract is written in are re-exported, so that a consumer of speech
//! synthesis alone depends on this crate and one adapter.

use std::error::Error;
use std::fmt;

pub use inference_audio;
pub use inference_core;
pub use inference_text;

use inference_audio::AudioFrame;
use inference_core::model::{Adapter, Capability};
use inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued, SegmentState,
    SegmentStateConflict, SynthesisCompleted,
};

/// The revision of this contract, for the `contract_revision` an adapter implementing it
/// declares.
pub const CONTRACT_REVISION: &str = env!("CARGO_PKG_VERSION");

/// One frame of the audio of the segment `segment_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentAudio {
    pub segment_id: SegmentId,
    pub frame: AudioFrame,
}

/// An event of one synthesis, as a [`SynthesisHandle`] yields it, in the order the implementation
/// produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthesisEvent {
    /// A frame of a segment's audio.
    Audio(SegmentAudio),
    /// `inference.stream.SynthesisCompleted`: the segment's audio is generated and not yet played.
    Completed(SynthesisCompleted),
}

impl SynthesisEvent {
    /// The segment the event is about.
    #[must_use]
    pub const fn segment_id(&self) -> SegmentId {
        match self {
            Self::Audio(audio) => audio.segment_id,
            Self::Completed(event) => event.segment_id,
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
    /// Why playback was not acknowledged; nothing changed either way.
    pub enum PlaybackRefused {
        /// The synthesis holds no such segment.
        SegmentNotHeld,
        /// The `wrong-state` outcome of `inference.stream.AcknowledgePlayback`: the segment is not
        /// Synthesized.
        SegmentStateConflict,
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

/// One synthesis of one utterance: the segments offered, in the order they were, the first
/// `taken` of them taken by the implementation, and the frames emitted of each, with the
/// capabilities of the adapter producing it. It is the one record of each segment offered to it:
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
    segments: Vec<OutputSegment>,
    frames: Vec<usize>,
    taken: usize,
    utterance_ended: bool,
}

impl SynthesisOutput {
    /// Starts a synthesis on behalf of `adapter`, whose claims it keeps.
    #[must_use]
    pub fn start(adapter: &Adapter) -> Self {
        Self {
            capabilities: adapter.capabilities().to_vec(),
            segments: Vec::new(),
            frames: Vec::new(),
            taken: 0,
            utterance_ended: false,
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
        let (queued, event) = segment.queue()?;
        self.segments.push(queued);
        self.frames.push(0);
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

    /// Gives the segments offered and not yet taken to the implementation, in the order they were
    /// offered, leaving out those discarded before they were taken.
    ///
    /// # Errors
    ///
    /// [`CapabilityNotClaimed`] naming [`Capability::IncrementalInput`] when the utterance is not
    /// complete and the adapter does not claim it; no segment is taken.
    pub fn take_input(&mut self) -> Result<Vec<OutputSegment>, CapabilityNotClaimed> {
        if !self.utterance_ended && !self.capabilities.contains(&Capability::IncrementalInput) {
            return Err(CapabilityNotClaimed {
                capability: Capability::IncrementalInput,
            });
        }
        let taken = self.segments[self.taken..]
            .iter()
            .filter(|segment| segment.state() == SegmentState::Queued)
            .cloned()
            .collect();
        self.taken = self.segments.len();
        Ok(taken)
    }

    /// Emits `frame` as audio of the Queued segment `segment_id`, before its synthesis completes,
    /// and returns its [`SynthesisEvent::Audio`].
    ///
    /// # Errors
    ///
    /// [`AudioRefused::SegmentNotTaken`] when the implementation has not taken the segment;
    /// [`AudioRefused::SegmentStateConflict`] carrying its state when it is not Queued;
    /// [`AudioRefused::CapabilityNotClaimed`] naming [`Capability::OutputStreaming`] when the
    /// adapter does not claim it; and [`AudioRefused::FrameLengthMismatch`] when the frame's data
    /// is not [`AudioFormat::data_len`](inference_audio::AudioFormat::data_len) of its sample
    /// count. Nothing is emitted.
    pub fn audio(
        &mut self,
        segment_id: SegmentId,
        frame: AudioFrame,
    ) -> Result<SynthesisEvent, AudioRefused> {
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
    /// [`CompletionRefused::SegmentNotTaken`] when the implementation has not taken the segment,
    /// [`CompletionRefused::SegmentStateConflict`] carrying the held state when it is not Queued,
    /// and [`CompletionRefused::FrameLengthMismatch`] for the first frame whose data length
    /// disagrees with its samples. Nothing moves and nothing is emitted.
    pub fn complete(
        &mut self,
        segment_id: SegmentId,
        frames: Vec<AudioFrame>,
    ) -> Result<Vec<SynthesisEvent>, CompletionRefused> {
        let index = self.taken_index(segment_id)?;
        let (synthesized, event) = self.segments[index].synthesize()?;
        for frame in &frames {
            check_length(frame)?;
        }
        self.segments[index] = synthesized;
        self.frames[index] += frames.len();
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
    /// `segment_id`: Synthesized to Played. Only the media sink calls it: a synthesiser never
    /// confirms playback.
    ///
    /// # Errors
    ///
    /// [`PlaybackRefused::SegmentNotHeld`] when the synthesis holds no such segment, and
    /// [`PlaybackRefused::SegmentStateConflict`] carrying its state when it is not Synthesized, a
    /// Queued one among them. Nothing moves.
    pub fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        let index = self.held_index(segment_id)?;
        let (played, event) = self.segments[index].play()?;
        self.segments[index] = played;
        Ok(event)
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
        let (discarded, event) = self.segments[index].discard()?;
        self.segments[index] = discarded;
        Ok(event)
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

    fn held_index(&self, segment_id: SegmentId) -> Result<usize, SegmentNotHeld> {
        self.segments
            .iter()
            .position(|segment| segment.segment_id() == segment_id)
            .ok_or(SegmentNotHeld { segment_id })
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

/// An implementation of the TTS contract: an `inference.model.Adapter` that synthesises speech.
pub trait Synthesiser {
    /// The handle of one running synthesis.
    type Handle: SynthesisHandle;

    /// The adapter record this synthesiser was declared with, the capabilities it claims among
    /// it.
    fn adapter(&self) -> &Adapter;

    /// Starts one synthesis of one utterance.
    fn start(&mut self) -> Self::Handle;
}

/// One running synthesis. A caller offers committed segments with
/// [`SynthesisHandle::offer_segment`] and marks the end of the utterance with
/// [`SynthesisHandle::end_utterance`]; iterating the handle yields the events the implementation
/// has delivered so far. `None` means nothing more is delivered until more input is offered or the
/// utterance ends, and, once it has ended, that nothing more is delivered at all.
/// [`SynthesisHandle::output`] shows where the synthesis stands.
///
/// The media sink acknowledges playback through [`SynthesisHandle::acknowledge_playback`], and a
/// caller discards a segment through [`SynthesisHandle::discard`]; both run on the synthesis's own
/// record. An implementation passes both through and never calls them itself.
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

    /// Runs `inference.stream.AcknowledgePlayback` through
    /// [`SynthesisOutput::acknowledge_playback`], on behalf of the media sink.
    ///
    /// # Errors
    ///
    /// The [`PlaybackRefused`] of [`SynthesisOutput::acknowledge_playback`]; nothing moves.
    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
    ) -> Result<AudioPlayed, PlaybackRefused>;

    /// Runs `inference.stream.DiscardSegment` through [`SynthesisOutput::discard`]; nothing of
    /// the segment is delivered afterwards.
    ///
    /// # Errors
    ///
    /// The [`DiscardRefused`] of [`SynthesisOutput::discard`]; nothing moves.
    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused>;
}
