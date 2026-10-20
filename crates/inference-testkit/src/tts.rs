//! The scripted mock synthesiser of the TTS contract of `inference-tts`, and a sink double.
//!
//! [`MockSynthesiser`] is an `inference.model.Adapter` of one model, declared in one of the four
//! [`MockConfiguration`]s of `incremental_input` and `output_streaming`, and speaks each segment
//! it takes as one frame of [`FRAME_SAMPLES`] samples of silence per word, at least one, in the
//! format it was declared with, placed one after another on the synthesis's sample clock from
//! sample 0. It yields its events to the caller through the [`MockSynthesis`] handle, one per
//! call, and records nothing in any store of this crate. It asks the contract for input and for
//! audio at every step and does not consult its own claims, so how it behaves follows from what
//! its configuration claims:
//!
//! * claiming `incremental_input`, it takes the segments offered so far at its next step that
//!   has nothing left to deliver: an iteration of the handle hands out the events already
//!   produced first, and only once none is left does it take input; it synthesises what it took
//!   before the utterance is complete. Otherwise it takes nothing until the utterance ends;
//! * claiming `output_streaming`, it emits each frame of a segment while the segment is Queued,
//!   then completes its synthesis; otherwise it completes the synthesis with the whole audio of
//!   the segment, delivered after `SynthesisCompleted`.
//!
//! A segment discarded through the handle is not synthesised, and its frames not yet delivered
//! are dropped. [`MockSink`] is the media sink: it is given frames and acknowledges playback of a
//! Synthesized segment once it has been given every frame of it, through
//! [`SynthesisHandle::acknowledge_playback`] with the [`PlaybackReceipt`] it makes by consuming
//! them.
//!
//! Behind the `run` feature, `TtsNode` is the graph node of a voice pipeline over any
//! synthesiser, the mock by default: it synthesises every committed segment an LLM node emits
//! and, playing, acknowledges its playback through the sink double.
//!
//! It declares a maximum concurrency of one and holds to it: while one synthesis of a
//! synthesiser, or of a clone of it, runs, a second start is refused with
//! [`ConcurrencyLimitReached`]. A synthesis runs until its handle is dropped.
//!
//! An offered segment waits for the next step with nothing left to deliver:
//!
//! ```
//! # use std::num::{NonZeroU16, NonZeroU32};
//! # use inference_tts::{SynthesisHandle, Synthesiser};
//! # use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
//! # use inference_tts::inference_core::generation::{Generation, StartGeneration};
//! # use inference_tts::inference_core::id::{NodeId, RunId};
//! # use inference_tts::inference_core::model::ModelId;
//! # use inference_tts::inference_text::OutputSegment;
//! # use inference_testkit::tts::{MockConfiguration, MockSynthesiser};
//! # let format = AudioFormat {
//! #     encoding: AudioEncoding::PcmS16le,
//! #     sample_rate: NonZeroU32::new(16_000).unwrap(),
//! #     channels: NonZeroU16::new(1).unwrap(),
//! # };
//! # let (generation, _) = Generation::start(StartGeneration {
//! #     run_id: RunId::generate(),
//! #     node_id: NodeId::generate(),
//! #     epoch: 1,
//! # });
//! # let (tentative, _) = OutputSegment::emit(&generation, 1, "hello".to_owned()).unwrap();
//! # let (segment, _) = tentative.commit().unwrap();
//! let mut synthesiser =
//!     MockSynthesiser::new(MockConfiguration::IncrementalStreaming, ModelId::generate(), format);
//! let mut handle = synthesiser.start().unwrap();
//! handle.offer_segment(segment).unwrap();
//! assert_eq!(handle.output().segments_taken(), 0, "offering takes nothing");
//! let frame = handle.next();
//! assert_eq!(handle.output().segments_taken(), 1, "the next step takes it");
//! assert!(frame.is_some());
//! ```

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use inference_tts::inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_tts::inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued, SegmentSnapshot,
    SegmentState, SegmentStateConflict,
};
use inference_tts::{
    AudioRefused, CONTRACT_REVISION, ConcurrencyLimitReached, DiscardRefused,
    FrameOfAnotherSegment, OfferRefused, PlaybackReceipt, PlaybackRefused, SegmentAudio,
    SegmentNotHeld, SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};

