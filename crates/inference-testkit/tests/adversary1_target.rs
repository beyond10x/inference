//! Adversary pass 1 against the testkit conformance target: the request edges the synthesized
//! suite never sends (undeclared fields, fractional and out-of-range integers, undeclared names,
//! unreached consistency tokens), the values the suite only ever sends one way (a refusal of `0`,
//! one capability, a concurrency of `1`), and the event observation no model scenario reaches.

mod target;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef, ViewRef};
use ess_conformance::target::{
    ConformanceTarget, Deadline, EventObservationRequest, ScenarioContext, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError,
};
use ess_conformance::{AdmittedSuite, Runner, Status};
use ess_primitives::consistency::{ConsistencyToken, QueryConsistency};
use ess_primitives::facts::Number;
use ess_primitives::ids::CorrelationId;
use ess_primitives::node::Node;
use ess_primitives::time::Timestamp;
use serde_json::{Value, json};

use target::Target;

fn correlation() -> CorrelationId {
    CorrelationId::new("adversary1").expect("a correlation id")
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

fn declare_model_input() -> Vec<(&'static str, Node)> {
    vec![
        ("family", text("family")),
        ("checkpoint", text("checkpoint")),
        ("code_license", text("code_license")),
        ("weight_license", text("weight_license")),
        ("checksum", text("checksum")),
        ("input_modalities", list(&["Text"])),
        ("output_modalities", list(&["Audio"])),
        ("provenance", text("provenance")),
        ("source_revision", text("source_revision")),
    ]
}

fn declare_adapter_input(model_id: Node, max_concurrency: Node) -> Vec<(&'static str, Node)> {
    vec![
        ("model_id", model_id),
        ("backend", text("backend")),
        ("contract_revision", text("contract_revision")),
        ("execution_form", text("in_process")),
        ("locality", text("same_process")),
        ("capabilities", list(&["tool_calling", "native_duplex"])),
        ("max_concurrency", max_concurrency),
    ]
}

fn view(name: &str, consistency: QueryConsistency) -> SemanticViewRequest {
    SemanticViewRequest {
        view: name.parse::<ViewRef>().expect("a view name"),
        params: BTreeMap::new(),
        consistency,
        correlation: correlation(),
        deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
    }
}

fn rows(target: &Target, name: &str) -> SemanticViewResult {
    target
        .query_view(view(name, QueryConsistency::Current))
        .unwrap_or_else(|error| panic!("`{name}` reads: {error}"))
}

fn outcome(command: &str, branch: &str) -> OutcomeRef {
    OutcomeRef::new(
        command.parse::<CommandRef>().expect("a command name"),
        branch.parse().expect("an outcome name"),
    )
}

fn begun() -> Target {
    let target = Target::default();
    let context = ScenarioContext::new(
        "inference.model.DeclareModel/outcome/declared"
            .parse()
            .expect("a scenario id"),
        correlation(),
    );
    target
        .begin_scenario(&context)
        .expect("the scenario begins");
    target
}

/// Declares one model and returns the `model_id` its event carried, as the runner captures it.
fn declared_model(target: &Target) -> Node {
    let result = target
        .execute_command(command(
            "inference.model.DeclareModel",
            declare_model_input(),
        ))
        .expect("DeclareModel answers");
    result.direct_events[0].payload["model_id"].clone()
}

fn assert_error_not_unsupported(answer: &Result<SemanticCommandResult, TargetError>, why: &str) {
    match answer {
        Err(TargetError::Unavailable { .. }) => {}
        other => panic!("{why}: expected an `error` (TargetError::Unavailable), got {other:?}"),
    }
}

#[test]
fn an_undeclared_input_field_is_an_error_and_declares_nothing() {
    let target = begun();
    let mut input = declare_model_input();
    input.push(("colour", text("red")));

    let answer = target.execute_command(command("inference.model.DeclareModel", input));

    assert_error_not_unsupported(&answer, "DeclareModel carrying `colour`");
    assert!(
        rows(&target, "inference.model.Models").rows.is_empty(),
        "a refused request stored a model"
    );
}

#[test]
fn a_missing_input_field_is_an_error() {
    let target = begun();
    let mut input = declare_model_input();
    input.retain(|(field, _)| *field != "checksum");

    let answer = target.execute_command(command("inference.model.DeclareModel", input));

    assert_error_not_unsupported(&answer, "DeclareModel without `checksum`");
}

#[test]
fn an_undeclared_enum_member_is_an_error() {
    let target = begun();
    let model_id = declared_model(&target);
    let mut input = declare_adapter_input(model_id, float(1.0));
    input[3] = ("execution_form", text("teleported"));

    let answer = target.execute_command(command("inference.model.DeclareAdapter", input));

    assert_error_not_unsupported(&answer, "DeclareAdapter with execution_form `teleported`");
}

#[test]
fn a_fractional_or_out_of_range_concurrency_is_an_error_and_declares_nothing() {
    for (value, why) in [
        (1.5, "a fractional 1.5"),
        (0.5, "a fractional 0.5, which truncates to the refusal 0"),
        (-0.5, "a fractional -0.5"),
        (9_223_372_036_854_775_808.0, "2^63, one past i64::MAX"),
        (1e20, "1e20"),
        (1e-40, "1e-40, a Binary64 magnitude"),
    ] {
        let target = begun();
        let model_id = declared_model(&target);

        let answer = target.execute_command(command(
            "inference.model.DeclareAdapter",
            declare_adapter_input(model_id, float(value)),
        ));

        assert_error_not_unsupported(&answer, why);
        assert!(
            rows(&target, "inference.model.Adapters").rows.is_empty(),
            "{why} stored an adapter"
        );
    }
}

#[test]
fn the_least_i64_is_read_and_refused_with_its_value() {
    let target = begun();
    let model_id = declared_model(&target);

    let result = target
        .execute_command(command(
            "inference.model.DeclareAdapter",
            declare_adapter_input(model_id, float(-9_223_372_036_854_775_808.0)),
        ))
        .expect("-2^63 is an i64");

    assert_eq!(
        result.outcome,
        Some(outcome("inference.model.DeclareAdapter", "refused"))
    );
    let error = result.error.expect("the refusal carries its error");
    assert_eq!(
        error.error,
        "inference.model.NonPositiveConcurrency"
            .parse::<ErrorRef>()
            .expect("an error name")
    );
    assert_eq!(
        error.fields["submitted"],
        Node::Number(Number::from(i64::MIN))
    );
}

#[test]
fn a_refusal_echoes_the_submitted_value_it_refused() {
    let target = begun();
    let model_id = declared_model(&target);

    let result = target
        .execute_command(command(
            "inference.model.DeclareAdapter",
            declare_adapter_input(model_id, float(-7.0)),
        ))
        .expect("DeclareAdapter answers");

    assert_eq!(
        result.outcome,
        Some(outcome("inference.model.DeclareAdapter", "refused"))
    );
    assert_eq!(
        result.error.expect("a refusal").fields["submitted"],
        Node::Number(Number::from(-7_i64)),
        "the target echoed something other than the refused -7"
    );
    assert!(result.direct_events.is_empty());
}

#[test]
fn the_adapters_view_publishes_the_stored_values_under_counter_identities() {
    let target = begun();
    let model_id = declared_model(&target);
    assert_eq!(model_id, text("00000001-0000-4000-8000-000000000001"));

    let result = target
        .execute_command(command(
            "inference.model.DeclareAdapter",
            declare_adapter_input(model_id.clone(), float(7.0)),
        ))
        .expect("DeclareAdapter answers");
    assert_eq!(
        result.outcome,
        Some(outcome("inference.model.DeclareAdapter", "declared"))
    );

    let rows = rows(&target, "inference.model.Adapters").rows;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        row["adapter_id"],
        text("00000001-0000-4000-8000-000000000002")
    );
    assert_eq!(row["model_id"], model_id);
    assert_eq!(row["max_concurrency"], Node::Number(Number::from(7_i64)));
    assert_eq!(
        row["capabilities"],
        list(&["tool_calling", "native_duplex"])
    );
    assert_eq!(row["execution_form"], text("in_process"));
    assert_eq!(row["locality"], text("same_process"));
}

