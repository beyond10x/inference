//! The tests of the conformance target of `tests/target/`, in the one test binary that runs
//! them, `tests/target_self.rs`.
//!
//! Sixteen test binaries include `tests/target/` for its target, and cargo-nextest runs every
//! test of a binary in a process of its own. While these tests stood in the files they test,
//! each of the sixteen ran all of them. They stand here under the module paths they had there,
//! `target::graph::tests` for `tests/target/graph.rs`, and reach what they test as
//! `crate::subject`.

mod graph;
mod policy;
mod session;
mod stream;
mod tools;
mod usage;

mod wire_integers {
    use crate::subject::integer;
    use ess_primitives::facts::Number;
    use ess_primitives::node::Node;

    fn number(value: f64) -> Node {
        Node::Number(Number::new(value).expect("finite"))
    }

    #[test]
    fn an_integral_float_is_the_integer() {
        assert_eq!(integer(&number(1.0)), Some(1));
        assert_eq!(integer(&number(0.0)), Some(0));
        assert_eq!(integer(&number(-3.0)), Some(-3));
    }

    #[test]
    fn a_fractional_or_out_of_range_number_is_refused() {
        assert_eq!(integer(&number(1.5)), None);
        assert_eq!(integer(&number(9.3e18)), None);
        assert_eq!(integer(&number(-9.3e18)), None);
        assert_eq!(integer(&Node::Text("1".to_owned())), None);
    }
}

mod distinct_identities {
    use std::collections::{BTreeMap, BTreeSet};

    use ess_conformance::scenario::CommandRef;
    use ess_conformance::target::{ConformanceTarget, SemanticCommandRequest};
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;
    use inference_core::generation::StartGeneration;
    use inference_core::id::{NodeId, RunId};
    use inference_core::model::{DeclareModel, Locality, Modality};
    use inference_core::policy::{DeclareProfile, Egress, ProfileKind};
    use inference_pipeline::{DeclarePipeline, InteractionMode};
    use inference_testkit::graph::GraphCatalog;
    use uuid::Uuid;

    use crate::subject::{Stores, Target};

    fn run(
        target: &Target,
        name: &str,
        input: &[(&str, Node)],
    ) -> (String, BTreeMap<String, Node>) {
        let result = target
            .execute_command(SemanticCommandRequest {
                command: name.parse::<CommandRef>().expect("a command name"),
                actor: None,
                caller: None,
                input: input
                    .iter()
                    .map(|(field, value)| ((*field).to_owned(), value.clone()))
                    .collect(),
                correlation: CorrelationId::new("distinct-identities").expect("a correlation id"),
            })
            .unwrap_or_else(|error| panic!("`{name}` answers: {error}"));
        let payload = result
            .direct_events
            .first()
            .map(|event| event.payload.clone().into_iter().collect())
            .unwrap_or_default();
        (
            result
                .outcome
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            payload,
        )
    }

    fn text(value: &str) -> Node {
        Node::Text(value.to_owned())
    }

    #[test]
    fn a_profile_identity_sent_where_a_pipeline_identity_belongs_names_no_pipeline() {
        let target = Target::default();
        let (_, profile) = run(
            &target,
            "inference.policy.DeclareProfile",
            &[
                ("kind", text("offline_strict")),
                ("egress", text("denied")),
                ("admitted", Node::Seq(Vec::new())),
            ],
        );
        let profile_id = profile["profile_id"].clone();
        let (declared, _) = run(
            &target,
            "inference.graph.DeclarePipeline",
            &[
                ("name", text("voice")),
                ("schema_revision", text("1")),
                ("mode", text("batch")),
                ("profile_id", profile_id.clone()),
                ("required_capabilities", Node::Seq(Vec::new())),
            ],
        );
        assert_eq!(declared, "inference.graph.DeclarePipeline/declared");

        let (answered, _) = run(
            &target,
            "inference.graph.AddNode",
            &[
                ("pipeline_id", profile_id),
                ("name", text("llm")),
                ("role", text("model")),
                ("requires", Node::Seq(Vec::new())),
            ],
        );
        assert_eq!(
            answered, "inference.graph.AddNode/unknown-pipeline",
            "a profile's identity is no pipeline's"
        );
    }

