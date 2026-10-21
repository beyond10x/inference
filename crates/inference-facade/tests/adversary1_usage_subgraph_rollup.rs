//! Adversary pass 1 on `story:usage-subgraph-rollup`, at the session: a child record is counted
//! once across a switch between two runs of a parent with a subgraph node, and the session's
//! cancel during a turn of the child's model keeps that turn's record in the session's total.
//!
//! The parent is the one of `usage_subgraph.rs`: a subgraph node over the A-02 text pipeline,
//! whose model's `usage` port the child exposes as `model usage`, and whose own `usage` port the
//! parent exposes as `usage`.

#![allow(dead_code)]

mod text;

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use inference_core::id::{GenerationId, RunId};
use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_core::usage::{UsageRecorded, UsageTotal};
use inference_facade::events::SessionEvent;
use inference_facade::live::{LiveSession, LiveSessions};
use inference_llm::llm_core::Usage;
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortId, PortTraits, USAGE_PORT,
    modality,
};
use inference_runtime::{SubgraphHandle, SubgraphNode};
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{MockGenerator, ModelNode};

use text::{Application, EGRESS, INGRESS, block_on, generator, reply};

const PATIENCE: Duration = Duration::from_secs(20);

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

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

const RECORDS: PortTraits = PortTraits {
    incremental: false,
    ..TEXT
};

fn spec(name: &str) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    }
}

fn capacity() -> NonZeroUsize {
    NonZeroUsize::new(16).expect("16")
}

struct Planned {
    declaration: Declaration,
    child: PlannedPipeline,
    parent: PlannedPipeline,
    usage: PortId,
}

fn planned(generator: &MockGenerator) -> Planned {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let adapter_id = generator.adapter().adapter_id();
    let catalog = Fixture {
        profile,
        adapter: generator.adapter().clone(),
    };
    let mut declaration = Declaration::new();
    let (child_id, usage) = declaration
        .build(&catalog, profile_id, spec("text session"), |pipeline| {
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
            let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
            let usage = pipeline.output::<modality::Structured>(&model, USAGE_PORT, RECORDS);
            pipeline.expose(&prompt, INGRESS, capacity(), OverloadPolicy::Pause);
            pipeline.expose(&answer, EGRESS, capacity(), OverloadPolicy::Pause);
            pipeline.expose(&usage, "model usage", capacity(), OverloadPolicy::Pause);
            pipeline.bind(&model, adapter_id).expect("held");
            usage.id()
        })
        .expect("the profile is declared");
    let validated = declaration.validate(child_id).expect("valid");
    let child = declaration.plan(validated, &catalog).expect("planned");
    let (parent_id, ()) = declaration
        .build(&catalog, profile_id, spec("subgraph session"), |pipeline| {
            let subgraph = pipeline
                .subgraph("frontend", NodeRole::Transform, child_id)
                .expect("the child is Planned");
            let into = pipeline.input::<modality::Text>(&subgraph, INGRESS, TEXT);
            let out = pipeline.output::<modality::Text>(&subgraph, EGRESS, TEXT);
            let usage = pipeline.output::<modality::Structured>(&subgraph, USAGE_PORT, RECORDS);
            pipeline.expose(&into, INGRESS, capacity(), OverloadPolicy::Pause);
            pipeline.expose(&out, EGRESS, capacity(), OverloadPolicy::Pause);
            pipeline.expose(&usage, USAGE_PORT, capacity(), OverloadPolicy::Pause);
        })
        .expect("the profile is declared");
    let validated = declaration.validate(parent_id).expect("valid");
    let parent = declaration.plan(validated, &catalog).expect("planned");
    Planned {
        declaration,
        child,
        parent,
        usage,
    }
}

