//! The `inference.policy` vocabulary of `ess/domains/policy.yaml`: which compute a pipeline may
//! admit and what its default egress is.
//!
//! A [`DeploymentProfile`] is one row of the deployment-profile table: a [`ProfileKind`], the
//! [`Egress`] the table gives that kind, and the [`Locality`] members it admits. It is created only
//! through its declaring command, [`DeploymentProfile::declare`], which returns the profile and the
//! [`ProfileDeclared`] event, or refuses a row outside the table with [`ProfileOutsideTable`] and
//! the [`TableRule`] the row breaks. Only a `hybrid_opt_in` profile admits an external provider.
//! Serialized names are the ESS names.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::uuid_newtype;
use crate::model::Locality;

uuid_newtype!(
    /// `inference.policy.ProfileId`: identifies a [`DeploymentProfile`].
    ProfileId
);

/// `inference.policy.ProfileKind`: the four deployment profiles of the design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// In-process or local workers only; no egress.
    OfflineStrict,
    /// The local machine and approved internal services.
    SelfHostedLan,
    /// Approved owned infrastructure.
    SelfHostedCluster,
    /// Local compute plus explicitly enabled remote services; the only kind that admits an
    /// external provider.
    HybridOptIn,
}

impl ProfileKind {
    /// Every kind, in the order `ess/domains/policy.yaml` lists them.
    pub const ALL: [Self; 4] = [
        Self::OfflineStrict,
        Self::SelfHostedLan,
        Self::SelfHostedCluster,
        Self::HybridOptIn,
    ];

    /// The default egress the deployment-profile table gives this kind.
    #[must_use]
    pub const fn egress(self) -> Egress {
        match self {
            Self::OfflineStrict => Egress::Denied,
            Self::SelfHostedLan => Egress::InternalAllowlist,
            Self::SelfHostedCluster => Egress::AuthenticatedInternal,
            Self::HybridOptIn => Egress::PerProviderAllowlist,
        }
    }
}

/// `inference.policy.Egress`: the default egress column of the deployment-profile table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Egress {
    Denied,
    InternalAllowlist,
    AuthenticatedInternal,
    PerProviderAllowlist,
}

impl Egress {
    /// Every egress, in the order `ess/domains/policy.yaml` lists them.
    pub const ALL: [Self; 4] = [
        Self::Denied,
        Self::InternalAllowlist,
        Self::AuthenticatedInternal,
        Self::PerProviderAllowlist,
    ];
}

/// `inference.policy.DeploymentProfile.State`: a declared profile is `Active` and stays so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProfileState {
    Active,
}

/// `inference.policy.DeclareProfile`: the input that declares a [`DeploymentProfile`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclareProfile {
    pub kind: ProfileKind,
    pub egress: Egress,
    pub admitted: Vec<Locality>,
}

/// `inference.policy.ProfileDeclared`: a profile was declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDeclared {
    pub profile_id: ProfileId,
    pub kind: ProfileKind,
    pub egress: Egress,
    pub admitted: Vec<Locality>,
}

/// The rule of the deployment-profile table a row breaks; each is one refusing outcome of
/// `inference.policy.DeclareProfile`, and they are decided in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TableRule {
    /// `wrong-egress`: the egress is not the table's default for the kind.
    WrongEgress,
    /// `remote-not-opted-in`: a kind other than `hybrid_opt_in` admits an external provider.
    RemoteNotOptedIn,
    /// `offline-admits-cluster`: an `offline_strict` profile admits a self-hosted cluster.
    OfflineAdmitsCluster,
}

impl TableRule {
    /// The first rule the row breaks, or none for a row of the table.
    #[must_use]
    pub fn broken_by(kind: ProfileKind, egress: Egress, admitted: &[Locality]) -> Option<Self> {
        if egress != kind.egress() {
            return Some(Self::WrongEgress);
        }
        if kind != ProfileKind::HybridOptIn && admitted.contains(&Locality::ExternalProvider) {
            return Some(Self::RemoteNotOptedIn);
        }
        if kind == ProfileKind::OfflineStrict && admitted.contains(&Locality::SelfHostedCluster) {
            return Some(Self::OfflineAdmitsCluster);
        }
        None
    }

