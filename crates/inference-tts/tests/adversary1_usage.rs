//! Adversary pass 1 on `story:usage-tts`: what a synthesis's usage record counts and what stays
//! open after it. A frame the contract refuses is not counted; audio a whole number of
//! milliseconds long reports exactly that many, whatever frame size carried it, and frames shorter
//! than a millisecond add up; characters are Unicode scalar values, not graphemes; playback and
//! discard stay open after the record and change none of its counts; a failed backend call that
//! left a taken segment Queued is recorded with it still Queued, not final; and the
//! contract places the record on no run, node, session, speech session or response.

use std::num::{NonZeroU16, NonZeroU32};

use inference_tts::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_tts::inference_core::usage::{Quantity, QuantityKind, UsageRecorded};
use inference_tts::inference_text::{OutputSegment, SegmentState};
use inference_tts::{
    AudioRefused, CONTRACT_REVISION, CompletionRefused, OfferRefused, PlaybackReceipt,
    SegmentNotSynthesized, SynthesisEvent, SynthesisOutput, UsageAlreadyRecorded, UsageRefused,
    UtteranceEnded,
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

fn frame_at(rate: u32, sample_count: u32) -> AudioFrame {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).expect("not zero"),
        channels: NonZeroU16::new(1).expect("not zero"),
    };
    AudioFrame {
        format,
        position: SampleClockPosition::new(0).expect("in range"),
        sample_count,
        data: vec![0; usize::try_from(format.data_len(sample_count)).expect("small")],
    }
}

fn quantity(recorded: &UsageRecorded, kind: QuantityKind) -> Quantity {
    let found: Vec<&Quantity> = recorded
        .quantities
        .iter()
        .filter(|quantity| quantity.kind() == kind)
        .collect();
    assert_eq!(found.len(), 1, "one {kind:?}: {recorded:?}");
    *found[0]
}

fn recorded(event: SynthesisEvent) -> UsageRecorded {
    match event {
        SynthesisEvent::Usage(recorded) => recorded,
        other => panic!("not a usage record: {other:?}"),
    }
}

/// The output audio milliseconds of one segment streamed as `count` frames of `sample_count`
/// samples at `rate`.
fn streamed_ms(count: usize, sample_count: u32, rate: u32) -> i64 {
    let mut output = SynthesisOutput::start(&adapter(vec![Capability::OutputStreaming]));
    let segment = committed("Streamed.");
    let id = segment.segment_id();
    output.offer(segment).expect("queued");
    output.end_utterance().expect("ends once");
    output.take_input().expect("the utterance has ended");
    for _ in 0..count {
        output
            .audio(id, frame_at(rate, sample_count))
            .expect("output streaming is claimed");
    }
    output.complete(id, Vec::new()).expect("completes");
    quantity(
        &recorded(output.usage().expect("synthesized")),
        QuantityKind::OutputAudioMs,
    )
    .value()
    .expect("measured")
}

#[test]
fn one_second_at_44_1_khz_in_single_sample_frames_counts_as_1000_ms() {
    assert_eq!(streamed_ms(44_100, 1, 44_100), 1_000);
}

#[test]
fn frames_shorter_than_a_millisecond_add_up() {
    assert_eq!(streamed_ms(48, 1, 48_000), 1, "48 samples at 48 kHz");
    assert_eq!(streamed_ms(3, 7, 22_050), 0, "21 samples at 22.05 kHz");
}

#[test]
fn a_frame_the_contract_refuses_is_not_counted() {
    let mut output = SynthesisOutput::start(&adapter(Vec::new()));
    let segment = committed("Refused.");
    let id = segment.segment_id();
    output.offer(segment).expect("queued");
    assert!(matches!(
        output.audio(id, frame_at(16_000, 160)),
        Err(AudioRefused::SegmentNotTaken(_))
    ));
    output.end_utterance().expect("ends once");
    output.take_input().expect("the utterance has ended");
    assert!(matches!(
        output.audio(id, frame_at(16_000, 160)),
        Err(AudioRefused::CapabilityNotClaimed(_))
    ));
    let mut short = frame_at(16_000, 160);
    short.data.pop();
    assert!(matches!(
        output.complete(id, vec![frame_at(16_000, 160), short]),
        Err(CompletionRefused::FrameLengthMismatch(_))
    ));
    assert_eq!(output.output_audio_ms(), 0, "nothing refused counts");
    output
        .complete(id, vec![frame_at(16_000, 160)])
        .expect("a well-formed frame completes");
    let recorded = recorded(output.usage().expect("synthesized"));
    assert_eq!(
        quantity(&recorded, QuantityKind::OutputAudioMs),
        Quantity::measured(QuantityKind::OutputAudioMs, 10)
    );
}