/// The parent's subgraph node, its child's model answering on `generator`; each node's handle
/// is kept in `handles`.
fn implement(
    planned: &Planned,
    generator: &MockGenerator,
    handles: &Arc<Mutex<Vec<SubgraphHandle>>>,
) -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<PipelinePayload>> + use<> {
    let declaration = planned.declaration.clone();
    let child = planned.child.clone();
    let usage = planned.usage;
    let generator = generator.clone();
    let handles = Arc::clone(handles);
    move |run_id: RunId, _: &Node| {
        let generator = generator.clone();
        let node = SubgraphNode::new(
            run_id,
            &declaration,
            &child,
            HashMap::<GenerationId, i64>::new(),
            Box::new(move |child_run: RunId, _: &Node| {
                Box::new(
                    ModelNode::new(child_run, generator.clone(), reply, RunLog::new())
                        .with_usage(usage),
                ) as Box<dyn NodeImplementation<PipelinePayload>>
            }),
        );
        handles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(node.handle());
        Box::new(node) as Box<dyn NodeImplementation<PipelinePayload>>
    }
}

fn child_runs(handles: &Arc<Mutex<Vec<SubgraphHandle>>>) -> Vec<RunId> {
    handles
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter_map(SubgraphHandle::child_run_id)
        .collect()
}

fn records(events: &[SessionEvent]) -> Vec<&UsageRecorded> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) => Some(recorded),
            _ => None,
        })
        .collect()
}

