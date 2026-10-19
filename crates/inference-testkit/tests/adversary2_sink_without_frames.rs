//! Adversary pass 2 of `story:contracts-review-fixes`, on the correction round: [`MockSink`]
//! refuses `NothingGiven` only for a segment the synthesis emitted frames of, and acknowledges a
//! segment synthesized without frames with the receipt of no frames. The mock synthesiser never
//! completes a segment without a frame, so a bare handle over [`SynthesisOutput`] does here.

#![cfg(feature = "tts")]

use std::num::{NonZeroU16, NonZeroU32};

use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_testkit::tts::{MockSink, SinkRefused};
use inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued, SegmentState,
};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::{
    CONTRACT_REVISION, DiscardRefused, OfferRefused, PlaybackReceipt, PlaybackRefused,
    SynthesisEvent, SynthesisHandle, SynthesisOutput, UtteranceEnded,
};

/// A handle that passes everything through to its synthesis and yields nothing; the test drives
/// the synthesis itself.
struct Bare {
    output: SynthesisOutput,
}

impl Iterator for Bare {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        None
    }
}

impl SynthesisHandle for Bare {
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
        self.output.discard(segment_id)
    }
}

fn committed(text: &str) -> OutputSegment {
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (tentative, _) =
        OutputSegment::emit(&generation, 1, text.to_owned()).expect("the generation is Active");
    tentative.commit().expect("a Tentative segment commits").0
}

fn frame() -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(16_000).expect("not zero"),
            channels: NonZeroU16::new(1).expect("not zero"),
        },
        position: SampleClockPosition::new(0).expect("in range"),
        sample_count: 2,
        data: vec![0; 4],
    }
}

/// A bare handle over a synthesis claiming `capabilities`, holding `segments`, all taken.
fn bare(capabilities: Vec<Capability>, segments: Vec<OutputSegment>) -> Bare {
    let (adapter, _) = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "adversary".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("a maximum concurrency of one is positive");
    let mut handle = Bare {
        output: SynthesisOutput::start(&adapter),
    };
    for segment in segments {
        handle.offer_segment(segment).expect("queued");
    }
    handle.end_utterance().expect("ends once");
    handle.output.take_input().expect("after the end");
    handle
}

/// Guard: a sink given nothing acknowledges a segment synthesized without frames, keeps the frames
/// of another segment, and refuses `NothingGiven` for a segment with frames.
#[test]
fn the_sink_acknowledges_a_segment_without_frames_and_refuses_one_with_frames_it_was_not_given() {
    for capabilities in [Vec::new(), vec![Capability::OutputStreaming]] {
        let (silent, spoken) = (committed(""), committed("spoken"));
        let (quiet, loud) = (silent.segment_id(), spoken.segment_id());
        let mut handle = bare(capabilities.clone(), vec![silent, spoken]);
        drop(
            handle
                .output
                .complete(quiet, Vec::new())
                .expect("Synthesized"),
        );
        let mut sink = MockSink::new();
        for event in handle
            .output
            .complete(loud, vec![frame()])
            .expect("Synthesized")
        {
            if let SynthesisEvent::Audio(audio) = event {
                sink.give(audio);
            }
        }

        let (played, event) = sink
            .acknowledge(&mut handle, quiet)
            .expect("a segment without frames is played on the receipt of none");
        assert_eq!(event.segment_id, quiet, "{capabilities:?}");
        assert_eq!(played.state(), SegmentState::Played, "{capabilities:?}");
        assert_eq!(
            sink.frames_given(loud),
            1,
            "{capabilities:?}: the sink keeps the frames of another segment"
        );
        assert_eq!(
            sink.acknowledge(&mut handle, quiet).map(|_| ()),
            Err(SinkRefused::SegmentStateConflict(
                inference_text::SegmentStateConflict {
                    current: SegmentState::Played,
                }
            )),
            "{capabilities:?}: a second acknowledgement is wrong-state"
        );

        let mut empty = MockSink::new();
        assert_eq!(
            empty.acknowledge(&mut handle, loud).map(|_| ()),
            Err(SinkRefused::NothingGiven { segment_id: loud }),
            "{capabilities:?}"
        );
        assert!(
            sink.acknowledge(&mut handle, loud).is_ok(),
            "{capabilities:?}"
        );
    }
}
