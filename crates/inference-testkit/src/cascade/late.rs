//! A synthesiser scripted to emit late: [`LateSynthesiser`] wraps any [`Synthesiser`] of
//! `inference-tts`, the [`MockSynthesiser`](crate::tts::MockSynthesiser) among them, and holds back
//! the frames of one segment, the one offered at the ordinal it was given, counted from 0 across
//! the whole synthesis. The synthesis still emits them, through the contract, while the segment is
//! Queued, so they are its own frames; the wrapper only delays their delivery. They are delivered
//! once the segment is discarded through the handle, as a backend that had them in flight when its
//! output was cut would deliver them, or, if it never is, once the utterance ends; from the end of
//! the utterance on, nothing more is held back. Every other event passes as the wrapped synthesis
//! delivers it, except the synthesis's usage record, which `inference-tts` makes its last event:
//! it waits until no frame is held back, so it stays last.
//!
//! A barge-in test scripts it on a segment the interruption discards, so that frames of a
//! superseded generation reach the sink after the interruption, which is to drop them
//! (`docs/design/inference.md` 637-641). [`LateHandle`] reads which segment was held back and how
//! many of its frames were delivered late.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use inference_tts::inference_core::model::Adapter;
use inference_tts::inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued,
};
use inference_tts::{
    ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt, PlaybackRefused,
    SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};

/// What the late synthesis has held back and delivered late.
#[derive(Debug, Default)]
struct Record {
    withheld: Option<SegmentId>,
    late: usize,
}

/// The test's side of a [`LateSynthesiser`]. Clones share one record.
#[derive(Debug, Clone, Default)]
pub struct LateHandle(Arc<Mutex<Record>>);

impl LateHandle {
    fn with<T>(&self, change: impl FnOnce(&mut Record) -> T) -> T {
        change(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The segment whose frames are held back, once it was offered.
    #[must_use]
    pub fn withheld(&self) -> Option<SegmentId> {
        self.with(|record| record.withheld)
    }

    /// How many frames of the held-back segment were delivered, all of them late.
    #[must_use]
    pub fn late_frames(&self) -> usize {
        self.with(|record| record.late)
    }
}

/// A synthesiser that delivers the frames of one segment late; see the module documentation.
#[derive(Debug, Clone)]
pub struct LateSynthesiser<S> {
    inner: S,
    ordinal: usize,
    handle: LateHandle,
}

impl<S: Synthesiser> LateSynthesiser<S> {
    /// `inner`, holding back the frames of the segment offered at `ordinal`, counted from 0.
    #[must_use]
    pub fn new(inner: S, ordinal: usize) -> Self {
        Self {
            inner,
            ordinal,
            handle: LateHandle::default(),
        }
    }

    /// The test's side of the synthesiser.
    #[must_use]
    pub fn handle(&self) -> LateHandle {
        self.handle.clone()
    }
}

impl<S: Synthesiser> Synthesiser for LateSynthesiser<S> {
    type Handle = LateSynthesis<S::Handle>;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn start(&mut self) -> Result<Self::Handle, ConcurrencyLimitReached> {
        Ok(LateSynthesis {
            inner: self.inner.start()?,
            ordinal: self.ordinal,
            offered: 0,
            held: Vec::new(),
            usage: None,
            released: VecDeque::new(),
            handle: self.handle.clone(),
        })
    }
}

/// One synthesis of a [`LateSynthesiser`].
#[derive(Debug)]
pub struct LateSynthesis<H> {
    inner: H,
    ordinal: usize,
    offered: usize,
    held: Vec<SynthesisEvent>,
    /// The usage record, waiting until no frame is held back.
    usage: Option<SynthesisEvent>,
    released: VecDeque<SynthesisEvent>,
    handle: LateHandle,
}

impl<H> LateSynthesis<H> {
    /// Delivers what is held back, from the next event on, the usage record after the frames.
    fn release(&mut self) {
        let late = self.held.len();
        self.released.extend(self.held.drain(..));
        self.released.extend(self.usage.take());
        self.handle.with(|record| record.late += late);
    }
}

impl<H: SynthesisHandle> Iterator for LateSynthesis<H> {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        if let Some(event) = self.released.pop_front() {
            return Some(event);
        }
        let withheld = self.handle.withheld();
        loop {
            let event = self.inner.next()?;
            match &event {
                SynthesisEvent::Audio(audio)
                    if Some(audio.segment_id()) == withheld
                        && !self.inner.output().utterance_ended() =>
                {
                    self.held.push(event);
                }
                SynthesisEvent::Usage(_) if !self.held.is_empty() => {
                    self.usage = Some(event);
                }
                _ => return Some(event),
            }
        }
    }
}

impl<H: SynthesisHandle> SynthesisHandle for LateSynthesis<H> {
    fn output(&self) -> &SynthesisOutput {
        self.inner.output()
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        let segment_id = segment.segment_id();
        let queued = self.inner.offer_segment(segment)?;
        if self.offered == self.ordinal {
            self.handle
                .with(|record| record.withheld = Some(segment_id));
        }
        self.offered += 1;
        Ok(queued)
    }

    fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        self.inner.end_utterance()?;
        self.release();
        Ok(())
    }

    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        self.inner.acknowledge_playback(segment_id, receipt)
    }

    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        let discarded = self.inner.discard(segment_id)?;
        if self.handle.withheld() == Some(segment_id) {
            self.release();
        }
        Ok(discarded)
    }
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};

    use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
    use inference_tts::inference_core::generation::{Generation, StartGeneration};
    use inference_tts::inference_core::id::{NodeId, RunId};
    use inference_tts::inference_core::model::ModelId;
    use inference_tts::inference_text::{OutputSegment, SegmentState};
    use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

    use super::LateSynthesiser;
    use crate::tts::{MockConfiguration, MockSynthesiser};

    #[test]
    fn the_frames_of_the_scripted_segment_are_delivered_only_once_it_is_discarded() {
        let format = AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(16_000).expect("16000"),
            channels: NonZeroU16::new(1).expect("1"),
        };
        let mock = MockSynthesiser::new(
            MockConfiguration::IncrementalStreaming,
            ModelId::generate(),
            format,
        );
        let mut late = LateSynthesiser::new(mock, 1);
        let handle = late.handle();
        let mut synthesis = late.start().expect("one synthesis");
        let (generation, _) = Generation::start(StartGeneration {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
        });
        let mut ids = Vec::new();
        for (sequence, text) in [(1, "one two"), (2, "three four five")] {
            let (tentative, _) =
                OutputSegment::emit(&generation, sequence, text.to_owned()).expect("Active");
            let (segment, _) = tentative.commit().expect("Tentative");
            ids.push(segment.segment_id());
            synthesis.offer_segment(segment).expect("Committed");
        }
        let frames_of = |events: &[SynthesisEvent], segment_id| {
            events
                .iter()
                .filter(|event| {
                    matches!(event, SynthesisEvent::Audio(audio) if audio.segment_id() == segment_id)
                })
                .count()
        };
        let first: Vec<SynthesisEvent> = synthesis.by_ref().collect();
        assert_eq!(frames_of(&first, ids[0]), 2);
        assert_eq!(frames_of(&first, ids[1]), 0, "held back");
        assert_eq!(handle.withheld(), Some(ids[1]));
        assert_eq!(
            synthesis.output().frames_emitted(ids[1]),
            Some(3),
            "emitted all the same"
        );
        synthesis.discard(ids[1]).expect("Synthesized");
        assert_eq!(
            synthesis.output().segment(ids[1]).map(OutputSegment::state),
            Some(SegmentState::Discarded)
        );
        let late: Vec<SynthesisEvent> = synthesis.by_ref().collect();
        assert_eq!(frames_of(&late, ids[1]), 3, "delivered late");
        assert_eq!(handle.late_frames(), 3);
    }
}