/// The TTS contract the mock implements, re-exported with the vocabulary crates it re-exports, so
/// that a test reaches `inference-tts`' types through the testkit alone.
pub use inference_tts;

/// The backend-neutral checks of the TTS adapter exit criteria, generic over any
/// [`Synthesiser`], with [`MockSink`] as the sink, for an adapter crate to run against its own
/// synthesiser.
pub mod conformance;

/// The TTS node of a voice pipeline, behind the `run` feature: a [`TtsNode`] synthesises the
/// committed segments of an LLM node on one synthesis of its synthesiser, a [`MockSynthesiser`]
/// by default.
#[cfg(feature = "run")]
mod node;

#[cfg(feature = "run")]
pub use node::{Spoken, TtsHandle, TtsNode, TtsPayload};

/// What a [`MockSynthesiser`] claims, and so how it behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MockConfiguration {
    /// Claims `incremental_input` and `output_streaming`.
    IncrementalStreaming,
    /// Claims `incremental_input` only.
    IncrementalInputOnly,
    /// Claims `output_streaming` only.
    OutputStreamingOnly,
    /// Claims neither: it takes the complete utterance and emits audio on synthesis completion.
    Neither,
}

impl MockConfiguration {
    /// Every configuration.
    pub const ALL: [Self; 4] = [
        Self::IncrementalStreaming,
        Self::IncrementalInputOnly,
        Self::OutputStreamingOnly,
        Self::Neither,
    ];

    /// The capabilities a synthesiser in this configuration claims.
    #[must_use]
    pub fn capabilities(self) -> Vec<Capability> {
        match self {
            Self::IncrementalStreaming => {
                vec![Capability::IncrementalInput, Capability::OutputStreaming]
            }
            Self::IncrementalInputOnly => vec![Capability::IncrementalInput],
            Self::OutputStreamingOnly => vec![Capability::OutputStreaming],
            Self::Neither => Vec::new(),
        }
    }
}

/// The samples per channel of each frame the mock emits.
pub const FRAME_SAMPLES: u32 = 160;

/// The syntheses of one synthesiser that run, shared by its clones, which are the same adapter.
#[derive(Debug, Clone, Default)]
struct Running(Arc<AtomicUsize>);

impl PartialEq for Running {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Running {}

impl Running {
    /// Counts one more running synthesis, if fewer than `max_concurrency` run.
    fn admit(&self, max_concurrency: i64) -> Result<Slot, ConcurrencyLimitReached> {
        let limit = usize::try_from(max_concurrency).unwrap_or(usize::MAX);
        let slot = Slot(Arc::clone(&self.0));
        if self.0.fetch_add(1, Ordering::AcqRel) < limit {
            Ok(slot)
        } else {
            drop(slot);
            Err(ConcurrencyLimitReached { max_concurrency })
        }
    }
}

/// The place of one running synthesis among [`Running`], given back when it is dropped.
#[derive(Debug)]
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl PartialEq for Slot {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Slot {}

/// The scripted mock synthesiser: an in-process adapter of one model with a maximum concurrency of
/// one, which it holds to, emitting frames in one format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockSynthesiser {
    adapter: Adapter,
    format: AudioFormat,
    running: Running,
}

impl MockSynthesiser {
    /// Declares a mock synthesiser of the model `model_id` in `configuration`, emitting frames in
    /// `format`.
    #[must_use]
    pub fn new(configuration: MockConfiguration, model_id: ModelId, format: AudioFormat) -> Self {
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id,
            backend: "inference-testkit mock synthesiser".to_owned(),
            contract_revision: CONTRACT_REVISION.to_owned(),
            execution_form: ExecutionForm::InProcess,
            locality: Locality::SameProcess,
            capabilities: configuration.capabilities(),
            max_concurrency: 1,
        })
        .expect("a maximum concurrency of one is positive");
        Self {
            adapter,
            format,
            running: Running::default(),
        }
    }
}

impl Synthesiser for MockSynthesiser {
    type Handle = MockSynthesis;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(&mut self) -> Result<MockSynthesis, ConcurrencyLimitReached> {
        let slot = self.running.admit(self.adapter.max_concurrency())?;
        Ok(MockSynthesis {
            output: SynthesisOutput::start(&self.adapter),
            format: self.format,
            next_sample: 0,
            pending: VecDeque::new(),
            delivered: VecDeque::new(),
            _slot: slot,
        })
    }
}

