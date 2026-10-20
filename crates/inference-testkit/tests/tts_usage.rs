//! The usage of a synthesis on the mock synthesiser (`story:usage-tts`): an utterance of two
//! committed segments of 12 and 30 characters ends with one `SynthesisEvent::Usage` whose
//! `input_characters` is 42 and whose `output_audio_ms` is the audio the mock emitted, both
//! `measured`, with `requests` 1 and `local_compute_ms` `unknown`, the mock computing nothing of
//! its own; a segment discarded after synthesis still counts; and the TTS node given a `usage`
//! port, exposed as an egress, delivers the record there with the run and the node set, as the
//! model node of `story:usage-llm` does.

#![cfg(all(feature = "tts", feature = "run"))]

use std::collections::HashMap;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32, NonZeroUsize};
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::{Generation, GenerationStarted, StartGeneration};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_core::usage::{Quantity, QuantityKind, UsageRecorded};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeRole, OverloadPolicy,
    PipelineSpec, PlannedPipeline, PortId, PortTraits, modality,
};
use inference_runtime::RunController;
use inference_runtime::run::RunState;
use inference_testkit::graph::RunLog;
use inference_testkit::stream::StreamEvent;
use inference_testkit::tts::{
    FRAME_SAMPLES, MockConfiguration, MockSynthesiser, Spoken, TtsNode, TtsPayload,
};
use inference_text::{OutputSegment, SegmentId, SegmentState, TextDelta, TextSegmentCommitted};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat, AudioFrame};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

/// 12 Unicode scalar values, two words.
const TWELVE: &str = "Grüße, café!";
/// 30 Unicode scalar values, six words.
const THIRTY: &str = "How are you doing this sunday?";

fn format(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).expect("not zero"),
        channels: NonZeroU16::new(1).expect("not zero"),
    }
}

fn committed(texts: &[&str]) -> Vec<OutputSegment> {
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    texts
        .iter()
        .zip(1..)
        .map(|(text, sequence)| {
            let (tentative, _) = OutputSegment::emit(&generation, sequence, (*text).to_owned())
                .expect("the generation is Active");
            tentative.commit().expect("a Tentative segment commits").0
        })
        .collect()
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

/// Milliseconds of `samples` samples per channel at `rate` Hz, rounded down once.
fn ms(samples: u64, rate: u32) -> i64 {
    i64::try_from(samples * 1_000 / u64::from(rate)).expect("short")
}

/// Asserts that `recorded` is the usage of 42 characters and `audio_ms` of audio on the mock
/// `adapter`.
fn assert_usage(recorded: &UsageRecorded, adapter: &Adapter, audio_ms: i64) {
    assert_eq!(
        quantity(recorded, QuantityKind::InputCharacters),
        Quantity::measured(QuantityKind::InputCharacters, 42)
    );
    assert_eq!(
        quantity(recorded, QuantityKind::OutputAudioMs),
        Quantity::measured(QuantityKind::OutputAudioMs, audio_ms)
    );
    assert_eq!(
        quantity(recorded, QuantityKind::Requests),
        Quantity::measured(QuantityKind::Requests, 1)
    );
    assert_eq!(
        quantity(recorded, QuantityKind::LocalComputeMs),
        Quantity::unknown(QuantityKind::LocalComputeMs),
        "the mock computes nothing of its own"
    );
    assert_eq!(recorded.adapter_id, adapter.adapter_id());
    assert_eq!(recorded.model_id, adapter.model_id());
    assert!(recorded.r#final);
}

#[test]
fn an_utterance_of_two_segments_of_12_and_30_characters_on_the_mock_ends_with_its_usage() {
    assert_eq!((TWELVE.chars().count(), THIRTY.chars().count()), (12, 30));
    for rate in [16_000, 44_100] {
        for configuration in MockConfiguration::ALL {
            let mut synthesiser =
                MockSynthesiser::new(configuration, ModelId::generate(), format(rate));
            let mut handle = synthesiser
                .start()
                .expect("a fresh synthesiser runs nothing else");
            let mut events = Vec::new();
            for segment in committed(&[TWELVE, THIRTY]) {
                handle.offer_segment(segment).expect("a Committed segment");
                events.extend(handle.by_ref());
            }
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event, SynthesisEvent::Usage(_))),
                "{configuration:?}: no usage is recorded before the end of the utterance"
            );
            handle.end_utterance().expect("the utterance ends once");
            events.extend(handle.by_ref());

            let samples: u64 = events
                .iter()
                .filter_map(|event| match event {
                    SynthesisEvent::Audio(audio) => Some(u64::from(audio.frame().sample_count)),
                    _ => None,
                })
                .sum();
            assert_eq!(
                samples,
                8 * u64::from(FRAME_SAMPLES),
                "{configuration:?}: a frame per word"
            );
            let usage = events
                .iter()
                .filter(|event| matches!(event, SynthesisEvent::Usage(_)))
                .count();
            assert_eq!(usage, 1, "{configuration:?}: one record: {events:?}");
            let Some(SynthesisEvent::Usage(recorded)) = events.last() else {
                panic!("{configuration:?}: the usage record is the last event: {events:?}")
            };
            assert_usage(recorded, synthesiser.adapter(), ms(samples, rate));
            assert_eq!(
                (recorded.run_id, recorded.node_id),
                (None, None),
                "the contract places it on no run or node"
            );
            assert!(handle.output().usage_recorded());
            assert!(
                handle.next().is_none(),
                "{configuration:?}: nothing follows the record"
            );
        }
    }
}

