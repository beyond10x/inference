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
    Binding, EXPOSURE_CAPACITY, Env, Error, ErrorKind, MAX_TOOL_ITERATIONS, Recipe, Role, Stack,
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
    assert_eq!(names.len(), 10);
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

/// A declaration holding `chat`'s pipeline, bound to the fixture's generator, and the catalog,
/// profile and pairs to declare more into it.
fn holding_chat() -> (Declaration, Fixture, ProfileId, Vec<(Role, AdapterId)>) {
    let (catalog, ids) = fixture(false);
    let profile_id = catalog.profile.profile_id();
    let pairs = vec![(Role::Llm, ids[1])];
    let mut declaration = Declaration::new();
    Recipe::chat()
        .declare_into(&mut declaration, &catalog, profile_id, &pairs, &[])
        .expect("chat declares");
    (declaration, catalog, profile_id, pairs)
}

#[test]
fn a_declare_into_refused_for_an_end_or_a_role_bound_node_leaves_the_declaration_as_it_was() {
    let (mut declaration, catalog, profile_id, pairs) = holding_chat();
    let before = declaration.clone();

    let error = Recipe::chat()
        .expose("no such port")
        .declare_into(&mut declaration, &catalog, profile_id, &pairs, &[])
        .expect_err("no port carries the name");
    assert_eq!(error.kind(), ErrorKind::Layer);
    assert!(error.to_string().contains("no such port"), "{error}");
    assert_eq!(declaration, before, "an end no port carries");

    let error = Recipe::custom("ask-absent", InteractionMode::TurnBased, ask)
        .role("absent", Role::Llm)
        .declare_into(&mut declaration, &catalog, profile_id, &pairs, &[])
        .expect_err("the declare function adds no node `absent`");
    assert_eq!(error.kind(), ErrorKind::Layer);
    assert!(error.to_string().contains("absent"), "{error}");
    assert_eq!(
        declaration, before,
        "a role-bound node the recipe did not add"
    );
}

