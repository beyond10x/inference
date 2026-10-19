//! The `inference.policy` vocabulary of `ess/domains/policy.yaml`: the deployment-profile table,
//! the order of its refusals and the ESS names.

use inference_core::model::Locality;
use inference_core::policy::{
    DeclareProfile, DeclareProfileError, DeploymentProfile, Egress, ProfileId, ProfileKind,
    ProfileNotFound, ProfileOutsideTable, ProfileState, TableRule,
};
use serde_json::json;
use uuid::Uuid;

fn declare(kind: ProfileKind, egress: Egress, admitted: &[Locality]) -> DeclareProfile {
    DeclareProfile {
        kind,
        egress,
        admitted: admitted.to_vec(),
    }
}

#[test]
fn every_kind_with_its_table_egress_declares_a_profile_and_its_event() {
    let local = [Locality::SameProcess, Locality::LocalWorker];
    for kind in ProfileKind::ALL {
        let profile_id = ProfileId::from(Uuid::from_u128(7));
        let (profile, event) =
            DeploymentProfile::declare_as(profile_id, declare(kind, kind.egress(), &local))
                .unwrap_or_else(|error| panic!("{kind:?} with its table egress: {error}"));
        assert_eq!(profile.profile_id(), profile_id);
        assert_eq!(profile.kind(), kind);
        assert_eq!(profile.egress(), kind.egress());
        assert_eq!(profile.admitted(), local);
        assert_eq!(profile.state(), ProfileState::Active);
        assert_eq!(event.profile_id, profile_id);
        assert_eq!(event.kind, kind);
        assert_eq!(event.egress, kind.egress());
        assert_eq!(event.admitted, local);
    }
}

#[test]
fn the_table_gives_one_egress_per_kind() {
    assert_eq!(ProfileKind::OfflineStrict.egress(), Egress::Denied);
    assert_eq!(
        ProfileKind::SelfHostedLan.egress(),
        Egress::InternalAllowlist
    );
    assert_eq!(
        ProfileKind::SelfHostedCluster.egress(),
        Egress::AuthenticatedInternal
    );
    assert_eq!(
        ProfileKind::HybridOptIn.egress(),
        Egress::PerProviderAllowlist
    );
}

#[test]
fn every_other_egress_is_refused_as_wrong_egress_with_the_row_sent() {
    for kind in ProfileKind::ALL {
        for egress in Egress::ALL
            .into_iter()
            .filter(|egress| *egress != kind.egress())
        {
            let admitted = [Locality::SameProcess];
            assert_eq!(
                DeploymentProfile::declare(declare(kind, egress, &admitted)),
                Err(DeclareProfileError {
                    rule: TableRule::WrongEgress,
                    error: ProfileOutsideTable {
                        kind,
                        egress,
                        admitted: admitted.to_vec(),
                    },
                }),
                "{kind:?} with {egress:?}"
            );
        }
    }
}

#[test]
fn only_hybrid_opt_in_admits_an_external_provider() {
    for kind in ProfileKind::ALL {
        let result =
            DeploymentProfile::declare(declare(kind, kind.egress(), &[Locality::ExternalProvider]));
        if kind == ProfileKind::HybridOptIn {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert_eq!(
                result.map(|_| ()).map_err(|error| error.rule),
                Err(TableRule::RemoteNotOptedIn),
                "{kind:?}"
            );
        }
    }
}

#[test]
fn only_offline_strict_refuses_a_self_hosted_cluster() {
    for kind in ProfileKind::ALL {
        let result = DeploymentProfile::declare(declare(
            kind,
            kind.egress(),
            &[Locality::SelfHostedCluster],
        ));
        if kind == ProfileKind::OfflineStrict {
            assert_eq!(
                result.map(|_| ()).map_err(|error| error.rule),
                Err(TableRule::OfflineAdmitsCluster)
            );
        } else {
            assert!(result.is_ok(), "{kind:?}: {result:?}");
        }
    }
}

#[test]
fn the_rules_are_decided_in_the_order_policy_yaml_lists_them() {
    let both = [Locality::ExternalProvider, Locality::SelfHostedCluster];
    assert_eq!(
        TableRule::broken_by(ProfileKind::OfflineStrict, Egress::InternalAllowlist, &both),
        Some(TableRule::WrongEgress)
    );
    assert_eq!(
        TableRule::broken_by(ProfileKind::OfflineStrict, Egress::Denied, &both),
        Some(TableRule::RemoteNotOptedIn)
    );
    assert_eq!(
        TableRule::broken_by(ProfileKind::OfflineStrict, Egress::Denied, &[]),
        None
    );
    assert_eq!(TableRule::WrongEgress.outcome(), "wrong-egress");
    assert_eq!(TableRule::RemoteNotOptedIn.outcome(), "remote-not-opted-in");
    assert_eq!(
        TableRule::OfflineAdmitsCluster.outcome(),
        "offline-admits-cluster"
    );
}

#[test]
fn values_serialize_as_the_ess_names() {
    assert_eq!(
        serde_json::to_value(ProfileKind::ALL).expect("serializes"),
        json!([
            "offline_strict",
            "self_hosted_lan",
            "self_hosted_cluster",
            "hybrid_opt_in"
        ])
    );
    assert_eq!(
        serde_json::to_value(Egress::ALL)
            .expect("serializes")
            .as_array()
            .cloned()
            .expect("a list"),
        [
            json!("denied"),
            json!("internal_allowlist"),
            json!("authenticated_internal"),
            json!("per_provider_allowlist")
        ]
    );
    assert_eq!(
        serde_json::to_value(ProfileState::Active).expect("serializes"),
        json!("Active")
    );
    let profile_id = ProfileId::from(Uuid::from_u128(1));
    assert_eq!(
        serde_json::to_value(ProfileNotFound { profile_id }).expect("serializes"),
        json!({ "profile_id": "00000000-0000-0000-0000-000000000001" })
    );
    assert_eq!(
        serde_json::to_value(ProfileOutsideTable {
            kind: ProfileKind::SelfHostedLan,
            egress: Egress::Denied,
            admitted: vec![Locality::LocalWorker],
        })
        .expect("serializes"),
        json!({ "kind": "self_hosted_lan", "egress": "denied", "admitted": ["local_worker"] })
    );
}
