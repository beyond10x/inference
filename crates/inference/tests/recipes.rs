//! The recipes of `docs/design/sdk.md` §3.3: their topologies, `declaration()`, `declare_into`
//! over a caller's catalog, application recipes and their registration, over the testkit mocks;
//! and an instantiated chat opened as a live session, which carries a turn to the mock's reply.

#[path = "fixtures/reference_cascade.rs"]
mod reference_cascade;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use inference::core::model::{Adapter, AdapterId, Capability, ModelId};
use inference::core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference::core::{model::Locality, policy::ProfileNotFound};
use inference::graph::{
    Builder, Delivery, Essentiality, InteractionMode, NodeRole, OverloadPolicy, PortTraits,
    modality,
};
use inference::pipeline::{AdapterNotFound, Catalog, Declaration, PipelineId, PlanPipelineError};
use inference::stt::Recogniser as _;
use inference::tts::Synthesiser as _;
use inference::{
    Binding, EXPOSURE_CAPACITY, Env, ErrorKind, MAX_TOOL_ITERATIONS, Recipe, Role, Stack,
};
use inference_testkit::duplex::mock::MockSpeechModel;
use inference_testkit::llm::{self, MockGenerator};
use inference_testkit::stt::{self, MockRecogniser};
use inference_testkit::tts::{self, MockSynthesiser};

/// A profile and the adapters of the testkit mocks, one per role.
struct Fixture {
    profile: DeploymentProfile,
    adapters: Vec<Adapter>,
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

fn offline_strict() -> DeploymentProfile {
    DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![Locality::SameProcess],
    })
    .expect("a row of the table")
    .0
}

fn pcm16() -> inference::audio::AudioFormat {
    inference::audio::AudioFormat {
        encoding: inference::audio::AudioEncoding::PcmS16le,
        sample_rate: std::num::NonZeroU32::new(24_000).expect("24 kHz"),
        channels: std::num::NonZeroU16::MIN,
    }
}

/// The catalog with one adapter per role, in the order `stt`, `llm`, `tts`, `speech`; `stt` is a
/// batch recogniser when `batch`.
fn fixture(batch: bool) -> (Fixture, [AdapterId; 4]) {
    let stt_config = if batch {
        stt::MockConfiguration::Batch
    } else {
        stt::MockConfiguration::Incremental
    };
    let adapters = vec![
        MockRecogniser::new(stt_config, ModelId::generate(), stt::script())
            .adapter()
            .clone(),
        MockGenerator::new(
            llm::MockConfiguration::StreamingOnly,
            ModelId::generate(),
            ["hi"],
        )
        .adapter()
        .clone(),
        MockSynthesiser::new(
            tts::MockConfiguration::IncrementalStreaming,
            ModelId::generate(),
            pcm16(),
        )
        .adapter()
        .clone(),
        inference::duplex::NativeSpeechModel::adapter(&MockSpeechModel::native(Vec::new())).clone(),
    ];
    let ids = [0, 1, 2, 3].map(|index: usize| adapters[index].adapter_id());
    (
        Fixture {
            profile: offline_strict(),
            adapters,
        },
        ids,
    )
}

fn cascade_pairs(ids: [AdapterId; 4]) -> Vec<(Role, AdapterId)> {
    vec![
        (Role::Stt, ids[0]),
        (Role::Llm, ids[1]),
        (Role::Tts, ids[2]),
    ]
}

/// A stack binding every role to the adapter `mock`.
fn mock_stack() -> Stack {
    Role::ALL
        .into_iter()
        .fold(Stack::new(), |stack, role| {
            stack.bind(Binding::named(role, "mock", BTreeMap::new()))
        })
        .resolve_with(&Env::new())
        .expect("the mock stack resolves")
}

