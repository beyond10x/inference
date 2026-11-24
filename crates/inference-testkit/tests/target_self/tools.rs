//! The tests of `tests/target/tools.rs`.

mod tests {
    use std::collections::BTreeMap;

    use ess_conformance::scenario::{CommandRef, EventRef, ViewRef};
    use ess_conformance::target::{
        Deadline, EventObservationRequest, ObservedEvent, SemanticCommandRequest,
        SemanticCommandResult, SemanticViewRequest, TargetError,
    };
    use ess_primitives::consistency::QueryConsistency;
    use ess_primitives::facts::Number;
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;
    use ess_primitives::time::Timestamp;
    use inference_core::generation::StartGeneration;
    use inference_core::id::{NodeId, RunId, StreamId};
    use inference_text::{EmitHypothesis, OpenTurn};
    use uuid::Uuid;

    use crate::subject::Stores;
    use crate::subject::tools::{execute_command, observe_events, query_view};

    const UNKNOWN: &str = "00000004-0000-4000-8000-6eced0bc8ba0";

    fn correlation() -> CorrelationId {
        CorrelationId::new("tools").expect("a correlation id")
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

    fn schema() -> Node {
        Node::Map(BTreeMap::from([
            ("type".to_owned(), text("object")),
            (
                "required".to_owned(),
                Node::Seq(vec![text("day"), Node::Number(Number::from(2_i64))]),
            ),
            ("strict".to_owned(), Node::Bool(true)),
            ("note".to_owned(), Node::Null),
        ]))
    }

    fn declared(stores: &mut Stores, side_effect: &str) -> Node {
        let result = run(
            stores,
            "inference.tools.DeclareTool",
            &[
                ("name", text("book")),
                ("description", text("Books a table for the day it names.")),
                ("schema", schema()),
                ("schema_revision", text("1")),
                ("side_effect", text(side_effect)),
                ("requires_confirmation", Node::Bool(true)),
            ],
        );
        assert_eq!(branch(&result), "inference.tools.DeclareTool/declared");
        result.direct_events[0].payload["tool_id"].clone()
    }

    /// A generation and a Committed span in the stream store, made past the run refusals, which
    /// these cases do not exercise; returns their identities as the runner captures them.
    fn basis(stores: &mut Stores) -> (Node, Node) {
        let run_id = RunId::from(Uuid::from_u128(0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d));
        let generation = stores.stream.start_generation(StartGeneration {
            run_id,
            node_id: NodeId::from(Uuid::from_u128(2)),
            epoch: 1,
        });
        let turn = stores.stream.open_turn(OpenTurn { run_id });
        let span = stores
            .stream
            .emit_hypothesis(EmitHypothesis {
                run_id,
                turn_id: turn.turn_id,
                stream_id: StreamId::from(Uuid::from_u128(3)),
                sequence: 1,
                text: "book it".to_owned(),
            })
            .expect("an Open turn admits a span");
        stores.stream.commit_span(span.span_id).expect("commits");
        (
            text(&generation.generation_id.to_string()),
            text(&span.span_id.to_string()),
        )
    }

    fn proposed(stores: &mut Stores, arguments: Node) -> Node {
        let tool = declared(stores, "idempotent_write");
        let (generation, span) = basis(stores);
        let result = run(
            stores,
            "inference.tools.ProposeToolCall",
            &[
                ("tool_id", tool),
                ("generation_id", generation),
                ("basis_span_id", span),
                ("arguments", arguments),
            ],
        );
        assert_eq!(
            branch(&result),
            "inference.tools.ProposeToolCall/proposed",
            "{result:?}"
        );
        result.direct_events[0].payload["call_id"].clone()
    }

    #[test]
    fn a_json_value_is_carried_as_sent_into_the_views_and_events() {
        let mut stores = Stores::default();
        let call = proposed(&mut stores, schema());
        let tools = rows(&mut stores, "inference.tools.Tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["schema"], schema());
        assert_eq!(
            tools[0]["description"],
            text("Books a table for the day it names.")
        );
        assert_eq!(tools[0]["requires_confirmation"], Node::Bool(true));
        let calls = rows(&mut stores, "inference.tools.ToolCalls");
        assert_eq!(calls[0]["arguments"], schema());
        assert_eq!(calls[0]["state"], text("Proposed"));
        assert_eq!(calls[0]["side_effect"], text("idempotent_write"));

        for (name, input) in [
            (
                "inference.tools.AuthorizeToolCall",
                vec![
                    ("call_id", call.clone()),
                    ("confirmation_required", Node::Bool(true)),
                    ("confirmed", Node::Bool(true)),
                ],
            ),
            (
                "inference.tools.StartToolCall",
                vec![("call_id", call.clone())],
            ),
            (
                "inference.tools.CompleteToolCall",
                vec![("call_id", call.clone()), ("result", schema())],
            ),
        ] {
            let result = run(&mut stores, name, &input);
            assert!(result.error.is_none(), "{name}: {result:?}");
        }
        let results = observed(&mut stores, "inference.tools.ToolCallResult");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].payload["result"], schema());
        assert_eq!(results[0].payload["call_id"], call);
        assert_eq!(
            rows(&mut stores, "inference.tools.ToolCalls")[0]["state"],
            text("Completed")
        );
    }