#[test]
fn a_segment_discarded_after_synthesis_on_the_mock_still_counts() {
    for configuration in MockConfiguration::ALL {
        let mut synthesiser =
            MockSynthesiser::new(configuration, ModelId::generate(), format(16_000));
        let mut handle = synthesiser
            .start()
            .expect("a fresh synthesiser runs nothing else");
        let segments = committed(&[TWELVE, THIRTY]);
        let first = segments[0].segment_id();
        for segment in segments {
            handle.offer_segment(segment).expect("a Committed segment");
        }
        handle.end_utterance().expect("the utterance ends once");
        let mut events = Vec::new();
        for event in handle.by_ref() {
            let completed =
                matches!(&event, SynthesisEvent::Completed(done) if done.segment_id == first);
            events.push(event);
            if completed {
                break;
            }
        }
        assert_eq!(
            handle.output().segment(first).map(OutputSegment::state),
            Some(SegmentState::Synthesized)
        );
        handle
            .discard(first)
            .expect("a Synthesized segment is discarded");
        events.extend(handle.by_ref());
        let Some(SynthesisEvent::Usage(recorded)) = events.last() else {
            panic!("{configuration:?}: the usage record is the last event: {events:?}")
        };
        assert_usage(
            recorded,
            synthesiser.adapter(),
            ms(8 * u64::from(FRAME_SAMPLES), 16_000),
        );
    }
}

#[test]
fn a_synthesis_dropped_before_its_utterance_ended_records_no_usage() {
    let mut synthesiser = MockSynthesiser::new(
        MockConfiguration::IncrementalStreaming,
        ModelId::generate(),
        format(16_000),
    );
    let mut handle = synthesiser
        .start()
        .expect("a fresh synthesiser runs nothing else");
    let mut events = Vec::new();
    for segment in committed(&[TWELVE, THIRTY]) {
        handle.offer_segment(segment).expect("a Committed segment");
        events.extend(handle.by_ref());
    }
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, SynthesisEvent::Usage(_))),
        "{events:?}"
    );
    assert!(!handle.output().usage_recorded());
    drop(handle);
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes: the runtime commits to no executor.
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park();
    }
}

/// The payload of the run: `inference.stream` events, frames, usage records and control events.
#[derive(Debug, Clone)]
enum Payload {
    Stream(StreamEvent),
    Audio(AudioFrame),
    Usage(UsageRecorded),
    Control(ControlKind),
}

impl From<ControlKind> for Payload {
    fn from(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl TtsPayload for Payload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn audio(frame: AudioFrame) -> Self {
        Self::Audio(frame)
    }

    fn usage(recorded: UsageRecorded) -> Option<Self> {
        Some(Self::Usage(recorded))
    }

    fn spoken(self) -> Spoken {
        match self {
            Self::Stream(event) => Spoken::Stream(event),
            Self::Control(kind) => Spoken::Control(kind),
            Self::Audio(_) | Self::Usage(_) => Spoken::Other,
        }
    }
}

struct Fixture {
    profile: DeploymentProfile,
    adapter: Adapter,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        (self.adapter.adapter_id() == adapter_id).then_some(&self.adapter)
    }
}

const TEXT: &str = "text";
const AUDIO: &str = "audio";
const USAGE: &str = "usage";

/// The TTS node alone, its input exposed as the ingress [`TEXT`] and its outputs as the egresses
/// [`AUDIO`] and [`USAGE`], the last pausing on overload so that no record is dropped; the
/// identity of the node and of its usage port.
fn planned(adapter: &Adapter) -> (Declaration, PlannedPipeline, NodeId, PortId) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let adapter_id = adapter.adapter_id();
    let catalog = Fixture {
        profile,
        adapter: adapter.clone(),
    };
    let traits = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let control = PortTraits {
        incremental: false,
        ..traits
    };
    let spec = PipelineSpec {
        name: "exposed synthesiser with usage".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let wide = NonZeroUsize::new(256).expect("256");
    let mut declaration = Declaration::new();
    let (pipeline_id, ids) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let tts = pipeline.node(
                "tts",
                NodeRole::Model,
                vec![Capability::IncrementalInput, Capability::OutputStreaming],
            );
            let text = pipeline.input::<modality::Text>(&tts, "text", traits);
            let audio = pipeline.output::<modality::Audio>(&tts, "audio", traits);
            let usage = pipeline.output::<modality::Structured>(&tts, "usage", control);
            pipeline.expose(&text, TEXT, wide, OverloadPolicy::Pause);
            pipeline.expose(&audio, AUDIO, wide, OverloadPolicy::Pause);
            pipeline.expose(&usage, USAGE, wide, OverloadPolicy::Pause);
            pipeline
                .bind(&tts, adapter_id)
                .expect("the catalog holds the mock synthesiser's adapter");
            (tts.id(), usage.id())
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("a synthesiser between an ingress and two egresses is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the synthesiser is bound to an adapter that claims what the node needs");
    let (node, usage) = ids;
    (declaration, planned, node, usage)
}

