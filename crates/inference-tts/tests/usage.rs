//! The usage of a synthesis, measured by the contract (`story:usage-tts`): once the utterance has
//! ended and its last segment is synthesized, the synthesis records one `inference.usage`
//! record, its last event, with the Unicode scalar values of the segment text taken, the audio
//! emitted, counted exactly per sample rate and rounded down to the millisecond once, one request
//! and the local compute the implementation added, `unknown` when it added none. Segments
//! discarded after synthesis still count, since their audio was generated.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_tts::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_tts::inference_core::usage::{Quantity, QuantityKind, UsageRecorded};
use inference_tts::inference_text::OutputSegment;
use inference_tts::{
    AudioRefused, CONTRACT_REVISION, CompletionRefused, SegmentNotSynthesized, SynthesisEnded,
    SynthesisEvent, SynthesisOutput, UsageAlreadyRecorded, UsageRefused, UtteranceNotEnded,
};

/// 12 Unicode scalar values in 15 bytes.
const TWELVE: &str = "Grüße, café!";
/// 30 Unicode scalar values.
const THIRTY: &str = "How are you doing this sunday?";

fn adapter(capabilities: Vec<Capability>) -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "contract test".to_owned(),
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

/// A frame of `sample_count` samples per channel of silence at `rate` Hz on `channels` channels.
fn frame_at(rate: u32, channels: u16, sample_count: u32) -> AudioFrame {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).expect("not zero"),
        channels: NonZeroU16::new(channels).expect("not zero"),
    };
    AudioFrame {
        format,
        position: SampleClockPosition::new(0).expect("in range"),
        sample_count,
        data: vec![0; usize::try_from(format.data_len(sample_count)).expect("small")],
    }
}

/// A 10 ms frame at 16 kHz, mono.
fn frame() -> AudioFrame {
    frame_at(16_000, 1, 160)
}

/// The one quantity of `kind` in `recorded`.
fn quantity(recorded: &UsageRecorded, kind: QuantityKind) -> Quantity {
    let found: Vec<&Quantity> = recorded
        .quantities
        .iter()
        .filter(|quantity| quantity.kind() == kind)
        .collect();
    let [quantity] = found.as_slice() else {
        panic!("one {kind:?} quantity in {recorded:?}")
    };
    **quantity
}

fn recorded(event: SynthesisEvent) -> UsageRecorded {
    let SynthesisEvent::Usage(recorded) = event else {
        panic!("a usage record: {event:?}")
    };
    recorded
}

/// Offers `texts`, ends the utterance, takes every segment and completes each with `frames`
/// frames of [`frame`]; the synthesis is then ready to record its usage.
fn synthesised(adapter: &Adapter, texts: &[(&str, usize)]) -> SynthesisOutput {
    let mut output = SynthesisOutput::start(adapter);
    let mut ids = Vec::new();
    for &(text, _) in texts {
        let segment = committed(text);
        ids.push(segment.segment_id());
        output
            .offer(segment)
            .expect("a Committed segment is queued");
    }
    output.end_utterance().expect("the utterance ends once");
    output.take_input().expect("the utterance has ended");
    for (&id, &(_, frames)) in ids.iter().zip(texts) {
        output
            .complete(id, (0..frames).map(|_| frame()).collect())
            .expect("a taken Queued segment completes");
    }
    output
}