fn total_of<'a>(records: impl IntoIterator<Item = &'a UsageRecorded>) -> UsageTotal {
    let mut total = UsageTotal::new();
    for recorded in records {
        total.add(&recorded.quantities, recorded.r#final);
    }
    total
}

/// Sends `text`, commits it and receives until `records` usage records arrived in all.
fn turn(
    session: &mut LiveSession<PipelinePayload>,
    application: &mut Application,
    text: &str,
    records_in_all: usize,
    received: &mut Vec<SessionEvent>,
) {
    block_on(session.send(application.text(text))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    while records(received).len() < records_in_all {
        received.push(block_on(session.receive()).expect("the turn's record arrives"));
    }
}

/// Runs `case` on a thread of its own, failing once it took longer than [`PATIENCE`].
fn bounded(case: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let runner = thread::spawn(move || {
        case();
        let _ = done.send(());
    });
    match finished.recv_timeout(PATIENCE) {
        Ok(()) => runner.join().expect("the case does not panic"),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            if let Err(panic) = runner.join() {
                std::panic::resume_unwind(panic);
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("the case did not end within {PATIENCE:?}"),
    }
}

/// Two runs of the parent, one after a switch, each answering one turn of its child's model:
/// each run's total counts its own child's record once, the session's counts both once, and no
/// child run has a total of its own.
#[test]
fn a_switch_between_two_parents_counts_each_childs_record_once() {
    bounded(|| {
        let generator = generator(Vec::<String>::new()).reporting([Some(Usage {
            input_tokens: Some(7),
            output_tokens: Some(2),
            ..Usage::default()
        })]);
        let planned = planned(&generator);
        let handles: Arc<Mutex<Vec<SubgraphHandle>>> = Arc::default();
        let sessions = LiveSessions::new();
        let (mut session, driver) = block_on(sessions.open(
            &planned.declaration,
            &planned.parent,
            InteractionMode::TurnBased,
            implement(&planned, &generator, &handles),
        ))
        .expect("opens");
        let first = session.run_id();
        let first_drive = thread::spawn(move || block_on(driver.drive()));
        let mut application = Application::new(first);
        let mut received = Vec::new();
        turn(&mut session, &mut application, "1", 1, &mut received);

        let driver = block_on(session.switch(
            &planned.declaration,
            &planned.parent,
            InteractionMode::TurnBased,
            implement(&planned, &generator, &handles),
        ))
        .expect("switches");
        let second = session.run_id();
        first_drive.join().expect("no panic").expect("ends");
        let second_drive = thread::spawn(move || block_on(driver.drive()));
        turn(&mut session, &mut application, "2", 2, &mut received);
        block_on(session.close()).expect("drains");
        while let Some(event) = block_on(session.receive()) {
            received.push(event);
        }
        second_drive.join().expect("no panic").expect("ends");

        let delivered = records(&received);
        let [one, two] = delivered.as_slice() else {
            panic!("one record per run, each once: {received:#?}")
        };
        let children = child_runs(&handles);
        assert_eq!(children.len(), 2);
        assert_eq!(one.run_id, Some(children[0]));
        assert_eq!(two.run_id, Some(children[1]));
        let usage = session.usage();
        assert_eq!(
            usage.runs,
            [(first, total_of([*one])), (second, total_of([*two])),]
        );
        assert_eq!(usage.session, total_of(delivered.iter().copied()));
        assert_eq!(usage.session.records(), 2);
        for child in children {
            assert_eq!(usage.run(child), None);
        }
    });
}

fn holding() -> MockGenerator {
    generator(Vec::<String>::new())
        .reporting([Some(Usage {
            input_tokens: Some(20),
            output_tokens: Some(1),
            ..Usage::default()
        })])
        .holding(0)
}

/// Sends one text to the open `session`, waits until the held turn streams, cancels the session
/// and receives until it ends; answers the usage records it counted.
fn cancelled_mid_turn(
    mut session: LiveSession<PipelinePayload>,
    drive: thread::JoinHandle<Result<inference_runtime::run::Run, inference_runtime::RunError>>,
) -> u64 {
    let mut application = Application::new(session.run_id());
    block_on(session.send(application.text("1"))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    let mut received = Vec::new();
    while !received
        .iter()
        .any(|event| matches!(event, SessionEvent::TextDelta(_)))
    {
        received.push(block_on(session.receive()).expect("the held turn streams"));
    }
    session.cancel().expect("an open session cancels");
    while let Some(event) = block_on(session.receive()) {
        received.push(event);
    }
    let _ended = drive.join().expect("no panic");
    session.usage().session.records()
}

/// Baseline: the model in the session's own run. Its cut turn's record counts.
#[test]
fn a_session_cancel_during_a_held_turn_of_its_own_model_counts_the_cut_turns_record() {
    bounded(|| {
        let generator = holding();
        let (declaration, planned, usage) = text::planned_with_usage(&generator);
        let answering = generator.clone();
        let sessions = LiveSessions::new();
        let (session, driver) = block_on(sessions.open(
            &declaration,
            &planned,
            InteractionMode::TurnBased,
            move |run_id: RunId, _: &Node| {
                Box::new(
                    ModelNode::new(run_id, answering.clone(), reply, RunLog::new())
                        .with_usage(usage),
                ) as Box<dyn NodeImplementation<PipelinePayload>>
            },
        ))
        .expect("opens");
        let drive = thread::spawn(move || block_on(driver.drive()));
        assert_eq!(cancelled_mid_turn(session, drive), 1);
    });
}

/// The session's cancel during a turn its child's model holds: the model node sends the cut
/// turn's record at its cancel (`story:llm-usage-on-cancel`), and a cancelled session still
/// counts what it reads (`crate::usage`), as it does when the model sits in the session's run.
/// The roll-up must bring that record to the parent so that the session's total counts it.
#[test]
fn a_session_cancel_during_a_childs_held_turn_still_counts_the_cut_turns_record() {
    bounded(|| {
        let generator = holding();
        let planned = planned(&generator);
        let handles: Arc<Mutex<Vec<SubgraphHandle>>> = Arc::default();
        let sessions = LiveSessions::new();
        let (session, driver) = block_on(sessions.open(
            &planned.declaration,
            &planned.parent,
            InteractionMode::TurnBased,
            implement(&planned, &generator, &handles),
        ))
        .expect("opens");
        let drive = thread::spawn(move || block_on(driver.drive()));
        assert_eq!(
            cancelled_mid_turn(session, drive),
            1,
            "the cut turn's record of the child counts in the session's total"
        );
    });
}