#[test]
fn a_node_given_by_a_name_the_recipe_has_no_node_for_is_refused_before_anything_is_declared() {
    let (mut declaration, catalog, profile_id, pairs) = holding_chat();
    let before = declaration.clone();
    let recipe = Recipe::chat().with_node("no such node", replacement(&Log::default()));

    let error = recipe
        .declare_into(&mut declaration, &catalog, profile_id, &pairs, &[])
        .expect_err("chat has no node of that name");
    assert_eq!(error.kind(), ErrorKind::Layer);
    assert!(error.to_string().contains("no such node"), "{error}");
    assert_eq!(declaration, before);

    let stack = Stack::new()
        .bind(Binding::named(Role::Llm, "mock", BTreeMap::new()))
        .resolve_with(&Env::new())
        .expect("resolves");
    let error = recipe
        .instantiate(&stack)
        .expect_err("instantiating refuses it too");
    assert_eq!(error.kind(), ErrorKind::Layer);
    assert!(error.to_string().contains("no such node"), "{error}");
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
        assert!(
            matches!(&error, Error::RecipeNameTaken { name } if name == taken),
            "{error:?}"
        );
        assert_eq!(error.kind(), ErrorKind::RecipeNameTaken);
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
    let (session, driver) = block_on(instantiation.open(&stack, &sessions)).expect("the run opens");
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

#[test]
fn a_chat_over_a_generator_without_concurrent_input_refuses_to_open() {
    use inference::Error;
    use inference::facade::live::LiveSessions;

    let stack = Stack::new()
        .bind(Binding::llm(MockGenerator::new(
            llm::MockConfiguration::StreamingOnly,
            ModelId::generate(),
            ["never sent"],
        )))
        .resolve_with(&Env::new())
        .expect("resolves");
    let mut instantiation = Recipe::chat().instantiate(&stack).expect("chat plans");
    let sessions = LiveSessions::new();
    let Err(error) = block_on(instantiation.open(&stack, &sessions)) else {
        panic!("opening refuses a generator that takes no input while it is Active");
    };
    assert_eq!(error.kind(), ErrorKind::ConcurrencyUnclaimed, "{error}");
    let Error::ConcurrencyUnclaimed { role, .. } = &error else {
        unreachable!("the kind is ConcurrencyUnclaimed");
    };
    assert_eq!(role, "llm");
    let shown = error.to_string();
    assert!(shown.contains("concurrent_input_output"), "{shown}");
    assert!(shown.contains("bind the role llm"), "{shown}");
    assert!(
        sessions.sessions_view().is_empty(),
        "no session is left open: {:?}",
        sessions.sessions_view()
    );
}

/// `native_voice` over the duplex mock, opened as a live session: a "lookup" tool whose executor
/// answers "Berlin" and counts its runs, and the mock proposing one call of it.
mod native {
    use std::collections::BTreeMap;
    use std::future;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use inference::Payload;
    use inference::core::envelope::{EventEnvelope, EventId, Timebase};
    use inference::core::id::{NodeId, StreamId};
    use inference::core::model::Locality;
    use inference::duplex::SpeechCallId;
    use inference::facade::events::SessionEvent;
    use inference::facade::live::{LiveSession, LiveSessions};
    use inference::runtime::run::RunState;
    use inference::tools::{
        CancellationSignal, DeclareTool, ExecutionFuture, SideEffectClass, Tool, ToolCall,
        ToolCallId, ToolExecutor,
    };
    use inference::{
        AdapterDescriptor, Binding, Env, Instantiation, Recipe, Registry, Role, Stack,
    };
    use inference_testkit::duplex::mock::{MockSpeechModel, Reply, ScriptedTurn, pcm, utterance};
    use serde_json::json;

    use super::block_on;

    /// Answers every call with "Berlin" and counts its runs.
    struct Berlin(Arc<AtomicUsize>);

    impl ToolExecutor for Berlin {
        fn execute<'a>(
            &'a self,
            _: &'a ToolCall,
            _: Instant,
            _: &'a CancellationSignal,
        ) -> ExecutionFuture<'a> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(future::ready(Ok(json!("Berlin"))))
        }
    }

    fn lookup(requires_confirmation: bool) -> Tool {
        Tool::declare(DeclareTool {
            name: "lookup".to_owned(),
            description: "looks a city up".to_owned(),
            schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
            schema_revision: "1".to_owned(),
            side_effect: SideEffectClass::ReadOnly,
            requires_confirmation,
        })
        .0
    }

    fn model() -> MockSpeechModel {
        MockSpeechModel::native(vec![ScriptedTurn {
            heard: "where is it".to_owned(),
            reply: Reply::Call {
                tool: "lookup".to_owned(),
                arguments: json!({"city": "Berlin"}),
                then: "It is {result}".to_owned(),
            },
        }])
    }

    /// A stack binding `speech` to the mock.
    fn stack() -> Stack {
        stack_over(model())
    }

    /// A stack binding `speech` to `model`.
    fn stack_over(model: MockSpeechModel) -> Stack {
        Stack::new()
            .bind(Binding::speech(model))
            .resolve_with(&Env::new())
            .expect("resolves")
    }

    /// `native_voice` given "lookup" and its executor, and the executor's run count.
    fn with_lookup(requires_confirmation: bool) -> (Recipe, Arc<AtomicUsize>) {
        let runs = Arc::new(AtomicUsize::new(0));
        let executor: Box<dyn ToolExecutor> = Box::new(Berlin(Arc::clone(&runs)));
        let recipe = Recipe::native_voice().tools([(lookup(requires_confirmation), executor)]);
        (recipe, runs)
    }

    type Ended = Result<inference::runtime::Run, inference::runtime::RunError>;

    struct Live {
        session: LiveSession<Payload>,
        ended: mpsc::Receiver<Ended>,
        stream_id: StreamId,
        sequence: i64,
        seen: Vec<SessionEvent>,
    }

    impl Live {
        fn open(instantiation: &mut Instantiation, stack: &Stack) -> Self {
            let sessions = LiveSessions::new();
            let (session, driver) =
                block_on(instantiation.open(stack, &sessions)).expect("the run opens");
            let (done, ended) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = done.send(block_on(driver.drive()));
            });
            Self {
                session,
                ended,
                stream_id: StreamId::generate(),
                sequence: 0,
                seen: Vec::new(),
            }
        }

        /// Feeds one utterance: its speech and the silence after it.
        fn speak(&mut self) {
            for frame in utterance(pcm(24_000), 0).frames() {
                self.sequence += 1;
                let envelope = EventEnvelope {
                    schema_version: 1,
                    event_id: EventId::generate(),
                    run_id: self.session.run_id(),
                    session_id: None,
                    stream_id: self.stream_id,
                    sequence: self.sequence,
                    producer: NodeId::generate(),
                    port: "audio".to_owned(),
                    kind: "inference.audio.AudioFrame".to_owned(),
                    generation_id: None,
                    timestamp: 0,
                    timebase: Timebase::WallTime,
                    causation_id: None,
                    payload: Payload::Audio(frame),
                };
                block_on(self.session.send(envelope)).expect("sent");
            }
        }

        /// Reads the session's events up to the first `stop` answers.
        fn until(&mut self, stop: impl Fn(&SessionEvent) -> bool) {
            loop {
                let event = block_on(self.session.receive())
                    .unwrap_or_else(|| panic!("the session ended first: {:#?}", self.seen));
                let done = stop(&event);
                self.seen.push(event);
                if done {
                    return;
                }
            }
        }

        /// Closes the session and reads it to its end; how long the close took.
        fn close(&mut self) -> Duration {
            let started = Instant::now();
            block_on(self.session.close()).expect("the session closes");
            let took = started.elapsed();
            while let Some(event) = block_on(self.session.receive()) {
                self.seen.push(event);
            }
            let run = self
                .ended
                .recv_timeout(Duration::from_secs(20))
                .expect("the run ends within 20 s")
                .expect("the run ends");
            assert_eq!(run.state(), RunState::Completed, "{:#?}", self.seen);
            took
        }

        /// The response transcript the session delivered.
        fn said(&self) -> String {
            self.seen
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::TextDelta(delta) => Some(delta.text.as_str()),
                    _ => None,
                })
                .collect()
        }

        fn position(&self, find: impl Fn(&SessionEvent) -> bool) -> Option<usize> {
            self.seen.iter().position(find)
        }
    }

    fn says(text: &'static str) -> impl Fn(&SessionEvent) -> bool {
        move |event| matches!(event, SessionEvent::TextDelta(delta) if delta.text.contains(text))
    }

    #[test]
    fn a_tool_call_is_answered_under_the_providers_call_id() {
        let stack = stack();
        let (recipe, runs) = with_lookup(false);
        let mut instantiation = recipe.instantiate(&stack).expect("native voice plans");
        let mut live = Live::open(&mut instantiation, &stack);
        live.speak();
        live.until(says("Berlin"));
        assert_eq!(live.said(), "It is \"Berlin\"", "{:#?}", live.seen);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        let proposed = live
            .position(|event| matches!(event, SessionEvent::ToolCallProposed(_)))
            .unwrap_or_else(|| panic!("the call reaches the session: {:#?}", live.seen));
        let said = live.position(says("Berlin")).expect("said");
        assert!(proposed < said, "{:#?}", live.seen);
        let SessionEvent::ToolCallProposed(call) = &live.seen[proposed] else {
            unreachable!("found as a proposal");
        };
        let call_ids = instantiation.call_ids().expect("the run's call ids");
        assert_eq!(
            call_ids.speech_call_id(call.call_id),
            Some(SpeechCallId::new("call_1"))
        );
        live.close();
    }

    #[test]
    fn with_no_tools_the_call_is_answered_by_a_rejection() {
        let mut offered = inference::default_session_config();
        offered.tools = vec![lookup(false)];
        Registry::register(
            AdapterDescriptor::new("recipes-lookup-offered", Locality::SameProcess)
                .role(Role::Speech, |_| Ok(Binding::speech(model()).into()))
                .session_config(offered),
        )
        .expect("registers");
        let stack = Stack::new()
            .bind(Binding::named(
                Role::Speech,
                "recipes-lookup-offered",
                BTreeMap::new(),
            ))
            .resolve_with(&Env::new())
            .expect("resolves");
        let mut instantiation = Recipe::native_voice()
            .instantiate(&stack)
            .expect("native voice plans");
        let mut live = Live::open(&mut instantiation, &stack);
        live.speak();
        live.until(says("denied"));
        live.close();
        let said = live.said();
        assert!(said.starts_with("It is "), "{said}");
        assert!(
            live.position(|event| matches!(
                event,
                SessionEvent::ToolCallProposed(_) | SessionEvent::ToolCallStarted(_)
            ))
            .is_none(),
            "the call never leaves the node, so no executor runs: {:#?}",
            live.seen
        );
    }

    /// Opens `native_voice` whose "lookup" requires confirmation and waits until its call is
    /// held: the authority holds it and the executor has not run.
    fn held() -> (Instantiation, Live, Arc<AtomicUsize>, ToolCallId) {
        let stack = stack();
        let (recipe, runs) = with_lookup(true);
        let mut instantiation = recipe.instantiate(&stack).expect("native voice plans");
        let mut live = Live::open(&mut instantiation, &stack);
        live.speak();
        live.until(|event| matches!(event, SessionEvent::ToolCallProposed(_)));
        let authority = instantiation.authority().expect("the run's authority");
        let deadline = Instant::now() + Duration::from_secs(5);
        let held = loop {
            let held = authority.held();
            if !held.is_empty() || Instant::now() > deadline {
                break held;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(held.len(), 1, "{:#?}", live.seen);
        assert_eq!(runs.load(Ordering::SeqCst), 0, "a held call does not run");
        let call_id = held[0].call_id();
        (instantiation, live, runs, call_id)
    }

    #[test]
    fn a_denied_confirmation_answers_the_model_with_a_rejection() {
        let (instantiation, mut live, runs, call_id) = held();
        assert!(instantiation.authority().expect("held").deny(call_id));
        live.until(says("rejected"));
        assert!(live.said().starts_with("It is "), "{}", live.said());
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        live.close();
    }

    #[test]
    fn a_confirmed_call_runs_the_executor() {
        let (instantiation, mut live, runs, call_id) = held();
        assert!(instantiation.authority().expect("held").confirm(call_id));
        live.until(says("Berlin"));
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        live.close();
    }

    #[test]
    fn closing_after_denying_every_held_call_ends_within_five_seconds() {
        let (instantiation, mut live, _, _) = held();
        let authority = instantiation.authority().expect("held");
        for call in authority.held() {
            assert!(authority.deny(call.call_id()));
        }
        let took = live.close();
        assert!(took <= Duration::from_secs(5), "the close took {took:?}");
    }

    #[test]
    fn the_models_close_reaches_the_session_once_after_the_reply() {
        let stack = stack_over(model().closes());
        let (recipe, _) = with_lookup(false);
        let mut instantiation = recipe.instantiate(&stack).expect("native voice plans");
        let mut live = Live::open(&mut instantiation, &stack);
        live.speak();
        live.until(|event| matches!(event, SessionEvent::SpeechSessionClosed(_)));
        live.close();
        let closed = |event: &SessionEvent| matches!(event, SessionEvent::SpeechSessionClosed(_));
        assert_eq!(
            live.seen.iter().filter(|event| closed(event)).count(),
            1,
            "{:#?}",
            live.seen
        );
        let said = live.position(says("Berlin")).expect("the reply is said");
        let closed = live.position(closed).expect("closed");
        assert!(said < closed, "{:#?}", live.seen);
    }

    /// Adversary: `Instantiation::authority` is documented as the confirmation hook of every
    /// opened `native_voice` run. Replacing the `speech` node with `with_node` leaves the run's
    /// authority in place, yet `take_tools` hands the hook out only when the duplex node's call
    /// ids were taken too, so a call held for confirmation can never be answered.
    #[test]
    fn adversary_native_voice_with_its_speech_node_replaced_keeps_its_confirmation_hook() {
        let stack = stack();
        let log = super::Log::default();
        let (recipe, _) = with_lookup(true);
        let mut instantiation = recipe
            .with_node("speech", super::replacement(&log))
            .instantiate(&stack)
            .expect("native voice plans with its speech node replaced");
        let sessions = LiveSessions::new();
        let opened = block_on(instantiation.open(&stack, &sessions));
        assert!(opened.is_ok(), "the run opens: {:?}", opened.as_ref().err());
        assert_eq!(super::logged(&log), ["speech", "started"]);
        assert!(
            instantiation.authority().is_some(),
            "the opened native_voice run's authority has no hook, though its authority node runs"
        );
    }

    /// Adversary: closing a live `native_voice` session while a call is still held for
    /// confirmation. Nothing answers the held call, so the authority waits for it, the duplex
    /// node waits for `tool_result` to end, and the close never returns.
    #[test]
    fn adversary_closing_with_a_held_call_unanswered_ends() {
        let (instantiation, mut live, runs, _) = held();
        let (done, closed) = mpsc::channel();
        std::thread::spawn(move || {
            let took = live.close();
            let _ = done.send(took);
        });
        match closed.recv_timeout(Duration::from_secs(15)) {
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => panic!(
                "closing a native_voice session with a call held unanswered does not end within 15 s"
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("the close panicked; see the thread's output")
            }
        }
        assert_eq!(runs.load(Ordering::SeqCst), 0, "a held call does not run");
        drop(instantiation);
    }

    #[test]
    fn closing_with_a_held_call_unanswered_rejects_it_denied() {
        use inference::pipeline::nodes::AuthorityOutcome;
        use inference::tools::RejectionReason;

        let (instantiation, mut live, runs, call_id) = held();
        let (done, closed) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(live.close());
        });
        let took = closed
            .recv_timeout(Duration::from_secs(15))
            .expect("the close ends within 15 s");
        assert!(took <= Duration::from_secs(5), "the close took {took:?}");
        let authority = instantiation.authority().expect("the run's authority");
        assert!(authority.held().is_empty(), "{:#?}", authority.held());
        let rejected: Vec<RejectionReason> = authority
            .decisions()
            .into_iter()
            .filter(|decision| decision.call.call_id() == call_id)
            .filter_map(|decision| match decision.outcome {
                AuthorityOutcome::Rejected(rejected) => Some(rejected.reason),
                _ => None,
            })
            .collect();
        assert_eq!(rejected, [RejectionReason::Denied]);
        assert!(
            !authority.confirm(call_id),
            "a call the close denied is not confirmed"
        );
        assert_eq!(
            runs.load(Ordering::SeqCst),
            0,
            "a call the close denied does not run"
        );
    }

    /// With its `authority` node replaced, `native_voice` builds no authority of its own: the
    /// executor answers to none and the run has no confirmation hook.
    #[test]
    fn native_voice_with_its_authority_replaced_has_no_confirmation_hook() {
        let stack = stack();
        let log = super::Log::default();
        let (recipe, _) = with_lookup(true);
        let mut instantiation = recipe
            .with_node("authority", super::replacement(&log))
            .instantiate(&stack)
            .expect("native voice plans with its authority replaced");
        let sessions = LiveSessions::new();
        let opened = block_on(instantiation.open(&stack, &sessions));
        assert!(opened.is_ok(), "the run opens: {:?}", opened.as_ref().err());
        assert!(instantiation.authority().is_none());
        assert!(instantiation.call_ids().is_some());
    }
}

