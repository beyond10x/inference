//! Adversary pass 1 on `story:node-instance-lifecycle`: the node instance lifecycle of
//! `ess/domains/graph.yaml` (`inference.graph.NodeInstance.lifecycle`), every state against every
//! command, read as written there. The synthesized suite proves one source state per transition
//! (cancel from Described and Started, fail from Described and Started, stop from Prepared and
//! Cancelling), so a transition that lost another source state passes conform; this table does
//! not let it.

use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, NodeRole, PipelineSpec, PlannedPipeline,
};
use inference_runtime::NodeId;
use inference_runtime::run::{
    NodeInstance, NodeInstanceId, NodeInstanceState, NodeInstanceStateConflict, Run,
};

use NodeInstanceState::{Cancelling, Described, Draining, Failed, Prepared, Started, Stopped};

struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

fn planned() -> (PlannedPipeline, NodeId) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Profiles(profile);
    let mut declaration = Declaration::new();
    let spec = PipelineSpec {
        name: "table".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let (pipeline_id, node_id) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            pipeline.node("llm", NodeRole::Model, Vec::new()).id()
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("a lone node is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("nothing is required of the lone node");
    (planned, node_id)
}

/// A Described instance of the lone node in a Preparing run.
fn described() -> NodeInstance {
    let (planned, node_id) = planned();
    let (run, _) = Run::create(&planned);
    let run = run.validate().expect("Created validates").0;
    let run = run.prepare().expect("Validating prepares").0;
    NodeInstance::describe(&run, node_id)
        .expect("a Preparing run describes")
        .0
}

/// An instance in `state`, reached by the transitions of the lifecycle only.
fn instance_in(state: NodeInstanceState) -> NodeInstance {
    let described = described();
    let reached = match state {
        Described => Ok(described),
        Prepared => described.prepare().map(|m| m.0),
        Started => described.prepare().and_then(|m| m.0.start()).map(|m| m.0),
        Draining => described
            .prepare()
            .and_then(|m| m.0.start())
            .and_then(|m| m.0.drain())
            .map(|m| m.0),
        Cancelling => described.cancel().map(|m| m.0),
        Stopped => described.cancel().and_then(|m| m.0.stop()).map(|m| m.0),
        Failed => described.fail("reason").map(|m| m.0),
    }
    .expect("the lifecycle reaches every state");
    assert_eq!(reached.state(), state);
    reached
}

/// The six commands, by the transition they run; each answers the moved instance and the
/// `instance_id` its event carries.
const COMMANDS: [&str; 6] = ["prepare", "start", "drain", "cancel", "stop", "fail"];

fn run_command(
    instance: &NodeInstance,
    command: &str,
) -> Result<(NodeInstance, NodeInstanceId), NodeInstanceStateConflict> {
    match command {
        "prepare" => instance.prepare().map(|(i, e)| (i, e.instance_id)),
        "start" => instance.start().map(|(i, e)| (i, e.instance_id)),
        "drain" => instance.drain().map(|(i, e)| (i, e.instance_id)),
        "cancel" => instance.cancel().map(|(i, e)| (i, e.instance_id)),
        "stop" => instance.stop().map(|(i, e)| (i, e.instance_id)),
        "fail" => instance.fail("reason").map(|(i, e)| (i, e.instance_id)),
        other => unreachable!("no command {other}"),
    }
}

/// `inference.graph.NodeInstance.lifecycle.transitions`, copied from `ess/domains/graph.yaml`.
fn declared(command: &str) -> (&'static [NodeInstanceState], NodeInstanceState) {
    match command {
        "prepare" => (&[Described], Prepared),
        "start" => (&[Prepared], Started),
        "drain" => (&[Started], Draining),
        "cancel" => (&[Described, Prepared, Started, Draining], Cancelling),
        "stop" => (&[Prepared, Draining, Cancelling], Stopped),
        "fail" => (&[Described, Prepared, Started, Draining], Failed),
        other => unreachable!("no command {other}"),
    }
}

#[test]
fn every_state_answers_every_command_as_graph_yaml_declares() {
    let mut wrong = Vec::new();
    for state in NodeInstanceState::ALL {
        for command in COMMANDS {
            let instance = instance_in(state);
            let (from, to) = declared(command);
            match (from.contains(&state), run_command(&instance, command)) {
                (true, Ok((moved, event_instance))) => {
                    if moved.state() != to
                        || event_instance != instance.instance_id()
                        || moved.instance_id() != instance.instance_id()
                        || moved.run_id() != instance.run_id()
                        || moved.node_id() != instance.node_id()
                    {
                        wrong.push(format!("{state:?} --{command}--> {:?}", moved.state()));
                    }
                }
                (true, Err(conflict)) => {
                    wrong.push(format!("{state:?} refuses {command}: {conflict}"));
                }
                (false, Ok((moved, _))) => {
                    wrong.push(format!("{state:?} --{command}--> {:?}", moved.state()));
                }
                (false, Err(conflict)) => {
                    if conflict.current != state || instance.state() != state {
                        wrong.push(format!("{state:?} refuses {command} as {conflict:?}"));
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "against graph.yaml: {wrong:#?}");
}

#[test]
fn the_terminal_states_are_stopped_and_failed_only() {
    let terminal: Vec<_> = NodeInstanceState::ALL
        .into_iter()
        .filter(|state| state.is_terminal())
        .collect();
    assert_eq!(terminal, vec![Stopped, Failed]);
    for state in NodeInstanceState::ALL {
        let leaves = COMMANDS
            .into_iter()
            .any(|command| run_command(&instance_in(state), command).is_ok());
        assert_eq!(leaves, !state.is_terminal(), "{state:?}");
    }
}

#[test]
fn the_states_and_the_conflict_serialize_by_their_ess_names() {
    let names: Vec<String> = NodeInstanceState::ALL
        .iter()
        .map(|state| serde_json::to_string(state).expect("a unit variant"))
        .collect();
    assert_eq!(
        names,
        [
            "\"Described\"",
            "\"Prepared\"",
            "\"Started\"",
            "\"Draining\"",
            "\"Cancelling\"",
            "\"Stopped\"",
            "\"Failed\"",
        ]
    );
    assert_eq!(
        serde_json::to_value(NodeInstanceStateConflict {
            current: Cancelling
        })
        .expect("a struct"),
        serde_json::json!({"current": "Cancelling"})
    );
    let (_, failed) = described()
        .fail("the device is gone")
        .expect("Described fails");
    let failed = serde_json::to_value(&failed).expect("a struct");
    assert_eq!(failed["reason"], "the device is gone");
    assert_eq!(
        failed.as_object().map(|o| o.len()),
        Some(2),
        "NodeInstanceFailed has instance_id and reason: {failed}"
    );
}
