//! Every event a synthesis yields is named by an event `ess/` declares
//! (`story:contract-usage-and-naming-drift`): the name is the `kind` of its envelope, and an
//! envelope's `kind` is the declared name of the payload's event, never a type's.

use std::num::{NonZeroU16, NonZeroU32};

use inference_tts::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_tts::inference_text::OutputSegment;
use inference_tts::{CONTRACT_REVISION, SynthesisEvent, SynthesisOutput};

const DOMAINS: [&str; 3] = [
    include_str!("../../../ess/domains/audio.yaml"),
    include_str!("../../../ess/domains/stream.yaml"),
    include_str!("../../../ess/domains/usage.yaml"),
];

/// The names declared under the top-level `events:` key of a domain document.
fn declared_events(domain: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut inside = false;
    for line in domain.lines() {
        if !line.starts_with(' ') && !line.starts_with('#') && !line.is_empty() {
            inside = line == "events:";
            continue;
        }
        if inside && let Some(name) = line.strip_prefix("  - name: ") {
            names.push(name.trim().to_owned());
        }
    }
    names
}

fn frame() -> AudioFrame {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("not zero"),
        channels: NonZeroU16::new(1).expect("not zero"),
    };
    AudioFrame {
        format,
        position: SampleClockPosition::new(0).expect("in range"),
        sample_count: 160,
        data: vec![0; usize::try_from(format.data_len(160)).expect("a small frame")],
    }
}

/// One event of each kind a synthesis yields: a frame, a completion and its usage record.
fn every_kind_of_event() -> Vec<SynthesisEvent> {
    let adapter = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "contract test".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: vec![Capability::OutputStreaming],
        max_concurrency: 1,
    })
    .expect("one is positive")
    .0;
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (tentative, _) =
        OutputSegment::emit(&generation, 1, "hello".to_owned()).expect("the generation is Active");
    let segment = tentative.commit().expect("a Tentative segment commits").0;
    let segment_id = segment.segment_id();
    let mut output = SynthesisOutput::start(&adapter);
    output
        .offer(segment)
        .expect("a Committed segment is queued");
    output.end_utterance().expect("the utterance ends once");
    output.take_input().expect("the utterance has ended");
    let mut events = vec![
        output
            .audio(segment_id, frame())
            .expect("a taken Queued segment streams"),
    ];
    events.extend(
        output
            .complete(segment_id, Vec::new())
            .expect("a taken Queued segment completes"),
    );
    events.push(output.usage().expect("the utterance is synthesized"));
    events
}

#[test]
fn every_synthesis_event_is_named_by_an_event_ess_declares() {
    let declared: Vec<String> = DOMAINS.iter().flat_map(|d| declared_events(d)).collect();
    assert!(
        declared
            .iter()
            .any(|name| name == "inference.usage.UsageRecorded"),
        "the reader finds the declared events: {declared:?}"
    );
    let events = every_kind_of_event();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SynthesisEvent::Audio(_))),
        "{events:?}"
    );
    for event in &events {
        assert!(
            declared.iter().any(|name| name == event.name()),
            "{} is no event ess/ declares (story:contract-usage-and-naming-drift): {declared:?}",
            event.name()
        );
    }
}
