//! The usage view of a live session: what its model calls consumed, per run and over the session.
//!
//! A live session adds every `inference.usage.UsageRecorded` it reads off its run, off an egress
//! or off the sink of a `usage` port no edge leaves (below), to a [`UsageTotal`] of that run, the
//! run the envelope travelled on, and to one of the session
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
//! A run's total includes the usage of the child runs of its subgraph nodes. A subgraph node
//! forwards every usage record of its child run on its own `usage` port, the record of a turn a
//! cancel cut among them, since a cancel
//! does not end the forwarding of a usage egress before the child run is terminal and the egress
//! closes; the record crosses to the parent run as any envelope does
//! (`inference_runtime::SubgraphNode`), so the session reads it off the run it holds and counts
//! it there, once, in that run's total and the session's. The record's own `run_id` still names
//! the child run it was made in, so the child's total stays derivable from the records; the
//! session keeps no total of a run it did not hold, so no record of a child run is counted twice.
//!
//! Usage counts whether or not an edge carries it (`story:usage-unconnected-port-counted`). A
//! model node's `usage` port, or a subgraph node's, that no edge leaves and no exposure carries is
//! handed a sink by the run (`inference_runtime::usage_sink`); the session reads every sink of
//! its run with its egresses and counts each record there in the run's total and the session's,
//! and delivers none, since no exposure carries it. A sink is a port no edge leaves, so no record
//! read off one also reaches an egress, and each is counted once. A child's `usage` port with no
//! edge rolls up through its subgraph node's `usage` port the same way. Only a subgraph node with
//! no `usage` port, whose child exposes no usage egress while one of its ports holds a sink, has
//! no way to pass that usage on; the session marks the run's total and its own partial, naming
//! the node ([`UsageTotal::uncounted`], `inference_runtime::uncounted_usage`), so that neither
//! reads as complete.

use inference_core::id::{NodeId, RunId};
use inference_core::usage::{UsageRecorded, UsageTotal};

/// The usage a live session counted: a total per run it held, in the order it attached them, and
/// one over the session; with the feature `cost`, what that usage cost.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionUsage {
    /// Each run the session held and the total of the usage read off its egresses and its usage
    /// sinks.
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

    /// Marks the total of the run `run_id`, and the session's, partial, naming the node `node_id`
    /// whose usage the session cannot read ([`UsageTotal::mark_uncounted`]).
    pub(crate) fn mark_uncounted(&mut self, run_id: RunId, node_id: NodeId) {
        self.total_of(run_id).mark_uncounted(node_id);
        self.session.mark_uncounted(node_id);
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
