//! The usage of a recognition on the mock recogniser (`story:usage-stt`): a recognition of 1.5 s
//! of audio ends, once input has ended, with one `TranscriptEvent::Usage` whose `input_audio_ms`
//! is 1500 `measured`, `requests` 1 and `local_compute_ms` `unknown`, the mock computing nothing
//! of its own; and the STT node given a `usage` port, exposed as an egress, delivers the record
//! there with the run and the node set, as the model node of `story:usage-llm` does.

#![cfg(all(feature = "stt", feature = "run"))]

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference_core::endpoint::OutputEndpoint;
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
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
use inference_stt::inference_audio::AudioFrame;
use inference_stt::inference_text::ReviseSpan;
use inference_stt::{
    Recogniser, RecognitionEnded, RecognitionHandle, RecognitionRequest, RevisionRefused,
    TranscriptEvent,
};
use inference_testkit::graph::RunLog;
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::{
    Cue, FRAME_SAMPLES, Heard, MockConfiguration, MockRecogniser, ScriptStep, SttNode, SttPayload,
    SttPorts, frames, script,
};
use inference_text::{TurnCompleted, TurnId, TurnOpened};

/// 1.5 s of the fixture's 10 ms frames.
const FRAMES: usize = 150;

fn request() -> RecognitionRequest {
    RecognitionRequest::new(
        RunId::generate(),
        TurnId::generate(),
        StreamId::generate(),
        NodeId::generate(),
        "transcript",
    )
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

/// Asserts that `recorded` is the usage of 1.5 s of audio on the mock `adapter`.
fn assert_one_and_a_half_seconds(recorded: &UsageRecorded, adapter: &Adapter) {
    assert_eq!(
        quantity(recorded, QuantityKind::InputAudioMs),
        Quantity::measured(QuantityKind::InputAudioMs, 1_500)
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
fn a_recognition_of_one_and_a_half_seconds_on_the_mock_ends_with_its_usage() {
    for configuration in MockConfiguration::ALL {
        let mut recogniser = MockRecogniser::new(configuration, ModelId::generate(), script());
        let mut handle = recogniser
            .start(request())
            .expect("a fresh recogniser runs nothing else");
        let mut before_end = Vec::new();
        for frame in frames(FRAMES) {
            handle.offer_frame(frame).expect("input has not ended");
            before_end.extend(handle.by_ref());
        }
        assert!(
            before_end
                .iter()
                .all(|envelope| !matches!(envelope.payload, TranscriptEvent::Usage(_))),
            "{configuration:?}: no usage is recorded before the end of input: {before_end:?}"
        );
        handle.end_input().expect("input ends once");
        let mut events = before_end;
        events.extend(handle.by_ref());

        let usage: Vec<&EventEnvelope<TranscriptEvent>> = events
            .iter()
            .filter(|envelope| matches!(envelope.payload, TranscriptEvent::Usage(_)))
            .collect();
        assert_eq!(usage.len(), 1, "{configuration:?}: one record: {events:?}");
        let last = events.last().expect("the recognition delivered events");
        let TranscriptEvent::Usage(recorded) = &last.payload else {
            panic!("{configuration:?}: the usage record is the last event: {events:?}")
        };
        assert_eq!(last.kind, UsageRecorded::KIND);
        assert_eq!(
            last.sequence,
            events[events.len() - 2].sequence + 1,
            "{configuration:?}: at the next sequence of the stream"
        );
        assert!(
            matches!(
                events[events.len() - 2].payload,
                TranscriptEvent::Committed(_)
            ),
            "{configuration:?}: after the last commit"
        );
        assert_one_and_a_half_seconds(recorded, recogniser.adapter());
        assert_eq!(recorded.run_id, None, "the contract places it on no run");
        assert!(
            handle.next().is_none(),
            "{configuration:?}: nothing follows the record"
        );
    }
}

#[test]
fn after_its_usage_record_a_recognition_refuses_a_revision_through_its_handle() {
    for configuration in MockConfiguration::ALL {
        let mut recogniser = MockRecogniser::new(configuration, ModelId::generate(), script());
        let mut handle = recogniser
            .start(request())
            .expect("a fresh recogniser runs nothing else");
        for frame in frames(6) {
            handle.offer_frame(frame).expect("input has not ended");
        }
        handle.end_input().expect("input ends once");
        let events: Vec<EventEnvelope<TranscriptEvent>> = handle.by_ref().collect();
        assert!(
            matches!(
                events.last().map(|envelope| &envelope.payload),
                Some(TranscriptEvent::Usage(_))
            ),
            "{configuration:?}: {events:?}"
        );
        let span_id = handle.output().spans()[0].span_id();

        assert_eq!(
            handle.revise(ReviseSpan {
                span_id,
                text: "a correction".to_owned(),
            }),
            Err(RevisionRefused::RecognitionEnded(RecognitionEnded)),
            "{configuration:?}: the usage record is the last event"
        );
        assert_eq!(handle.next(), None, "{configuration:?}: nothing follows it");
    }
}

#[test]
fn a_script_that_leaves_a_span_uncommitted_records_no_usage() {
    for configuration in MockConfiguration::ALL {
        let cues = vec![
            Cue {
                after_frames: 1,
                step: ScriptStep::Hypothesis("one".to_owned()),
            },
            Cue {
                after_frames: 2,
                step: ScriptStep::Commit { span: 0 },
            },
            Cue {
                after_frames: 3,
                step: ScriptStep::Hypothesis("left open".to_owned()),
            },
        ];
        let mut handle = MockRecogniser::new(configuration, ModelId::generate(), cues)
            .start(request())
            .expect("a fresh recogniser runs nothing else");
        for frame in frames(3) {
            handle.offer_frame(frame).expect("input has not ended");
        }
        handle.end_input().expect("input ends once");
        let events: Vec<EventEnvelope<TranscriptEvent>> = handle.by_ref().collect();
        assert_eq!(
            events.len(),
            3,
            "{configuration:?}: the three cues: {events:?}"
        );
        assert!(
            events
                .iter()
                .all(|envelope| !matches!(envelope.payload, TranscriptEvent::Usage(_))),
            "{configuration:?}: the last span is not committed, so no usage is recorded"
        );
        assert!(!handle.output().usage_recorded());
    }
}

#[test]
fn a_recognition_dropped_before_its_input_ended_records_no_usage() {
    let mut recogniser = MockRecogniser::new(
        MockConfiguration::Incremental,
        ModelId::generate(),
        script(),
    );
    let mut handle = recogniser
        .start(request())
        .expect("a fresh recogniser runs nothing else");
    let mut events = Vec::new();
    for frame in frames(FRAMES) {
        handle.offer_frame(frame).expect("input has not ended");
        events.extend(handle.by_ref());
    }
    assert_eq!(events.len(), 6, "every cue ran: {events:?}");
    assert!(
        events
            .iter()
            .all(|envelope| !matches!(envelope.payload, TranscriptEvent::Usage(_))),
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

/// The payload of the run: frames, `inference.stream` events, usage records and control events.
#[derive(Debug, Clone)]
enum Payload {
    Audio(AudioFrame),
    Stream(StreamEvent),
    Usage(UsageRecorded),
    Control(ControlKind),
}

impl From<ControlKind> for Payload {
    fn from(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl SttPayload for Payload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn usage(recorded: UsageRecorded) -> Option<Self> {
        Some(Self::Usage(recorded))
    }

    fn heard(self) -> Heard {
        match self {
            Self::Audio(frame) => Heard::Audio(frame),
            Self::Stream(event) => Heard::Stream(event),
            Self::Control(kind) => Heard::Control(kind),
            Self::Usage(_) => Heard::Other,
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

const AUDIO: &str = "audio";
const TURNS: &str = "turns";
const COMMITTED: &str = "committed text";
const TURNS_OUT: &str = "turns out";
const USAGE: &str = "usage";

/// The STT node alone, its inputs exposed as the ingresses [`AUDIO`] and [`TURNS`] and its
/// outputs as the egresses [`COMMITTED`], [`TURNS_OUT`] and [`USAGE`], the last pausing on
/// overload so that no record is dropped; the identity of the node and its ports.
fn planned(adapter: &Adapter) -> (Declaration, PlannedPipeline, NodeId, SttPorts, PortId) {
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
    let audio = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let control = PortTraits {
        incremental: false,
        ..audio
    };
    let spec = PipelineSpec {
        name: "exposed recogniser with usage".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let wide = NonZeroUsize::new(256).expect("256");
    let mut declaration = Declaration::new();
    let (pipeline_id, ids) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let stt = pipeline.node(
                "stt",
                NodeRole::Model,
                vec![Capability::IncrementalInput, Capability::OutputStreaming],
            );
            let audio_in = pipeline.input::<modality::Audio>(&stt, "audio", audio);
            let turns_in = pipeline.input::<modality::Control>(&stt, "turns", control);
            let committed = pipeline.output::<modality::Text>(&stt, "committed_text", audio);
            let turns_out = pipeline.output::<modality::Control>(&stt, "turns_out", control);
            let usage = pipeline.output::<modality::Structured>(&stt, "usage", control);
            pipeline.expose(&audio_in, AUDIO, wide, OverloadPolicy::Pause);
            pipeline.expose(&turns_in, TURNS, wide, OverloadPolicy::Pause);
            pipeline.expose(&committed, COMMITTED, wide, OverloadPolicy::Pause);
            pipeline.expose(&turns_out, TURNS_OUT, wide, OverloadPolicy::Pause);
            pipeline.expose(&usage, USAGE, wide, OverloadPolicy::Pause);
            pipeline
                .bind(&stt, adapter_id)
                .expect("the catalog holds the mock recogniser's adapter");
            (
                stt.id(),
                SttPorts {
                    audio: audio_in.id(),
                    turns: turns_in.id(),
                    committed_text: committed.id(),
                    turns_out: turns_out.id(),
                    transcript: None,
                },
                usage.id(),
            )
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("a recogniser between two ingresses and three egresses is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the recogniser is bound to an adapter that claims incremental input");
    let (node, ports, usage) = ids;
    (declaration, planned, node, ports, usage)
}

/// What the application sends on one ingress: an envelope of its own stream, numbered from 1.
struct Sending {
    run_id: RunId,
    producer: NodeId,
    port: &'static str,
    stream_id: StreamId,
    sequence: i64,
}

impl Sending {
    fn new(run_id: RunId, port: &'static str) -> Self {
        Self {
            run_id,
            producer: NodeId::generate(),
            port,
            stream_id: StreamId::generate(),
            sequence: 0,
        }
    }

    fn envelope(
        &mut self,
        kind: &str,
        timestamp: i64,
        timebase: Timebase,
        payload: Payload,
    ) -> EventEnvelope<Payload> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: self.port.to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp,
            timebase,
            causation_id: None,
            payload,
        }
    }

    fn turn(&mut self, event: StreamEvent, at: i64) -> EventEnvelope<Payload> {
        self.envelope(
            event.name(),
            at,
            Timebase::AudioFrameTime,
            Payload::Stream(event),
        )
    }

    fn stop(&mut self) -> EventEnvelope<Payload> {
        self.envelope(
            ControlKind::Stop.as_str(),
            0,
            Timebase::WallTime,
            Payload::Control(ControlKind::Stop),
        )
    }
}

/// Every envelope `ingress` delivers until its end is gone.
fn drained(
    mut egress: Box<dyn inference_core::endpoint::InputEndpoint<Payload>>,
) -> Vec<EventEnvelope<Payload>> {
    let mut received = Vec::new();
    while let Some(envelope) = block_on(egress.recv()) {
        received.push(envelope);
    }
    received
}

#[test]
fn the_usage_of_a_recognition_reaches_the_exposed_usage_egress_with_run_and_node_set() {
    let recogniser = MockRecogniser::new(
        MockConfiguration::Incremental,
        ModelId::generate(),
        script(),
    );
    let adapter = recogniser.adapter().clone();
    let (declaration, planned, stt, ports, usage_port) = planned(&adapter);
    let run_id = RunId::generate();
    let mut node =
        Some(SttNode::new(run_id, recogniser, ports, RunLog::new()).with_usage(usage_port));
    let mut controller = RunController::create_as(
        run_id,
        &planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| Box::new(node.take().expect("the pipeline has one node")),
    )
    .expect("the pipeline is planned and no exposure cancels or supersedes");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("the recogniser prepares");
    let mut ends = controller.exposed();
    let audio: Box<dyn OutputEndpoint<Payload>> = ends.ingress(AUDIO).expect("the audio ingress");
    let turns: Box<dyn OutputEndpoint<Payload>> = ends.ingress(TURNS).expect("the turns ingress");
    let committed = ends.egress(COMMITTED).expect("the committed text egress");
    let turns_out = ends.egress(TURNS_OUT).expect("the turns egress");
    let usage = ends.egress(USAGE).expect("the usage egress");
    block_on(controller.activate()).expect("the recogniser starts with the inner ends");
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });

    let turn_id = TurnId::generate();
    let mut sending_turns = Sending::new(run_id, TURNS);
    let mut sending_audio = Sending::new(run_id, AUDIO);
    block_on(
        turns.send(sending_turns.turn(StreamEvent::TurnOpened(TurnOpened { turn_id, run_id }), 0)),
    )
    .expect("the turns ingress takes the opening");
    for frame in frames(FRAMES) {
        let at = i64::try_from(frame.position.get()).expect("a short clip");
        let envelope = sending_audio.envelope(
            "inference.audio.AudioFrame",
            at,
            Timebase::AudioFrameTime,
            Payload::Audio(frame),
        );
        block_on(audio.send(envelope)).expect("the audio ingress takes the frame");
    }
    let end = i64::try_from(FRAMES).expect("few") * i64::from(FRAME_SAMPLES);
    block_on(
        turns.send(sending_turns.turn(StreamEvent::TurnCompleted(TurnCompleted { turn_id }), end)),
    )
    .expect("the turns ingress takes the completion");
    block_on(audio.send(sending_audio.stop())).expect("the audio ingress takes the end");
    block_on(turns.send(sending_turns.stop())).expect("the turns ingress takes the end");
    let committed = drained(committed);
    let turns_out = drained(turns_out);
    let received = drained(usage);
    drop((audio, turns, ends));
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    result.expect("the drive loop ends the run");
    assert_eq!(controller.run().state(), RunState::Completed);

    assert_eq!(
        committed
            .iter()
            .filter(|envelope| matches!(
                envelope.payload,
                Payload::Stream(StreamEvent::TranscriptCommitted(_))
            ))
            .count(),
        2,
        "the script's two spans: {committed:?}"
    );
    assert!(
        turns_out
            .iter()
            .all(|envelope| !matches!(envelope.payload, Payload::Usage(_))),
        "{turns_out:?}"
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
        panic!("one record for the one recognition on the usage egress: {received:?}")
    };
    assert_eq!(envelope.kind, UsageRecorded::KIND);
    assert_eq!(envelope.run_id, run_id);
    assert_eq!(envelope.producer, stt);
    assert_eq!(envelope.sequence, 1, "the usage port's own stream, from 1");
    assert_eq!(record.run_id, Some(run_id), "the node places it on its run");
    assert_eq!(record.node_id, Some(stt), "the node names itself");
    assert_eq!(
        record.session_id, None,
        "the run's envelopes name no session"
    );
    assert_one_and_a_half_seconds(record, &adapter);
}