    #[test]
    fn no_two_domains_issue_the_same_identity() {
        let mut stores = Stores::default();
        let model = stores
            .model
            .declare_model(DeclareModel {
                family: "f".to_owned(),
                checkpoint: "c".to_owned(),
                code_license: "l".to_owned(),
                weight_license: "w".to_owned(),
                checksum: "s".to_owned(),
                input_modalities: vec![Modality::Text],
                output_modalities: vec![Modality::Text],
                provenance: "p".to_owned(),
                source_revision: "r".to_owned(),
            })
            .model_id;
        let profile = stores
            .policy
            .declare_profile(DeclareProfile {
                kind: ProfileKind::OfflineStrict,
                egress: Egress::Denied,
                admitted: vec![Locality::SameProcess],
            })
            .expect("a row of the table")
            .profile_id;
        let catalog = GraphCatalog {
            policy: &stores.policy,
            model: &stores.model,
        };
        let pipeline = stores
            .graph
            .declare_pipeline(
                DeclarePipeline {
                    name: "voice".to_owned(),
                    schema_revision: "1".to_owned(),
                    mode: InteractionMode::Batch,
                    profile_id: profile,
                    required_capabilities: Vec::new(),
                },
                &catalog,
            )
            .expect("the profile is declared")
            .pipeline_id;
        let generation = stores
            .stream
            .start_generation(StartGeneration {
                run_id: RunId::from(Uuid::from_u128(1)),
                node_id: NodeId::from(Uuid::from_u128(2)),
                epoch: 1,
            })
            .generation_id;

        let first = [
            ("inference.model", model.to_string()),
            ("inference.policy", profile.to_string()),
            ("inference.graph", pipeline.to_string()),
            ("inference.stream", generation.to_string()),
        ];
        let distinct: BTreeSet<&str> = first.iter().map(|(_, id)| id.as_str()).collect();
        assert_eq!(
            distinct.len(),
            first.len(),
            "each domain's first identity is its own: {first:#?}"
        );
    }
}

mod one_sequence {
    use std::collections::BTreeMap;

    use ess_conformance::scenario::{CommandRef, ViewRef};
    use ess_conformance::target::{
        ConformanceTarget, Deadline, SemanticCommandRequest, SemanticViewRequest, TargetError,
    };
    use ess_primitives::consistency::{ConsistencyToken, QueryConsistency};
    use ess_primitives::facts::Number;
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;
    use ess_primitives::time::Timestamp;
    use inference_core::generation::StartGeneration;
    use inference_core::id::{NodeId, RunId};
    use uuid::Uuid;

    use crate::subject::Target;

    fn correlation() -> CorrelationId {
        CorrelationId::new("one-sequence").expect("a correlation id")
    }

    fn at(view: &str, token: ConsistencyToken) -> SemanticViewRequest {
        SemanticViewRequest {
            view: view.parse::<ViewRef>().expect("a view name"),
            params: BTreeMap::new(),
            consistency: QueryConsistency::at_least(token),
            correlation: correlation(),
            deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
        }
    }

    #[test]
    fn a_model_view_read_at_a_stream_commands_token_is_answered() {
        let target = Target::default();
        let started = target
            .stores
            .borrow_mut()
            .stream
            .start_generation(StartGeneration {
                run_id: RunId::from(Uuid::from_u128(1)),
                node_id: NodeId::from(Uuid::from_u128(2)),
                epoch: 1,
            });
        let delta = target
            .execute_command(SemanticCommandRequest {
                command: "inference.stream.EmitTextDelta"
                    .parse::<CommandRef>()
                    .expect("a command name"),
                actor: None,
                caller: None,
                input: [
                    (
                        "generation_id".to_owned(),
                        Node::Text(started.generation_id.to_string()),
                    ),
                    ("sequence".to_owned(), Node::Number(Number::from(1_i64))),
                    ("text".to_owned(), Node::Text("hello".to_owned())),
                ]
                .into_iter()
                .collect(),
                correlation: correlation(),
            })
            .expect("EmitTextDelta answers");
        let token = delta.consistency.expect("a command answers a token");
        assert_eq!(
            token.as_str(),
            "seq:2",
            "every event any store recorded counts"
        );

        let models = target.query_view(at("inference.model.Models", token));
        assert!(
            models.is_ok_and(|answer| answer.rows.is_empty()),
            "a model view at a stream token"
        );

        let ahead = ConsistencyToken::new("seq:3").expect("a token");
        let unreached = target.query_view(at("inference.stream.Segments", ahead));
        assert!(
            matches!(unreached, Err(TargetError::Unavailable { .. })),
            "{unreached:?}"
        );
    }
}
