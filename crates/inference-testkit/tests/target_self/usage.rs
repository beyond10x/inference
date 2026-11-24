//! The tests of `tests/target/usage.rs`.

mod tests {
    use std::collections::BTreeMap;

    use ess_conformance::scenario::CommandRef;
    use ess_conformance::target::SemanticCommandRequest;
    use ess_primitives::facts::Number;
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;
    use inference_core::model::{
        AdapterDescriptor, DeclareAdapter, DeclareModel, ExecutionForm, Locality, Modality,
    };
    use inference_core::usage::UnknownReason;

    use crate::subject::Stores;
    use crate::subject::usage::{PRICE_USAGE, RECORD_USAGE, execute_command};

    fn request(quantity: BTreeMap<String, Node>) -> SemanticCommandRequest {
        SemanticCommandRequest {
            command: RECORD_USAGE.parse::<CommandRef>().expect("a command name"),
            actor: None,
            caller: None,
            input: [
                (
                    "adapter_id",
                    Node::Text("00000001-0000-4000-8000-000000000002".to_owned()),
                ),
                (
                    "model_id",
                    Node::Text("00000001-0000-4000-8000-000000000001".to_owned()),
                ),
                ("started_at_ms", Node::Number(Number::from(1_i64))),
                ("ended_at_ms", Node::Number(Number::from(2_i64))),
                ("final", Node::Bool(true)),
                ("quantities", Node::Seq(vec![Node::Map(quantity)])),
            ]
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
            correlation: CorrelationId::new("usage").expect("a correlation id"),
        }
    }

    fn quantity(fields: &[(&str, Node)]) -> BTreeMap<String, Node> {
        fields
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect()
    }

    /// Stores holding the model and the adapter [`request`] names, the first of each the model
    /// store issues.
    fn declared() -> Stores {
        let mut stores = Stores::default();
        let model = stores.model.declare_model(DeclareModel {
            family: "family".to_owned(),
            checkpoint: "checkpoint".to_owned(),
            code_license: "MIT".to_owned(),
            weight_license: "MIT".to_owned(),
            checksum: "checksum".to_owned(),
            input_modalities: vec![Modality::Text],
            output_modalities: vec![Modality::Text],
            provenance: "provenance".to_owned(),
            source_revision: "revision".to_owned(),
        });
        stores
            .model
            .declare_adapter(DeclareAdapter {
                model_id: model.model_id,
                backend: "backend".to_owned(),
                contract_revision: "1".to_owned(),
                execution_form: ExecutionForm::InProcess,
                locality: Locality::SameProcess,
                capabilities: Vec::new(),
                max_concurrency: 1,
                descriptor: AdapterDescriptor::default(),
            })
            .expect("the model is declared");
        stores
    }

    #[test]
    fn a_quantity_whose_value_and_source_disagree_is_an_error_not_an_outcome() {
        let mut stores = declared();
        let answer = execute_command(
            &mut stores,
            &request(quantity(&[
                ("kind", Node::Text("requests".to_owned())),
                ("source", Node::Text("unknown".to_owned())),
                ("value", Node::Number(Number::from(1_i64))),
            ])),
        );
        assert!(answer.is_err(), "{answer:?}");
        assert!(stores.usage.events().is_empty());
    }

    #[test]
    fn an_unknown_adapter_is_answered_before_a_quantity_whose_value_and_source_disagree() {
        let mut stores = Stores::default();
        let answer = execute_command(
            &mut stores,
            &request(quantity(&[
                ("kind", Node::Text("input_text_tokens".to_owned())),
                ("source", Node::Text("provider_reported".to_owned())),
            ])),
        )
        .expect("the request is read");
        assert_eq!(
            answer.outcome.map(|outcome| outcome.to_string()),
            Some("inference.usage.RecordUsage/unknown-adapter".to_owned())
        );
        assert!(stores.usage.events().is_empty());
    }

    #[test]
    fn a_quantity_with_an_undeclared_field_is_an_error() {
        let mut stores = Stores::default();
        let answer = execute_command(
            &mut stores,
            &request(quantity(&[
                ("kind", Node::Text("requests".to_owned())),
                ("source", Node::Text("unknown".to_owned())),
                ("unit", Node::Text("x".to_owned())),
            ])),
        );
        assert!(answer.is_err(), "{answer:?}");
    }