    /// The name of the outcome this rule refuses with, as `ess/domains/policy.yaml` writes it.
    #[must_use]
    pub const fn outcome(self) -> &'static str {
        match self {
            Self::WrongEgress => "wrong-egress",
            Self::RemoteNotOptedIn => "remote-not-opted-in",
            Self::OfflineAdmitsCluster => "offline-admits-cluster",
        }
    }
}

/// `inference.policy.ProfileOutsideTable`: the kind, default egress and admitted compute do not
/// form a row of the deployment-profile table, so no profile was declared. The fields are the
/// ones that were sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileOutsideTable {
    pub kind: ProfileKind,
    pub egress: Egress,
    pub admitted: Vec<Locality>,
}

impl fmt::Display for ProfileOutsideTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} with egress {:?} admitting {:?} is not a row of the deployment-profile table",
            self.kind, self.egress, self.admitted
        )
    }
}

impl Error for ProfileOutsideTable {}

/// Why `inference.policy.DeclareProfile` declared nothing: the [`TableRule`] the row breaks, which
/// names the outcome, and the [`ProfileOutsideTable`] every one of those outcomes carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclareProfileError {
    pub rule: TableRule,
    pub error: ProfileOutsideTable,
}

impl fmt::Display for DeclareProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.rule.outcome(), self.error)
    }
}

impl Error for DeclareProfileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.error)
    }
}

/// `inference.policy.ProfileNotFound`: no deployment profile carries the identity a command named.
/// `profile_id` is the identity that was sent. Only a store holding the declared profiles can
/// decide it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileNotFound {
    pub profile_id: ProfileId,
}

impl fmt::Display for ProfileNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no deployment profile carries the identity {}",
            self.profile_id
        )
    }
}

impl Error for ProfileNotFound {}

/// `inference.policy.DeploymentProfile`: the compute a pipeline admits and its default egress. A
/// pipeline references it by `profile_id`; the profile stands on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentProfile {
    profile_id: ProfileId,
    kind: ProfileKind,
    egress: Egress,
    admitted: Vec<Locality>,
}

impl DeploymentProfile {
    /// Runs `inference.policy.DeclareProfile`.
    ///
    /// # Errors
    ///
    /// [`DeclareProfileError`] naming the first [`TableRule`] the row breaks; nothing is declared.
    pub fn declare(input: DeclareProfile) -> Result<(Self, ProfileDeclared), DeclareProfileError> {
        Self::declare_as(ProfileId::generate(), input)
    }

    /// Runs `inference.policy.DeclareProfile` under `profile_id` instead of a generated one, and
    /// otherwise as [`DeploymentProfile::declare`]. It is for a store that assigns the identities
    /// itself, such as the counter of `inference-testkit`.
    ///
    /// # Errors
    ///
    /// [`DeclareProfileError`] naming the first [`TableRule`] the row breaks; nothing is declared
    /// and `profile_id` is not used.
    pub fn declare_as(
        profile_id: ProfileId,
        input: DeclareProfile,
    ) -> Result<(Self, ProfileDeclared), DeclareProfileError> {
        if let Some(rule) = TableRule::broken_by(input.kind, input.egress, &input.admitted) {
            return Err(DeclareProfileError {
                rule,
                error: ProfileOutsideTable {
                    kind: input.kind,
                    egress: input.egress,
                    admitted: input.admitted,
                },
            });
        }
        let event = ProfileDeclared {
            profile_id,
            kind: input.kind,
            egress: input.egress,
            admitted: input.admitted.clone(),
        };
        let profile = Self {
            profile_id,
            kind: input.kind,
            egress: input.egress,
            admitted: input.admitted,
        };
        Ok((profile, event))
    }

    #[must_use]
    pub const fn profile_id(&self) -> ProfileId {
        self.profile_id
    }

    #[must_use]
    pub const fn kind(&self) -> ProfileKind {
        self.kind
    }

    /// Always the table's default for [`DeploymentProfile::kind`].
    #[must_use]
    pub const fn egress(&self) -> Egress {
        self.egress
    }

    #[must_use]
    pub fn admitted(&self) -> &[Locality] {
        &self.admitted
    }

    /// Always [`ProfileState::Active`]: a profile has one state.
    #[must_use]
    pub const fn state(&self) -> ProfileState {
        ProfileState::Active
    }
}
