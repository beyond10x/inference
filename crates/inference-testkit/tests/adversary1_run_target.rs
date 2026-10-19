//! Adversary pass 1 against the run commands of the testkit conformance target
//! (`story:run-lifecycle`): what the synthesized suite does not arrange.
//!
//! The suite reaches each refusal of the Run once, from a run that owns no generation, so a
//! `CancelRun` refused from a terminal state is never sent to a run whose generations and segments
//! a cascade could touch. The authored cascade scenario cancels from Active only. The cases here
//! send both, and walk the whole transition table of `ess/domains/graph.yaml` through the target
//! rather than through `inference_runtime::run::Run` alone.

mod target;

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ViewRef};
use ess_conformance::target::{
    ConformanceTarget, Deadline, ScenarioContext, SemanticCommandRequest, SemanticCommandResult,
};
use ess_primitives::consistency::QueryConsistency;
use ess_primitives::facts::Number;
use ess_primitives::ids::CorrelationId;
use ess_primitives::node::Node;
use ess_primitives::time::Timestamp;

use target::Target;

type Row = BTreeMap<String, Node>;

fn correlation() -> CorrelationId {
    CorrelationId::new("adversary1-run").expect("a correlation id")
}

fn text(value: &str) -> Node {
    Node::Text(value.to_owned())
}

fn number(value: f64) -> Node {
    Node::Number(Number::new(value).expect("finite"))
}

fn begun() -> Target {
    let target = Target::default();
    let context = ScenarioContext::new(
        "inference.graph.CreateRun/outcome/created"
            .parse()
            .expect("a scenario id"),
        correlation(),
    );
    target
        .begin_scenario(&context)
        .expect("the scenario begins");
    target
}

fn send(target: &Target, name: &str, input: Vec<(&str, Node)>) -> SemanticCommandResult {
    target
        .execute_command(SemanticCommandRequest {
            command: name.parse::<CommandRef>().expect("a command name"),
            actor: None,
            caller: None,
            input: input
                .into_iter()
                .map(|(field, value)| (field.to_owned(), value))
                .collect(),
            correlation: correlation(),
        })
        .unwrap_or_else(|error| panic!("`{name}` answers: {error}"))
}

fn branch(result: &SemanticCommandResult) -> String {
    result
        .outcome
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default()
}

fn emitted(result: &SemanticCommandResult, field: &str) -> Node {
    result.direct_events[0].payload[field].clone()
}

fn rows(target: &Target, name: &str) -> Vec<Row> {
    target
        .query_view(ess_conformance::target::SemanticViewRequest {
            view: name.parse::<ViewRef>().expect("a view name"),
            params: BTreeMap::new(),
            consistency: QueryConsistency::Current,
            correlation: correlation(),
            deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
        })
        .unwrap_or_else(|error| panic!("`{name}` reads: {error}"))
        .rows
}

fn run_state(target: &Target, run_id: &Node) -> Node {
    rows(target, "inference.graph.Runs")
        .into_iter()
        .find(|row| &row["run_id"] == run_id)
        .map(|row| row["state"].clone())
        .expect("the run is a row of Runs")
}

/// A Planned pipeline and a node of a second pipeline, as the authored cascade scenario arranges
/// them; returns the planned pipeline's identity and the node's.
fn planned(target: &Target) -> (Node, Node) {
    let profile = send(
        target,
        "inference.policy.DeclareProfile",
        vec![
            ("kind", text("offline_strict")),
            ("egress", text("denied")),
            ("admitted", Node::Seq(Vec::new())),
        ],
    );
    let profile_id = emitted(&profile, "profile_id");
    let declare = |name: &str| {
        let declared = send(
            target,
            "inference.graph.DeclarePipeline",
            vec![
                ("name", text(name)),
                ("schema_revision", text("r1")),
                ("mode", text("turn_based")),
                ("profile_id", profile_id.clone()),
                (
                    "required_capabilities",
                    Node::Seq(vec![text("output_streaming")]),
                ),
            ],
        );
        emitted(&declared, "pipeline_id")
    };
    let pipeline_id = declare("runs");
    for command in [
        "inference.graph.ValidatePipeline",
        "inference.graph.PlanPipeline",
    ] {
        send(target, command, vec![("pipeline_id", pipeline_id.clone())]);
    }
    let holder = declare("node-holder");
    let added = send(
        target,
        "inference.graph.AddNode",
        vec![
            ("name", text("generator")),
            ("pipeline_id", holder),
            ("requires", Node::Seq(vec![text("output_streaming")])),
            ("role", text("model")),
        ],
    );
    (pipeline_id, emitted(&added, "node_id"))
}