/// The node names, port shapes and edge endpoints of `pipeline_id`, by name.
fn shape(
    declaration: &Declaration,
    pipeline_id: PipelineId,
) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
    let node_name = |node_id| {
        declaration
            .nodes()
            .find(|node| node.node_id() == node_id)
            .map(|node| node.name().to_owned())
            .expect("a port's node is declared")
    };
    let port_name = |port_id| {
        let port = declaration
            .ports()
            .find(|port| port.port_id() == port_id)
            .expect("an edge's port is declared");
        format!("{}.{}", node_name(port.node_id()), port.name())
    };
    let nodes = declaration
        .nodes()
        .filter(|node| node.pipeline_id() == pipeline_id)
        .map(|node| format!("{} {:?} {:?}", node.name(), node.role(), node.requires()))
        .collect();
    let ports = declaration
        .ports()
        .filter(|port| port.pipeline_id() == pipeline_id)
        .map(|port| {
            format!(
                "{}.{} {:?} {:?} {:?} {:?} {}",
                node_name(port.node_id()),
                port.name(),
                port.direction(),
                port.modality(),
                port.essentiality(),
                port.delivery(),
                port.incremental(),
            )
        })
        .collect();
    let edges = declaration
        .edges()
        .filter(|edge| edge.pipeline_id() == pipeline_id)
        .map(|edge| {
            format!(
                "{} -> {} {} {:?}",
                port_name(edge.from_port()),
                port_name(edge.to_port()),
                edge.capacity(),
                edge.overload()
            )
        })
        .collect();
    (nodes, ports, edges)
}

fn counts(declaration: &Declaration) -> [usize; 4] {
    [
        declaration.pipelines().count(),
        declaration.nodes().count(),
        declaration.ports().count(),
        declaration.bindings().count(),
    ]
}

fn five() -> [Recipe; 5] {
    [
        Recipe::chat(),
        Recipe::transcribe(),
        Recipe::speak(),
        Recipe::voice_cascade(),
        Recipe::native_voice(),
    ]
}

fn validates_and_plans(
    declaration: &mut Declaration,
    pipeline_id: PipelineId,
    catalog: &dyn Catalog,
) {
    let validated = declaration
        .validate(pipeline_id)
        .unwrap_or_else(|error| panic!("the pipeline validates: {error}"));
    let _planned = declaration
        .plan(validated, catalog)
        .unwrap_or_else(|error| panic!("the pipeline plans: {error}"));
}

#[test]
fn voice_cascade_has_the_reference_cascades_nodes_ports_edges_and_traits() {
    let (catalog, ids) = fixture(false);
    let profile_id = catalog.profile.profile_id();
    let mut reference = Declaration::new();
    let expected = reference_cascade::declare(
        &mut reference,
        &catalog,
        profile_id,
        [ids[0], ids[1], ids[2]],
    );
    validates_and_plans(&mut reference, expected, &catalog);

    let mut declaration = Declaration::new();
    let pipeline_id = Recipe::voice_cascade()
        .declare_into(
            &mut declaration,
            &catalog,
            profile_id,
            &cascade_pairs(ids),
            &[],
        )
        .expect("the cascade declares");
    validates_and_plans(&mut declaration, pipeline_id, &catalog);

    assert_eq!(
        shape(&declaration, pipeline_id),
        shape(&reference, expected)
    );
    assert_eq!(declaration.bindings().count(), 3);
}

#[test]
fn native_voice_is_one_duplex_node_and_the_tool_nodes_with_one_egress() {
    let (catalog, ids) = fixture(false);
    let mut declaration = Declaration::new();
    let pipeline_id = Recipe::native_voice()
        .declare_into(
            &mut declaration,
            &catalog,
            catalog.profile.profile_id(),
            &[(Role::Speech, ids[3])],
            &[],
        )
        .expect("native voice declares");
    let roles: BTreeMap<String, NodeRole> = declaration
        .nodes()
        .map(|node| (node.name().to_owned(), node.role()))
        .collect();
    assert_eq!(
        roles,
        BTreeMap::from([
            ("speech".to_owned(), NodeRole::Model),
            ("authority".to_owned(), NodeRole::Policy),
            ("executor".to_owned(), NodeRole::Tool),
        ])
    );
    validates_and_plans(&mut declaration, pipeline_id, &catalog);

    let egresses: Vec<String> = declaration
        .exposures()
        .filter(|exposure| {
            declaration.ports().any(|port| {
                port.port_id() == exposure.port_id()
                    && format!("{:?}", port.direction()).contains("Output")
            })
        })
        .map(|exposure| exposure.name().to_owned())
        .collect();
    assert_eq!(egresses, ["events"]);
}

