//! Adversary pass 1 against `inference.policy.DeclareProfile`: every row a caller can send.
//!
//! The cases of `policy_vocabulary.rs` send an `admitted` list of one locality, or one list of two
//! with the external provider first. Here every kind, every egress and every subset of the four
//! localities, in both orders, is decided by `DeploymentProfile::declare` and by the outcomes of
//! `ess/domains/policy.yaml` transcribed as written: the first outcome whose `when` holds, in the
//! order the file lists them, and `declared` when none does.

use inference_core::model::Locality;
use inference_core::policy::{
    DeclareProfile, DeclareProfileError, DeploymentProfile, Egress, ProfileKind,
    ProfileOutsideTable, TableRule,
};

/// The outcome policy.yaml names for the row, `None` for `declared`.
fn outcome_of_the_yaml(
    kind: ProfileKind,
    egress: Egress,
    admitted: &[Locality],
) -> Option<TableRule> {
    use Egress::{AuthenticatedInternal, Denied, InternalAllowlist, PerProviderAllowlist};
    use ProfileKind::{HybridOptIn, OfflineStrict, SelfHostedCluster, SelfHostedLan};

    let wrong_egress = (kind == OfflineStrict && egress != Denied)
        || (kind == SelfHostedLan && egress != InternalAllowlist)
        || (kind == SelfHostedCluster && egress != AuthenticatedInternal)
        || (kind == HybridOptIn && egress != PerProviderAllowlist);
    let remote_not_opted_in = kind != HybridOptIn && admitted.contains(&Locality::ExternalProvider);
    let offline_admits_cluster =
        kind == OfflineStrict && admitted.contains(&Locality::SelfHostedCluster);

    if wrong_egress {
        Some(TableRule::WrongEgress)
    } else if remote_not_opted_in {
        Some(TableRule::RemoteNotOptedIn)
    } else if offline_admits_cluster {
        Some(TableRule::OfflineAdmitsCluster)
    } else {
        None
    }
}

/// Every subset of the localities, each in declaration order and reversed.
fn admitted_lists() -> Vec<Vec<Locality>> {
    let mut lists = Vec::new();
    for mask in 0_u8..16 {
        let subset: Vec<Locality> = Locality::ALL
            .into_iter()
            .enumerate()
            .filter(|(bit, _)| mask & (1 << bit) != 0)
            .map(|(_, locality)| locality)
            .collect();
        let mut reversed = subset.clone();
        reversed.reverse();
        lists.push(subset);
        lists.push(reversed);
    }
    lists
}

#[test]
fn every_row_is_decided_as_policy_yaml_decides_it() {
    let mut decided = 0;
    for kind in ProfileKind::ALL {
        for egress in Egress::ALL {
            for admitted in admitted_lists() {
                let row = DeclareProfile {
                    kind,
                    egress,
                    admitted: admitted.clone(),
                };
                let answer = DeploymentProfile::declare(row);
                match outcome_of_the_yaml(kind, egress, &admitted) {
                    None => {
                        let (profile, event) = answer.unwrap_or_else(|error| {
                            panic!("{kind:?} {egress:?} {admitted:?} is a row: {error}")
                        });
                        assert_eq!(profile.admitted(), admitted.as_slice());
                        assert_eq!(event.admitted, admitted);
                    }
                    Some(rule) => assert_eq!(
                        answer.map(|_| ()),
                        Err(DeclareProfileError {
                            rule,
                            error: ProfileOutsideTable {
                                kind,
                                egress,
                                admitted: admitted.clone(),
                            },
                        }),
                        "{kind:?} {egress:?} {admitted:?}"
                    ),
                }
                decided += 1;
            }
        }
    }
    assert_eq!(decided, 4 * 4 * 32);
}
