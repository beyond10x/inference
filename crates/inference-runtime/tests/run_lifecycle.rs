//! A run of a planned pipeline moves through its states by command and refuses the rest
//! (`story:run-lifecycle`, step 5): a run built from the Planned form reaches Completed through
//! the six states of the design, `CancelRun` from each non-terminal state reaches Cancelled, and
//! every command a state does not leave by is refused with the state the run holds, moving
//! nothing.

use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, NodeRole, PipelineSpec, PlannedPipeline,
};
use inference_runtime::run::{
    AdmissionRefusal, FailureReason, NodeInstance, NodeInstanceState, Run, RunActivated,
    RunCancelled, RunCompleted, RunCreated, RunDraining, RunFailed, RunNotRunning, RunPreparing,
    RunState, RunStateConflict, RunValidating,
};
use inference_runtime::{NodeId, RunId};

/// The one profile the case declares, and no adapter: what `Declaration::build` reads.
struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

/// A one-node pipeline declared, validated and planned through `inference-pipeline`: the Planned
/// form a run is built from, and the node.
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
        name: "run".to_owned(),
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

/// The run of `planned`, driven by command to `state` along the path the design gives it.
fn driven_to(planned: &PlannedPipeline, state: RunState) -> Run {
    let (mut run, _) = Run::create(planned);
    let path = [
        RunState::Validating,
        RunState::Preparing,
        RunState::Active,
        RunState::Draining,
    ];
    for next in path {
        if run.state() == state {
            break;
        }
        run = match next {
            RunState::Validating => run.validate().map(|(run, _)| run),
            RunState::Preparing => run.prepare().map(|(run, _)| run),
            RunState::Active => run.activate().map(|(run, _)| run),
            _ => run.drain().map(|(run, _)| run),
        }
        .unwrap_or_else(|conflict| panic!("{next:?} is reached: {conflict}"));
    }
    assert_eq!(run.state(), state);
    run
}

#[test]
fn a_run_built_from_the_planned_form_reaches_completed_through_the_six_states() {
    let (planned, _) = planned();
    let (created, event) = Run::create(&planned);
    let run_id = created.run_id();
    assert_eq!(
        event,
        RunCreated {
            run_id,
            pipeline_id: planned.pipeline_id()
        }
    );
    assert_eq!(created.state(), RunState::Created);
    assert_eq!(created.pipeline_id(), planned.pipeline_id());
    assert_eq!(created.failure(), None);

    let (validating, event) = created.validate().expect("Created validates");
    assert_eq!(event, RunValidating { run_id });
    assert_eq!(validating.state(), RunState::Validating);
    let (preparing, event) = validating.prepare().expect("Validating prepares");
    assert_eq!(event, RunPreparing { run_id });
    assert_eq!(preparing.state(), RunState::Preparing);
    let (active, event) = preparing.activate().expect("Preparing activates");
    assert_eq!(event, RunActivated { run_id });
    assert_eq!(active.state(), RunState::Active);
    let (draining, event) = active.drain().expect("Active drains");
    assert_eq!(event, RunDraining { run_id });
    assert_eq!(draining.state(), RunState::Draining);
    let (completed, event) = draining.complete().expect("Draining completes");
    assert_eq!(event, RunCompleted { run_id });
    assert_eq!(completed.state(), RunState::Completed);
    assert_eq!(completed.run_id(), run_id);
    assert_eq!(completed.failure(), None);
}

#[test]
fn cancel_run_from_each_non_terminal_state_reaches_cancelled() {
    let (planned, _) = planned();
    let non_terminal = [
        RunState::Created,
        RunState::Validating,
        RunState::Preparing,
        RunState::Active,
        RunState::Draining,
    ];
    for state in non_terminal {
        let run = driven_to(&planned, state);
        let (cancelled, event) = run
            .cancel()
            .unwrap_or_else(|conflict| panic!("{state:?} cancels: {conflict}"));
        assert_eq!(
            event,
            RunCancelled {
                run_id: run.run_id()
            }
        );
        assert_eq!(cancelled.state(), RunState::Cancelled, "from {state:?}");
        assert_eq!(cancelled.failure(), None);
    }
}

#[test]
fn fail_run_from_each_state_work_runs_in_records_the_failure() {
    let (planned, node_id) = planned();
    let failure = FailureReason {
        node_id: Some(node_id),
        detail: "worker lost".to_owned(),
    };
    for state in [
        RunState::Validating,
        RunState::Preparing,
        RunState::Active,
        RunState::Draining,
    ] {
        let run = driven_to(&planned, state);
        let (failed, event) = run
            .fail(failure.clone())
            .unwrap_or_else(|conflict| panic!("{state:?} fails: {conflict}"));
        assert_eq!(
            event,
            RunFailed {
                run_id: run.run_id(),
                failure: failure.clone()
            }
        );
        assert_eq!(failed.state(), RunState::Failed);
        assert_eq!(failed.failure(), Some(&failure));
    }
}

