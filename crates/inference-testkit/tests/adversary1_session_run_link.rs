//! Adversary pass 1 against `story:session-run-link`, through the conformance target: what the
//! synthesized suite does not reach. The suite attaches only an identity no run carries, so the
//! store's pipeline comparison is never decided against a session over another pipeline than the
//! first one opened; and no scenario reads the consistency token a session command answers or
//! places a session event between graph events.

#![cfg(feature = "run")]

mod target;

use ess_conformance::scenario::{CommandRef, EventRef};
use ess_conformance::target::{
    ConformanceTarget, Deadline, EventObservationRequest, ObservedEvent, ScenarioContext,
    SemanticCommandRequest, SemanticCommandResult,
};
use ess_primitives::ids::CorrelationId;
use ess_primitives::node::Node;
use ess_primitives::time::Timestamp;

use target::Target;

fn correlation() -> CorrelationId {
    CorrelationId::new("adversary1-session").expect("a correlation id")
}

fn text(value: &str) -> Node {
    Node::Text(value.to_owned())
}

fn command(name: &str, input: Vec<(&str, Node)>) -> SemanticCommandRequest {
    SemanticCommandRequest {
        command: name.parse::<CommandRef>().expect("a command name"),
        actor: None,
        caller: None,
        input: input
            .into_iter()
            .map(|(field, value)| (field.to_owned(), value))
            .collect(),
        correlation: correlation(),
    }
}

fn begun() -> Target {
    let target = Target::default();
    let context = ScenarioContext::new(
        "inference.session.AttachRun/outcome/attached"
            .parse()
            .expect("a scenario id"),
        correlation(),
    );
    target
        .begin_scenario(&context)
        .expect("the scenario begins");
    target
}

fn execute(target: &Target, name: &str, input: Vec<(&str, Node)>) -> SemanticCommandResult {
    target
        .execute_command(command(name, input))
        .unwrap_or_else(|error| panic!("`{name}` answers: {error}"))
}

fn branch(result: &SemanticCommandResult) -> String {
    result
        .outcome
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default()
}

fn token(result: &SemanticCommandResult) -> String {
    result
        .consistency
        .as_ref()
        .expect("a command answers a consistency token")
        .as_str()
        .to_owned()
}

/// The field `field` of the one event `result` emitted.
fn emitted(result: &SemanticCommandResult, field: &str) -> Node {
    assert_eq!(result.direct_events.len(), 1, "{result:?}");
    result.direct_events[0].payload[field].clone()
}

fn observed(target: &Target, name: &str) -> Vec<ObservedEvent> {
    target
        .observe_events(EventObservationRequest {
            event: name.parse::<EventRef>().expect("an event name"),
            correlation: correlation(),
            deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
        })
        .unwrap_or_else(|error| panic!("`{name}` is observed: {error}"))
}

/// A Planned batch pipeline, declared, validated and planned through the target.
fn planned_pipeline(target: &Target, name: &str) -> Node {
    let profile = execute(
        target,
        "inference.policy.DeclareProfile",
        vec![
            ("kind", text("offline_strict")),
            ("egress", text("denied")),
            ("admitted", Node::Seq(Vec::new())),
        ],
    );
    let profile_id = emitted(&profile, "profile_id");
    let declared = execute(
        target,
        "inference.graph.DeclarePipeline",
        vec![
            ("name", text(name)),
            ("schema_revision", text("1")),
            ("mode", text("batch")),
            ("profile_id", profile_id),
            ("required_capabilities", Node::Seq(Vec::new())),
        ],
    );
    let pipeline_id = emitted(&declared, "pipeline_id");
    for step in [
        "inference.graph.ValidatePipeline",
        "inference.graph.PlanPipeline",
    ] {
        let result = execute(target, step, vec![("pipeline_id", pipeline_id.clone())]);
        assert!(result.error.is_none(), "{step}: {result:?}");
    }
    pipeline_id
}

fn open(target: &Target, pipeline_id: &Node) -> SemanticCommandResult {
    let result = execute(
        target,
        "inference.session.OpenSession",
        vec![
            ("pipeline_id", pipeline_id.clone()),
            ("mode", text("batch")),
        ],
    );
    assert_eq!(
        branch(&result),
        "inference.session.OpenSession/opened",
        "{result:?}"
    );
    result
}

fn create_run(target: &Target, pipeline_id: &Node) -> SemanticCommandResult {
    let result = execute(
        target,
        "inference.graph.CreateRun",
        vec![("pipeline_id", pipeline_id.clone())],
    );
    assert_eq!(
        branch(&result),
        "inference.graph.CreateRun/created",
        "{result:?}"
    );
    result
}

