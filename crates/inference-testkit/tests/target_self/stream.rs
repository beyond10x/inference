//! The tests of `tests/target/stream.rs`.

mod tests {
    use std::collections::BTreeMap;

    use ess_conformance::scenario::{CommandRef, EventRef, ViewRef};
    use ess_conformance::target::{
        Deadline, EventObservationRequest, SemanticCommandRequest, SemanticCommandResult,
        SemanticViewRequest, TargetError,
    };
    use ess_primitives::consistency::QueryConsistency;
    use ess_primitives::facts::Number;
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;
    use ess_primitives::time::Timestamp;
    use inference_core::generation::StartGeneration;
    use inference_core::id::{NodeId, RunId, StreamId};
    use inference_core::policy::{DeclareProfile, Egress, ProfileKind};
    use inference_pipeline::{
        AddNode, DeclarePipeline, InteractionMode, NodeRole, PlanPipeline, ValidatePipeline,
    };
    use inference_testkit::graph::GraphCatalog;
    use inference_text::{EmitHypothesis, OpenTurn};
    use uuid::Uuid;

    use crate::subject::Stores;
    use crate::subject::stream::{execute_command, observe_events, query_view};

    const UNKNOWN: &str = "00000000-0000-4000-8000-6eced0bc8ba0";

    fn correlation() -> CorrelationId {
        CorrelationId::new("stream").expect("a correlation id")
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

    fn rows(stores: &mut Stores, name: &str) -> Vec<BTreeMap<String, Node>> {
        query_view(
            stores,
            &SemanticViewRequest {
                view: name.parse::<ViewRef>().expect("a view name"),
                params: BTreeMap::new(),
                consistency: QueryConsistency::Current,
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .unwrap_or_else(|error| panic!("`{name}` reads: {error}"))
        .rows
    }

    /// A generation in the store, started past the run refusals, which these cases do not
    /// exercise; returns its identity as the runner captures it.
    fn started(stores: &mut Stores) -> Node {
        let event = stores.stream.start_generation(StartGeneration {
            run_id: RunId::from(Uuid::from_u128(0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d)),
            node_id: NodeId::from(Uuid::from_u128(0x1a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d)),
            epoch: 1,
        });
        text(&event.generation_id.to_string())
    }

    fn emitted(stores: &mut Stores, generation: &Node, sequence: i64) -> Node {
        let result = run(
            stores,
            "inference.stream.EmitTextDelta",
            &[
                ("generation_id", generation.clone()),
                ("sequence", Node::Number(Number::from(sequence))),
                ("text", text("hello")),
            ],
        );
        assert_eq!(
            branch(&result),
            "inference.stream.EmitTextDelta/emitted",
            "{result:?}"
        );
        result.direct_events[0].payload["segment_id"].clone()
    }

    #[test]
    fn the_eight_wrong_state_commands_answer_an_unknown_identity_with_no_current() {
        let cases = [
            (
                "inference.stream.CompleteGeneration",
                "generation_id",
                "GenerationStateConflict",
            ),
            (
                "inference.stream.SupersedeGeneration",
                "generation_id",
                "GenerationStateConflict",
            ),
            (
                "inference.stream.CancelGeneration",
                "generation_id",
                "GenerationStateConflict",
            ),
            (
                "inference.stream.CommitSegment",
                "segment_id",
                "SegmentStateConflict",
            ),
            (
                "inference.stream.QueueForSynthesis",
                "segment_id",
                "SegmentStateConflict",
            ),
            (
                "inference.stream.CompleteSynthesis",
                "segment_id",
                "SegmentStateConflict",
            ),
            (
                "inference.stream.AcknowledgePlayback",
                "segment_id",
                "SegmentStateConflict",
            ),
            (
                "inference.stream.DiscardSegment",
                "segment_id",
                "SegmentStateConflict",
            ),
        ];
        for (name, field, error) in cases {
            let mut stores = Stores::default();
            let result = run(&mut stores, name, &[(field, text(UNKNOWN))]);
            assert_eq!(branch(&result), format!("{name}/wrong-state"));
            let carried = result.error.expect("a declared error");
            assert_eq!(
                carried.error.to_string(),
                format!("inference.stream.{error}")
            );
            assert!(
                carried.fields.is_empty(),
                "{name}: no record holds a state to report: {:?}",
                carried.fields
            );
            assert!(result.direct_events.is_empty(), "{name} published an event");
        }
    }

    #[test]
    fn a_conflict_carries_the_state_the_record_holds() {
        let mut stores = Stores::default();
        let generation = started(&mut stores);
        let segment = emitted(&mut stores, &generation, 1);

        let refused = run(
            &mut stores,
            "inference.stream.QueueForSynthesis",
            &[("segment_id", segment.clone())],
        );
        assert_eq!(
            branch(&refused),
            "inference.stream.QueueForSynthesis/wrong-state"
        );
        assert_eq!(
            refused.error.expect("a declared error").fields["current"],
            text("Tentative")
        );

        let completed = run(
            &mut stores,
            "inference.stream.CompleteGeneration",
            &[("generation_id", generation.clone())],
        );
        assert_eq!(
            branch(&completed),
            "inference.stream.CompleteGeneration/completed"
        );
        let again = run(
            &mut stores,
            "inference.stream.CompleteGeneration",
            &[("generation_id", generation)],
        );
        assert_eq!(
            again.error.expect("a declared error").fields["current"],
            text("Completed")
        );
    }

    #[test]
    fn supersede_through_the_target_discards_the_unplayed_segments_and_the_views_show_it() {
        let mut stores = Stores::default();
        let generation = started(&mut stores);
        let tentative = emitted(&mut stores, &generation, 1);
        let played = emitted(&mut stores, &generation, 2);
        for name in [
            "inference.stream.CommitSegment",
            "inference.stream.QueueForSynthesis",
            "inference.stream.CompleteSynthesis",
            "inference.stream.AcknowledgePlayback",
        ] {
            let result = run(&mut stores, name, &[("segment_id", played.clone())]);
            assert!(result.error.is_none(), "{name}: {result:?}");
        }

        let result = run(
            &mut stores,
            "inference.stream.SupersedeGeneration",
            &[("generation_id", generation.clone())],
        );
        assert_eq!(
            branch(&result),
            "inference.stream.SupersedeGeneration/superseded"
        );
        let published: Vec<String> = result
            .direct_events
            .iter()
            .map(|event| event.event.to_string())
            .collect();
        assert_eq!(published, ["inference.stream.GenerationSuperseded"]);

        let generations = rows(&mut stores, "inference.stream.Generations");
        assert_eq!(generations.len(), 1);
        assert_eq!(generations[0]["generation_id"], generation);
        assert_eq!(generations[0]["state"], text("Superseded"));
        assert_eq!(generations[0]["epoch"], Node::Number(Number::from(1_i64)));

        let segments = rows(&mut stores, "inference.stream.Segments");
        let state_of = |segment: &Node| {
            segments
                .iter()
                .find(|row| &row["segment_id"] == segment)
                .expect("a row")["state"]
                .clone()
        };
        assert_eq!(state_of(&tentative), text("Discarded"));
        assert_eq!(state_of(&played), text("Played"));

        let stale = run(
            &mut stores,
            "inference.stream.EmitTextDelta",
            &[
                ("generation_id", generation),
                ("sequence", Node::Number(Number::from(3_i64))),
                ("text", text("stale")),
            ],
        );
        assert_eq!(
            branch(&stale),
            "inference.stream.EmitTextDelta/stale-generation"
        );
        assert_eq!(
            stale.error.expect("a declared error").error.to_string(),
            "inference.stream.GenerationStateConflict"
        );
    }

    #[test]
    fn a_delta_into_an_unknown_generation_is_refused_with_its_identity() {
        let mut stores = Stores::default();
        let result = run(
            &mut stores,
            "inference.stream.EmitTextDelta",
            &[
                ("generation_id", text(UNKNOWN)),
                ("sequence", Node::Number(Number::from(1_i64))),
                ("text", text("lost")),
            ],
        );
        assert_eq!(
            branch(&result),
            "inference.stream.EmitTextDelta/unknown-generation"
        );
        let carried = result.error.expect("a declared error");
        assert_eq!(
            carried.error.to_string(),
            "inference.stream.GenerationNotFound"
        );
        assert_eq!(carried.fields["generation_id"], text(UNKNOWN));
    }

    /// Two runs of one Planned pipeline holding one node, arranged in the graph store: the first
    /// Created, the second Active. Returns the two run identities and the node's.
    fn runs(stores: &mut Stores) -> (Node, Node, Node) {
        let profile_id = stores
            .policy
            .declare_profile(DeclareProfile {
                kind: ProfileKind::OfflineStrict,
                egress: Egress::Denied,
                admitted: Vec::new(),
            })
            .expect("a row of the table")
            .profile_id;
        let catalog = GraphCatalog {
            policy: &stores.policy,
            model: &stores.model,
        };
        let graph = &mut stores.graph;
        let pipeline_id = graph
            .declare_pipeline(
                DeclarePipeline {
                    name: "p".to_owned(),
                    schema_revision: "1".to_owned(),
                    mode: InteractionMode::TurnBased,
                    profile_id,
                    required_capabilities: Vec::new(),
                },
                &catalog,
            )
            .expect("the profile is declared")
            .pipeline_id;
        let node_id = graph
            .add_node(AddNode {
                pipeline_id,
                name: "llm".to_owned(),
                role: NodeRole::Model,
                requires: Vec::new(),
            })
            .expect("the pipeline is declared")
            .node_id;
        graph
            .validate_pipeline(ValidatePipeline { pipeline_id })
            .expect("a lone node is valid");
        graph
            .plan_pipeline(PlanPipeline { pipeline_id }, &catalog)
            .expect("nothing is required");
        let created = graph.create_run(pipeline_id).expect("planned").run_id;
        let active = graph.create_run(pipeline_id).expect("planned").run_id;
        graph.validate_run(active).expect("Created");
        graph.prepare_run(active).expect("Validating");
        graph.activate_run(active).expect("Preparing");
        let node = |id: String| text(&id);
        (
            node(created.to_string()),
            node(active.to_string()),
            node(node_id.to_string()),
        )
    }

    #[test]
    fn start_generation_refuses_an_unknown_run_then_node_then_a_run_that_is_not_active() {
        let mut stores = Stores::default();
        let (created, active, node_id) = runs(&mut stores);
        let start = |stores: &mut Stores, run_id: &Node, node_id: &Node| {
            run(
                stores,
                "inference.stream.StartGeneration",
                &[
                    ("run_id", run_id.clone()),
                    ("node_id", node_id.clone()),
                    ("epoch", Node::Number(Number::from(1_i64))),
                ],
            )
        };

        let unknown_run = start(&mut stores, &text(UNKNOWN), &text(UNKNOWN));
        assert_eq!(
            branch(&unknown_run),
            "inference.stream.StartGeneration/unknown-run"
        );
        let carried = unknown_run.error.expect("a declared error");
        assert_eq!(carried.error.to_string(), "inference.graph.RunNotFound");
        assert_eq!(
            carried.fields,
            BTreeMap::from([("run_id".to_owned(), text(UNKNOWN))])
        );

        let unknown_node = start(&mut stores, &created, &text(UNKNOWN));
        assert_eq!(
            branch(&unknown_node),
            "inference.stream.StartGeneration/unknown-node",
            "an unknown node is refused before the Created run's state"
        );
        let carried = unknown_node.error.expect("a declared error");
        assert_eq!(carried.error.to_string(), "inference.graph.NodeNotFound");
        assert_eq!(
            carried.fields,
            BTreeMap::from([("node_id".to_owned(), text(UNKNOWN))])
        );

        let not_active = start(&mut stores, &created, &node_id);
        assert_eq!(
            branch(&not_active),
            "inference.stream.StartGeneration/run-not-active"
        );
        let carried = not_active.error.expect("a declared error");
        assert_eq!(
            carried.error.to_string(),
            "inference.graph.RunStateConflict"
        );
        assert_eq!(
            carried.fields,
            BTreeMap::from([("current".to_owned(), text("Created"))])
        );
        assert!(stores.stream.generations().is_empty());
        assert!(stores.stream.events().is_empty());

        let started = start(&mut stores, &active, &node_id);
        assert_eq!(branch(&started), "inference.stream.StartGeneration/started");
        let generations = rows(&mut stores, "inference.stream.Generations");
        assert_eq!(generations.len(), 1);
        assert_eq!(generations[0]["run_id"], active);
        assert_eq!(generations[0]["node_id"], node_id);
    }

    #[test]
    fn an_undeclared_or_missing_field_is_an_error_and_moves_nothing() {
        let mut stores = Stores::default();
        let generation = started(&mut stores);
        let extra = execute_command(
            &mut stores,
            &command(
                "inference.stream.CompleteGeneration",
                &[("generation_id", generation), ("reason", text("x"))],
            ),
        );
        assert!(
            matches!(&extra, Err(TargetError::Unavailable { .. })),
            "{extra:?}"
        );
        let missing = execute_command(&mut stores, &command("inference.stream.CommitSegment", &[]));
        assert!(
            matches!(&missing, Err(TargetError::Unavailable { .. })),
            "{missing:?}"
        );
        assert_eq!(
            rows(&mut stores, "inference.stream.Generations")[0]["state"],
            text("Active")
        );
    }

    #[test]
    fn observed_events_are_the_store_events_in_emission_order() {
        let mut stores = Stores::default();
        let generation = started(&mut stores);
        let segment = emitted(&mut stores, &generation, 5);
        let observe = |stores: &mut Stores, name: &str| {
            observe_events(
                stores,
                &EventObservationRequest {
                    event: name.parse::<EventRef>().expect("an event name"),
                    correlation: correlation(),
                    deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
                },
            )
            .unwrap_or_else(|error| panic!("`{name}` is observed: {error}"))
        };

        let started_events = observe(&mut stores, "inference.stream.GenerationStarted");
        assert_eq!(started_events.len(), 1);
        assert_eq!(started_events[0].sequence, Some(1));
        assert_eq!(started_events[0].payload["generation_id"], generation);

        let deltas = observe(&mut stores, "inference.stream.TextDelta");
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].sequence, Some(2));
        assert_eq!(deltas[0].payload["segment_id"], segment);
        assert_eq!(
            deltas[0].payload["sequence"],
            Node::Number(Number::from(5_i64))
        );
        assert_eq!(deltas[0].payload["text"], text("hello"));

        assert!(observe(&mut stores, "inference.stream.AudioPlayed").is_empty());
    }

    fn observed(stores: &mut Stores, name: &str) -> Vec<ess_conformance::target::ObservedEvent> {
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

    /// An Open turn in the store, opened past the run refusals, which these cases do not
    /// exercise; returns its identity as the runner captures it.
    fn turn(stores: &mut Stores) -> Node {
        let event = stores.stream.open_turn(OpenTurn {
            run_id: RunId::from(Uuid::from_u128(0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d)),
        });
        text(&event.turn_id.to_string())
    }

    /// A Hypothesis span of `turn`, emitted past the run refusals, which these cases do not
    /// exercise.
    fn span(stores: &mut Stores, turn: &Node, sequence: i64) -> Node {
        let Node::Text(turn_id) = turn else {
            panic!("a turn identity is a string: {turn:?}")
        };
        let event = stores
            .stream
            .emit_hypothesis(EmitHypothesis {
                run_id: RunId::from(Uuid::from_u128(0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d)),
                turn_id: turn_id.parse::<Uuid>().expect("a Uuid").into(),
                stream_id: StreamId::from(Uuid::from_u128(
                    0x2a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d,
                )),
                sequence,
                text: "hello wor".to_owned(),
            })
            .expect("an Open turn admits a span");
        text(&event.span_id.to_string())
    }

    #[test]
    fn complete_turn_and_commit_span_answer_an_unknown_identity_with_no_current() {
        for (name, field, error) in [
            (
                "inference.stream.CompleteTurn",
                "turn_id",
                "inference.stream.TurnStateConflict",
            ),
            (
                "inference.stream.CommitSpan",
                "span_id",
                "inference.stream.SpanStateConflict",
            ),
        ] {
            let mut stores = Stores::default();
            let result = run(&mut stores, name, &[(field, text(UNKNOWN))]);
            assert_eq!(branch(&result), format!("{name}/wrong-state"));
            let carried = result.error.expect("a declared error");
            assert_eq!(carried.error.to_string(), error);
            assert!(
                carried.fields.is_empty(),
                "{name}: no record holds a state to report: {:?}",
                carried.fields
            );
            assert!(result.direct_events.is_empty(), "{name} published an event");
        }
    }

    #[test]
    fn revise_span_answers_an_unknown_span_with_its_identity() {
        let mut stores = Stores::default();
        let result = run(
            &mut stores,
            "inference.stream.ReviseSpan",
            &[("span_id", text(UNKNOWN)), ("text", text("lost"))],
        );
        assert_eq!(branch(&result), "inference.stream.ReviseSpan/no-such-span");
        let carried = result.error.expect("a declared error");
        assert_eq!(carried.error.to_string(), "inference.stream.SpanNotFound");
        assert_eq!(carried.fields["span_id"], text(UNKNOWN));
        assert!(result.direct_events.is_empty());
    }

    #[test]
    fn open_turn_and_emit_hypothesis_read_the_run_store_and_refuse_an_identity_before_a_state() {
        let mut stores = Stores::default();
        let (created, active, _) = runs(&mut stores);
        let open = |stores: &mut Stores, run_id: &Node| {
            run(
                stores,
                "inference.stream.OpenTurn",
                &[("run_id", run_id.clone())],
            )
        };
        let unknown = open(&mut stores, &text(UNKNOWN));
        assert_eq!(branch(&unknown), "inference.stream.OpenTurn/unknown-run");
        assert_eq!(
            unknown.error.expect("a declared error").fields,
            BTreeMap::from([("run_id".to_owned(), text(UNKNOWN))])
        );
        let not_active = open(&mut stores, &created);
        assert_eq!(
            branch(&not_active),
            "inference.stream.OpenTurn/run-not-active"
        );
        let carried = not_active.error.expect("a declared error");
        assert_eq!(
            carried.error.to_string(),
            "inference.graph.RunStateConflict"
        );
        assert_eq!(
            carried.fields,
            BTreeMap::from([("current".to_owned(), text("Created"))])
        );
        assert!(stores.stream.turns().is_empty());
        let opened = open(&mut stores, &active);
        assert_eq!(branch(&opened), "inference.stream.OpenTurn/opened");
        let turn_id = opened.direct_events[0].payload["turn_id"].clone();
        assert_eq!(opened.direct_events[0].payload["run_id"], active);
        let completed = open(&mut stores, &active).direct_events[0].payload["turn_id"].clone();
        run(
            &mut stores,
            "inference.stream.CompleteTurn",
            &[("turn_id", completed.clone())],
        );

        let emit = |stores: &mut Stores, run_id: &Node, turn_id: &Node| {
            run(
                stores,
                "inference.stream.EmitHypothesis",
                &[
                    ("run_id", run_id.clone()),
                    ("turn_id", turn_id.clone()),
                    ("stream_id", text(UNKNOWN)),
                    ("sequence", Node::Number(Number::from(1_i64))),
                    ("text", text("hello")),
                ],
            )
        };
        for (run_id, turn_id, outcome, error) in [
            (
                text(UNKNOWN),
                text(UNKNOWN),
                "unknown-run",
                "inference.graph.RunNotFound",
            ),
            (
                created.clone(),
                text(UNKNOWN),
                "unknown-turn",
                "inference.stream.TurnNotFound",
            ),
            (
                created.clone(),
                completed.clone(),
                "run-not-active",
                "inference.graph.RunStateConflict",
            ),
            (
                active.clone(),
                completed.clone(),
                "turn-not-open",
                "inference.stream.TurnStateConflict",
            ),
        ] {
            let refused = emit(&mut stores, &run_id, &turn_id);
            assert_eq!(
                branch(&refused),
                format!("inference.stream.EmitHypothesis/{outcome}")
            );
            assert_eq!(
                refused.error.expect("a declared error").error.to_string(),
                error
            );
            assert!(refused.direct_events.is_empty());
        }
        assert!(stores.stream.spans().is_empty());

        let emitted = emit(&mut stores, &active, &turn_id);
        assert_eq!(branch(&emitted), "inference.stream.EmitHypothesis/emitted");
        let spans = rows(&mut stores, "inference.stream.Spans");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0]["run_id"], active);
        assert_eq!(spans[0]["turn_id"], turn_id);
    }