/// The states each command leaves by, as `ess/domains/graph.yaml` declares the transitions.
fn leaves(command: &str) -> &'static [RunState] {
    match command {
        "validate" => &[RunState::Created],
        "prepare" => &[RunState::Validating],
        "activate" => &[RunState::Preparing],
        "drain" => &[RunState::Active],
        "complete" => &[RunState::Draining],
        "fail" => &[
            RunState::Validating,
            RunState::Preparing,
            RunState::Active,
            RunState::Draining,
        ],
        _ => &[
            RunState::Created,
            RunState::Validating,
            RunState::Preparing,
            RunState::Active,
            RunState::Draining,
        ],
    }
}

/// `command` run on `run`: the moved state, or the conflict.
fn issue(run: &Run, command: &str) -> Result<RunState, RunStateConflict> {
    let failure = FailureReason {
        node_id: None,
        detail: "d".to_owned(),
    };
    match command {
        "validate" => run.validate().map(|(run, _)| run.state()),
        "prepare" => run.prepare().map(|(run, _)| run.state()),
        "activate" => run.activate().map(|(run, _)| run.state()),
        "drain" => run.drain().map(|(run, _)| run.state()),
        "complete" => run.complete().map(|(run, _)| run.state()),
        "fail" => run.fail(failure).map(|(run, _)| run.state()),
        _ => run.cancel().map(|(run, _)| run.state()),
    }
}

/// A run in each of the eight states.
fn every_state(planned: &PlannedPipeline) -> Vec<Run> {
    let mut runs: Vec<Run> = [
        RunState::Created,
        RunState::Validating,
        RunState::Preparing,
        RunState::Active,
        RunState::Draining,
    ]
    .into_iter()
    .map(|state| driven_to(planned, state))
    .collect();
    let draining = driven_to(planned, RunState::Draining);
    runs.push(draining.complete().expect("completes").0);
    let failure = FailureReason {
        node_id: None,
        detail: "d".to_owned(),
    };
    runs.push(draining.fail(failure).expect("fails").0);
    runs.push(draining.cancel().expect("cancels").0);
    runs
}

#[test]
fn every_command_from_a_state_its_transition_does_not_leave_is_refused_with_that_state() {
    let (planned, _) = planned();
    let runs = every_state(&planned);
    assert_eq!(
        runs.iter().map(Run::state).collect::<Vec<_>>(),
        RunState::ALL.to_vec()
    );
    let mut refused = 0;
    for run in &runs {
        for command in [
            "validate", "prepare", "activate", "drain", "complete", "fail", "cancel",
        ] {
            let answer = issue(run, command);
            if leaves(command).contains(&run.state()) {
                assert!(
                    answer.is_ok(),
                    "{command} from {:?}: {answer:?}",
                    run.state()
                );
            } else {
                refused += 1;
                assert_eq!(
                    answer,
                    Err(RunStateConflict {
                        current: run.state()
                    }),
                    "{command} from {:?}",
                    run.state()
                );
            }
        }
    }
    assert_eq!(
        refused,
        7 * 8 - 14,
        "the transition table has 14 sources, and graph.yaml 42 state refusals"
    );
}

#[test]
fn a_node_instance_is_described_only_while_the_run_prepares_or_runs() {
    let (planned, node_id) = planned();
    for run in every_state(&planned) {
        let described = NodeInstance::describe(&run, node_id);
        if matches!(run.state(), RunState::Preparing | RunState::Active) {
            let (instance, event) = described.expect("a running run describes");
            assert_eq!(instance.run_id(), run.run_id());
            assert_eq!(instance.node_id(), node_id);
            assert_eq!(instance.state(), NodeInstanceState::Described);
            assert_eq!(event.instance_id, instance.instance_id());
            assert_eq!(event.run_id, run.run_id());
            assert_eq!(event.node_id, node_id);
        } else {
            assert_eq!(
                described,
                Err(RunNotRunning {
                    run_id: run.run_id()
                }),
                "{:?}",
                run.state()
            );
        }
    }
}

#[test]
fn the_wire_names_are_the_ess_names() {
    let names: Vec<String> = RunState::ALL
        .iter()
        .map(|state| serde_json::to_string(state).expect("serializes"))
        .collect();
    assert_eq!(
        names,
        [
            "\"Created\"",
            "\"Validating\"",
            "\"Preparing\"",
            "\"Active\"",
            "\"Draining\"",
            "\"Completed\"",
            "\"Failed\"",
            "\"Cancelled\""
        ]
    );
    let refusals: Vec<String> = AdmissionRefusal::ALL
        .iter()
        .map(|reason| serde_json::to_string(reason).expect("serializes"))
        .collect();
    assert_eq!(
        refusals,
        [
            "\"not_enough_vram\"",
            "\"backend_busy\"",
            "\"deadline_unachievable\""
        ]
    );
    assert_eq!(
        serde_json::to_string(&NodeInstanceState::Described).expect("serializes"),
        "\"Described\""
    );
    let run_id = RunId::generate();
    assert_eq!(
        serde_json::to_value(RunStateConflict {
            current: RunState::Created
        })
        .expect("serializes"),
        serde_json::json!({"current": "Created"})
    );
    assert_eq!(
        serde_json::to_value(RunNotRunning { run_id }).expect("serializes"),
        serde_json::json!({"run_id": run_id.to_string()})
    );
}
