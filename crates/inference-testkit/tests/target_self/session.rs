//! The tests of `tests/target/session.rs`.

mod tests {
    use std::collections::BTreeMap;

    use ess_conformance::scenario::{CommandRef, EventRef, OutcomeRef, ViewRef};
    use ess_conformance::target::{
        Deadline, EventObservationRequest, ExternalOutcomeControl, ObservedEvent,
        SemanticCommandRequest, SemanticCommandResult, SemanticViewRequest, TargetError,
    };
    use ess_primitives::consistency::QueryConsistency;
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;
    use ess_primitives::time::Timestamp;
    use inference_core::model::Locality;
    use inference_core::policy::{DeclareProfile, Egress, ProfileKind};
    use inference_pipeline::{DeclarePipeline, InteractionMode, PlanPipeline, ValidatePipeline};
    use inference_testkit::graph::GraphCatalog;

    use crate::subject::Stores;
    use crate::subject::session::{
        configure_external_outcome, execute_command, observe_events, query_view,
    };

    const UNKNOWN: &str = "00000005-0000-4000-8000-6eced0bc8ba0";

    fn correlation() -> CorrelationId {
        CorrelationId::new("session").expect("a correlation id")
    }

    fn text(value: &str) -> Node {
        Node::Text(value.to_owned())
    }

    fn command(name: &str, input: &[(&str, Node)]) -> SemanticCommandRequest {
        SemanticCommandRequest {
            command: name.parse::<CommandRef>().expect("a command name"),
            actor: None,
            caller: None,
            input: input
                .iter()
                .map(|(field, value)| ((*field).to_owned(), value.clone()))
                .collect(),
            correlation: correlation(),
        }
    }

    fn run(stores: &mut Stores, name: &str, input: &[(&str, Node)]) -> SemanticCommandResult {
        execute_command(stores, &command(name, input))
            .unwrap_or_else(|error| panic!("`{name}` answers: {error}"))
    }