    #[test]
    fn a_committed_span_refuses_a_revision_through_the_target_and_its_text_is_unchanged() {
        let mut stores = Stores::default();
        let turn = turn(&mut stores);
        let span = span(&mut stores, &turn, 3);

        let revised = run(
            &mut stores,
            "inference.stream.ReviseSpan",
            &[("span_id", span.clone()), ("text", text("hello world"))],
        );
        assert_eq!(branch(&revised), "inference.stream.ReviseSpan/revised");
        assert_eq!(
            revised.direct_events[0].event.to_string(),
            "inference.stream.TranscriptRevision"
        );
        assert_eq!(revised.direct_events[0].payload["span_id"], span);
        assert_eq!(
            revised.direct_events[0].payload["text"],
            text("hello world")
        );

        let committed = run(
            &mut stores,
            "inference.stream.CommitSpan",
            &[("span_id", span.clone())],
        );
        assert_eq!(branch(&committed), "inference.stream.CommitSpan/committed");
        assert_eq!(
            committed.direct_events[0].payload["text"],
            text("hello world")
        );

        let refused = run(
            &mut stores,
            "inference.stream.ReviseSpan",
            &[("span_id", span.clone()), ("text", text("goodbye"))],
        );
        assert_eq!(branch(&refused), "inference.stream.ReviseSpan/immutable");
        let carried = refused.error.expect("a declared error");
        assert_eq!(
            carried.error.to_string(),
            "inference.stream.CommittedSpanImmutable"
        );
        assert_eq!(carried.fields["span_id"], span);
        assert!(refused.direct_events.is_empty());

        let again = run(
            &mut stores,
            "inference.stream.CommitSpan",
            &[("span_id", span.clone())],
        );
        assert_eq!(branch(&again), "inference.stream.CommitSpan/wrong-state");
        assert_eq!(
            again.error.expect("a declared error").fields["current"],
            text("Committed")
        );

        let spans = rows(&mut stores, "inference.stream.Spans");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0]["span_id"], span);
        assert_eq!(spans[0]["turn_id"], turn);
        assert_eq!(
            spans[0]["text"],
            text("hello world"),
            "the text is unchanged"
        );
        assert_eq!(spans[0]["state"], text("Committed"));
        assert_eq!(spans[0]["sequence"], Node::Number(Number::from(3_i64)));
    }

    #[test]
    fn complete_turn_completes_once_and_leaves_its_spans_as_they_are() {
        let mut stores = Stores::default();
        let turn = turn(&mut stores);
        let span = span(&mut stores, &turn, 1);

        let completed = run(
            &mut stores,
            "inference.stream.CompleteTurn",
            &[("turn_id", turn.clone())],
        );
        assert_eq!(
            branch(&completed),
            "inference.stream.CompleteTurn/completed"
        );
        assert_eq!(
            completed.direct_events[0].event.to_string(),
            "inference.stream.TurnCompleted"
        );
        assert_eq!(completed.direct_events[0].payload["turn_id"], turn);

        let again = run(
            &mut stores,
            "inference.stream.CompleteTurn",
            &[("turn_id", turn.clone())],
        );
        assert_eq!(branch(&again), "inference.stream.CompleteTurn/wrong-state");
        assert_eq!(
            again.error.expect("a declared error").fields["current"],
            text("Completed")
        );

        let turns = rows(&mut stores, "inference.stream.Turns");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0]["turn_id"], turn);
        assert_eq!(turns[0]["state"], text("Completed"));
        let spans = rows(&mut stores, "inference.stream.Spans");
        assert_eq!(spans[0]["span_id"], span);
        assert_eq!(spans[0]["state"], text("Hypothesis"));
    }

    #[test]
    fn the_turn_and_span_events_are_observed_with_every_declared_field() {
        let mut stores = Stores::default();
        let turn = turn(&mut stores);
        let span = span(&mut stores, &turn, 2);
        run(
            &mut stores,
            "inference.stream.CommitSpan",
            &[("span_id", span.clone())],
        );

        let opened = observed(&mut stores, "inference.stream.TurnOpened");
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].sequence, Some(1));
        assert_eq!(opened[0].payload["turn_id"], turn);
        assert!(opened[0].payload.contains_key("run_id"));

        let hypotheses = observed(&mut stores, "inference.stream.TranscriptHypothesis");
        assert_eq!(hypotheses.len(), 1);
        assert_eq!(hypotheses[0].sequence, Some(2));
        assert_eq!(hypotheses[0].payload["span_id"], span);
        assert_eq!(hypotheses[0].payload["turn_id"], turn);
        assert_eq!(
            hypotheses[0].payload["sequence"],
            Node::Number(Number::from(2_i64))
        );
        assert_eq!(hypotheses[0].payload["text"], text("hello wor"));

        let committed = observed(&mut stores, "inference.stream.TranscriptCommitted");
        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].sequence, Some(3));
        assert_eq!(committed[0].payload["text"], text("hello wor"));

        assert!(observed(&mut stores, "inference.stream.TranscriptRevision").is_empty());
        assert!(observed(&mut stores, "inference.stream.TurnCompleted").is_empty());
    }
}