/// Adversary: `Recipe::chat().expose("committed_text")` exposes the agent's output port a second
/// time, beside its own `assistant_text` exposure of that port. `expose` promises "that exposed
/// end and nothing else": the reply still arrives once and the run completes.
#[test]
fn adversary_exposing_an_exposed_port_under_its_port_name_keeps_one_reply() {
    use inference::Payload;
    use inference::core::envelope::{EventEnvelope, EventId, Timebase};
    use inference::core::id::{NodeId, StreamId};
    use inference::facade::events::SessionEvent;
    use inference::facade::live::LiveSessions;
    use inference::pipeline::nodes::log::StreamEvent;
    use inference::runtime::run::RunState;
    use inference::text::{SpanId, TranscriptCommitted};

    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let stack = Stack::new()
            .bind(Binding::named(Role::Llm, "mock", BTreeMap::new()))
            .resolve_with(&Env::new())
            .expect("resolves");
        let mut instantiation = Recipe::chat()
            .expose("committed_text")
            .instantiate(&stack)
            .expect("chat plans with the port exposed under its own name too");
        let sessions = LiveSessions::new();
        let (session, driver) =
            block_on(instantiation.open(&stack, &sessions)).expect("the run opens");
        let run_id = session.run_id();
        let (ran, ended) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = ran.send(block_on(driver.drive()));
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
        let reply: String = received
            .iter()
            .filter_map(|event| match event {
                SessionEvent::TextDelta(delta) => Some(delta.text.clone()),
                _ => None,
            })
            .collect();
        let _ = done.send((run.state(), reply, format!("{received:#?}")));
    });
    let (state, reply, received) = finished
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the case finishes within 30 s without panicking");
    assert_eq!(state, RunState::Completed, "{received}");
    assert_eq!(reply, "Hello from the mock", "{received}");
}

