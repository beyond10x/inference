//! The `inference.policy` domain of `ess/domains/policy.yaml`, held in memory.
//!
//! [`PolicyDomain`] holds the declared deployment profiles and records every event it emits in
//! order. `DeclareProfile` runs [`DeploymentProfile::declare_as`] of `inference-core` under an
//! identity the domain issues from a counter rendered as a Uuid, so that two runs of the same
//! commands report the same identities; a refused row takes none. The view
//! `inference.policy.Profiles` is [`PolicyDomain::profiles`], read at the call, so a write is
//! visible to the next read.

use uuid::Uuid;

use inference_core::policy::{
    DeclareProfile, DeclareProfileError, DeploymentProfile, ProfileDeclared, ProfileId,
};

/// An event of `inference.policy`, as the domain emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyEvent {
    /// `inference.policy.ProfileDeclared`.
    ProfileDeclared(ProfileDeclared),
}

/// The `inference.policy` domain in memory: the declared profiles and the events it emitted.
#[derive(Debug, Clone, Default)]
pub struct PolicyDomain {
    profiles: Vec<DeploymentProfile>,
    events: Vec<PolicyEvent>,
    issued: u128,
}

impl PolicyDomain {
    /// An empty domain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `inference.policy.DeclareProfile`: stores the profile
    /// [`DeploymentProfile::declare_as`] returns and records its [`ProfileDeclared`].
    ///
    /// # Errors
    ///
    /// [`DeclareProfileError`] naming the rule of the table the row breaks; nothing is stored, no
    /// event is recorded and no identity is taken.
    pub fn declare_profile(
        &mut self,
        input: DeclareProfile,
    ) -> Result<ProfileDeclared, DeclareProfileError> {
        let profile_id = ProfileId::from(identity(self.issued + 1));
        let (profile, event) = DeploymentProfile::declare_as(profile_id, input)?;
        self.issued += 1;
        self.profiles.push(profile);
        self.events
            .push(PolicyEvent::ProfileDeclared(event.clone()));
        Ok(event)
    }

    /// The profile carrying `profile_id`, if one was declared.
    #[must_use]
    pub fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.profile_id() == profile_id)
    }

    /// The view `inference.policy.Profiles`: every declared profile, in declaration order.
    pub fn profiles(&self) -> impl Iterator<Item = &DeploymentProfile> {
        self.profiles.iter()
    }

    /// Every event emitted, in the order it was emitted.
    #[must_use]
    pub fn events(&self) -> &[PolicyEvent] {
        &self.events
    }
}

/// The `count`th identity: a version 4, RFC 4122 variant Uuid whose low bits are `count`,
/// `00000000-0000-4000-8000-000000000001` first.
const fn identity(count: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | count)
}

#[cfg(test)]
mod tests {
    use inference_core::model::Locality;
    use inference_core::policy::{DeclareProfile, Egress, ProfileKind, ProfileState, TableRule};

    use super::{PolicyDomain, PolicyEvent};

    fn declare(kind: ProfileKind, egress: Egress) -> DeclareProfile {
        DeclareProfile {
            kind,
            egress,
            admitted: vec![Locality::SameProcess],
        }
    }

    #[test]
    fn identities_come_from_a_counter_and_a_refusal_takes_none() {
        let mut domain = PolicyDomain::new();
        let first = domain
            .declare_profile(declare(ProfileKind::OfflineStrict, Egress::Denied))
            .expect("a row of the table");
        let refused = domain.declare_profile(declare(ProfileKind::SelfHostedLan, Egress::Denied));
        let declared = domain
            .declare_profile(declare(
                ProfileKind::SelfHostedLan,
                Egress::InternalAllowlist,
            ))
            .expect("a row of the table");

        assert_eq!(
            first.profile_id.to_string(),
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(
            refused.map(|_| ()).map_err(|error| error.rule),
            Err(TableRule::WrongEgress)
        );
        assert_eq!(
            declared.profile_id.to_string(),
            "00000000-0000-4000-8000-000000000002"
        );
        assert_eq!(domain.profiles().count(), 2);
        assert_eq!(domain.events().len(), 2);
    }

    #[test]
    fn a_declared_profile_is_stored_recorded_and_active() {
        let mut domain = PolicyDomain::new();
        let event = domain
            .declare_profile(declare(
                ProfileKind::HybridOptIn,
                Egress::PerProviderAllowlist,
            ))
            .expect("a row of the table");

        let profile = domain.profile(event.profile_id).expect("stored");
        assert_eq!(profile.kind(), ProfileKind::HybridOptIn);
        assert_eq!(profile.egress(), Egress::PerProviderAllowlist);
        assert_eq!(profile.admitted(), [Locality::SameProcess]);
        assert_eq!(profile.state(), ProfileState::Active);
        assert_eq!(domain.events(), [PolicyEvent::ProfileDeclared(event)]);
    }
}