#[test]
fn an_utterance_of_two_segments_of_12_and_30_characters_records_42_characters_and_its_audio() {
    assert_eq!(TWELVE.chars().count(), 12);
    assert_ne!(TWELVE.len(), 12, "scalar values, not bytes");
    assert_eq!(THIRTY.chars().count(), 30);
    let adapter = adapter(Vec::new());
    let mut output = synthesised(&adapter, &[(TWELVE, 3), (THIRTY, 5)]);
    assert!(!output.usage_recorded());

    let before = output.segments().len();
    let recorded = recorded(output.usage().expect("the utterance is synthesized"));

    assert_eq!(
        quantity(&recorded, QuantityKind::InputCharacters),
        Quantity::measured(QuantityKind::InputCharacters, 42)
    );
    assert_eq!(
        quantity(&recorded, QuantityKind::OutputAudioMs),
        Quantity::measured(QuantityKind::OutputAudioMs, 80),
        "eight frames of 10 ms"
    );
    assert_eq!(
        quantity(&recorded, QuantityKind::Requests),
        Quantity::measured(QuantityKind::Requests, 1)
    );
    assert_eq!(
        quantity(&recorded, QuantityKind::LocalComputeMs),
        Quantity::unknown(QuantityKind::LocalComputeMs),
        "the implementation added no compute"
    );
    assert_eq!(recorded.quantities.len(), 4);
    assert_eq!(recorded.adapter_id, adapter.adapter_id());
    assert_eq!(recorded.model_id, adapter.model_id());
    assert!(recorded.r#final);
    assert!(recorded.started_at_ms <= recorded.ended_at_ms);
    assert_eq!(
        (recorded.run_id, recorded.node_id, recorded.session_id),
        (None, None, None),
        "the contract places it on no run, node or session"
    );
    assert!(output.usage_recorded());
    assert_eq!(output.input_characters(), 42);
    assert_eq!(output.output_audio_ms(), 80);
    assert_eq!(output.segments().len(), before);
}

#[test]
fn the_usage_is_refused_before_the_utterance_ends_and_while_a_segment_is_not_synthesized() {
    let mut output = SynthesisOutput::start(&adapter(vec![Capability::IncrementalInput]));
    let first = committed(TWELVE);
    let first_id = first.segment_id();
    output.offer(first).expect("queued");
    output.take_input().expect("incremental input is claimed");
    output
        .complete(first_id, vec![frame()])
        .expect("a taken Queued segment completes");
    assert_eq!(
        output.usage(),
        Err(UsageRefused::UtteranceNotEnded(UtteranceNotEnded))
    );

    let second = committed(THIRTY);
    let second_id = second.segment_id();
    output.offer(second).expect("queued");
    output.end_utterance().expect("the utterance ends once");
    assert_eq!(
        output.usage(),
        Err(UsageRefused::SegmentNotSynthesized(SegmentNotSynthesized {
            segment_id: second_id,
        })),
        "a segment not yet taken is not synthesized"
    );
    output.take_input().expect("the utterance has ended");
    assert_eq!(
        output.usage(),
        Err(UsageRefused::SegmentNotSynthesized(SegmentNotSynthesized {
            segment_id: second_id,
        })),
        "a taken Queued segment is not synthesized"
    );
    assert!(!output.usage_recorded());
    output
        .complete(second_id, vec![frame(), frame()])
        .expect("a taken Queued segment completes");
    let recorded = recorded(output.usage().expect("every segment is synthesized"));
    assert_eq!(
        quantity(&recorded, QuantityKind::InputCharacters),
        Quantity::measured(QuantityKind::InputCharacters, 42)
    );
    assert_eq!(
        quantity(&recorded, QuantityKind::OutputAudioMs),
        Quantity::measured(QuantityKind::OutputAudioMs, 30)
    );
}

#[test]
fn the_usage_is_the_last_event_of_the_synthesis() {
    let mut output = synthesised(&adapter(vec![Capability::OutputStreaming]), &[(TWELVE, 1)]);
    let id = output.segments()[0].segment_id();
    recorded(output.usage().expect("the utterance is synthesized"));

    assert_eq!(
        output.usage(),
        Err(UsageRefused::UsageAlreadyRecorded(UsageAlreadyRecorded))
    );
    assert_eq!(
        output.audio(id, frame()),
        Err(AudioRefused::SynthesisEnded(SynthesisEnded))
    );
    assert_eq!(
        output.complete(id, Vec::new()),
        Err(CompletionRefused::SynthesisEnded(SynthesisEnded))
    );
    assert_eq!(output.frames_emitted(id), Some(1), "nothing was emitted");
    assert_eq!(output.output_audio_ms(), 10);
}

#[test]
fn segments_discarded_after_synthesis_still_count_and_segments_never_taken_do_not() {
    let mut output = SynthesisOutput::start(&adapter(vec![Capability::IncrementalInput]));
    let spoken = committed(TWELVE);
    let spoken_id = spoken.segment_id();
    output.offer(spoken).expect("queued");
    output.take_input().expect("incremental input is claimed");
    output
        .complete(spoken_id, vec![frame(), frame()])
        .expect("a taken Queued segment completes");
    output
        .discard(spoken_id)
        .expect("a Synthesized segment is discarded");

    let never_taken = committed(THIRTY);
    let never_taken_id = never_taken.segment_id();
    output.offer(never_taken).expect("queued");
    output
        .discard(never_taken_id)
        .expect("a Queued segment is discarded");

    let taken_unspoken = committed("Four.");
    let taken_unspoken_id = taken_unspoken.segment_id();
    output.offer(taken_unspoken).expect("queued");
    output.end_utterance().expect("the utterance ends once");
    let taken = output.take_input().expect("the utterance has ended");
    assert_eq!(taken.len(), 1, "a discarded segment is not taken");
    output
        .discard(taken_unspoken_id)
        .expect("a taken Queued segment is discarded");

    let recorded = recorded(output.usage().expect("no segment is Queued"));
    assert_eq!(
        quantity(&recorded, QuantityKind::InputCharacters),
        Quantity::measured(QuantityKind::InputCharacters, 12 + 5),
        "the text taken counts, discarded after or not; the text never taken does not"
    );
    assert_eq!(
        quantity(&recorded, QuantityKind::OutputAudioMs),
        Quantity::measured(QuantityKind::OutputAudioMs, 20),
        "the audio generated of a segment discarded after synthesis counts"
    );
}

#[test]
fn audio_emitted_while_queued_counts_as_audio_emitted_at_completion_does() {
    let mut output = SynthesisOutput::start(&adapter(vec![Capability::OutputStreaming]));
    let segment = committed(TWELVE);
    let id = segment.segment_id();
    output.offer(segment).expect("queued");
    output.end_utterance().expect("the utterance ends once");
    output.take_input().expect("the utterance has ended");
    output
        .audio(id, frame())
        .expect("output streaming is claimed");
    output
        .audio(id, frame())
        .expect("output streaming is claimed");
    output
        .complete(id, vec![frame()])
        .expect("a taken Queued segment completes");
    let recorded = recorded(output.usage().expect("the utterance is synthesized"));
    assert_eq!(
        quantity(&recorded, QuantityKind::OutputAudioMs),
        Quantity::measured(QuantityKind::OutputAudioMs, 30)
    );
}

/// The output audio milliseconds of one segment spoken as `frames`, streamed.
fn audio_ms_of(frames: Vec<AudioFrame>) -> i64 {
    let mut output = SynthesisOutput::start(&adapter(vec![Capability::OutputStreaming]));
    let segment = committed(TWELVE);
    let id = segment.segment_id();
    output.offer(segment).expect("queued");
    output.end_utterance().expect("the utterance ends once");
    output.take_input().expect("the utterance has ended");
    for frame in frames {
        output
            .audio(id, frame)
            .expect("output streaming is claimed");
    }
    output.complete(id, Vec::new()).expect("completes");
    let recorded = recorded(output.usage().expect("the utterance is synthesized"));
    let ms = quantity(&recorded, QuantityKind::OutputAudioMs);
    assert_eq!(ms.value(), Some(output.output_audio_ms()));
    ms.value().expect("measured")
}

#[test]
fn audio_time_is_counted_exactly_per_sample_rate_and_rounded_down_once() {
    assert_eq!(
        audio_ms_of((0..3).map(|_| frame_at(44_100, 1, 147)).collect()),
        10,
        "441 samples at 44.1 kHz are 10 ms; per frame, 3 ms each would make 9"
    );
    assert_eq!(
        audio_ms_of(vec![frame_at(16_000, 2, 160)]),
        10,
        "samples per channel: 160 at 16 kHz are 10 ms on two channels as on one"
    );
    assert_eq!(
        audio_ms_of(vec![
            frame_at(44_100, 1, 147),
            frame_at(16_000, 1, 8),
            frame_at(44_100, 1, 147),
        ]),
        7,
        "294 samples at 44.1 kHz and 8 at 16 kHz are 7.17 ms; rounded per rate, 6 and 0 \
         would make 6"
    );
}

#[test]
fn local_compute_is_measured_once_added_and_a_failed_backend_call_is_not_final() {
    let mut output = synthesised(&adapter(Vec::new()), &[(TWELVE, 1)]);
    assert_eq!(output.local_compute(), None);
    output.add_local_compute(Duration::from_millis(7));
    output.add_local_compute(Duration::from_micros(5_500));
    assert_eq!(output.local_compute(), Some(Duration::from_micros(12_500)));
    assert!(!output.failed());
    output.mark_failed();
    assert!(output.failed());
    let recorded = recorded(output.usage().expect("the utterance is synthesized"));
    assert_eq!(
        quantity(&recorded, QuantityKind::LocalComputeMs),
        Quantity::measured(QuantityKind::LocalComputeMs, 12)
    );
    assert!(
        !recorded.r#final,
        "a failed backend call's counts are not final"
    );
}

#[test]
fn an_empty_utterance_records_no_characters_and_no_audio() {
    let mut output = SynthesisOutput::start(&adapter(Vec::new()));
    output.end_utterance().expect("the utterance ends once");
    let recorded = recorded(output.usage().expect("nothing is left to synthesize"));
    assert_eq!(
        quantity(&recorded, QuantityKind::InputCharacters),
        Quantity::measured(QuantityKind::InputCharacters, 0)
    );
    assert_eq!(
        quantity(&recorded, QuantityKind::OutputAudioMs),
        Quantity::measured(QuantityKind::OutputAudioMs, 0)
    );
}

#[test]
fn a_usage_event_names_no_segment() {
    let mut output = synthesised(&adapter(Vec::new()), &[(TWELVE, 1)]);
    let event = output.usage().expect("the utterance is synthesized");
    assert_eq!(event.segment_id(), None);
    assert_eq!(event.name(), UsageRecorded::KIND);
}

#[test]
fn a_failed_backend_call_is_recorded_with_its_taken_segments_still_queued_and_ends_the_synthesis() {
    let mut output = SynthesisOutput::start(&adapter(vec![Capability::OutputStreaming]));
    let spoken = committed(TWELVE);
    let spoken_id = spoken.segment_id();
    let cut = committed(THIRTY);
    let cut_id = cut.segment_id();
    output.offer(spoken).expect("queued");
    output.offer(cut).expect("queued");
    output.end_utterance().expect("the utterance ends once");
    output.take_input().expect("the utterance has ended");
    output
        .complete(spoken_id, vec![frame()])
        .expect("a taken Queued segment completes");
    output
        .audio(cut_id, frame())
        .expect("output streaming is claimed");
    assert_eq!(
        output.usage(),
        Err(UsageRefused::SegmentNotSynthesized(SegmentNotSynthesized {
            segment_id: cut_id,
        })),
        "a call that has not failed waits for its last segment"
    );

    output.mark_failed();
    let recorded = recorded(
        output
            .usage()
            .expect("a failed call will not synthesize the rest"),
    );
    assert!(!recorded.r#final);
    assert_eq!(
        quantity(&recorded, QuantityKind::InputCharacters),
        Quantity::measured(QuantityKind::InputCharacters, 42),
        "the text of a taken segment left Queued counts"
    );
    assert_eq!(
        quantity(&recorded, QuantityKind::OutputAudioMs),
        Quantity::measured(QuantityKind::OutputAudioMs, 20),
        "the audio emitted counts, and no more"
    );
    assert_eq!(
        output.audio(cut_id, frame()),
        Err(AudioRefused::SynthesisEnded(SynthesisEnded))
    );
    assert_eq!(
        output.complete(cut_id, Vec::new()),
        Err(CompletionRefused::SynthesisEnded(SynthesisEnded))
    );
    assert_eq!(
        output.usage(),
        Err(UsageRefused::UsageAlreadyRecorded(UsageAlreadyRecorded))
    );
    assert_eq!(output.output_audio_ms(), 20);
}

#[test]
fn a_failed_backend_call_still_waits_for_a_segment_it_has_not_taken() {
    let mut output = SynthesisOutput::start(&adapter(Vec::new()));
    let segment = committed(TWELVE);
    let id = segment.segment_id();
    output.offer(segment).expect("queued");
    output.end_utterance().expect("the utterance ends once");
    output.mark_failed();
    assert_eq!(
        output.usage(),
        Err(UsageRefused::SegmentNotSynthesized(SegmentNotSynthesized {
            segment_id: id,
        })),
        "a segment never taken is the implementation's to take"
    );
    output.take_input().expect("the utterance has ended");
    let recorded = recorded(output.usage().expect("a taken segment of a failed call"));
    assert!(!recorded.r#final);
    assert_eq!(
        quantity(&recorded, QuantityKind::InputCharacters),
        Quantity::measured(QuantityKind::InputCharacters, 12)
    );
}