/// A segment taken and not yet synthesised, with the frames of it left to emit.
#[derive(Debug, PartialEq, Eq)]
struct Job {
    segment_id: SegmentId,
    frames_left: usize,
}

/// One synthesis of a [`MockSynthesiser`]; it runs until it is dropped.
#[derive(Debug, PartialEq, Eq)]
pub struct MockSynthesis {
    output: SynthesisOutput,
    format: AudioFormat,
    next_sample: u64,
    pending: VecDeque<Job>,
    delivered: VecDeque<SynthesisEvent>,
    _slot: Slot,
}

impl Iterator for MockSynthesis {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        loop {
            if let Some(event) = self.delivered.pop_front() {
                return Some(event);
            }
            if let Ok(taken) = self.output.take_input() {
                self.pending.extend(taken.iter().map(|segment| Job {
                    segment_id: segment.segment_id(),
                    frames_left: segment.text().split_whitespace().count().max(1),
                }));
            }
            let (segment_id, frames_left) = self
                .pending
                .front()
                .map(|job| (job.segment_id, job.frames_left))?;
            let state = self
                .output
                .segment(segment_id)
                .map(OutputSegment::state)
                .expect("a taken segment is held");
            if state != SegmentState::Queued {
                self.pending.pop_front();
                continue;
            }
            if frames_left > 0 {
                match self.output.audio(segment_id, self.silence(0)) {
                    Ok(event) => {
                        self.next_sample += u64::from(FRAME_SAMPLES);
                        if let Some(job) = self.pending.front_mut() {
                            job.frames_left -= 1;
                        }
                        return Some(event);
                    }
                    Err(AudioRefused::CapabilityNotClaimed(_)) => {}
                    Err(error) => panic!("the mock's frame is refused: {error}"),
                }
            }
            let frames = (0..frames_left as u64)
                .map(|index| self.silence(index))
                .collect();
            let events = self
                .output
                .complete(segment_id, frames)
                .expect("a taken Queued segment completes with well-formed frames");
            self.next_sample += frames_left as u64 * u64::from(FRAME_SAMPLES);
            self.pending.pop_front();
            self.delivered.extend(events);
        }
    }
}

impl MockSynthesis {
    /// A frame of [`FRAME_SAMPLES`] samples of silence, `offset` frames past the next sample.
    fn silence(&self, offset: u64) -> AudioFrame {
        let sample = self.next_sample + offset * u64::from(FRAME_SAMPLES);
        AudioFrame {
            format: self.format,
            position: SampleClockPosition::new(sample)
                .expect("the mock stays within i64::MAX samples"),
            sample_count: FRAME_SAMPLES,
            data: vec![
                self.format.encoding.silence();
                usize::try_from(self.format.data_len(FRAME_SAMPLES))
                    .expect("a frame of the mock fits in memory")
            ],
        }
    }
}

impl SynthesisHandle for MockSynthesis {
    fn output(&self) -> &SynthesisOutput {
        &self.output
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        self.output.offer(segment)
    }

    fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        self.output.end_utterance()
    }

    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        self.output.acknowledge_playback(segment_id, receipt)
    }

    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        let event = self.output.discard(segment_id)?;
        self.delivered
            .retain(|event| event.segment_id() != segment_id);
        Ok(event)
    }
}

/// Why the sink refused to acknowledge playback; nothing changed either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkRefused {
    /// The synthesis holds no such segment.
    SegmentNotHeld(SegmentNotHeld),
    /// The `wrong-state` outcome of `inference.stream.AcknowledgePlayback`: the segment is not
    /// Synthesized, in the state carried.
    SegmentStateConflict(SegmentStateConflict),
    /// The sink has been given no audio of a segment the synthesis emitted frames of, so it
    /// played none.
    NothingGiven { segment_id: SegmentId },
    /// The receipt's frames are of another segment.
    FrameOfAnotherSegment(FrameOfAnotherSegment),
    /// The sink has been given `given` frames of the segment, those the synthesis already counts
    /// as played among them, where the synthesis emitted `emitted`, so it has not played exactly
    /// its audio.
    FramesOutstanding {
        segment_id: SegmentId,
        given: usize,
        emitted: usize,
    },
}

