//! The replay of a recorded run: its inputs fed into a new run of the same pipeline, and the
//! control decisions of the two runs compared (`docs/design/inference.md` §19.2, lines 995-1001;
//! §23.3, line 1130). Replay reproduces the framework's control decisions when the inputs and the
//! policy version are fixed; it does not promise identical model output (line 1217).
//!
//! The caller creates the replayed run as it created the recorded one, from the same planned
//! pipeline, its nodes wrapped by a fresh [`Recorder`] carrying the policy version it drives the
//! replay under, and its adapters answering as the recorded ones did: a mock under the same
//! script, or an adapter scripted from what the recording shows the node emitted
//! ([`Recording::emitted_by`]). [`Replay::feed`] then sends each recorded input, in merge order,
//! on the ingress it was sent on, and hands each recorded host action to the caller to redo.
//!
//! The run's nodes run concurrently, so no order holds across them (line 320): what one node
//! decides is ordered by what it received, not by when another node decided. Before each input,
//! the feed waits until every node of the replay has made at least as many decisions as it had
//! made before that input in the recording, so the input, a host action above all, lands at the
//! same point of each node's decisions. The wait ends early once the replay has diverged, and
//! after `patience`; a wait that timed out ends the waiting for the rest of the feed.
//!
//! [`Replay::report`] compares the decisions node by node, each node's in the order it made them,
//! by [`ControlDecision::same_as`], and names the first divergence ([`first_divergence`]): of
//! every node whose decisions differ, the one whose first differing decision came earliest in the
//! recording's merge order.

use std::collections::HashMap;
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::OutputEndpoint;
use inference_core::id::{NodeId, RunId};

use crate::controller::ExposedEnds;
use crate::record::{ControlDecision, Entry, Recorder, Recording};

/// What [`Replay::feed`] did: how many recorded inputs it fed, how many of those the ingress
/// refused, how many it could not feed because the run exposes no ingress of their name, and how
/// many waits before an input timed out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fed {
    pub inputs: usize,
    pub refused: usize,
    pub missing: usize,
    pub gates_timed_out: usize,
}

/// The first decision on which a replay departed from its recording: the `index`th decision of
/// the node `node_id`, as recorded and as replayed; none on the side that made no such decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub node_id: NodeId,
    pub index: usize,
    pub recorded: Option<ControlDecision>,
    pub replayed: Option<ControlDecision>,
}

/// The comparison of a replay with its recording: the policy version of each run, the decisions
/// of each in merge order, and the first divergence, none when every node made the same decisions
/// in the same order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayReport {
    pub recorded_policy: String,
    pub replayed_policy: String,
    pub recorded: Vec<ControlDecision>,
    pub replayed: Vec<ControlDecision>,
    pub divergence: Option<Divergence>,
}

/// `decisions` by the node that made them, in the order of each node's first decision.
fn by_node(decisions: &[ControlDecision]) -> Vec<(NodeId, Vec<&ControlDecision>)> {
    let mut nodes: Vec<(NodeId, Vec<&ControlDecision>)> = Vec::new();
    for decision in decisions {
        match nodes
            .iter_mut()
            .find(|(node_id, _)| *node_id == decision.node_id)
        {
            Some((_, made)) => made.push(decision),
            None => nodes.push((decision.node_id, vec![decision])),
        }
    }
    nodes
}

/// The decisions of `nodes` the node `node_id` made; none when it made none.
fn made_by<'a>(
    nodes: &'a [(NodeId, Vec<&'a ControlDecision>)],
    node_id: NodeId,
) -> &'a [&'a ControlDecision] {
    nodes
        .iter()
        .find(|(made_by, _)| *made_by == node_id)
        .map(|(_, made)| made.as_slice())
        .unwrap_or_default()
}

