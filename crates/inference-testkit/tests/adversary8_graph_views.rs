//! Adversary pass 1 against the `inference.graph` declaration views and events of the testkit
//! target, the ones no passing scenario reads.
//!
//! Of the fourteen scenarios this story makes pass, only `DeclarePipeline/declared`,
//! `AddPort/added` and `ConnectPorts/unbounded` read a graph view (`Pipelines`, `Ports`), and none
//! reads `Nodes`, `Edges` or `Bindings` or the payload of `NodeAdded`, `PortsConnected` or
//! `NodeBound`: the scenarios that do arrange a validation and stay skipped. The suite sends the
//! three port flags all `true`, and every domain counts its identities from
//! `00000000-0000-4000-8000-000000000001`, so in an arrangement like the module tests' the first
//! adapter and the first node carry the same identity, and a row that writes one where the other
//! belongs reads the same. Each case here arranges identities that differ and flags that differ,
//! and reads every field.

mod target;

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, EventRef, ViewRef};
use ess_conformance::target::{
    ConformanceTarget, Deadline, EventObservationRequest, ScenarioContext, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest,
};
use ess_primitives::consistency::QueryConsistency;
use ess_primitives::facts::Number;
use ess_primitives::ids::CorrelationId;
use ess_primitives::node::Node;
use ess_primitives::time::Timestamp;

use target::Target;

fn correlation() -> CorrelationId {
    CorrelationId::new("adversary8").expect("a correlation id")
}

fn text(value: &str) -> Node {
    Node::Text(value.to_owned())
}

fn float(value: f64) -> Node {
    Node::Number(Number::new(value).expect("finite"))
}

fn list(values: &[&str]) -> Node {
    Node::Seq(values.iter().map(|value| text(value)).collect())
}

fn begun() -> Target {
    let target = Target::default();
    let context = ScenarioContext::new(
        "inference.graph.DeclarePipeline/outcome/declared"
            .parse()
            .expect("a scenario id"),
        correlation(),
    );
    target
        .begin_scenario(&context)
        .expect("the scenario begins");
    target
}