    /// A `PriceUsage` of `usage_record_id` whose line carries `line` and whose total carries
    /// `total`.
    fn price_request(
        usage_record_id: &str,
        line: &[(&str, Node)],
        total: &[(&str, Node)],
    ) -> SemanticCommandRequest {
        SemanticCommandRequest {
            command: PRICE_USAGE.parse::<CommandRef>().expect("a command name"),
            actor: None,
            caller: None,
            input: [
                ("usage_record_id", Node::Text(usage_record_id.to_owned())),
                ("currency", Node::Text("EUR".to_owned())),
                ("price_revision", Node::Text("1".to_owned())),
                ("price_digest", Node::Text("prices-sha256:00".to_owned())),
                ("lines", Node::Seq(vec![Node::Map(quantity(line))])),
                ("total", Node::Map(quantity(total))),
            ]
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
            correlation: CorrelationId::new("usage").expect("a correlation id"),
        }
    }

    fn outcome_of(answer: &ess_conformance::target::SemanticCommandResult) -> Option<String> {
        answer.outcome.as_ref().map(ToString::to_string)
    }

    #[test]
    fn a_minimal_price_input_without_its_optional_fields_is_read_and_its_usage_decided() {
        let mut stores = Stores::default();
        let answer = execute_command(
            &mut stores,
            &price_request(
                "00000000-0000-4000-8000-c33919ac60ea",
                &[("kind", Node::Text("input_text_tokens".to_owned()))],
                &[
                    ("known_subtotal", Node::Text("0".to_owned())),
                    ("unknown_lines", Node::Number(Number::from(1_i64))),
                ],
            ),
        )
        .expect("the request is read");
        assert_eq!(
            outcome_of(&answer),
            Some("inference.usage.PriceUsage/unknown-usage".to_owned())
        );
        assert!(stores.usage.events().is_empty());
    }

    #[test]
    fn a_cost_of_a_recorded_usage_is_priced_with_its_lines_as_sent() {
        let mut stores = declared();
        execute_command(
            &mut stores,
            &request(quantity(&[
                ("kind", Node::Text("requests".to_owned())),
                ("source", Node::Text("measured".to_owned())),
                ("value", Node::Number(Number::from(1_i64))),
            ])),
        )
        .expect("recorded");
        let answer = execute_command(
            &mut stores,
            &price_request(
                "00000007-0000-4000-8000-000000000001",
                &[
                    ("kind", Node::Text("requests".to_owned())),
                    ("quantity", Node::Number(Number::from(1_i64))),
                    ("amount", Node::Null),
                    ("unknown", Node::Text("price_unknown".to_owned())),
                ],
                &[
                    ("known_subtotal", Node::Text("0".to_owned())),
                    ("unknown_lines", Node::Number(Number::from(1_i64))),
                    ("complete_total", Node::Null),
                ],
            ),
        )
        .expect("the request is read");
        assert_eq!(
            outcome_of(&answer),
            Some("inference.usage.PriceUsage/priced".to_owned())
        );
        let cost = stores.usage.costs().iter().next().expect("stored");
        assert_eq!(cost.lines()[0].unknown, Some(UnknownReason::PriceUnknown));
        assert_eq!(cost.lines()[0].amount, None);
        assert_eq!(cost.total().complete_total, None);
    }

    #[test]
    fn a_cost_line_with_an_undeclared_field_or_a_float_amount_is_an_error() {
        for line in [
            vec![
                ("kind", Node::Text("requests".to_owned())),
                ("unit", Node::Text("request".to_owned())),
            ],
            vec![
                ("kind", Node::Text("requests".to_owned())),
                ("amount", Node::Number(Number::from(1_i64))),
            ],
        ] {
            let mut stores = Stores::default();
            let answer = execute_command(
                &mut stores,
                &price_request(
                    "00000000-0000-4000-8000-c33919ac60ea",
                    &line,
                    &[
                        ("known_subtotal", Node::Text("0".to_owned())),
                        ("unknown_lines", Node::Number(Number::from(0_i64))),
                    ],
                ),
            );
            assert!(answer.is_err(), "{line:?}: {answer:?}");
        }
    }

    #[test]
    fn an_unknown_quantity_without_a_value_is_read_and_its_adapter_decided() {
        let mut stores = Stores::default();
        let answer = execute_command(
            &mut stores,
            &request(quantity(&[
                ("kind", Node::Text("requests".to_owned())),
                ("source", Node::Text("unknown".to_owned())),
            ])),
        )
        .expect("the request is read");
        assert_eq!(
            answer.outcome.map(|outcome| outcome.to_string()),
            Some("inference.usage.RecordUsage/unknown-adapter".to_owned())
        );
    }
}