/// The first decision on which `replayed` departs from `recorded`; see the module documentation.
/// A node that made fewer decisions in one run than in the other departs at the first it did not
/// make. Among the nodes that depart, the one whose recorded decision at that point has the
/// lowest merge order is first; a node that departs only by deciding more than it was recorded
/// to is placed just after its last recorded decision.
#[must_use]
pub fn first_divergence(
    recorded: &[ControlDecision],
    replayed: &[ControlDecision],
) -> Option<Divergence> {
    let recorded = by_node(recorded);
    let replayed = by_node(replayed);
    let mut nodes: Vec<NodeId> = recorded.iter().map(|(node_id, _)| *node_id).collect();
    for (node_id, _) in &replayed {
        if !nodes.contains(node_id) {
            nodes.push(*node_id);
        }
    }
    let mut first: Option<((u64, u8), Divergence)> = None;
    for node_id in nodes {
        let (was, is) = (made_by(&recorded, node_id), made_by(&replayed, node_id));
        let Some(index) =
            (0..was.len().max(is.len())).find(|&index| match (was.get(index), is.get(index)) {
                (Some(was), Some(is)) => !was.same_as(is),
                _ => true,
            })
        else {
            continue;
        };
        let place = match was.get(index) {
            Some(decision) => (decision.merge_order, 0),
            None => (
                index
                    .checked_sub(1)
                    .and_then(|last| was.get(last))
                    .map_or(0, |decision| decision.merge_order),
                1,
            ),
        };
        if first.as_ref().is_none_or(|(earliest, _)| place < *earliest) {
            first = Some((
                place,
                Divergence {
                    node_id,
                    index,
                    recorded: was.get(index).map(|decision| (*decision).clone()),
                    replayed: is.get(index).map(|decision| (*decision).clone()),
                },
            ));
        }
    }
    first.map(|(_, divergence)| divergence)
}

/// Whether `replayed` already departs from `recorded`: a node made a decision other than the one
/// recorded at its place, or more than were recorded.
fn departs(recorded: &[(NodeId, Vec<&ControlDecision>)], replayed: &[ControlDecision]) -> bool {
    by_node(replayed).iter().any(|(node_id, made)| {
        let was = made_by(recorded, *node_id);
        made.len() > was.len() || made.iter().zip(was).any(|(is, was)| !is.same_as(was))
    })
}

/// Whether every node of `required` made at least its count of decisions in `replayed`.
fn reached(required: &HashMap<NodeId, usize>, replayed: &[ControlDecision]) -> bool {
    required.iter().all(|(node_id, count)| {
        replayed
            .iter()
            .filter(|decision| decision.node_id == *node_id)
            .count()
            >= *count
    })
}

/// The replay of `recording` into the run whose nodes `recorder` wraps; see the module
/// documentation.
pub struct Replay<'a, P> {
    recording: &'a Recording<P>,
    recorder: Recorder<P>,
}

