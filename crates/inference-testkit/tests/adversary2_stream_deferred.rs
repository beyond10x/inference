//! Adversary pass 1 against the `inference.stream` target module: the 58 generation and segment
//! scenarios of the synthesized suite whose arrangement creates a Run, run against the module
//! itself apart from the run store.
//!
//! A stand-in answers the arranging `inference.policy` and `inference.graph` commands with the
//! outcome and identity each scenario expects, and `inference.stream.StartGeneration` with the
//! store's own `start_generation`; every other `inference.stream` command, view and event goes to
//! the module under test, unchanged. The stand-in lives in this test only: it fakes nothing the
//! target ships. A scenario that does not pass here is a step the stream module answers wrongly,
//! whatever the run store answers.
//!
//! `target/stream.rs` is loaded a second time, as a module of this file, so that the stand-in can
//! reach the module's functions, which `target/mod.rs` keeps private.

#![allow(clippy::duplicate_mod)]

mod target;

#[allow(dead_code)]
#[path = "target/stream.rs"]
mod stream;

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use ess_conformance::scenario::{CommandRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    ConformanceTarget, EventObservationRequest, ExternalOutcomeControl, ImplementationIdentity,
    ObservedEvent, RedeliveryRequest, ScenarioContext, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError,
};
use ess_conformance::{AdmittedSuite, Runner, Status};
use ess_primitives::consistency::ConsistencyToken;
use ess_primitives::node::Node;
use inference_core::generation::StartGeneration;
use serde_json::{Value, json};
use uuid::Uuid;

use target::{Input, Stores, integer_node, named_node, text_node, unreadable, unsupported};

/// The stand-in target: the module under test behind a stub of the arranging commands.
#[derive(Default)]
struct Harness {
    stores: RefCell<Stores>,
    stubbed: RefCell<u128>,
}

impl Harness {
    fn token(stores: &Stores) -> ConsistencyToken {
        ConsistencyToken::new(format!("seq:{}", stores.stream.events().len())).expect("a token")
    }

    /// An identity no store issues, for the stubbed arranging commands.
    fn stub_identity(&self) -> Node {
        let mut next = self.stubbed.borrow_mut();
        *next += 1;
        text_node(&Uuid::from_u128(0xabcd_0000_0000_4000_8000_0000_0000_0000 | *next).to_string())
    }

    fn stubbed(
        &self,
        request: &SemanticCommandRequest,
        stores: &Stores,
    ) -> Result<SemanticCommandResult, TargetError> {
        let name = request.command.to_string();
        let (branch, event) = match name.as_str() {
            "inference.policy.DeclareProfile" => (
                "declared",
                Some(("inference.policy.ProfileDeclared", "profile_id")),
            ),
            "inference.graph.DeclarePipeline" => (
                "declared",
                Some(("inference.graph.PipelineDeclared", "pipeline_id")),
            ),
            "inference.graph.ValidatePipeline" => ("validated", None),
            "inference.graph.PlanPipeline" => ("planned", None),
            "inference.graph.AddNode" => ("added", Some(("inference.graph.NodeAdded", "node_id"))),
            "inference.graph.CreateRun" => {
                ("created", Some(("inference.graph.RunCreated", "run_id")))
            }
            "inference.graph.ValidateRun" => ("validating", None),
            "inference.graph.PrepareRun" => ("preparing", None),
            "inference.graph.ActivateRun" => ("active", None),
            _ => return Err(unsupported(&name)),
        };
        let mut result = SemanticCommandResult::took(OutcomeRef::new(
            name.parse::<CommandRef>().expect("a command"),
            branch.parse().expect("an outcome"),
        ));
        if let Some((event, field)) = event {
            result = result.emitting(
                ObservedEvent::new(event.parse::<EventRef>().expect("an event"))
                    .with(field, self.stub_identity())
                    .in_activity(request.correlation.clone()),
            );
        }
        Ok(result.with_consistency(Self::token(stores)))
    }
}

impl ConformanceTarget for Harness {
    fn identity(&self) -> Result<ImplementationIdentity, TargetError> {
        Ok(ImplementationIdentity::new("adversary2-stream", "0"))
    }

    fn begin_scenario(&self, _scenario: &ScenarioContext) -> Result<(), TargetError> {
        *self.stores.borrow_mut() = Stores::default();
        Ok(())
    }

    fn execute_command(
        &self,
        request: SemanticCommandRequest,
    ) -> Result<SemanticCommandResult, TargetError> {
        let stores = &mut *self.stores.borrow_mut();
        let name = request.command.to_string();
        if name == "inference.stream.StartGeneration" {
            let mut input = Input::of(&request);
            let start = StartGeneration {
                run_id: input.named("run_id")?,
                node_id: input.named("node_id")?,
                epoch: input.integer("epoch")?,
            };
            input.finish()?;
            let event = stores.stream.start_generation(start);
            let observed = ObservedEvent::new(
                "inference.stream.GenerationStarted"
                    .parse::<EventRef>()
                    .expect("an event"),
            )
            .with("generation_id", named_node(&event.generation_id)?)
            .with("run_id", named_node(&event.run_id)?)
            .with("epoch", integer_node(event.epoch))
            .in_activity(request.correlation.clone());
            return Ok(SemanticCommandResult::took(OutcomeRef::new(
                name.parse::<CommandRef>().expect("a command"),
                "started".parse().expect("an outcome"),
            ))
            .emitting(observed)
            .with_consistency(Self::token(stores)));
        }
        if name.starts_with("inference.stream.") {
            return stream::execute_command(stores, &request);
        }
        self.stubbed(&request, stores)
    }

