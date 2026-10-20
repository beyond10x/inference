//! Adversary pass 1 of `story:contracts-review-fixes`: the story decides that playback
//! acknowledgement "takes a value only a sink can make ..., so the synthesiser cannot confirm its
//! own playback". The frames a [`PlaybackReceipt`] is made of are returned to whoever calls
//! [`SynthesisOutput::complete`] or [`SynthesisOutput::audio`]: the synthesiser itself. And
//! `inference.stream.AcknowledgePlayback` (`ess/domains/stream.yaml`) moves every Synthesized
//! segment to Played, while a segment completed without frames has no receipt at all.
//! Correction round 1 narrowed the claim to a synthesiser that delivered its frames; see the
//! first case.

use std::num::{NonZeroU16, NonZeroU32};

use inference_tts::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_tts::inference_text::{OutputSegment, SegmentState};
use inference_tts::{
    CONTRACT_REVISION, PlaybackReceipt, SegmentAudio, SynthesisEvent, SynthesisOutput,
};

fn adapter(capabilities: Vec<Capability>) -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "adversary".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("a maximum concurrency of one is positive")
    .0
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

fn frames_of(events: Vec<SynthesisEvent>) -> Vec<SegmentAudio> {
    events
        .into_iter()
        .filter_map(|event| match event {
            SynthesisEvent::Audio(audio) => Some(audio),
            SynthesisEvent::Completed(_) | SynthesisEvent::Usage(_) => None,
        })
        .collect()
}

/// Correction round 1 narrowed the claim. A receipt is made by consuming the delivered frames,
/// so once the synthesiser has moved them to a sink it holds nothing to make one from; that half
/// is the `compile_fail,E0382` example on `PlaybackReceipt`, since a use of moved frames does
/// not compile. A synthesiser that withholds every frame from every sink is outside what a type
/// prevents in process, and the docs of `PlaybackReceipt` and `acknowledge_playback` say so.
/// What runs: the sink's receipt of every delivered frame confirms playback, and an empty
/// receipt, which anyone can make, does not confirm a segment that has frames.
#[test]
fn only_the_receipt_of_the_frames_delivered_to_a_sink_confirms_playback() {
    let segment = committed("hello");
    let id = segment.segment_id();
    let mut output = SynthesisOutput::start(&adapter(Vec::new()));
    output.offer(segment).expect("queued");
    output.end_utterance().expect("ends once");
    output.take_input().expect("after the end");
    let delivered = frames_of(
        output
            .complete(id, vec![frame(), frame()])
            .expect("Queued to Synthesized"),
    );
    let mut sink: Vec<SegmentAudio> = Vec::new();
    sink.extend(delivered);

    let empty = PlaybackReceipt::from_frames(Vec::new()).expect("the receipt of no frames");
    assert!(
        output.acknowledge_playback(id, empty).is_err(),
        "a receipt anyone can make does not confirm a segment with frames"
    );
    assert_eq!(
        output.segment(id).map(OutputSegment::state),
        Some(SegmentState::Synthesized)
    );

    let receipt = PlaybackReceipt::from_frames(sink).expect("frames of one segment");
    assert!(output.acknowledge_playback(id, receipt).is_ok());
    assert_eq!(
        output.segment(id).map(OutputSegment::state),
        Some(SegmentState::Played)
    );
}

/// `complete` accepts an empty set of frames ("one that does [stream] may pass none", and nothing
/// refuses it without streaming either). The segment is then Synthesized with no frame, so a sink
/// that played every frame emitted of it, none, has nothing to make a receipt of, and
/// `AcknowledgePlayback`, whose `played` outcome the specification gives every Synthesized
/// segment, can never run on it.
#[test]
fn a_segment_synthesized_without_frames_can_still_be_acknowledged() {
    for capabilities in [Vec::new(), vec![Capability::OutputStreaming]] {
        let segment = committed("");
        let id = segment.segment_id();
        let mut output = SynthesisOutput::start(&adapter(capabilities.clone()));
        output.offer(segment).expect("queued");
        output.end_utterance().expect("ends once");
        output.take_input().expect("after the end");
        let delivered = frames_of(
            output
                .complete(id, Vec::new())
                .expect("Queued to Synthesized"),
        );
        assert_eq!(
            output.segment(id).map(OutputSegment::state),
            Some(SegmentState::Synthesized),
            "{capabilities:?}: completed without frames"
        );
        assert_eq!(output.frames_emitted(id), Some(0));

        let receipt = PlaybackReceipt::from_frames(delivered);
        assert!(
            receipt.is_ok(),
            "{capabilities:?}: the sink played all of the segment's audio, none, and cannot \
             make a receipt of it: {receipt:?}; the Synthesized segment can never be Played"
        );
    }
}