impl<'a, P: Clone + Send + 'static> Replay<'a, P> {
    /// The replay of `recording` into the run whose nodes `recorder` wraps.
    #[must_use]
    pub fn new(recording: &'a Recording<P>, recorder: &Recorder<P>) -> Self {
        Self {
            recording,
            recorder: recorder.clone(),
        }
    }

    /// Feeds every input of the recording into the Active run `run_id`, in merge order: each
    /// envelope sent on an ingress, under the replayed run's identity, on the ingress of the same
    /// name taken from `ends` and wrapped by the replay's recorder, and each host action recorded
    /// by that recorder and handed to `host`. Before each input it waits, for at most `patience`,
    /// as the module documentation says. Blocks the calling thread until the last input is taken;
    /// the run's drive loop runs elsewhere.
    pub fn feed(
        &self,
        run_id: RunId,
        ends: &mut ExposedEnds<P>,
        mut host: impl FnMut(&str),
        patience: Duration,
    ) -> Fed {
        let recorded = self.recording.decisions();
        let recorded_by_node = by_node(&recorded);
        let mut fed = Fed::default();
        let mut ingresses: HashMap<String, Box<dyn OutputEndpoint<P>>> = HashMap::new();
        let mut waiting = true;
        let mut required: HashMap<NodeId, usize> = HashMap::new();
        let mut before = recorded.iter().peekable();
        for input in self.recording.inputs() {
            while let Some(decision) =
                before.next_if(|decision| decision.merge_order < input.merge_order)
            {
                *required.entry(decision.node_id).or_default() += 1;
            }
            if waiting {
                let mut departed = false;
                let ready = self.recorder.wait_until(patience, |replayed| {
                    departed = departs(&recorded_by_node, replayed);
                    departed || reached(&required, replayed)
                });
                if !ready {
                    fed.gates_timed_out += 1;
                }
                waiting = ready && !departed;
            }
            match &input.entry {
                Entry::Ingress { name, envelope } => {
                    if !ingresses.contains_key(name) {
                        let Some(endpoint) = ends.ingress(name) else {
                            fed.missing += 1;
                            continue;
                        };
                        ingresses
                            .insert(name.clone(), self.recorder.ingress(name.clone(), endpoint));
                    }
                    let mut envelope = envelope.clone();
                    envelope.run_id = run_id;
                    let taken = ingresses
                        .get(name)
                        .is_some_and(|ingress| block_on(ingress.send(envelope)).is_ok());
                    fed.inputs += 1;
                    if !taken {
                        fed.refused += 1;
                    }
                }
                Entry::Host { action } => {
                    self.recorder.host(action.clone());
                    host(action);
                    fed.inputs += 1;
                }
                Entry::Emitted { .. } => {}
            }
        }
        fed
    }

    /// The decisions of the recording and of the replay so far, compared; call it once the
    /// replayed run has ended.
    #[must_use]
    pub fn report(&self) -> ReplayReport {
        let recorded = self.recording.decisions();
        let replayed = self.recorder.decisions();
        let divergence = first_divergence(&recorded, &replayed);
        ReplayReport {
            recorded_policy: self.recording.policy_version.clone(),
            replayed_policy: self.recorder.policy_version().to_owned(),
            recorded,
            replayed,
            divergence,
        }
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes.
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use inference_core::id::NodeId;
    use inference_pipeline::PortId;

    use super::{Divergence, first_divergence};
    use crate::record::{ControlDecision, DecisionKind};

    /// A decision of `kind` by `node_id`, every decision of the cases on one port.
    fn decision(merge_order: u64, node_id: NodeId, kind: DecisionKind) -> ControlDecision {
        static PORT: OnceLock<PortId> = OnceLock::new();
        ControlDecision {
            merge_order,
            node_id,
            port_id: *PORT.get_or_init(PortId::generate),
            kind,
            event: format!("{kind:?}"),
        }
    }

    #[test]
    fn the_same_decisions_per_node_in_another_merge_order_do_not_diverge() {
        let (a, b) = (NodeId::generate(), NodeId::generate());
        let recorded = [
            decision(0, a, DecisionKind::Commit),
            decision(1, b, DecisionKind::ToolAuthorized),
            decision(2, a, DecisionKind::Supersede),
        ];
        let replayed = [
            decision(0, b, DecisionKind::ToolAuthorized),
            decision(5, a, DecisionKind::Commit),
            decision(9, a, DecisionKind::Supersede),
        ];
        assert_eq!(first_divergence(&recorded, &replayed), None);
    }

    #[test]
    fn the_divergence_earliest_in_the_recording_is_first_and_a_missing_decision_counts() {
        let (a, b) = (NodeId::generate(), NodeId::generate());
        let recorded = [
            decision(0, a, DecisionKind::Commit),
            decision(1, b, DecisionKind::ToolAuthorized),
            decision(2, a, DecisionKind::Supersede),
        ];
        let replayed = [
            decision(0, a, DecisionKind::Commit),
            decision(1, a, DecisionKind::Cancel),
            decision(2, b, DecisionKind::ToolRejected),
        ];
        assert_eq!(
            first_divergence(&recorded, &replayed),
            Some(Divergence {
                node_id: b,
                index: 0,
                recorded: Some(recorded[1].clone()),
                replayed: Some(replayed[2].clone()),
            })
        );
        assert_eq!(
            first_divergence(&recorded, &recorded[..2]),
            Some(Divergence {
                node_id: a,
                index: 1,
                recorded: Some(recorded[2].clone()),
                replayed: None,
            })
        );
        assert_eq!(
            first_divergence(&recorded[..2], &recorded),
            Some(Divergence {
                node_id: a,
                index: 1,
                recorded: None,
                replayed: Some(recorded[2].clone()),
            })
        );
    }
}