/// Adversary: the story carries the sdk-stack review's finding that no SDK code refuses a denied
/// locality at the binding accessors (R213 "covers those accessors too, not only planning").
/// Under the default `offline_strict` profile, a hosted generator is refused by planning, and
/// `StackBinding::generator()` still hands it out.
mod adversary_locality {
    use std::sync::Arc;

    use inference::core::model::{
        Adapter, AdapterDescriptor as ModelDescriptor, DeclareAdapter, ExecutionForm, Locality,
        ModelId,
    };
    use inference::llm::Generator;
    use inference::llm::llm_core::{
        BoxFuture, Cancel, Capabilities, Id, Item, Model, Protocol, Provenance, StopReason,
        StreamSink, TurnObservation, TurnOutcome, TurnRequest,
    };
    use inference::{Binding, Env, ErrorKind, Recipe, Role, Stack};

    struct HostedModel {
        provenance: Provenance,
        capabilities: Capabilities,
    }

    impl Model for HostedModel {
        fn provenance(&self) -> &Provenance {
            &self.provenance
        }

        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn turn<'a>(
            &'a self,
            _request: &'a TurnRequest,
            _sink: &'a mut dyn StreamSink,
            _cancel: &'a Cancel,
        ) -> BoxFuture<'a, Result<TurnOutcome, inference::llm::llm_core::Error>> {
            Box::pin(async move {
                Ok(TurnOutcome {
                    stop_reason: StopReason::EndTurn,
                    items: vec![Item::assistant("hosted".to_owned())],
                    observation: TurnObservation::new(self.provenance.clone()),
                })
            })
        }
    }

    fn hosted_generator() -> Generator {
        let id = |value: &str| Id::new(value).expect("a printable identifier");
        let model = HostedModel {
            provenance: Provenance {
                protocol: Protocol::Responses,
                provider: id("adversary"),
                account: id("hosted"),
                endpoint: id("remote"),
                model: id("hosted"),
                binding_revision: id("1"),
            },
            capabilities: Capabilities::text(1 << 12, 1 << 10),
        };
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "adversary hosted generator".to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: Vec::new(),
            max_concurrency: 1,
            descriptor: ModelDescriptor::default(),
        })
        .expect("a concurrency of one is positive");
        Generator::new(adapter, Arc::new(model))
            .expect("the adapter claims nothing the model does not declare")
    }

    #[test]
    fn adversary_an_offline_strict_stack_does_not_hand_out_a_hosted_generator() {
        let resolved = Stack::new()
            .bind(Binding::llm(hosted_generator()))
            .resolve_with(&Env::new());
        let stack = match resolved {
            Err(error) => {
                assert_eq!(error.kind(), ErrorKind::ProfileDenies, "{error}");
                return;
            }
            Ok(stack) => stack,
        };
        let Err(error) = Recipe::chat().instantiate(&stack) else {
            panic!("planning admits a hosted generator under offline_strict");
        };
        assert_eq!(error.kind(), ErrorKind::ProfileDenies, "{error}");
        assert!(
            stack
                .adapter(Role::Llm)
                .and_then(inference::StackBinding::generator)
                .is_none(),
            "planning refuses the hosted generator as ProfileDenies, and the stack's llm binding \
             still hands it out under offline_strict"
        );
    }
}