fn attach(target: &Target, session_id: &Node, run_id: &Node) -> SemanticCommandResult {
    execute(
        target,
        "inference.session.AttachRun",
        vec![
            ("session_id", session_id.clone()),
            ("run_id", run_id.clone()),
        ],
    )
}

/// A session over the second pipeline opened compares a run with its own pipeline, not the first
/// session's: the suite opens every session it attaches to over a pipeline the store never
/// compares with a real run, so a comparison read from the wrong session passes all 28 scenarios.
#[test]
fn each_session_compares_a_run_with_its_own_pipeline() {
    let target = begun();
    let first = planned_pipeline(&target, "first");
    let second = planned_pipeline(&target, "second");
    let over_first = emitted(&open(&target, &first), "session_id");
    let over_second = emitted(&open(&target, &second), "session_id");
    let run_of_first = emitted(&create_run(&target, &first), "run_id");
    let run_of_second = emitted(&create_run(&target, &second), "run_id");

    let own = attach(&target, &over_second, &run_of_second);
    assert_eq!(
        branch(&own),
        "inference.session.AttachRun/attached",
        "a run of the session's own pipeline is attached: {own:?}"
    );
    let foreign = attach(&target, &over_second, &run_of_first);
    assert_eq!(
        branch(&foreign),
        "inference.session.AttachRun/pipeline-mismatch",
        "a run of the first session's pipeline is not the second session's: {foreign:?}"
    );
    let crossed = attach(&target, &over_first, &run_of_second);
    assert_eq!(
        branch(&crossed),
        "inference.session.AttachRun/pipeline-mismatch",
        "{crossed:?}"
    );
}

/// The target's one token advances by the events a session command publishes and stays where it
/// was on a refusal, as it does for every other domain; no scenario of the suite reads it.
#[test]
fn a_session_command_advances_the_target_token_by_its_events() {
    let target = begun();
    let pipeline_id = planned_pipeline(&target, "tokens");
    let opened = open(&target, &pipeline_id);
    let session_id = emitted(&opened, "session_id");
    let after_open = token(&opened);

    let refused = execute(
        &target,
        "inference.session.MarkSessionReady",
        vec![("session_id", session_id.clone())],
    );
    assert_eq!(
        branch(&refused),
        "inference.session.MarkSessionReady/no-current-run"
    );
    assert_eq!(token(&refused), after_open, "a refusal publishes nothing");

    let created = create_run(&target, &pipeline_id);
    let run_id = emitted(&created, "run_id");
    let after_run = token(&created);

    let mut expected = after_run
        .strip_prefix("seq:")
        .and_then(|digits| digits.parse::<u64>().ok())
        .expect("the target writes `seq:<n>`");
    let attached = attach(&target, &session_id, &run_id);
    expected += 1;
    assert_eq!(token(&attached), format!("seq:{expected}"), "AttachRun");
    for name in [
        "inference.session.MarkSessionReady",
        "inference.session.CloseSession",
        "inference.session.CompleteSessionClose",
    ] {
        let result = execute(&target, name, vec![("session_id", session_id.clone())]);
        assert!(result.error.is_none(), "{name}: {result:?}");
        expected += 1;
        assert_eq!(token(&result), format!("seq:{expected}"), "{name}");
    }
}

/// Session and graph events interleaved by the commands that emitted them keep that order in the
/// target's one publication order, and the last one's position is the token its command answered.
#[test]
fn session_events_interleave_with_graph_events_in_command_order() {
    let target = begun();
    let pipeline_id = planned_pipeline(&target, "order");
    let session_id = emitted(&open(&target, &pipeline_id), "session_id");
    let run_id = emitted(&create_run(&target, &pipeline_id), "run_id");
    let attached = attach(&target, &session_id, &run_id);
    assert_eq!(branch(&attached), "inference.session.AttachRun/attached");
    let second_run = emitted(&create_run(&target, &pipeline_id), "run_id");
    let reattached = attach(&target, &session_id, &second_run);
    assert_eq!(branch(&reattached), "inference.session.AttachRun/attached");

    let position = |name: &str, index: usize| {
        observed(&target, name)[index]
            .sequence
            .unwrap_or_else(|| panic!("`{name}` #{index} carries its position"))
    };
    let order = [
        position("inference.session.SessionOpened", 0),
        position("inference.graph.RunCreated", 0),
        position("inference.session.RunAttached", 0),
        position("inference.graph.RunCreated", 1),
        position("inference.session.RunAttached", 1),
    ];
    assert!(
        order.windows(2).all(|pair| pair[0] + 1 == pair[1]),
        "one event per command, in command order: {order:?}"
    );
    assert_eq!(format!("seq:{}", order[4]), token(&reattached));
}