fn on_run(target: &Target, command: &str, run_id: &Node) -> SemanticCommandResult {
    let mut input = vec![("run_id", run_id.clone())];
    if command == "FailRun" {
        input.push((
            "failure",
            Node::Map(BTreeMap::from([("detail".to_owned(), text("lost"))])),
        ));
    }
    send(target, &format!("inference.graph.{command}"), input)
}

/// A run created of `pipeline_id` and moved by `path`, each step answered by its moving outcome.
fn run_through(target: &Target, pipeline_id: &Node, path: &[&str]) -> Node {
    let created = send(
        target,
        "inference.graph.CreateRun",
        vec![("pipeline_id", pipeline_id.clone())],
    );
    assert_eq!(branch(&created), "inference.graph.CreateRun/created");
    let run_id = emitted(&created, "run_id");
    for command in path {
        let moved = on_run(target, command, &run_id);
        assert!(
            !branch(&moved).ends_with("/wrong-state"),
            "{command}: {moved:?}"
        );
    }
    run_id
}

/// An Active generation of `run_id` with one Tentative segment; returns their identities.
fn generation_with_segment(target: &Target, run_id: &Node, node_id: &Node) -> (Node, Node) {
    let started = send(
        target,
        "inference.stream.StartGeneration",
        vec![
            ("run_id", run_id.clone()),
            ("node_id", node_id.clone()),
            ("epoch", number(1.0)),
        ],
    );
    assert_eq!(branch(&started), "inference.stream.StartGeneration/started");
    let generation_id = emitted(&started, "generation_id");
    let delta = send(
        target,
        "inference.stream.EmitTextDelta",
        vec![
            ("generation_id", generation_id.clone()),
            ("sequence", number(1.0)),
            ("text", text("one")),
        ],
    );
    assert_eq!(branch(&delta), "inference.stream.EmitTextDelta/emitted");
    (generation_id, emitted(&delta, "segment_id"))
}

const ACTIVE: [&str; 3] = ["ValidateRun", "PrepareRun", "ActivateRun"];

/// A `CancelRun` refused from a terminal state moves nothing: not the run, and not the Active
/// generation or the unplayed segment the run still owns. graph.yaml: "a refused command moves
/// nothing" (`wrong_state`), and the `affects` cascade belongs to the `cancelled` outcome only.
#[test]
fn a_cancel_run_refused_from_a_terminal_state_touches_none_of_its_generations_or_segments() {
    for (terminal, path) in [
        ("Completed", &["DrainRun", "CompleteRun"][..]),
        ("Failed", &["FailRun"][..]),
    ] {
        let target = begun();
        let (pipeline_id, node_id) = planned(&target);
        let run_id = run_through(&target, &pipeline_id, &ACTIVE);
        let (generation_id, segment_id) = generation_with_segment(&target, &run_id, &node_id);
        for command in path {
            on_run(&target, command, &run_id);
        }
        assert_eq!(run_state(&target, &run_id), text(terminal));
        let generations = rows(&target, "inference.stream.Generations");
        let segments = rows(&target, "inference.stream.Segments");

        let refused = on_run(&target, "CancelRun", &run_id);

        assert_eq!(
            branch(&refused),
            "inference.graph.CancelRun/wrong-state",
            "from {terminal}"
        );
        assert_eq!(
            refused.error.as_ref().map(|error| error.fields.clone()),
            Some(BTreeMap::from([("current".to_owned(), text(terminal))]))
        );
        assert!(refused.direct_events.is_empty());
        assert_eq!(
            rows(&target, "inference.stream.Generations"),
            generations,
            "a CancelRun refused from {terminal} moved generation {generation_id:?}"
        );
        assert_eq!(
            rows(&target, "inference.stream.Segments"),
            segments,
            "a CancelRun refused from {terminal} moved segment {segment_id:?}"
        );
    }
}