#[test]
fn characters_are_unicode_scalar_values_not_graphemes_or_bytes() {
    let mut output = SynthesisOutput::start(&adapter(Vec::new()));
    let text = "e\u{301}\u{1F44D}\u{1F3FD}";
    let segment = committed(text);
    let id = segment.segment_id();
    output.offer(segment).expect("queued");
    output.end_utterance().expect("ends once");
    output.take_input().expect("the utterance has ended");
    output.complete(id, Vec::new()).expect("completes");
    let recorded = recorded(output.usage().expect("synthesized"));
    assert_eq!(
        quantity(&recorded, QuantityKind::InputCharacters),
        Quantity::measured(QuantityKind::InputCharacters, 4),
        "two graphemes, four scalar values, eleven bytes"
    );
}

#[test]
fn playback_and_discard_stay_open_after_the_record_and_change_none_of_its_counts() {
    let mut output = SynthesisOutput::start(&adapter(Vec::new()));
    let played = committed("Played.");
    let played_id = played.segment_id();
    let dropped = committed("Dropped.");
    let dropped_id = dropped.segment_id();
    output.offer(played).expect("queued");
    output.offer(dropped).expect("queued");
    output.end_utterance().expect("ends once");
    output.take_input().expect("the utterance has ended");
    let events = output
        .complete(
            played_id,
            vec![frame_at(16_000, 160), frame_at(16_000, 160)],
        )
        .expect("completes");
    output
        .complete(dropped_id, vec![frame_at(16_000, 160)])
        .expect("completes");
    let first = recorded(output.usage().expect("synthesized"));
    let frames = events
        .into_iter()
        .filter_map(|event| match event {
            SynthesisEvent::Audio(audio) => Some(audio),
            _ => None,
        })
        .collect();
    output
        .acknowledge_playback(
            played_id,
            PlaybackReceipt::from_frames(frames).expect("one segment"),
        )
        .expect("the sink's move stays open");
    output
        .discard(dropped_id)
        .expect("the caller's move stays open");
    assert_eq!(
        output.segment(played_id).map(OutputSegment::state),
        Some(SegmentState::Played)
    );
    assert_eq!(
        output.offer(committed("Late.")),
        Err(OfferRefused::UtteranceEnded(UtteranceEnded))
    );
    assert_eq!(
        output.take_input().map(|taken| taken.len()),
        Ok(0),
        "nothing more is taken"
    );
    assert_eq!(output.input_characters(), 7 + 8);
    assert_eq!(output.output_audio_ms(), 30);
    assert_eq!(
        quantity(&first, QuantityKind::OutputAudioMs).value(),
        Some(output.output_audio_ms())
    );
    assert_eq!(
        output.usage(),
        Err(UsageRefused::UsageAlreadyRecorded(UsageAlreadyRecorded))
    );
}

#[test]
fn a_failed_backend_call_with_a_taken_segment_left_queued_is_recorded_without_its_discard() {
    let mut output = SynthesisOutput::start(&adapter(vec![Capability::OutputStreaming]));
    let segment = committed("Cut short.");
    let id = segment.segment_id();
    output.offer(segment).expect("queued");
    output.end_utterance().expect("ends once");
    output.take_input().expect("the utterance has ended");
    output
        .audio(id, frame_at(16_000, 160))
        .expect("output streaming is claimed");
    assert_eq!(
        output.usage(),
        Err(UsageRefused::SegmentNotSynthesized(SegmentNotSynthesized {
            segment_id: id
        })),
        "a call that has not failed is not recorded while its segment is Queued"
    );
    output.mark_failed();
    let recorded = recorded(
        output
            .usage()
            .expect("a failed call is recorded with its taken segment still Queued"),
    );
    output
        .discard(id)
        .expect("a Queued segment is discarded after the record");
    assert!(!recorded.r#final);
    assert_eq!(
        quantity(&recorded, QuantityKind::InputCharacters),
        Quantity::measured(QuantityKind::InputCharacters, 10)
    );
    assert_eq!(
        quantity(&recorded, QuantityKind::OutputAudioMs),
        Quantity::measured(QuantityKind::OutputAudioMs, 10)
    );
}

#[test]
fn the_contract_places_the_record_on_nothing_and_names_its_adapter_and_model() {
    let adapter = adapter(Vec::new());
    let mut output = SynthesisOutput::start(&adapter);
    output.end_utterance().expect("ends once");
    let recorded = recorded(output.usage().expect("nothing to synthesize"));
    assert_eq!(recorded.adapter_id, adapter.adapter_id());
    assert_eq!(recorded.model_id, adapter.model_id());
    assert_eq!(
        (
            recorded.run_id,
            recorded.node_id,
            recorded.session_id,
            recorded.speech_session_id,
            recorded.response_id,
            recorded.upstream_model.as_deref(),
        ),
        (None, None, None, None, None, None)
    );
    assert!(recorded.started_at_ms > 0);
    assert!(recorded.started_at_ms <= recorded.ended_at_ms);
    assert!(recorded.r#final);
    assert_eq!(
        quantity(&recorded, QuantityKind::Requests),
        Quantity::measured(QuantityKind::Requests, 1)
    );
    assert_eq!(
        quantity(&recorded, QuantityKind::LocalComputeMs),
        Quantity::unknown(QuantityKind::LocalComputeMs)
    );
    assert_eq!(recorded.quantities.len(), 4, "{recorded:?}");
}
