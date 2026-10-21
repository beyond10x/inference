//! The usage view of a live session: what its model calls consumed, per run and over the session.
//!
//! A live session adds every `inference.usage.UsageRecorded` it reads off an egress of its run to
//! a [`UsageTotal`] of that run, the run the envelope travelled on, and to one of the session
//! ([`SessionUsage`], read through [`crate::live::LiveSession::usage`]). It keeps a total for each
//! run it held, in the order it attached them, one with no record among them, and the session's
//! total spans them all, across [`crate::live::LiveSession::switch`]. A record is counted when
//! the session reads it, whether or not it is delivered: an interrupt does not cut it, and a
//! cancelled session, which delivers nothing more, still counts what it reads. Unknown quantities
//! stay unknown and a record whose `final` is false counts as partial, so neither total reads as
//! complete over them ([`UsageTotal`]). Usage travels on the run's egresses, the reliable path of
//! committed facts, so it never depends on telemetry, which drops what its consumer does not
//! take.
//!
//! Scope: pipelines whose nodes are not subgraph nodes. Whether the total of a run includes the
//! usage of the child runs of its subgraph nodes is not decided here; a record of a child run is
//! counted under the run whose egress carried it.

use inference_core::id::RunId;
use inference_core::usage::{UsageRecorded, UsageTotal};

/// The usage a live session counted: a total per run it held, in the order it attached them, and
/// one over the session; with the feature `cost`, what that usage cost.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionUsage {
    /// Each run the session held and the total of the usage read off its egresses.
    pub runs: Vec<(RunId, UsageTotal)>,
    /// The total over every run the session held.
    pub session: UsageTotal,
    /// The cost record of each usage record the session priced and their total, beside the
    /// session's usage total ([`crate::cost`]).
    #[cfg(feature = "cost")]
    pub cost: crate::cost::SessionCost,
}

impl SessionUsage {
    /// The total of the run `run_id`; none for a run the session did not hold.
    #[must_use]
    pub fn run(&self, run_id: RunId) -> Option<&UsageTotal> {
        self.runs
            .iter()
            .find(|(held, _)| *held == run_id)
            .map(|(_, total)| total)
    }

    /// The session holds the run `run_id`: it has a total from now on, of no record yet.
    pub(crate) fn hold(&mut self, run_id: RunId) {
        self.total_of(run_id);
    }

    /// Adds `recorded`, read off an egress of the run `run_id`, to that run's total and the
    /// session's.
    pub(crate) fn add(&mut self, run_id: RunId, recorded: &UsageRecorded) {
        self.total_of(run_id)
            .add(&recorded.quantities, recorded.r#final);
        self.session.add(&recorded.quantities, recorded.r#final);
    }

    /// The total of the run `run_id`, a new one of no record if the session had none.
    fn total_of(&mut self, run_id: RunId) -> &mut UsageTotal {
        let at = match self.runs.iter().position(|(held, _)| *held == run_id) {
            Some(at) => at,
            None => {
                self.runs.push((run_id, UsageTotal::new()));
                self.runs.len() - 1
            }
        };
        &mut self.runs[at].1
    }
}