impl From<PlaybackRefused> for SinkRefused {
    fn from(error: PlaybackRefused) -> Self {
        match error {
            PlaybackRefused::SegmentNotHeld(error) => Self::SegmentNotHeld(error),
            PlaybackRefused::SegmentStateConflict(error) => Self::SegmentStateConflict(error),
            PlaybackRefused::FrameOfAnotherSegment(error) => Self::FrameOfAnotherSegment(error),
            PlaybackRefused::PlayedFramesMismatch(error) => Self::FramesOutstanding {
                segment_id: error.segment_id,
                given: error.played,
                emitted: error.emitted,
            },
        }
    }
}

/// The sink double: it is given the frames of a synthesis and acknowledges playback of a
/// Synthesized segment once it has been given every frame the synthesis emitted of it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MockSink {
    given: Vec<SegmentAudio>,
}

impl MockSink {
    /// A sink that has been given nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Gives the sink a frame to play.
    pub fn give(&mut self, audio: SegmentAudio) {
        self.given.push(audio);
    }

    /// How many frames of the segment `segment_id` the sink has been given and not yet
    /// acknowledged.
    #[must_use]
    pub fn frames_given(&self, segment_id: SegmentId) -> usize {
        self.given
            .iter()
            .filter(|audio| audio.segment_id() == segment_id)
            .count()
    }

    /// Runs `inference.stream.AcknowledgePlayback` on the segment `segment_id` of the synthesis
    /// `handle` runs, through [`SynthesisHandle::acknowledge_playback`] with the
    /// [`PlaybackReceipt`] it makes by consuming the frames of the segment it was given, none for
    /// a segment synthesized without frames: the synthesis's record moves from Synthesized to
    /// Played. Frames of the segment the synthesis already counts as played, from a receipt it
    /// refused earlier, count as given. Returns a snapshot of the Played record and its
    /// `AudioPlayed`.
    ///
    /// # Errors
    ///
    /// [`SinkRefused::SegmentNotHeld`] when the synthesis holds no such segment;
    /// [`SinkRefused::SegmentStateConflict`] carrying its state when it is not Synthesized, a
    /// Queued or Played one among them; [`SinkRefused::NothingGiven`] when neither the sink nor
    /// the synthesis's count has a frame of a segment the synthesis emitted frames of; and
    /// [`SinkRefused::FramesOutstanding`] when the two together are not exactly the frames the
    /// synthesis emitted of it. On each of these the sink refuses before it makes a receipt:
    /// nothing moves and the sink keeps its frames. When the handle refuses the receipt the sink
    /// made, the sink no longer holds those frames, since a receipt consumes them and is not
    /// handed back; the synthesis counts them as played, so they are not lost, and the next
    /// acknowledgement confirms them.
    pub fn acknowledge<H: SynthesisHandle>(
        &mut self,
        handle: &mut H,
        segment_id: SegmentId,
    ) -> Result<(SegmentSnapshot, AudioPlayed), SinkRefused> {
        let current = handle
            .output()
            .segment(segment_id)
            .ok_or(SegmentNotHeld { segment_id })
            .map_err(SinkRefused::SegmentNotHeld)?
            .state();
        if current != SegmentState::Synthesized {
            return Err(SinkRefused::SegmentStateConflict(SegmentStateConflict {
                current,
            }));
        }
        let counted = handle
            .output()
            .frames_played(segment_id)
            .unwrap_or_default();
        let given = self.frames_given(segment_id) + counted;
        let emitted = handle
            .output()
            .frames_emitted(segment_id)
            .unwrap_or_default();
        if given == 0 && emitted > 0 {
            return Err(SinkRefused::NothingGiven { segment_id });
        }
        if given != emitted {
            return Err(SinkRefused::FramesOutstanding {
                segment_id,
                given,
                emitted,
            });
        }
        let (played, kept): (Vec<SegmentAudio>, Vec<SegmentAudio>) = self
            .given
            .drain(..)
            .partition(|audio| audio.segment_id() == segment_id);
        self.given = kept;
        let receipt = PlaybackReceipt::from_frames(played)
            .expect("the sink was given frames of the segment, and only of it");
        let event = handle.acknowledge_playback(segment_id, receipt)?;
        let played = handle
            .output()
            .segment(segment_id)
            .expect("an acknowledged segment is held")
            .snapshot();
        Ok((played, event))
    }
}