#[test]
fn native_voice_bounds_its_tool_loop_at_four() {
    let declaration = Recipe::native_voice().declaration();
    let loops: Vec<_> = declaration.feedback_loops().collect();
    assert_eq!(loops.len(), 1);
    assert_eq!(loops[0].max_iterations(), MAX_TOOL_ITERATIONS);
    assert_eq!(MAX_TOOL_ITERATIONS.get(), 4);
    let edge = declaration
        .edges()
        .find(|edge| edge.edge_id() == loops[0].edge_id())
        .expect("the loop's edge is declared");
    let port = |port_id| {
        let port = declaration
            .ports()
            .find(|port| port.port_id() == port_id)
            .expect("declared");
        let node = declaration.node(port.node_id()).expect("declared");
        format!("{}.{}", node.name(), port.name())
    };
    assert_eq!(port(edge.from_port()), "speech.tool_call");
    assert_eq!(port(edge.to_port()), "authority.proposed");
}

#[test]
fn chat_transcribe_and_speak_plan_the_table_topologies() {
    let (catalog, ids) = fixture(false);
    let profile_id = catalog.profile.profile_id();
    let cases = [
        (
            Recipe::chat(),
            (Role::Llm, ids[1]),
            "agent",
            ["Text", "Text"],
        ),
        (
            Recipe::transcribe(),
            (Role::Stt, ids[0]),
            "stt",
            ["Audio", "Text"],
        ),
        (
            Recipe::speak(),
            (Role::Tts, ids[2]),
            "tts",
            ["Text", "Audio"],
        ),
    ];
    for (recipe, pair, node, [ingress, egress]) in cases {
        let mut declaration = Declaration::new();
        let pipeline_id = recipe
            .declare_into(&mut declaration, &catalog, profile_id, &[pair], &[])
            .expect("the recipe declares");
        validates_and_plans(&mut declaration, pipeline_id, &catalog);
        let nodes: Vec<_> = declaration
            .nodes()
            .map(|node| (node.name().to_owned(), node.role()))
            .collect();
        assert_eq!(
            nodes,
            [(node.to_owned(), NodeRole::Model)],
            "{}",
            recipe.name()
        );
        let mut ends: Vec<String> = declaration
            .exposures()
            .map(|exposure| {
                let port = declaration
                    .ports()
                    .find(|port| port.port_id() == exposure.port_id())
                    .expect("declared");
                format!("{:?} {:?}", port.direction(), port.modality())
            })
            .collect();
        ends.sort();
        let mut wanted = vec![format!("Input {ingress}"), format!("Output {egress}")];
        wanted.sort();
        assert_eq!(ends, wanted, "{}", recipe.name());
    }
}

#[test]
fn the_declaration_is_the_stack_free_form_of_declare_into() {
    let stack = mock_stack();
    for recipe in five() {
        let free = recipe.declaration();
        assert_eq!(free.pipelines().count(), 1, "{}", recipe.name());
        assert_eq!(free.bindings().count(), 0, "{}", recipe.name());
        let free_id = free.pipelines().next().expect("one").pipeline_id();

        let mut declaration = Declaration::new();
        let pipeline_id = recipe
            .declare_into(
                &mut declaration,
                &stack,
                stack.profile_id(),
                &stack.adapter_ids(),
                &[],
            )
            .unwrap_or_else(|error| panic!("{} declares: {error}", recipe.name()));
        validates_and_plans(&mut declaration, pipeline_id, &stack);
        assert_eq!(
            shape(&free, free_id),
            shape(&declaration, pipeline_id),
            "{}",
            recipe.name()
        );
    }
}