fn run(target: &Target, name: &str, input: Vec<(&str, Node)>) -> SemanticCommandResult {
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

/// The field `field` of the one event `result` emitted.
fn emitted(result: &SemanticCommandResult, field: &str) -> Node {
    assert_eq!(result.direct_events.len(), 1, "{result:?}");
    result.direct_events[0].payload[field].clone()
}

fn rows(target: &Target, name: &str) -> Vec<BTreeMap<String, Node>> {
    target
        .query_view(SemanticViewRequest {
            view: name.parse::<ViewRef>().expect("a view name"),
            params: BTreeMap::new(),
            consistency: QueryConsistency::Current,
            correlation: correlation(),
            deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
        })
        .unwrap_or_else(|error| panic!("`{name}` reads: {error}"))
        .rows
}

fn observed(target: &Target, name: &str) -> Vec<BTreeMap<String, Node>> {
    target
        .observe_events(EventObservationRequest {
            event: name.parse::<EventRef>().expect("an event name"),
            correlation: correlation(),
            deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
        })
        .unwrap_or_else(|error| panic!("`{name}` is observed: {error}"))
        .into_iter()
        .map(|event| event.payload.into_iter().collect())
        .collect()
}

fn profile(target: &Target, kind: &str, egress: &str) -> Node {
    emitted(
        &run(
            target,
            "inference.policy.DeclareProfile",
            vec![
                ("kind", text(kind)),
                ("egress", text(egress)),
                ("admitted", list(&["local_worker"])),
            ],
        ),
        "profile_id",
    )
}

fn pipeline(target: &Target, profile_id: Node, name: &str) -> Node {
    emitted(
        &run(
            target,
            "inference.graph.DeclarePipeline",
            vec![
                ("name", text(name)),
                ("schema_revision", text("7")),
                ("mode", text("duplex")),
                ("profile_id", profile_id),
                ("required_capabilities", list(&["interruptible"])),
            ],
        ),
        "pipeline_id",
    )
}

fn node(target: &Target, pipeline_id: Node, name: &str, role: &str, requires: &[&str]) -> Node {
    emitted(
        &run(
            target,
            "inference.graph.AddNode",
            vec![
                ("pipeline_id", pipeline_id),
                ("name", text(name)),
                ("role", text(role)),
                ("requires", list(requires)),
            ],
        ),
        "node_id",
    )
}

fn port(target: &Target, node_id: Node, name: &str, direction: &str, flags: [bool; 3]) -> Node {
    emitted(
        &run(
            target,
            "inference.graph.AddPort",
            vec![
                ("node_id", node_id),
                ("name", text(name)),
                ("direction", text(direction)),
                ("modality", text("Audio")),
                ("essentiality", text("optional")),
                ("delivery", text("droppable")),
                ("incremental", Node::Bool(flags[0])),
                ("end_marker_required", Node::Bool(flags[1])),
                ("provisional_allowed", Node::Bool(flags[2])),
            ],
        ),
        "port_id",
    )
}

fn adapter(target: &Target) -> Node {
    let model_id = emitted(
        &run(
            target,
            "inference.model.DeclareModel",
            vec![
                ("family", text("f")),
                ("checkpoint", text("c")),
                ("code_license", text("l")),
                ("weight_license", text("w")),
                ("checksum", text("s")),
                ("input_modalities", list(&["Audio"])),
                ("output_modalities", list(&["Text"])),
                ("provenance", text("p")),
                ("source_revision", text("r")),
            ],
        ),
        "model_id",
    );
    emitted(
        &run(
            target,
            "inference.model.DeclareAdapter",
            vec![
                ("model_id", model_id),
                ("backend", text("b")),
                ("contract_revision", text("c")),
                ("execution_form", text("in_process")),
                ("locality", text("same_process")),
                ("capabilities", list(&["interruptible"])),
                ("max_concurrency", float(1.0)),
            ],
        ),
        "adapter_id",
    )
}

#[test]
fn each_port_flag_set_alone_is_the_one_the_ports_view_reports() {
    let target = begun();
    let profile_id = profile(&target, "offline_strict", "denied");
    let pipeline_id = pipeline(&target, profile_id, "flags");
    let node_id = node(&target, pipeline_id, "asr", "model", &[]);
    let one_hot = [
        [true, false, false],
        [false, true, false],
        [false, false, true],
    ];
    let ports: Vec<Node> = one_hot
        .iter()
        .enumerate()
        .map(|(index, flags)| {
            port(
                &target,
                node_id.clone(),
                &format!("p{index}"),
                "input",
                *flags,
            )
        })
        .collect();

    let rows = rows(&target, "inference.graph.Ports");
    assert_eq!(rows.len(), 3);
    for ((row, port_id), flags) in rows.iter().zip(&ports).zip(one_hot) {
        assert_eq!(&row["port_id"], port_id);
        assert_eq!(
            [
                row["incremental"].clone(),
                row["end_marker_required"].clone(),
                row["provisional_allowed"].clone(),
            ],
            flags.map(Node::Bool),
            "the flags sent, in the fields they were sent in"
        );
        assert_eq!(row["direction"], text("input"));
        assert_eq!(row["modality"], text("Audio"));
        assert_eq!(row["essentiality"], text("optional"));
        assert_eq!(row["delivery"], text("droppable"));
        assert_eq!(row["node_id"], node_id);
    }
}

#[test]
fn a_pipeline_row_names_its_profile_not_itself() {
    let target = begun();
    let first = profile(&target, "offline_strict", "denied");
    let second = profile(&target, "self_hosted_lan", "internal_allowlist");
    let pipeline_id = pipeline(&target, second.clone(), "voice");
    assert_ne!(first, second);
    assert_ne!(
        pipeline_id, second,
        "the arrangement keeps the identities apart"
    );

    let rows = rows(&target, "inference.graph.Pipelines");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["pipeline_id"], pipeline_id);
    assert_eq!(rows[0]["profile_id"], second);
    assert_eq!(rows[0]["name"], text("voice"));
    assert_eq!(rows[0]["schema_revision"], text("7"));
    assert_eq!(rows[0]["mode"], text("duplex"));
    assert_eq!(rows[0]["required_capabilities"], list(&["interruptible"]));
}

