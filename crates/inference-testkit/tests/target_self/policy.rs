//! The tests of `tests/target/policy.rs`.

mod tests {
    use std::collections::BTreeMap;

    use ess_conformance::scenario::{CommandRef, ViewRef};
    use ess_conformance::target::{
        Deadline, SemanticCommandRequest, SemanticCommandResult, SemanticViewRequest, TargetError,
    };
    use ess_primitives::consistency::QueryConsistency;
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;
    use ess_primitives::time::Timestamp;

    use crate::subject::Stores;
    use crate::subject::policy::{execute_command, query_view};

    fn text(value: &str) -> Node {
        Node::Text(value.to_owned())
    }

    fn list(values: &[&str]) -> Node {
        Node::Seq(values.iter().map(|value| text(value)).collect())
    }

    fn declare(
        stores: &mut Stores,
        kind: &str,
        egress: &str,
        admitted: &[&str],
    ) -> Result<SemanticCommandResult, TargetError> {
        execute_command(
            stores,
            &SemanticCommandRequest {
                command: "inference.policy.DeclareProfile"
                    .parse::<CommandRef>()
                    .expect("a command name"),
                actor: None,
                caller: None,
                input: [
                    ("kind".to_owned(), text(kind)),
                    ("egress".to_owned(), text(egress)),
                    ("admitted".to_owned(), list(admitted)),
                ]
                .into_iter()
                .collect(),
                correlation: CorrelationId::new("policy").expect("a correlation id"),
            },
        )
    }

    fn profiles(stores: &mut Stores) -> Vec<BTreeMap<String, Node>> {
        query_view(
            stores,
            &SemanticViewRequest {
                view: "inference.policy.Profiles"
                    .parse::<ViewRef>()
                    .expect("a view name"),
                params: BTreeMap::new(),
                consistency: QueryConsistency::Current,
                correlation: CorrelationId::new("policy").expect("a correlation id"),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .expect("Profiles reads")
        .rows
    }

    #[test]
    fn each_rule_of_the_table_is_its_own_outcome_carrying_the_row_sent() {
        let mut stores = Stores::default();
        for (kind, egress, admitted, outcome) in [
            (
                "hybrid_opt_in",
                "denied",
                &["external_provider"][..],
                "wrong-egress",
            ),
            (
                "self_hosted_cluster",
                "authenticated_internal",
                &["external_provider"][..],
                "remote-not-opted-in",
            ),
            (
                "offline_strict",
                "denied",
                &["local_worker", "self_hosted_cluster"][..],
                "offline-admits-cluster",
            ),
        ] {
            let result = declare(&mut stores, kind, egress, admitted).expect("answered");
            assert_eq!(
                result.outcome.as_ref().map(ToString::to_string),
                Some(format!("inference.policy.DeclareProfile/{outcome}"))
            );
            let error = result.error.as_ref().expect("an error");
            assert_eq!(
                error.error.to_string(),
                "inference.policy.ProfileOutsideTable"
            );
            assert_eq!(error.fields["kind"], text(kind));
            assert_eq!(error.fields["egress"], text(egress));
            assert_eq!(error.fields["admitted"], list(admitted));
            assert!(result.direct_events.is_empty());
        }
        assert!(profiles(&mut stores).is_empty());
    }

    #[test]
    fn a_declared_profile_is_an_active_row_and_an_undeclared_member_is_an_error() {
        let mut stores = Stores::default();
        let declared = declare(
            &mut stores,
            "self_hosted_lan",
            "internal_allowlist",
            &["same_process"],
        )
        .expect("answered");
        let profile_id = declared.direct_events[0].payload["profile_id"].clone();
        assert_eq!(profile_id, text("00000002-0000-4000-8000-000000000001"));

        let unknown = declare(
            &mut stores,
            "self_hosted_lan",
            "internal_allowlist",
            &["moon"],
        );
        assert!(
            matches!(&unknown, Err(TargetError::Unavailable { .. })),
            "{unknown:?}"
        );

        let rows = profiles(&mut stores);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["profile_id"], profile_id);
        assert_eq!(rows[0]["kind"], text("self_hosted_lan"));
        assert_eq!(rows[0]["egress"], text("internal_allowlist"));
        assert_eq!(rows[0]["admitted"], list(&["same_process"]));
        assert_eq!(rows[0]["state"], text("Active"));
        assert_eq!(rows[0].len(), 5);
    }
}