/// A node implementation that records, in the log it shares, that it started, and at its drain
/// takes its inputs to their stop and sends nothing.
struct Replacement(
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    Option<inference::pipeline::NodeEndpoints<inference::Payload>>,
);

impl std::fmt::Debug for Replacement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Replacement").field(&self.0).finish()
    }
}

impl Replacement {
    fn note(&self, what: &str) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(what.to_owned());
    }
}

impl inference::pipeline::NodeImplementation<inference::Payload> for Replacement {
    fn describe(&mut self, _: &inference::graph::Node) -> inference::pipeline::NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn prepare(
        &mut self,
    ) -> inference::pipeline::NodeFuture<'_, Result<(), inference::graph::NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn start(
        &mut self,
        endpoints: inference::pipeline::NodeEndpoints<inference::Payload>,
    ) -> inference::pipeline::NodeFuture<'_, Result<(), inference::graph::NodeError>> {
        self.note("started");
        self.1 = Some(endpoints);
        Box::pin(std::future::ready(Ok(())))
    }

    fn drain(
        &mut self,
    ) -> inference::pipeline::NodeFuture<'_, Result<(), inference::graph::NodeError>> {
        let endpoints = self.1.take();
        Box::pin(async move {
            for mut input in endpoints
                .map(|endpoints| endpoints.inputs)
                .unwrap_or_default()
            {
                while let Some(envelope) = input.endpoint.recv().await {
                    if matches!(
                        envelope.payload,
                        inference::Payload::Control(inference::graph::ControlKind::Stop)
                    ) {
                        break;
                    }
                }
            }
            Ok(())
        })
    }

    fn cancel(&mut self) -> inference::pipeline::NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn stop(&mut self) -> inference::pipeline::NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }
}