#[test]
fn a_read_demanding_a_token_the_store_has_not_reached_is_an_error() {
    let target = begun();
    let token = target
        .execute_command(command(
            "inference.model.DeclareModel",
            declare_model_input(),
        ))
        .expect("DeclareModel answers")
        .consistency
        .expect("a command answers a consistency token");

    let reached = target.query_view(view(
        "inference.model.Models",
        QueryConsistency::at_least(token),
    ));
    assert_eq!(reached.expect("the token was reached").rows.len(), 1);

    let ahead = target.query_view(view(
        "inference.model.Models",
        QueryConsistency::at_least(ConsistencyToken::new("seq:5").expect("a token")),
    ));
    assert!(
        matches!(ahead, Err(TargetError::Unavailable { .. })),
        "a read demanding seq:5 of a store at seq:1 answered {ahead:?}"
    );
}

#[test]
fn names_the_model_module_does_not_declare_are_unsupported() {
    let target = begun();

    let command_answer = target.execute_command(command("inference.model.RetireModel", Vec::new()));
    assert!(
        matches!(&command_answer, Err(error) if error.is_unsupported()),
        "{command_answer:?}"
    );

    let view_answer = target.query_view(view("inference.model.Retired", QueryConsistency::Current));
    assert!(
        matches!(&view_answer, Err(error) if error.is_unsupported()),
        "{view_answer:?}"
    );

    let other_domain = target.execute_command(command("inference.graph.CreateRun", Vec::new()));
    assert!(
        matches!(&other_domain, Err(error) if error.is_unsupported()),
        "{other_domain:?}"
    );
}