#[test]
fn declare_into_adds_a_pipeline_beside_another() {
    let (catalog, ids) = fixture(false);
    let profile_id = catalog.profile.profile_id();
    let mut declaration = Declaration::new();
    let chat = Recipe::chat()
        .declare_into(
            &mut declaration,
            &catalog,
            profile_id,
            &[(Role::Llm, ids[1])],
            &[],
        )
        .expect("chat declares");
    let cascade = Recipe::voice_cascade()
        .declare_into(
            &mut declaration,
            &catalog,
            profile_id,
            &cascade_pairs(ids),
            &[],
        )
        .expect("the cascade declares");
    assert_ne!(chat, cascade);
    let names: BTreeSet<&str> = declaration
        .nodes()
        .filter(|node| node.pipeline_id() == cascade)
        .map(|node| node.name())
        .collect();
    let reference = {
        let mut reference = Declaration::new();
        let id = reference_cascade::declare(
            &mut reference,
            &catalog,
            profile_id,
            [ids[0], ids[1], ids[2]],
        );
        reference
            .nodes()
            .filter(|node| node.pipeline_id() == id)
            .map(|node| node.name().to_owned())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(names.len(), 11);
    assert_eq!(
        names,
        reference
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
    );
    validates_and_plans(&mut declaration, cascade, &catalog);
}

#[test]
fn a_requirement_replaces_the_default_and_planning_names_the_node() {
    let (catalog, ids) = fixture(true);
    let mut declaration = Declaration::new();
    let streaming: &[Capability] = &[Capability::IncrementalInput, Capability::OutputStreaming];
    let pipeline_id = Recipe::voice_cascade()
        .declare_into(
            &mut declaration,
            &catalog,
            catalog.profile.profile_id(),
            &cascade_pairs(ids),
            &[(Role::Stt, streaming)],
        )
        .expect("the cascade declares");
    let validated = declaration.validate(pipeline_id).expect("it validates");
    let Err(PlanPipelineError::Mismatch(mismatch)) = declaration.plan(validated, &catalog) else {
        panic!("planning refuses the batch recogniser");
    };
    let node = declaration
        .node(mismatch.node_id.expect("a node is named"))
        .expect("declared");
    assert_eq!(node.name(), "stt");
}

#[test]
fn a_refused_declare_into_leaves_the_declaration_as_it_was() {
    let (catalog, ids) = fixture(false);
    let profile_id = catalog.profile.profile_id();
    let mut declaration = Declaration::new();
    Recipe::chat()
        .declare_into(
            &mut declaration,
            &catalog,
            profile_id,
            &[(Role::Llm, ids[1])],
            &[],
        )
        .expect("chat declares");
    let before = counts(&declaration);

    let other = offline_strict().profile_id();
    let error = Recipe::voice_cascade()
        .declare_into(&mut declaration, &catalog, other, &cascade_pairs(ids), &[])
        .expect_err("the profile is not in the catalog");
    assert_eq!(error.kind(), ErrorKind::Layer);
    assert!(error.to_string().contains(&other.to_string()), "{error}");
    let missing = std::error::Error::source(&error)
        .and_then(|source| source.downcast_ref::<ProfileNotFound>())
        .expect("the source is ProfileNotFound");
    assert_eq!(missing.profile_id, other);
    assert_eq!(counts(&declaration), before);

    let absent = AdapterId::generate();
    let error = Recipe::voice_cascade()
        .declare_into(
            &mut declaration,
            &catalog,
            profile_id,
            &[
                (Role::Stt, ids[0]),
                (Role::Llm, ids[1]),
                (Role::Tts, absent),
            ],
            &[],
        )
        .expect_err("the tts adapter is not in the catalog");
    assert_eq!(error.kind(), ErrorKind::Layer);
    let shown = error.to_string();
    assert!(
        shown.contains(&absent.to_string()) && shown.contains("tts"),
        "{shown}"
    );
    let missing = std::error::Error::source(&error)
        .and_then(|source| source.downcast_ref::<AdapterNotFound>())
        .expect("the source is AdapterNotFound");
    assert_eq!(missing.adapter_id, absent);
    assert_eq!(counts(&declaration), before);

    let error = Recipe::voice_cascade()
        .declare_into(
            &mut declaration,
            &catalog,
            profile_id,
            &[(Role::Stt, ids[0]), (Role::Llm, ids[1])],
            &[],
        )
        .expect_err("no tts pair");
    assert_eq!(error.kind(), ErrorKind::Layer);
    assert!(error.to_string().contains("tts"), "{error}");
    assert_eq!(counts(&declaration), before);
}

#[test]
fn an_extra_role_is_ignored() {
    let (catalog, ids) = fixture(false);
    let mut declaration = Declaration::new();
    let mut pairs = cascade_pairs(ids);
    pairs.push((Role::Speech, ids[3]));
    let pipeline_id = Recipe::voice_cascade()
        .declare_into(
            &mut declaration,
            &catalog,
            catalog.profile.profile_id(),
            &pairs,
            &[],
        )
        .expect("the cascade declares");
    let bound: Vec<AdapterId> = declaration
        .bindings()
        .filter(|binding| binding.pipeline_id() == pipeline_id)
        .map(|binding| binding.adapter_id())
        .collect();
    assert_eq!(bound.len(), 3);
    assert!(!bound.contains(&ids[3]));
    validates_and_plans(&mut declaration, pipeline_id, &catalog);
}

/// A text ingress, one model node named `agent` and a text egress.
fn ask(pipeline: &mut Builder<'_, '_>) -> inference::Result<()> {
    let text = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let agent = pipeline.node("agent", NodeRole::Model, vec![Capability::OutputStreaming]);
    let input = pipeline.input::<modality::Text>(&agent, "user_input", text);
    let output = pipeline.output::<modality::Text>(&agent, "committed_text", text);
    pipeline.expose(
        &input,
        "user_input",
        EXPOSURE_CAPACITY,
        OverloadPolicy::Pause,
    );
    pipeline.expose(
        &output,
        "assistant_text",
        EXPOSURE_CAPACITY,
        OverloadPolicy::Pause,
    );
    Ok(())
}

/// [`ask`] and a synthesiser node named `speaker` with an audio egress.
fn ask_aloud(pipeline: &mut Builder<'_, '_>) -> inference::Result<()> {
    ask(pipeline)?;
    let traits = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let speaker = pipeline.node("speaker", NodeRole::Model, Vec::new());
    let audio = pipeline.output::<modality::Audio>(&speaker, "audio", traits);
    pipeline.expose(&audio, "audio", EXPOSURE_CAPACITY, OverloadPolicy::Pause);
    Ok(())
}

#[test]
fn a_custom_recipe_binds_its_named_roles() {
    let (catalog, ids) = fixture(false);
    let profile_id = catalog.profile.profile_id();
    let recipe = Recipe::custom("ask", InteractionMode::TurnBased, ask).role("agent", Role::Llm);
    let mut declaration = Declaration::new();
    Recipe::chat()
        .declare_into(
            &mut declaration,
            &catalog,
            profile_id,
            &[(Role::Llm, ids[1])],
            &[],
        )
        .expect("chat declares");
    let before = counts(&declaration);
    let error = recipe
        .declare_into(
            &mut declaration,
            &catalog,
            profile_id,
            &[(Role::Stt, ids[0])],
            &[],
        )
        .expect_err("no llm pair");
    assert_eq!(error.kind(), ErrorKind::Layer);
    assert!(error.to_string().contains("llm"), "{error}");
    assert_eq!(counts(&declaration), before);

    let pipeline_id = recipe
        .declare_into(
            &mut declaration,
            &catalog,
            profile_id,
            &[(Role::Llm, ids[1])],
            &[],
        )
        .expect("the custom recipe declares");
    let bound: Vec<String> = declaration
        .bindings()
        .filter(|binding| binding.pipeline_id() == pipeline_id)
        .map(|binding| {
            declaration
                .node(binding.node_id())
                .expect("declared")
                .name()
                .to_owned()
        })
        .collect();
    assert_eq!(bound, ["agent"]);
}

#[test]
fn an_application_recipe_is_registered_and_found_by_name() {
    let (catalog, ids) = fixture(false);
    Recipe::custom("ask-reg", InteractionMode::TurnBased, ask)
        .role("agent", Role::Llm)
        .register()
        .expect("the name is free");
    let found = Recipe::named("ask-reg").expect("registered");
    let mut declaration = Declaration::new();
    let pipeline_id = found
        .declare_into(
            &mut declaration,
            &catalog,
            catalog.profile.profile_id(),
            &[(Role::Llm, ids[1])],
            &[],
        )
        .expect("the found recipe declares");
    validates_and_plans(&mut declaration, pipeline_id, &catalog);

    for taken in ["ask-reg", "chat"] {
        let error = Recipe::custom(taken, InteractionMode::TurnBased, ask)
            .register()
            .expect_err("the name is taken");
        assert_eq!(error.kind(), ErrorKind::Layer);
        assert!(error.to_string().contains(taken), "{error}");
    }
    assert_eq!(Recipe::named("chat").expect("built in").name(), "chat");
    assert!(Recipe::named("absent").is_none());
}

#[test]
fn a_custom_recipe_reports_its_roles_in_role_order() {
    let (catalog, ids) = fixture(false);
    let error = Recipe::custom("ask-order", InteractionMode::TurnBased, ask_aloud)
        .role("speaker", Role::Tts)
        .role("agent", Role::Llm)
        .declare_into(
            &mut Declaration::new(),
            &catalog,
            catalog.profile.profile_id(),
            &[(Role::Stt, ids[0])],
            &[],
        )
        .expect_err("neither role is bound");
    assert_eq!(error.kind(), ErrorKind::Layer);
    let shown = error.to_string();
    assert!(shown.contains("tts") && !shown.contains("llm"), "{shown}");
}

#[test]
fn every_exposure_has_the_recipe_capacity_under_pause() {
    for recipe in five() {
        let declaration = recipe.declaration();
        assert!(declaration.exposures().count() > 0, "{}", recipe.name());
        for exposure in declaration.exposures() {
            assert_eq!(exposure.capacity(), EXPOSURE_CAPACITY, "{}", recipe.name());
            assert_eq!(exposure.overload(), OverloadPolicy::Pause);
        }
    }
    assert_eq!(EXPOSURE_CAPACITY.get(), 64);
    let declaration = Recipe::native_voice()
        .exposure_capacity(NonZeroUsize::MIN)
        .declaration();
    for exposure in declaration.exposures() {
        assert_eq!(exposure.capacity(), NonZeroUsize::MIN);
        assert_eq!(exposure.overload(), OverloadPolicy::Pause);
    }
}

#[test]
fn instantiating_over_a_stack_plans_the_recipe() {
    let stack = mock_stack();
    let instantiation = Recipe::chat().instantiate(&stack).expect("chat plans");
    assert_eq!(instantiation.recipe(), "chat");
    assert_eq!(
        instantiation
            .declaration()
            .bindings()
            .filter(|binding| binding.pipeline_id() == instantiation.pipeline_id())
            .count(),
        1
    );
}

#[test]
fn instantiating_without_a_role_is_role_unbound() {
    let stack = Stack::new()
        .bind(Binding::named(Role::Llm, "mock", BTreeMap::new()))
        .resolve_with(&Env::new())
        .expect("resolves");
    let error = Recipe::speak().instantiate(&stack).expect_err("no tts");
    assert_eq!(error.kind(), ErrorKind::RoleUnbound);
    assert!(error.to_string().contains("tts"), "{error}");
}

#[test]
fn a_custom_recipe_instantiates_over_its_one_role() {
    let stack = Stack::new()
        .bind(Binding::named(Role::Llm, "mock", BTreeMap::new()))
        .resolve_with(&Env::new())
        .expect("resolves");
    let instantiation = Recipe::custom("ask-instantiate", InteractionMode::TurnBased, ask)
        .role("agent", Role::Llm)
        .instantiate(&stack)
        .expect("the custom recipe plans");
    assert_eq!(instantiation.declaration().bindings().count(), 1);
}

struct Unpark(std::thread::Thread);

impl std::task::Wake for Unpark {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes: the facade commits to no executor.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let waker = std::task::Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
    let mut future = std::pin::pin!(future);
    loop {
        if let std::task::Poll::Ready(output) = future
            .as_mut()
            .poll(&mut std::task::Context::from_waker(&waker))
        {
            return output;
        }
        std::thread::park_timeout(std::time::Duration::from_millis(10));
    }
}

#[test]
fn an_instantiated_chat_opens_a_run_that_carries_a_turn() {
    use inference::Payload;
    use inference::core::envelope::{EventEnvelope, EventId, Timebase};
    use inference::core::id::{NodeId, StreamId};
    use inference::facade::events::SessionEvent;
    use inference::facade::live::LiveSessions;
    use inference::pipeline::nodes::log::StreamEvent;
    use inference::runtime::run::RunState;
    use inference::text::{SpanId, TranscriptCommitted};

    let stack = Stack::new()
        .bind(Binding::named(Role::Llm, "mock", BTreeMap::new()))
        .resolve_with(&Env::new())
        .expect("resolves");
    let mut instantiation = Recipe::chat().instantiate(&stack).expect("chat plans");
    let sessions = LiveSessions::new();
    let (mut session, driver) =
        block_on(instantiation.open(&stack, &sessions)).expect("the run opens");
    let run_id = session.run_id();
    let (done, ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(block_on(driver.drive()));
    });
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
        port: "user_input".to_owned(),
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
    let run = ended
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the run ends within 20 s")
        .expect("the run ends");
    assert_eq!(run.state(), RunState::Completed, "{received:#?}");
    let reply: String = received
        .iter()
        .filter_map(|event| match event {
            SessionEvent::TextDelta(delta) => Some(delta.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reply, "Hello from the mock", "{received:#?}");
}