/// The log a [`replacement`] factory and its nodes write.
type Log = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

fn logged(log: &Log) -> Vec<String> {
    log.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// A factory of [`Replacement`]s recording, in `log`, the name of each node it builds.
fn replacement(
    log: &Log,
) -> impl Fn(inference::core::id::RunId, &inference::graph::Node) -> inference::graph::Implementation
+ Send
+ Sync
+ 'static {
    let log = log.clone();
    move |_, node| {
        let replaced = Replacement(log.clone(), None);
        replaced.note(node.name());
        Box::new(replaced)
    }
}

/// The names of the nodes `declaration` binds, in binding order.
fn bound_names(declaration: &Declaration) -> Vec<String> {
    declaration
        .bindings()
        .filter_map(|binding| declaration.node(binding.node_id()))
        .map(|node| node.name().to_owned())
        .collect()
}

#[test]
fn with_node_replaces_the_implementation_of_that_node_and_no_other() {
    use inference::facade::live::LiveSessions;

    let stack = mock_stack();
    let log = Log::default();
    let replaced = Recipe::voice_cascade().with_node("tts", replacement(&log));
    assert_eq!(replaced.roles(), Recipe::voice_cascade().roles());
    let mut instantiation = replaced.instantiate(&stack).expect("the cascade plans");
    let unchanged = Recipe::voice_cascade()
        .instantiate(&stack)
        .expect("the cascade plans");
    assert_eq!(
        counts(instantiation.declaration()),
        counts(unchanged.declaration())
    );
    assert_eq!(
        bound_names(instantiation.declaration()),
        bound_names(unchanged.declaration()),
        "the replaced node keeps its role and its binding"
    );
    let sessions = LiveSessions::new();
    let Err(error) = block_on(instantiation.open(&stack, &sessions)) else {
        panic!("the cascade's speaker needs an agent's audio route");
    };
    assert!(error.to_string().contains("audio route"), "{error}");
    assert_eq!(
        logged(&log),
        ["tts"],
        "the factory builds the node `tts` and no other"
    );

    let log = Log::default();
    let mut instantiation = Recipe::speak()
        .with_node("tts", replacement(&log))
        .instantiate(&stack)
        .expect("speak plans");
    let (session, driver) = block_on(instantiation.open(&stack, &sessions)).expect("the run opens");
    let (done, ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(block_on(driver.drive()));
    });
    block_on(session.close()).expect("the session closes");
    let _ = ended
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the run ends within 20 s");
    assert_eq!(
        logged(&log),
        ["tts", "started"],
        "the run starts the replacement in place of the stack's synthesiser"
    );

    let added = Recipe::chat().with_node("extra", replacement(&log));
    assert_eq!(
        added.roles(),
        [Role::Llm],
        "a node added by name has no role"
    );

    let unclaimed = Stack::new()
        .bind(Binding::llm(MockGenerator::new(
            llm::MockConfiguration::StreamingOnly,
            ModelId::generate(),
            ["never sent"],
        )))
        .resolve_with(&Env::new())
        .expect("resolves");
    let mut instantiation = Recipe::chat()
        .with_node("agent", replacement(&log))
        .instantiate(&unclaimed)
        .expect("chat plans");
    let (session, driver) = block_on(instantiation.open(&unclaimed, &sessions)).expect(
        "a replaced model node asks the binding for no concurrent_input_output, and the run opens",
    );
    let (done, ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(block_on(driver.drive()));
    });
    block_on(session.close()).expect("the session closes");
    let _ = ended
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the run ends within 20 s");
}

