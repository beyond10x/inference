//! `inference::graph` over `inference::Payload` (`story:sdk-payload`).
//!
//! | case | acceptance |
//! |---|---|
//! | [`the_defaulted_port_traits_plan_the_reference_cascade_as_the_explicit_ones_do`] | the reference cascade's ports declared with the explicit traits `local_voice_cascade::declare` writes and with each modality's default traits give equal declarations and equal planned pipelines |
//! | [`a_model_nodes_usage_reaches_the_session`] | a model node over `Payload` given its `usage` port, exposed: after one turn the session's usage holds the mock generator's one record |

use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference::Payload;
use inference::core::envelope::{EventEnvelope, EventId, Timebase};
use inference::core::id::{NodeId, StreamId};
use inference::core::usage::{QuantityKind, UsageRecorded};
use inference::facade::events::SessionEvent;
use inference::facade::live::{LiveSession, LiveSessions};
use inference::graph::{
    Adapter, AdapterDescriptor, AdapterId, Builder, Capability, Catalog, Declaration,
    DeclareAdapter, DeclareProfile, Delivery, DeploymentProfile, Egress, Essentiality,
    ExecutionForm, Implementation, InteractionMode, Locality, Modality, ModelId, NodeRole,
    OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits, ProfileId, ProfileKind, USAGE_PORT,
    modality,
};
use inference::llm::llm_core::Usage;
use inference::pipeline::nodes::log::{RunLog, StreamEvent};
use inference::runtime::run::RunState;
use inference::text::{SpanId, TranscriptCommitted};
use inference_testkit::llm::{MockConfiguration, MockGenerator, ModelNode};

/// A profile and the adapters the pipelines of a case bind.
struct Fixture {
    profile: DeploymentProfile,
    adapters: Vec<Adapter>,
}

impl Fixture {
    fn new(adapters: Vec<Adapter>) -> Self {
        let (profile, _) = DeploymentProfile::declare(DeclareProfile {
            kind: ProfileKind::OfflineStrict,
            egress: Egress::Denied,
            admitted: vec![Locality::SameProcess],
        })
        .expect("a row of the table");
        Self { profile, adapters }
    }
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.adapter_id() == adapter_id)
    }
}

/// An in-process adapter claiming `capabilities`.
fn adapter(capabilities: Vec<Capability>) -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "mock".to_owned(),
        contract_revision: "1".to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
        descriptor: AdapterDescriptor::default(),
    })
    .expect("a positive concurrency")
    .0
}

fn capacity() -> NonZeroUsize {
    NonZeroUsize::new(64).expect("64")
}

/// How a case declares the ports of the reference cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Traits {
    /// The explicit `PortTraits` `local_voice_cascade::declare` writes.
    Explicit,
    /// Each modality's default traits, provisional text named where a port admits it.
    Defaulted,
}