/// `CancelRun` from Draining cancels the run's Active generation and discards its unplayed
/// segment, as from Active: the authored scenario cancels from Active only.
#[test]
fn cancel_run_from_draining_cancels_the_runs_active_generation_and_discards_its_segment() {
    let target = begun();
    let (pipeline_id, node_id) = planned(&target);
    let run_id = run_through(&target, &pipeline_id, &ACTIVE);
    let (generation_id, segment_id) = generation_with_segment(&target, &run_id, &node_id);
    on_run(&target, "DrainRun", &run_id);

    let cancelled = on_run(&target, "CancelRun", &run_id);

    assert_eq!(branch(&cancelled), "inference.graph.CancelRun/cancelled");
    let state_of = |view: &str, field: &str, id: &Node| {
        rows(&target, view)
            .into_iter()
            .find(|row| &row[field] == id)
            .map(|row| row["state"].clone())
    };
    assert_eq!(
        state_of(
            "inference.stream.Generations",
            "generation_id",
            &generation_id
        ),
        Some(text("Cancelled"))
    );
    assert_eq!(
        state_of("inference.stream.Segments", "segment_id", &segment_id),
        Some(text("Discarded"))
    );
}

/// The path from Created to each state, by command.
fn path_to(state: &str) -> &'static [&'static str] {
    match state {
        "Created" => &[],
        "Validating" => &["ValidateRun"],
        "Preparing" => &["ValidateRun", "PrepareRun"],
        "Active" => &ACTIVE,
        "Draining" => &["ValidateRun", "PrepareRun", "ActivateRun", "DrainRun"],
        "Completed" => &[
            "ValidateRun",
            "PrepareRun",
            "ActivateRun",
            "DrainRun",
            "CompleteRun",
        ],
        "Failed" => &["ValidateRun", "FailRun"],
        _ => &["CancelRun"],
    }
}

/// The transition table of `ess/domains/graph.yaml`: the command, its moving outcome, its
/// sources and its target.
const TABLE: [(&str, &str, &[&str], &str); 7] = [
    ("ValidateRun", "validating", &["Created"], "Validating"),
    ("PrepareRun", "preparing", &["Validating"], "Preparing"),
    ("ActivateRun", "active", &["Preparing"], "Active"),
    ("DrainRun", "draining", &["Active"], "Draining"),
    ("CompleteRun", "completed", &["Draining"], "Completed"),
    (
        "FailRun",
        "failed",
        &["Validating", "Preparing", "Active", "Draining"],
        "Failed",
    ),
    (
        "CancelRun",
        "cancelled",
        &["Created", "Validating", "Preparing", "Active", "Draining"],
        "Cancelled",
    ),
];

/// Every command from every state, through the target: a declared source moves to the declared
/// target with the one event naming the run; any other state answers `wrong-state` with
/// `RunStateConflict { current }`, no event, and an unchanged `Runs` row.
#[test]
fn every_run_command_from_every_state_answers_as_the_transition_table_declares() {
    let states = [
        "Created",
        "Validating",
        "Preparing",
        "Active",
        "Draining",
        "Completed",
        "Failed",
        "Cancelled",
    ];
    let mut moved = 0;
    let mut refused = 0;
    for state in states {
        for (command, branch_name, sources, to) in TABLE {
            let target = begun();
            let (pipeline_id, _) = planned(&target);
            let run_id = run_through(&target, &pipeline_id, path_to(state));
            assert_eq!(run_state(&target, &run_id), text(state));
            let before = rows(&target, "inference.graph.Runs");

            let answer = on_run(&target, command, &run_id);

            if sources.contains(&state) {
                moved += 1;
                assert_eq!(
                    branch(&answer),
                    format!("inference.graph.{command}/{branch_name}"),
                    "{command} from {state}"
                );
                assert_eq!(answer.direct_events.len(), 1, "{command} from {state}");
                assert_eq!(emitted(&answer, "run_id"), run_id);
                assert_eq!(
                    run_state(&target, &run_id),
                    text(to),
                    "{command} from {state}"
                );
            } else {
                refused += 1;
                assert_eq!(
                    branch(&answer),
                    format!("inference.graph.{command}/wrong-state"),
                    "{command} from {state}"
                );
                assert_eq!(
                    answer.error.as_ref().map(|error| error.fields.clone()),
                    Some(BTreeMap::from([("current".to_owned(), text(state))])),
                    "{command} from {state}"
                );
                assert!(answer.direct_events.is_empty(), "{command} from {state}");
                assert_eq!(
                    rows(&target, "inference.graph.Runs"),
                    before,
                    "{command} from {state}"
                );
            }
        }
    }
    assert_eq!((moved, refused), (14, 42));
}
