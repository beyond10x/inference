//! Adversary pass 1 on `story:usage-stt`: the STT node over two recognitions, one per turn. Each
//! recognition's record reaches the exposed `usage` egress with its own audio, the records and
//! the end of input numbered 1, 2, 3 on the one stream of the usage port; and a recognition that
//! was offered no audio still records its one request and 0 ms, measured.

#![cfg(all(feature = "stt", feature = "run"))]

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
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
use inference_stt::{Recogniser, RecognitionHandle, RecognitionRequest, TranscriptEvent};
use inference_testkit::graph::RunLog;
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::{
    FRAME_SAMPLES, Heard, MockConfiguration, MockRecogniser, SttNode, SttPayload, SttPorts, frames,
    script,
};
use inference_text::{TurnCompleted, TurnId, TurnOpened};

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

#[test]
fn a_recognition_offered_no_audio_records_one_request_and_zero_ms() {
    for configuration in MockConfiguration::ALL {
        let mut recogniser = MockRecogniser::new(configuration, ModelId::generate(), Vec::new());
        let mut handle = recogniser
            .start(RecognitionRequest::new(
                RunId::generate(),
                TurnId::generate(),
                StreamId::generate(),
                NodeId::generate(),
                "transcript",
            ))
            .expect("a fresh recogniser runs nothing else");
        handle.end_input().expect("input ends once");
        let events: Vec<EventEnvelope<TranscriptEvent>> = handle.by_ref().collect();
        let [envelope] = events.as_slice() else {
            panic!("{configuration:?}: the usage record alone: {events:?}")
        };
        let TranscriptEvent::Usage(recorded) = &envelope.payload else {
            panic!("{configuration:?}: a usage record: {envelope:?}")
        };
        assert_eq!(envelope.sequence, 1, "the first event of the stream");
        assert_eq!(
            quantity(recorded, QuantityKind::InputAudioMs),
            Quantity::measured(QuantityKind::InputAudioMs, 0),
            "{configuration:?}: no audio, counted"
        );
        assert_eq!(
            quantity(recorded, QuantityKind::Requests),
            Quantity::measured(QuantityKind::Requests, 1)
        );
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

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
        name: "two recognitions with usage".to_owned(),
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
    let validated = declaration.validate(pipeline_id).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");
    let (node, ports, usage) = ids;
    (declaration, planned, node, ports, usage)
}

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

    fn frame(&mut self, frame: AudioFrame) -> EventEnvelope<Payload> {
        let at = i64::try_from(frame.position.get()).expect("a short clip");
        self.envelope(
            "inference.audio.AudioFrame",
            at,
            Timebase::AudioFrameTime,
            Payload::Audio(frame),
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

fn drained(mut egress: Box<dyn InputEndpoint<Payload>>) -> Vec<EventEnvelope<Payload>> {
    let mut received = Vec::new();
    while let Some(envelope) = block_on(egress.recv()) {
        received.push(envelope);
    }
    received
}

/// Receives from `egress` until the `TurnCompleted` of `turn_id` arrives.
fn until_completed(egress: &mut Box<dyn InputEndpoint<Payload>>, turn_id: TurnId) {
    loop {
        let envelope = block_on(egress.recv()).expect("the turn completes before the end");
        if matches!(
            envelope.payload,
            Payload::Stream(StreamEvent::TurnCompleted(TurnCompleted { turn_id: done })) if done == turn_id
        ) {
            return;
        }
    }
}

#[test]
fn two_recognitions_send_two_records_and_the_end_on_one_usage_stream_numbered_from_1() {
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
    .expect("planned");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("the recogniser prepares");
    let mut ends = controller.exposed();
    let audio: Box<dyn OutputEndpoint<Payload>> = ends.ingress(AUDIO).expect("audio");
    let turns: Box<dyn OutputEndpoint<Payload>> = ends.ingress(TURNS).expect("turns");
    let committed = ends.egress(COMMITTED).expect("committed text");
    let mut turns_out = ends.egress(TURNS_OUT).expect("turns out");
    let usage = ends.egress(USAGE).expect("usage");
    block_on(controller.activate()).expect("the recogniser starts");
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });

    let mut sending_turns = Sending::new(run_id, TURNS);
    let mut sending_audio = Sending::new(run_id, AUDIO);
    let all = frames(150);
    let (first_audio, second_audio) = all.split_at(100);
    for (turn_id, audio_of_turn) in [
        (TurnId::generate(), first_audio),
        (TurnId::generate(), second_audio),
    ] {
        let start = i64::try_from(audio_of_turn[0].position.get()).expect("short");
        block_on(turns.send(sending_turns.turn(
            StreamEvent::TurnOpened(TurnOpened { turn_id, run_id }),
            start,
        )))
        .expect("the opening");
        for frame in audio_of_turn {
            block_on(audio.send(sending_audio.frame(frame.clone()))).expect("the frame");
        }
        let end =
            start + i64::try_from(audio_of_turn.len()).expect("few") * i64::from(FRAME_SAMPLES);
        block_on(
            turns.send(
                sending_turns.turn(StreamEvent::TurnCompleted(TurnCompleted { turn_id }), end),
            ),
        )
        .expect("the completion");
        until_completed(&mut turns_out, turn_id);
    }
    block_on(audio.send(sending_audio.stop())).expect("the audio end");
    block_on(turns.send(sending_turns.stop())).expect("the turns end");
    let _committed = drained(committed);
    let _turns_out = drained(turns_out);
    let received = drained(usage);
    drop((audio, turns, ends));
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    result.expect("the drive loop ends the run");
    assert_eq!(controller.run().state(), RunState::Completed);

    assert_eq!(received.len(), 3, "two records and the end: {received:?}");
    assert_eq!(
        received
            .iter()
            .map(|envelope| envelope.sequence)
            .collect::<Vec<_>>(),
        [1, 2, 3],
        "one stream, numbered from 1: {received:?}"
    );
    assert!(
        received
            .iter()
            .all(|envelope| envelope.stream_id == received[0].stream_id),
        "the usage port's one stream: {received:?}"
    );
    assert_eq!(received[2].kind, ControlKind::Stop.as_str());
    let records: Vec<&UsageRecorded> = received[..2]
        .iter()
        .map(|envelope| match &envelope.payload {
            Payload::Usage(recorded) => recorded,
            other => panic!("a usage record, not {other:?}"),
        })
        .collect();
    assert_eq!(
        quantity(records[0], QuantityKind::InputAudioMs),
        Quantity::measured(QuantityKind::InputAudioMs, 1_000),
        "the first turn's 100 frames"
    );
    assert_eq!(
        quantity(records[1], QuantityKind::InputAudioMs),
        Quantity::measured(QuantityKind::InputAudioMs, 500),
        "the second turn's 50 frames"
    );
    assert_ne!(records[0].usage_record_id, records[1].usage_record_id);
    for record in &records {
        assert_eq!(record.run_id, Some(run_id));
        assert_eq!(record.node_id, Some(stt));
        assert_eq!(record.adapter_id, adapter.adapter_id());
    }
}