#[test]
fn observed_events_are_the_store_events_in_emission_order() {
    let target = begun();
    let model_id = declared_model(&target);
    target
        .execute_command(command(
            "inference.model.DeclareAdapter",
            declare_adapter_input(model_id.clone(), float(0.0)),
        ))
        .expect("the refusal answers");
    target
        .execute_command(command(
            "inference.model.DeclareAdapter",
            declare_adapter_input(model_id.clone(), float(2.0)),
        ))
        .expect("the declaration answers");

    let observe = |name: &str| {
        target
            .observe_events(EventObservationRequest {
                event: name.parse::<EventRef>().expect("an event name"),
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            })
            .unwrap_or_else(|error| panic!("`{name}` is observed: {error}"))
    };

    let models = observe("inference.model.ModelDeclared");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].sequence, Some(1));
    assert_eq!(models[0].payload["model_id"], model_id);

    let adapters = observe("inference.model.AdapterDeclared");
    assert_eq!(adapters.len(), 1, "the refusal recorded an event");
    assert_eq!(adapters[0].sequence, Some(2));
    assert_eq!(
        adapters[0].payload["adapter_id"],
        text("00000001-0000-4000-8000-000000000002")
    );
    assert_eq!(adapters[0].payload["model_id"], model_id);
}

#[test]
fn a_new_scenario_starts_from_empty_stores_and_the_first_identity() {
    let target = begun();
    declared_model(&target);
    declared_model(&target);
    let context = ScenarioContext::new(
        "inference.model.DeclareAdapter/outcome/declared"
            .parse()
            .expect("a scenario id"),
        correlation(),
    );
    target.end_scenario(&context).expect("the scenario ends");
    target.begin_scenario(&context).expect("the next begins");

    assert!(rows(&target, "inference.model.Models").rows.is_empty());
    assert_eq!(
        declared_model(&target),
        text("00000001-0000-4000-8000-000000000001")
    );
}

/// The synthesized suite of `ess/`, synthesized into a fresh directory of this test.
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
        .join(format!("adversary1-{millis}-{}", std::process::id()));
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

/// Runs one scenario of `suite`, with its first `command` step's input edited by `edit`, and
/// returns the status the runner recorded.
fn status_with(
    suite: &Value,
    scenario: &str,
    command: &str,
    edit: impl Fn(&mut serde_json::Map<String, Value>),
) -> Status {
    let mut one = suite.clone();
    let mut kept = one["scenarios"][scenario].clone();
    let steps = kept["steps"].as_array_mut().expect("steps");
    let step = steps
        .iter_mut()
        .rfind(|step| step["step"] == "execute_command" && step["command"] == command)
        .expect("the scenario executes the command");
    edit(step["input"].as_object_mut().expect("an input"));
    one["scenarios"] = json!({ scenario: kept });
    let admitted = AdmittedSuite::from_json(&one.to_string())
        .unwrap_or_else(|error| panic!("the edited suite is admitted: {error:?}"));
    let run = Runner::for_suite(admitted.suite()).run_admitted(&admitted, &Target::default());
    assert_eq!(run.scenarios.len(), 1);
    run.scenarios[0].status
}

#[test]
fn through_the_runner_a_fractional_or_out_of_range_integer_or_an_extra_field_is_error() {
    let suite = synthesized_suite();
    let declared = "inference.model.DeclareAdapter/outcome/declared";
    let adapter = "inference.model.DeclareAdapter";

    assert_eq!(
        status_with(&suite, declared, adapter, |_| {}),
        Status::Passed,
        "the unedited scenario is the control"
    );
    for value in [json!(1.5), json!(9.3e18), json!(1e20)] {
        let edited = value.clone();
        assert_eq!(
            status_with(&suite, declared, adapter, move |input| {
                input["max_concurrency"] = json!({"kind": "literal", "value": edited.clone()});
            }),
            Status::Error,
            "max_concurrency {value}"
        );
    }
    assert_eq!(
        status_with(&suite, declared, adapter, |input| {
            input.insert(
                "colour".to_owned(),
                json!({"kind": "literal", "value": "red"}),
            );
        }),
        Status::Error,
        "an undeclared `colour`"
    );
}