    fn branch(result: &SemanticCommandResult) -> String {
        result
            .outcome
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    fn sessions(stores: &mut Stores) -> Vec<BTreeMap<String, Node>> {
        query_view(
            stores,
            &SemanticViewRequest {
                view: "inference.session.Sessions"
                    .parse::<ViewRef>()
                    .expect("a view name"),
                params: BTreeMap::new(),
                consistency: QueryConsistency::Current,
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .unwrap_or_else(|error| panic!("`Sessions` reads: {error}"))
        .rows
    }

    fn observed(stores: &mut Stores, name: &str) -> Vec<ObservedEvent> {
        observe_events(
            stores,
            &EventObservationRequest {
                event: name.parse::<EventRef>().expect("an event name"),
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .unwrap_or_else(|error| panic!("`{name}` is observed: {error}"))
    }

    /// Two Planned batch pipelines in the graph store, declared and planned past the target, and
    /// a run of each; returns the pipelines and the runs as the runner captures them.
    fn planned(stores: &mut Stores) -> [Node; 4] {
        let profile_id = stores
            .policy
            .declare_profile(DeclareProfile {
                kind: ProfileKind::OfflineStrict,
                egress: Egress::Denied,
                admitted: vec![Locality::SameProcess],
            })
            .expect("a row of the table")
            .profile_id;
        let mut ids = Vec::new();
        for name in ["first", "second"] {
            let catalog = GraphCatalog {
                policy: &stores.policy,
                model: &stores.model,
            };
            let pipeline_id = stores
                .graph
                .declare_pipeline(
                    DeclarePipeline {
                        name: name.to_owned(),
                        schema_revision: "1".to_owned(),
                        mode: InteractionMode::Batch,
                        profile_id,
                        required_capabilities: Vec::new(),
                    },
                    &catalog,
                )
                .expect("declared")
                .pipeline_id;
            stores
                .graph
                .validate_pipeline(ValidatePipeline { pipeline_id })
                .expect("validates");
            stores
                .graph
                .plan_pipeline(PlanPipeline { pipeline_id }, &catalog)
                .expect("plans");
            ids.push(pipeline_id);
        }
        let first = stores.graph.create_run(ids[0]).expect("planned").run_id;
        let second = stores.graph.create_run(ids[1]).expect("planned").run_id;
        [
            text(&ids[0].to_string()),
            text(&ids[1].to_string()),
            text(&first.to_string()),
            text(&second.to_string()),
        ]
    }

    fn opened(stores: &mut Stores, pipeline_id: Node) -> Node {
        let result = run(
            stores,
            "inference.session.OpenSession",
            &[("pipeline_id", pipeline_id), ("mode", text("batch"))],
        );
        assert_eq!(
            branch(&result),
            "inference.session.OpenSession/opened",
            "{result:?}"
        );
        result.direct_events[0].payload["session_id"].clone()
    }

    fn attach(stores: &mut Stores, session_id: &Node, run_id: &Node) -> SemanticCommandResult {
        run(
            stores,
            "inference.session.AttachRun",
            &[
                ("session_id", session_id.clone()),
                ("run_id", run_id.clone()),
            ],
        )
    }

    #[test]
    fn a_session_holds_its_current_run_in_the_view_and_the_events() {
        let mut stores = Stores::default();
        let [first, _, run_id, _] = planned(&mut stores);
        let session_id = opened(&mut stores, first.clone());
        assert_eq!(
            session_id,
            text("00000005-0000-4000-8000-000000000001"),
            "the session counts in its own space"
        );
        let row = &sessions(&mut stores)[0];
        assert_eq!(row["run_id"], Node::Null, "no current run yet");
        assert_eq!(row["state"], text("Opened"));
        assert_eq!(row["mode"], text("batch"));
        assert_eq!(row["pipeline_id"], first);

        let refused = run(
            &mut stores,
            "inference.session.MarkSessionReady",
            &[("session_id", session_id.clone())],
        );
        assert_eq!(
            branch(&refused),
            "inference.session.MarkSessionReady/no-current-run"
        );
        let carried = refused.error.expect("a declared error");
        assert_eq!(carried.error.to_string(), "inference.session.NoCurrentRun");
        assert_eq!(carried.fields["session_id"], session_id);

        let attached = attach(&mut stores, &session_id, &run_id);
        assert_eq!(
            branch(&attached),
            "inference.session.AttachRun/attached",
            "{attached:?}"
        );
        let ready = run(
            &mut stores,
            "inference.session.MarkSessionReady",
            &[("session_id", session_id.clone())],
        );
        assert_eq!(branch(&ready), "inference.session.MarkSessionReady/ready");
        let row = &sessions(&mut stores)[0];
        assert_eq!(row["run_id"], run_id);
        assert_eq!(row["state"], text("Ready"));

        let events = observed(&mut stores, "inference.session.RunAttached");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["run_id"], run_id);
        assert_eq!(events[0].payload["session_id"], session_id);
        assert_eq!(
            events[0].sequence,
            Some(u64::try_from(stores.graph.events().len() + 3).expect("small")),
            "placed after the policy and graph events recorded before it and the session's opening"
        );
    }

    #[test]
    fn a_run_of_another_pipeline_is_refused_and_moves_nothing() {
        let mut stores = Stores::default();
        let [first, _, _, foreign] = planned(&mut stores);
        let session_id = opened(&mut stores, first);
        let recorded = stores.session.events().len();
        let result = attach(&mut stores, &session_id, &foreign);
        assert_eq!(
            branch(&result),
            "inference.session.AttachRun/pipeline-mismatch"
        );
        let carried = result.error.expect("a declared error");
        assert_eq!(
            carried.error.to_string(),
            "inference.session.PipelineMismatch"
        );
        assert_eq!(carried.fields["session_id"], session_id);
        assert_eq!(carried.fields["run_id"], foreign);
        assert!(result.direct_events.is_empty());
        assert_eq!(stores.session.events().len(), recorded);
        assert_eq!(sessions(&mut stores)[0]["run_id"], Node::Null);
    }

    #[test]
    fn a_forced_mismatch_answers_for_an_unknown_session_and_lapses() {
        let mut stores = Stores::default();
        let force = |stores: &mut Stores, branch: &str| {
            configure_external_outcome(
                stores,
                &ExternalOutcomeControl {
                    force: OutcomeRef::new(
                        "inference.session.AttachRun"
                            .parse::<CommandRef>()
                            .expect("a command name"),
                        branch.parse().expect("an outcome name"),
                    ),
                    correlation: correlation(),
                },
            )
        };
        assert!(matches!(
            force(&mut stores, "attached"),
            Err(TargetError::Unavailable { .. })
        ));
        force(&mut stores, "pipeline-mismatch").expect("pipeline-mismatch is external");
        let forced = attach(&mut stores, &text(UNKNOWN), &text(UNKNOWN));
        assert_eq!(
            branch(&forced),
            "inference.session.AttachRun/pipeline-mismatch"
        );
        let carried = forced.error.expect("a declared error");
        assert_eq!(carried.fields["session_id"], text(UNKNOWN));
        assert_eq!(carried.fields["run_id"], text(UNKNOWN));

        let lapsed = attach(&mut stores, &text(UNKNOWN), &text(UNKNOWN));
        assert_eq!(
            branch(&lapsed),
            "inference.session.AttachRun/no-such-session",
            "the force lapses with the invocation it answered"
        );
        assert!(stores.session.events().is_empty());
    }

    #[test]
    fn an_undeclared_or_malformed_field_is_an_error_and_moves_nothing() {
        let mut stores = Stores::default();
        let [first, _, run_id, _] = planned(&mut stores);
        let session_id = opened(&mut stores, first.clone());
        let recorded = stores.session.events().len();
        for (name, input) in [
            (
                "inference.session.OpenSession",
                vec![
                    ("pipeline_id", first.clone()),
                    ("mode", text("batch")),
                    ("run_id", run_id.clone()),
                ],
            ),
            (
                "inference.session.OpenSession",
                vec![("pipeline_id", first), ("mode", text("chatty"))],
            ),
            (
                "inference.session.AttachRun",
                vec![("session_id", session_id.clone())],
            ),
            (
                "inference.session.AttachRun",
                vec![
                    ("session_id", session_id.clone()),
                    ("run_id", run_id.clone()),
                    ("pipeline_id", text(UNKNOWN)),
                ],
            ),
            (
                "inference.session.CloseSession",
                vec![
                    ("session_id", session_id.clone()),
                    ("cancel", Node::Bool(true)),
                ],
            ),
            (
                "inference.session.MarkSessionReady",
                vec![("session_id", Node::Bool(true))],
            ),
        ] {
            let answer = execute_command(&mut stores, &command(name, &input));
            assert!(
                matches!(answer, Err(TargetError::Unavailable { .. })),
                "{name} {input:?}: {answer:?}"
            );
        }
        assert_eq!(stores.session.events().len(), recorded);
        assert_eq!(sessions(&mut stores)[0]["state"], text("Opened"));
    }
}