/// Declares the reference cascade of `examples/local-voice-cascade` (`declare_requiring`) in
/// `declaration`, its ports declared as `traits` says, its `stt`, `agent` and `tts` nodes bound to
/// the fixture's three adapters, and validates and plans it.
#[allow(
    clippy::too_many_lines,
    reason = "the cascade's ten nodes, thirty-one ports and seventeen edges, as the example declares them"
)]
fn reference_cascade(
    declaration: &mut Declaration,
    catalog: &Fixture,
    traits: Traits,
) -> PlannedPipeline {
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
    let text = audio;
    let provisional = PortTraits {
        provisional_allowed: true,
        ..text
    };
    let spec = PipelineSpec {
        name: "local voice cascade".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let pause = OverloadPolicy::Pause;
    let streaming = vec![Capability::IncrementalInput, Capability::OutputStreaming];
    let (pipeline_id, ()) = declaration
        .build(
            catalog,
            catalog.profile.profile_id(),
            spec,
            |pipeline: &mut Builder<'_, '_>| {
                let defaulted = traits == Traits::Defaulted;
                macro_rules! input {
                    ($modality:ident, $node:expr, $name:expr, $explicit:expr) => {
                        if defaulted {
                            pipeline.input_port::<modality::$modality>($node, $name)
                        } else {
                            pipeline.input::<modality::$modality>($node, $name, $explicit)
                        }
                    };
                }
                macro_rules! output {
                    ($modality:ident, $node:expr, $name:expr, $explicit:expr) => {
                        if defaulted {
                            pipeline.output_port::<modality::$modality>($node, $name)
                        } else {
                            pipeline.output::<modality::$modality>($node, $name, $explicit)
                        }
                    };
                }
                let provisional_text = if defaulted {
                    PortTraits::of(Modality::Text).provisional()
                } else {
                    provisional
                };

                let split = pipeline.node("split", NodeRole::FanOut, Vec::new());
                let vad = pipeline.node("vad", NodeRole::Policy, Vec::new());
                let policy = pipeline.node("turn policy", NodeRole::Policy, Vec::new());
                let stt = pipeline.node("stt", NodeRole::Model, Vec::new());
                let input_gate = pipeline.node("input gate", NodeRole::Policy, Vec::new());
                let llm =
                    pipeline.node("agent", NodeRole::Model, vec![Capability::OutputStreaming]);
                let text_split = pipeline.node("text split", NodeRole::FanOut, Vec::new());
                let output_gate = pipeline.node("output gate", NodeRole::Policy, Vec::new());
                let tts = pipeline.node("tts", NodeRole::Model, streaming);
                let sink = pipeline.node("speaker", NodeRole::Sink, Vec::new());

                let microphone = input!(Audio, &split, "audio", audio);
                let to_vad = output!(Audio, &split, "vad", audio);
                let to_stt = output!(Audio, &split, "stt", audio);
                let heard = input!(Audio, &vad, "audio", audio);
                let detected = output!(Control, &vad, "activity", control);
                let activity = input!(Control, &policy, "activity", control);
                let turns = output!(Control, &policy, "turns", control);
                let stt_audio = input!(Audio, &stt, "audio", audio);
                let stt_turns = input!(Control, &stt, "turns", control);
                let transcript =
                    pipeline.output::<modality::Text>(&stt, "transcript", provisional_text);
                let committed = output!(Text, &stt, "committed_text", text);
                let turns_out = output!(Control, &stt, "turns_out", control);
                let gate_in =
                    pipeline.input::<modality::Text>(&input_gate, "input", provisional_text);
                let gate_out = output!(Text, &input_gate, "output", text);
                let gate_turns = input!(Control, &input_gate, "turns", control);
                let gate_turns_out = output!(Control, &input_gate, "turns_out", control);
                let user_input = input!(Text, &llm, "user_input", text);
                let answer = output!(Text, &llm, "committed_text", text);
                let answered = input!(Text, &text_split, "text", text);
                let to_gate = output!(Text, &text_split, "gate", text);
                let to_app = output!(Text, &text_split, "assistant_text", text);
                let held =
                    pipeline.input::<modality::Text>(&output_gate, "input", provisional_text);
                let passed = output!(Text, &output_gate, "output", text);
                let speak = input!(Text, &tts, "text", text);
                let played_back = input!(Control, &tts, "played", control);
                let spoken = output!(Audio, &tts, "audio", audio);
                let playing = input!(Audio, &sink, "audio", audio);
                let receipts = output!(Control, &sink, "played", control);
                let sink_turns = input!(Control, &sink, "turns", control);
                let tts_playback = output!(Control, &tts, "playback", control);
                let llm_played = input!(Control, &llm, "played", control);

                pipeline.connect(&to_vad, &heard, capacity(), pause);
                pipeline.connect(&to_stt, &stt_audio, capacity(), pause);
                pipeline.connect(&detected, &activity, capacity(), pause);
                pipeline.connect(&turns, &stt_turns, capacity(), pause);
                pipeline.connect(&transcript, &gate_in, capacity(), pause);
                pipeline.connect(&turns_out, &gate_turns, capacity(), pause);
                pipeline.connect(&gate_out, &user_input, capacity(), pause);
                pipeline.connect(&gate_turns_out, &sink_turns, capacity(), pause);
                pipeline.connect(&answer, &answered, capacity(), pause);
                pipeline.connect(&to_gate, &held, capacity(), pause);
                pipeline.connect(&passed, &speak, capacity(), pause);
                pipeline.connect(&spoken, &playing, capacity(), pause);
                pipeline.connect_feedback(&receipts, &played_back, capacity(), pause, capacity());
                pipeline.connect_feedback(
                    &tts_playback,
                    &llm_played,
                    capacity(),
                    pause,
                    capacity(),
                );
                pipeline.expose(&microphone, "audio", capacity(), pause);
                pipeline.expose(&committed, "transcript", capacity(), pause);
                pipeline.expose(&to_app, "assistant_text", capacity(), pause);
                for (node, adapter) in [&stt, &llm, &tts].into_iter().zip(&catalog.adapters) {
                    pipeline
                        .bind(node, adapter.adapter_id())
                        .expect("the catalog holds the adapter");
                }
            },
        )
        .expect("the catalog holds the profile");
    let validated = declaration
        .validate(pipeline_id)
        .expect("the reference cascade validates");
    declaration
        .plan(validated, catalog)
        .expect("the reference cascade plans")
}

#[test]
fn the_defaulted_port_traits_plan_the_reference_cascade_as_the_explicit_ones_do() {
    let catalog = Fixture::new(vec![
        adapter(Vec::new()),
        adapter(vec![Capability::OutputStreaming]),
        adapter(vec![
            Capability::IncrementalInput,
            Capability::OutputStreaming,
        ]),
    ]);
    let mut explicit = Declaration::counted();
    let explicit_planned = reference_cascade(&mut explicit, &catalog, Traits::Explicit);
    let mut defaulted = Declaration::counted();
    let defaulted_planned = reference_cascade(&mut defaulted, &catalog, Traits::Defaulted);

    assert_eq!(explicit.nodes().count(), 10);
    assert_eq!(explicit.ports().count(), 31);
    assert_eq!(explicit.edges().count(), 14);
    assert_eq!(
        explicit
            .ports()
            .filter(|port| port.provisional_allowed())
            .count(),
        3
    );
    let nodes = |declaration: &Declaration| declaration.nodes().cloned().collect::<Vec<_>>();
    let ports = |declaration: &Declaration| declaration.ports().cloned().collect::<Vec<_>>();
    let edges = |declaration: &Declaration| declaration.edges().cloned().collect::<Vec<_>>();
    assert_eq!(nodes(&defaulted), nodes(&explicit), "the same nodes");
    assert_eq!(
        ports(&defaulted),
        ports(&explicit),
        "the same ports, each with the same traits"
    );
    assert_eq!(edges(&defaulted), edges(&explicit), "the same edges");
    assert_eq!(
        defaulted, explicit,
        "the same declaration, entity for entity"
    );
    assert_eq!(
        defaulted_planned, explicit_planned,
        "the same planned pipeline"
    );
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes: the facade commits to no executor.
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

const INGRESS: &str = "user text";
const EGRESS: &str = "reply";
const USAGE_EGRESS: &str = "llm_usage";
const INPUT_TOKENS: u64 = 7;
const OUTPUT_TOKENS: u64 = 3;

fn reply(input: &str) -> String {
    format!("s{input}")
}

#[test]
fn a_model_nodes_usage_reaches_the_session() {
    let generator = MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .reporting([Some(Usage {
        input_tokens: Some(INPUT_TOKENS),
        output_tokens: Some(OUTPUT_TOKENS),
        ..Usage::default()
    })]);
    let catalog = Fixture::new(vec![generator.adapter().clone()]);
    let adapter_id = generator.adapter().adapter_id();
    let spec = PipelineSpec {
        name: "text session".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let mut declaration = Declaration::new();
    let (pipeline_id, usage) = declaration
        .build(&catalog, catalog.profile.profile_id(), spec, |pipeline| {
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let prompt = pipeline.input_port::<modality::Text>(&model, "prompt");
            let answer = pipeline.output_port::<modality::Text>(&model, "answer");
            let usage = pipeline.output_port::<modality::Structured>(&model, USAGE_PORT);
            pipeline.expose(&prompt, INGRESS, capacity(), OverloadPolicy::Pause);
            pipeline.expose(&answer, EGRESS, capacity(), OverloadPolicy::Pause);
            pipeline.expose(&usage, USAGE_EGRESS, capacity(), OverloadPolicy::Pause);
            pipeline
                .bind(&model, adapter_id)
                .expect("the catalog holds the mock generator's adapter");
            usage.id()
        })
        .expect("the catalog holds the profile");
    let validated = declaration.validate(pipeline_id).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");

    let sessions = LiveSessions::new();
    let (session, driver): (LiveSession<Payload>, _) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        move |run_id, _| -> Implementation {
            Box::new(
                ModelNode::<Payload>::carrying(run_id, generator.clone(), reply, RunLog::new())
                    .with_usage(usage),
            )
        },
    ))
    .expect("a session opens");
    let run_id = session.run_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
        span_id: SpanId::generate(),
        text: "hello".to_owned(),
    });
    let envelope = EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id,
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: NodeId::generate(),
        port: INGRESS.to_owned(),
        kind: event.name().to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: Payload::Stream(event),
    };
    block_on(session.send(envelope)).expect("sent");
    block_on(session.commit_input()).expect("committed");
    block_on(session.close()).expect("a Ready session drains");
    let mut received = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        received.push(event);
    }
    let ended = drive.join().expect("no panic").expect("the run ends");
    assert_eq!(ended.state(), RunState::Completed, "{received:#?}");

    let delivered: Vec<&UsageRecorded> = received
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) => Some(recorded),
            _ => None,
        })
        .collect();
    assert_eq!(delivered.len(), 1, "one record delivered: {received:#?}");
    let usage = session.usage();
    assert_eq!(usage.session.records(), 1, "{usage:#?}");
    let known = |kind| usage.session.kind(kind).map(|total| total.known);
    assert_eq!(
        known(QuantityKind::InputTextTokens),
        Some(i128::from(INPUT_TOKENS))
    );
    assert_eq!(
        known(QuantityKind::OutputTextTokens),
        Some(i128::from(OUTPUT_TOKENS))
    );
}