#[test]
fn expose_refuses_a_name_no_port_carries() {
    let Err(error) = Recipe::chat().expose("absent").instantiate(&mock_stack()) else {
        panic!("no port of chat is named `absent`");
    };
    assert_eq!(error.kind(), ErrorKind::Layer, "{error}");
    let shown = error.to_string();
    assert!(
        shown.contains("`absent`") && shown.contains("0 of its ports"),
        "{shown}"
    );
}

/// [`ask`] with its model's output port named `assistant_text`, optional and not exposed.
fn ask_unexposed(pipeline: &mut Builder<'_, '_>) -> inference::Result<()> {
    let text = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let agent = pipeline.node("agent", NodeRole::Model, vec![Capability::OutputStreaming]);
    let input = pipeline.input::<modality::Text>(&agent, "user_input", text);
    let optional = PortTraits {
        essentiality: Essentiality::Optional,
        ..text
    };
    pipeline.output::<modality::Text>(&agent, "assistant_text", optional);
    pipeline.expose(
        &input,
        "user_input",
        EXPOSURE_CAPACITY,
        OverloadPolicy::Pause,
    );
    Ok(())
}

/// Each exposure of `declaration`: its name, its port's name, capacity and overload policy.
fn exposures(declaration: &Declaration) -> Vec<(String, String, usize, OverloadPolicy)> {
    declaration
        .exposures()
        .map(|exposure| {
            let port = declaration
                .ports()
                .find(|port| port.port_id() == exposure.port_id())
                .map_or_else(String::new, |port| port.name().to_owned());
            (
                exposure.name().to_owned(),
                port,
                exposure.capacity().get(),
                exposure.overload(),
            )
        })
        .collect()
}

#[test]
fn expose_adds_that_exposed_end_to_the_planned_pipeline_and_nothing_else() {
    let stack = mock_stack();
    let base = Recipe::custom("ask-expose", InteractionMode::TurnBased, ask_unexposed)
        .role("agent", Role::Llm);
    let before = base.instantiate(&stack).expect("the recipe plans");
    let after = base
        .clone()
        .expose("assistant_text")
        .instantiate(&stack)
        .expect("the recipe plans with the exposed end");
    let mut expected = exposures(before.declaration());
    expected.push((
        "assistant_text".to_owned(),
        "assistant_text".to_owned(),
        64,
        OverloadPolicy::Pause,
    ));
    assert_eq!(exposures(after.declaration()), expected);
    assert_eq!(counts(after.declaration()), counts(before.declaration()));
    assert_eq!(
        after.declaration().edges().count(),
        before.declaration().edges().count()
    );
    assert_eq!(
        bound_names(after.declaration()),
        bound_names(before.declaration())
    );

    let chat = Recipe::chat().expose("assistant_text").declaration();
    assert_eq!(
        exposures(&chat),
        exposures(&Recipe::chat().declaration()),
        "an end the recipe already exposes is not exposed twice"
    );
}