    fn query_view(&self, request: SemanticViewRequest) -> Result<SemanticViewResult, TargetError> {
        let stores = &mut *self.stores.borrow_mut();
        if request.view.to_string().starts_with("inference.stream.") {
            return stream::query_view(stores, &request);
        }
        Err(unreadable("a view", "the harness reads stream views only"))
    }

    fn observe_events(
        &self,
        request: EventObservationRequest,
    ) -> Result<Vec<ObservedEvent>, TargetError> {
        let stores = &mut *self.stores.borrow_mut();
        if request.event.to_string().starts_with("inference.stream.") {
            return stream::observe_events(stores, &request);
        }
        Err(unsupported(&request.event.to_string()))
    }

    fn configure_external_outcome(
        &self,
        request: ExternalOutcomeControl,
    ) -> Result<(), TargetError> {
        Err(unsupported(&request.force.to_string()))
    }

    fn redeliver_event(&self, request: RedeliveryRequest) -> Result<(), TargetError> {
        Err(unsupported(&request.event.to_string()))
    }

    fn end_scenario(&self, _scenario: &ScenarioContext) -> Result<(), TargetError> {
        *self.stores.borrow_mut() = Stores::default();
        Ok(())
    }
}

/// The scenarios of the story whose only missing piece is a Run: every generation and segment
/// scenario but `StartGeneration`'s own, whose refusals read the run and node stores.
fn deferred(id: &str) -> bool {
    const COMMANDS: [&str; 9] = [
        "CompleteGeneration",
        "SupersedeGeneration",
        "CancelGeneration",
        "EmitTextDelta",
        "CommitSegment",
        "QueueForSynthesis",
        "CompleteSynthesis",
        "AcknowledgePlayback",
        "DiscardSegment",
    ];
    id.starts_with("inference.stream.Generation/")
        || id.starts_with("inference.stream.OutputSegment/")
        || COMMANDS
            .iter()
            .any(|command| id.starts_with(&format!("inference.stream.{command}/")))
        || id == "inference.stream/authored/supersede-discards-unplayed-segments"
        || id == "inference.stream/authored/cancel-generation-discards-unplayed-segments"
}

fn synthesized_suite() -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves");
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_millis();
    let dir: PathBuf = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("adversary2-stream-{millis}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("the directory is created");
    let suite = dir.join("suite.json");
    let output = Command::new("ess")
        .args([
            "verify",
            "conform",
            "synthesize",
            "--path",
            "ess",
            "--scenarios",
            "ess/scenarios",
            "--out",
            suite.to_str().expect("UTF-8"),
        ])
        .current_dir(&root)
        .output()
        .expect("`ess` runs");
    assert!(output.status.success(), "ess synthesize: {output:?}");
    serde_json::from_str(&fs::read_to_string(&suite).expect("suite.json")).expect("JSON")
}

#[test]
fn every_deferred_generation_and_segment_scenario_passes_against_the_stream_module() {
    let mut suite = synthesized_suite();
    let kept: serde_json::Map<String, Value> = suite["scenarios"]
        .as_object()
        .expect("scenarios")
        .iter()
        .filter(|(id, _)| deferred(id))
        .map(|(id, scenario)| (id.clone(), scenario.clone()))
        .collect();
    assert_eq!(
        kept.len(),
        60,
        "the 62 ids of the story less StartGeneration's 4, and the cancelled and superseded outcomes ess 0.69.0 synthesizes"
    );
    suite["scenarios"] = json!(kept);
    let admitted = AdmittedSuite::from_json(&suite.to_string())
        .unwrap_or_else(|error| panic!("the filtered suite is admitted: {error:?}"));
    let run = Runner::for_suite(admitted.suite()).run_admitted(&admitted, &Harness::default());

    let not_passed: Vec<String> = run
        .scenarios
        .iter()
        .filter(|scenario| scenario.status != Status::Passed)
        .map(|scenario| {
            let diagnostics: Vec<String> =
                scenario.diagnostics().map(ToString::to_string).collect();
            format!(
                "{} [{}]: {}",
                scenario.scenario,
                scenario.status.as_str(),
                diagnostics.join(" | ")
            )
        })
        .collect();
    assert_eq!(run.scenarios.len(), 60);
    assert!(
        not_passed.is_empty(),
        "{} of 60 did not pass:\n{}",
        not_passed.len(),
        not_passed.join("\n")
    );
}

#[test]
fn the_shipped_target_is_the_control() {
    let target = target::Target::default();
    assert!(target.identity().is_ok());
}