    #[test]
    fn a_move_of_an_unknown_call_is_wrong_state_with_no_current() {
        for (name, extra) in [
            (
                "inference.tools.AuthorizeToolCall",
                vec![
                    ("confirmation_required", Node::Bool(false)),
                    ("confirmed", Node::Bool(false)),
                ],
            ),
            (
                "inference.tools.RejectToolCall",
                vec![("reason", text("denied"))],
            ),
            ("inference.tools.StartToolCall", vec![]),
            (
                "inference.tools.CompleteToolCall",
                vec![("result", Node::Null)],
            ),
            (
                "inference.tools.FailToolCall",
                vec![(
                    "failure",
                    Node::Map(BTreeMap::from([
                        ("kind".to_owned(), text("cancelled")),
                        ("detail".to_owned(), text("gone")),
                    ])),
                )],
            ),
        ] {
            let mut stores = Stores::default();
            let mut input = vec![("call_id", text(UNKNOWN))];
            input.extend(extra);
            let result = run(&mut stores, name, &input);
            assert_eq!(branch(&result), format!("{name}/wrong-state"));
            let carried = result.error.expect("a declared error");
            assert_eq!(
                carried.error.to_string(),
                "inference.tools.ToolCallStateConflict"
            );
            assert!(carried.fields.is_empty(), "{name}: {:?}", carried.fields);
            assert!(result.direct_events.is_empty());
        }
    }

    #[test]
    fn unconfirmed_answers_before_the_call_is_looked_up_and_names_the_call() {
        let mut stores = Stores::default();
        let result = run(
            &mut stores,
            "inference.tools.AuthorizeToolCall",
            &[
                ("call_id", text(UNKNOWN)),
                ("confirmation_required", Node::Bool(true)),
                ("confirmed", Node::Bool(false)),
            ],
        );
        assert_eq!(
            branch(&result),
            "inference.tools.AuthorizeToolCall/unconfirmed"
        );
        let carried = result.error.expect("a declared error");
        assert_eq!(
            carried.error.to_string(),
            "inference.tools.ConfirmationRequired"
        );
        assert_eq!(carried.fields["call_id"], text(UNKNOWN));
    }

    #[test]
    fn an_undeclared_or_malformed_field_is_an_error_and_moves_nothing() {
        let mut stores = Stores::default();
        let call = proposed(&mut stores, Node::Null);
        let recorded = stores.tools.events().len();
        let failure = |fields: &[(&str, Node)]| {
            Node::Map(
                fields
                    .iter()
                    .map(|(field, value)| ((*field).to_owned(), value.clone()))
                    .collect(),
            )
        };
        for input in [
            vec![("call_id", call.clone())],
            vec![
                ("call_id", call.clone()),
                ("failure", failure(&[("kind", text("cancelled"))])),
            ],
            vec![
                ("call_id", call.clone()),
                (
                    "failure",
                    failure(&[("kind", text("timeout")), ("detail", text("x"))]),
                ),
            ],
            vec![
                ("call_id", call.clone()),
                (
                    "failure",
                    failure(&[
                        ("kind", text("cancelled")),
                        ("detail", text("x")),
                        ("code", text("E1")),
                    ]),
                ),
            ],
            vec![
                ("call_id", call.clone()),
                (
                    "failure",
                    failure(&[("kind", text("cancelled")), ("detail", text("x"))]),
                ),
                ("retry", Node::Bool(true)),
            ],
        ] {
            let answer = execute_command(
                &mut stores,
                &command("inference.tools.FailToolCall", &input),
            );
            assert!(
                matches!(answer, Err(TargetError::Unavailable { .. })),
                "{input:?}: {answer:?}"
            );
        }
        let undeclared = execute_command(
            &mut stores,
            &command(
                "inference.tools.StartToolCall",
                &[("call_id", call.clone()), ("deadline", Node::Null)],
            ),
        );
        assert!(matches!(undeclared, Err(TargetError::Unavailable { .. })));
        assert_eq!(stores.tools.events().len(), recorded);
        assert_eq!(
            rows(&mut stores, "inference.tools.ToolCalls")[0]["state"],
            text("Proposed")
        );
    }

    #[test]
    fn tools_events_take_their_place_in_the_one_publication_order() {
        let mut stores = Stores::default();
        let tool = declared(&mut stores, "read_only");
        let (generation, _) = basis(&mut stores);
        let declared = observed(&mut stores, "inference.tools.ToolDeclared");
        assert_eq!(declared.len(), 1);
        assert_eq!(
            declared[0].sequence,
            Some(1),
            "declared before the stream events"
        );
        assert_eq!(declared[0].payload["tool_id"], tool);
        assert_eq!(declared[0].payload["side_effect"], text("read_only"));

        let span = basis(&mut stores).1;
        let result = run(
            &mut stores,
            "inference.tools.ProposeSpeculativeToolCall",
            &[
                ("tool_id", tool),
                ("generation_id", generation),
                ("basis_span_id", span),
                ("arguments", Node::Seq(Vec::new())),
            ],
        );
        assert_eq!(
            branch(&result),
            "inference.tools.ProposeSpeculativeToolCall/proposed"
        );
        let token = result.consistency.expect("a token");
        assert_eq!(
            token.as_str(),
            format!("seq:{}", stores.stream.events().len() + 2),
            "the token counts the tools events with every other store's"
        );
        let proposals = observed(&mut stores, "inference.tools.ToolCallProposed");
        assert_eq!(
            proposals[0].sequence,
            Some(u64::try_from(stores.stream.events().len() + 2).expect("small")),
            "the proposal is placed after the stream events recorded before it"
        );
    }
}