#[test]
fn node_rows_and_node_added_carry_their_own_pipeline_name_role_and_requirements() {
    let target = begun();
    let profile_id = profile(&target, "offline_strict", "denied");
    let first = pipeline(&target, profile_id.clone(), "first");
    let second = pipeline(&target, profile_id, "second");
    let source = node(&target, first.clone(), "mic", "source", &[]);
    let model = node(&target, second.clone(), "llm", "model", &["tool_calling"]);

    let rows = rows(&target, "inference.graph.Nodes");
    assert_eq!(rows.len(), 2);
    let expected = [
        (source, first, "mic", "source", list(&[])),
        (model, second, "llm", "model", list(&["tool_calling"])),
    ];
    for (row, (node_id, pipeline_id, name, role, requires)) in rows.iter().zip(&expected) {
        assert_eq!(&row["node_id"], node_id);
        assert_eq!(&row["pipeline_id"], pipeline_id);
        assert_eq!(row["name"], text(name));
        assert_eq!(row["role"], text(role));
        assert_eq!(&row["requires"], requires);
        assert_eq!(row.len(), 5);
    }

    let added = observed(&target, "inference.graph.NodeAdded");
    assert_eq!(added.len(), 2);
    for (event, (node_id, pipeline_id, name, ..)) in added.iter().zip(&expected) {
        assert_eq!(&event["node_id"], node_id);
        assert_eq!(&event["pipeline_id"], pipeline_id);
        assert_eq!(event["name"], text(name));
    }
}

#[test]
fn an_edge_row_and_ports_connected_run_from_the_source_to_the_sink() {
    let target = begun();
    let profile_id = profile(&target, "offline_strict", "denied");
    let pipeline_id = pipeline(&target, profile_id, "edge");
    let node_id = node(&target, pipeline_id.clone(), "vad", "transform", &[]);
    let source = port(
        &target,
        node_id.clone(),
        "out",
        "output",
        [true, true, true],
    );
    let sink = port(&target, node_id, "in", "input", [true, true, true]);
    let connected = run(
        &target,
        "inference.graph.ConnectPorts",
        vec![
            ("pipeline_id", pipeline_id),
            ("from_port", source.clone()),
            ("to_port", sink.clone()),
            ("capacity", float(3.0)),
            ("overload", text("supersede")),
        ],
    );
    let edge_id = emitted(&connected, "edge_id");
    assert_eq!(emitted(&connected, "from_port"), source);
    assert_eq!(emitted(&connected, "to_port"), sink);

    let rows = rows(&target, "inference.graph.Edges");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["edge_id"], edge_id);
    assert_eq!(rows[0]["from_port"], source);
    assert_eq!(rows[0]["to_port"], sink);
    assert_eq!(rows[0]["capacity"], Node::Number(Number::from(3_i64)));
    assert_eq!(rows[0]["overload"], text("supersede"));
    assert_eq!(rows[0].len(), 5);

    let observed = observed(&target, "inference.graph.PortsConnected");
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0]["edge_id"], edge_id);
    assert_eq!(observed[0]["from_port"], source);
    assert_eq!(observed[0]["to_port"], sink);
}

#[test]
fn a_binding_row_and_node_bound_name_the_node_and_the_adapter_apart() {
    let target = begun();
    let adapter_id = adapter(&target);
    let profile_id = profile(&target, "offline_strict", "denied");
    let pipeline_id = pipeline(&target, profile_id, "bound");
    let unbound = node(&target, pipeline_id.clone(), "mic", "source", &[]);
    let bound = node(&target, pipeline_id.clone(), "llm", "model", &[]);
    assert_eq!(unbound, adapter_id, "the per-domain counters meet here");
    assert_ne!(bound, adapter_id, "and not here, which is the node bound");

    let result = run(
        &target,
        "inference.graph.BindNode",
        vec![
            ("pipeline_id", pipeline_id),
            ("node_id", bound.clone()),
            ("adapter_id", adapter_id.clone()),
        ],
    );
    let binding_id = emitted(&result, "binding_id");
    assert_eq!(emitted(&result, "node_id"), bound);
    assert_eq!(emitted(&result, "adapter_id"), adapter_id);

    let rows = rows(&target, "inference.graph.Bindings");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["binding_id"], binding_id);
    assert_eq!(rows[0]["node_id"], bound);
    assert_eq!(rows[0]["adapter_id"], adapter_id);
    assert_eq!(rows[0].len(), 3);

    let observed = observed(&target, "inference.graph.NodeBound");
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0]["node_id"], bound);
    assert_eq!(observed[0]["adapter_id"], adapter_id);
}