/// What the application sends on the text ingress: one stream of an LLM node's output.
struct Sending {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Sending {
    fn envelope(&mut self, kind: &str, payload: Payload) -> EventEnvelope<Payload> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: TEXT.to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp: self.sequence,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }

    fn stream(&mut self, event: StreamEvent) -> EventEnvelope<Payload> {
        self.envelope(event.name(), Payload::Stream(event))
    }
}

/// Every envelope `egress` delivers until its end is gone.
fn drained(mut egress: Box<dyn InputEndpoint<Payload>>) -> Vec<EventEnvelope<Payload>> {
    let mut received = Vec::new();
    while let Some(envelope) = block_on(egress.recv()) {
        received.push(envelope);
    }
    received
}

#[test]
fn the_usage_of_a_synthesis_reaches_the_exposed_usage_egress_with_run_and_node_set() {
    let rate = 16_000;
    let synthesiser = MockSynthesiser::new(
        MockConfiguration::IncrementalStreaming,
        ModelId::generate(),
        format(rate),
    );
    let adapter = synthesiser.adapter().clone();
    let (declaration, planned, tts, usage_port) = planned(&adapter);
    let run_id = RunId::generate();
    let mut node = Some(TtsNode::new(run_id, synthesiser, RunLog::new()).with_usage(usage_port));
    let mut controller = RunController::create_as(
        run_id,
        &planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| Box::new(node.take().expect("the pipeline has one node")),
    )
    .expect("the pipeline is planned and no exposure cancels or supersedes");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("the synthesiser prepares");
    let mut ends = controller.exposed();
    let text: Box<dyn OutputEndpoint<Payload>> = ends.ingress(TEXT).expect("the text ingress");
    let audio = ends.egress(AUDIO).expect("the audio egress");
    let usage = ends.egress(USAGE).expect("the usage egress");
    block_on(controller.activate()).expect("the synthesiser starts with the inner ends");
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });

    let mut sending = Sending {
        run_id,
        producer: NodeId::generate(),
        stream_id: StreamId::generate(),
        sequence: 0,
    };
    let generation_id = GenerationId::generate();
    block_on(text.send(
        sending.stream(StreamEvent::GenerationStarted(GenerationStarted {
            generation_id,
            run_id,
            epoch: 1,
        })),
    ))
    .expect("the text ingress takes the start");
    for (sequence, words) in [TWELVE, THIRTY].into_iter().enumerate() {
        let segment_id = SegmentId::generate();
        block_on(text.send(sending.stream(StreamEvent::TextDelta(TextDelta {
            segment_id,
            generation_id,
            sequence: i64::try_from(sequence).expect("few") + 1,
            text: words.to_owned(),
        }))))
        .expect("the text ingress takes the delta");
        block_on(text.send(sending.stream(StreamEvent::TextSegmentCommitted(
            TextSegmentCommitted {
                segment_id,
                text: words.to_owned(),
            },
        ))))
        .expect("the text ingress takes the commit");
    }
    let stop = sending.envelope(
        ControlKind::Stop.as_str(),
        Payload::Control(ControlKind::Stop),
    );
    block_on(text.send(stop)).expect("the text ingress takes the end");
    let spoken = drained(audio);
    let received = drained(usage);
    drop((text, ends));
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    result.expect("the drive loop ends the run");
    assert_eq!(controller.run().state(), RunState::Completed);

    let samples: u64 = spoken
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            Payload::Audio(frame) => Some(u64::from(frame.sample_count)),
            _ => None,
        })
        .sum();
    assert_eq!(samples, 8 * u64::from(FRAME_SAMPLES), "a frame per word");
    assert!(
        spoken
            .iter()
            .all(|envelope| !matches!(envelope.payload, Payload::Usage(_))),
        "{spoken:?}"
    );
    assert!(
        received
            .last()
            .is_some_and(|envelope| envelope.kind == ControlKind::Stop.as_str()),
        "the end of input reaches the usage egress last: {received:?}"
    );
    let records: Vec<(&EventEnvelope<Payload>, &UsageRecorded)> = received
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            Payload::Usage(recorded) => Some((envelope, recorded)),
            _ => None,
        })
        .collect();
    let [(envelope, record)] = records.as_slice() else {
        panic!("one record for the one synthesis on the usage egress: {received:?}")
    };
    assert_eq!(envelope.kind, UsageRecorded::KIND);
    assert_eq!(envelope.run_id, run_id);
    assert_eq!(envelope.producer, tts);
    assert_eq!(envelope.sequence, 1, "the usage port's own stream, from 1");
    assert_eq!(record.run_id, Some(run_id), "the node places it on its run");
    assert_eq!(record.node_id, Some(tts), "the node names itself");
    assert_eq!(
        record.session_id, None,
        "the run's envelopes name no session"
    );
    assert_usage(record, &adapter, ms(samples, rate));
}
