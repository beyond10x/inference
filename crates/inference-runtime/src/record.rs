//! The recorder of a run's event path: every envelope a node of the run emits and every envelope
//! its caller sends on an ingress, each with the merge order the recorder assigns
//! (`docs/design/inference.md` line 320), and the control decisions among them that a replay
//! compares (§19.2, lines 995-1001; §23.3, line 1130).
//!
//! A [`Recorder`] sits on the event path without the run controller knowing it: the caller wraps
//! each node's implementation as it hands it to [`RunController::create`]
//! ([`Recorder::wrap`]), and the recorder wraps every output endpoint the node is handed when it
//! starts, so each envelope the node sends is recorded before it reaches the edge. The caller
//! wraps the end of each ingress it takes ([`Recorder::ingress`]) the same way, and notes what it
//! does to the run outside the event path, such as interrupting a generation at a barge-in, as a
//! host action ([`Recorder::host`]). The envelope's own `sequence` orders one stream only; the
//! merge order is the one total order of the recording, the order the recorder took its entries
//! in, and is not a promise that the run happened in that order across streams.
//!
//! Every entry is kept in memory, for [`Recorder::recording`] and a replay, and written to each
//! [`RecordSink`] the caller supplies ([`Recorder::with_sink`]), such as a file, in merge order. A
//! [`TelemetrySink`] attached to the recorder ([`Recorder::with_telemetry`]) is offered one
//! record per envelope; it is best-effort and never waits.
//!
//! A control decision ([`ControlDecision`]) is an envelope a node emitted whose `kind` names a
//! decision of the framework rather than content, as [`DecisionKind::of`] reads it: a commit, a
//! supersede, a cancel, a discard, a tool call authorized or rejected. An envelope a node passes on
//! unchanged, carrying an event identity the recorder has already taken, such as a fan-out's copy,
//! is the same decision and is not counted again. A decision is compared by the node that made
//! it, the port it left by, its kind and its event's name, never by its payload: a replay
//! reproduces the framework's control decisions, not identical model output (line 1217).
//!
//! [`RunController::create`]: crate::RunController::create

use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use inference_core::endpoint::{EndpointFuture, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId};
use inference_core::id::NodeId;
use inference_pipeline::{
    Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeInstanceId, NodeOutput,
    PortId,
};

use crate::telemetry::{Telemetry, TelemetrySink};

/// What the recorder took.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry<P> {
    /// An envelope the node `node_id` sent on its output port `port_id`.
    Emitted {
        node_id: NodeId,
        port_id: PortId,
        envelope: EventEnvelope<P>,
    },
    /// An envelope the caller sent on the ingress exposed under `name`: an input of the run.
    Ingress {
        name: String,
        envelope: EventEnvelope<P>,
    },
    /// Something the caller did to the run outside the event path, named by `action`: an input
    /// of the run a replay redoes at the same point.
    Host { action: String },
}

/// One entry of a recording, with the merge order the recorder assigned it: 0 for the first entry
/// of a recorder, one more for each next.
#[derive(Debug, Clone, PartialEq)]
pub struct Recorded<P> {
    pub merge_order: u64,
    pub entry: Entry<P>,
}

impl<P> Recorded<P> {
    /// The envelope the entry carries; none for a host action.
    #[must_use]
    pub const fn envelope(&self) -> Option<&EventEnvelope<P>> {
        match &self.entry {
            Entry::Emitted { envelope, .. } | Entry::Ingress { envelope, .. } => Some(envelope),
            Entry::Host { .. } => None,
        }
    }

    /// Whether the entry is an input of the run: an envelope sent on an ingress or a host action.
    #[must_use]
    pub const fn is_input(&self) -> bool {
        matches!(self.entry, Entry::Ingress { .. } | Entry::Host { .. })
    }
}

/// The control decisions of the framework a replay compares, read off an envelope's `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecisionKind {
    /// A commit: the control event `commit`, a span committed (`TranscriptCommitted`), a text
    /// segment committed (`TextSegmentCommitted`) or a turn completed by the turn policy
    /// (`TurnCompleted`).
    Commit,
    /// A generation superseded (`GenerationSuperseded`).
    Supersede,
    /// A cancel: the control event `cancel` or a generation cancelled (`GenerationCancelled`).
    Cancel,
    /// An output segment discarded (`SegmentDiscarded`).
    Discard,
    /// A tool call authorized (`inference.tools.ToolCallAuthorized`).
    ToolAuthorized,
    /// A tool call rejected (`inference.tools.ToolCallRejected`).
    ToolRejected,
}

impl DecisionKind {
    /// The decision an envelope of `kind` carries, if it carries one.
    #[must_use]
    pub fn of(kind: &str) -> Option<Self> {
        if let Some(control) = ControlKind::of(kind) {
            return match control {
                ControlKind::Commit => Some(Self::Commit),
                ControlKind::Cancel => Some(Self::Cancel),
                ControlKind::Start
                | ControlKind::Stop
                | ControlKind::Revise
                | ControlKind::Error => None,
            };
        }
        match kind {
            "inference.stream.TranscriptCommitted"
            | "inference.stream.TextSegmentCommitted"
            | "inference.stream.TurnCompleted" => Some(Self::Commit),
            "inference.stream.GenerationSuperseded" => Some(Self::Supersede),
            "inference.stream.GenerationCancelled" => Some(Self::Cancel),
            "inference.stream.SegmentDiscarded" => Some(Self::Discard),
            "inference.tools.ToolCallAuthorized" => Some(Self::ToolAuthorized),
            "inference.tools.ToolCallRejected" => Some(Self::ToolRejected),
            _ => None,
        }
    }
}

/// One control decision of a run: the node `node_id` sent an envelope of `event`, carrying a
/// decision of `kind`, on its port `port_id`, recorded at `merge_order`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlDecision {
    pub merge_order: u64,
    pub node_id: NodeId,
    pub port_id: PortId,
    pub kind: DecisionKind,
    pub event: String,
}

impl ControlDecision {
    /// Whether `other` is the same decision: made by the same node, on the same port, of the
    /// same kind and event, whatever its merge order.
    #[must_use]
    pub fn same_as(&self, other: &Self) -> bool {
        self.node_id == other.node_id
            && self.port_id == other.port_id
            && self.kind == other.kind
            && self.event == other.event
    }
}

/// The decision `recorded` carries, when a node emitted it and its event identity is not among
/// `seen`; every envelope's identity is added to `seen`.
fn decision<P>(recorded: &Recorded<P>, seen: &mut HashSet<EventId>) -> Option<ControlDecision> {
    let envelope = recorded.envelope()?;
    if !seen.insert(envelope.event_id) {
        return None;
    }
    let Entry::Emitted {
        node_id, port_id, ..
    } = &recorded.entry
    else {
        return None;
    };
    Some(ControlDecision {
        merge_order: recorded.merge_order,
        node_id: *node_id,
        port_id: *port_id,
        kind: DecisionKind::of(&envelope.kind)?,
        event: envelope.kind.clone(),
    })
}

/// A recording of one run: what the recorder took, in merge order, under the policy version the
/// run was driven under.
#[derive(Debug, Clone, PartialEq)]
pub struct Recording<P> {
    pub policy_version: String,
    pub entries: Vec<Recorded<P>>,
}

impl<P> Recording<P> {
    /// The control decisions of the run, in merge order.
    #[must_use]
    pub fn decisions(&self) -> Vec<ControlDecision> {
        let mut seen = HashSet::new();
        self.entries
            .iter()
            .filter_map(|recorded| decision(recorded, &mut seen))
            .collect()
    }

    /// The inputs of the run, in merge order: what the caller sent on an ingress and the host
    /// actions it noted.
    pub fn inputs(&self) -> impl Iterator<Item = &Recorded<P>> {
        self.entries.iter().filter(|recorded| recorded.is_input())
    }

    /// Every envelope the node `node_id` emitted, in merge order: for a node bound to an adapter,
    /// the adapter's output as the node carried it.
    #[must_use]
    pub fn emitted_by(&self, node_id: NodeId) -> Vec<&EventEnvelope<P>> {
        self.entries
            .iter()
            .filter_map(|recorded| match &recorded.entry {
                Entry::Emitted {
                    node_id: emitter,
                    envelope,
                    ..
                } if *emitter == node_id => Some(envelope),
                _ => None,
            })
            .collect()
    }
}

/// Where a recorder writes each entry, in merge order, as it takes it: a file, a log, a store.
/// An implementation must not wait long: the recorder writes on the path every node sends on.
/// Any `Fn(&Recorded<P>)` is one.
pub trait RecordSink<P>: Send + Sync {
    /// Writes `recorded`.
    fn record(&self, recorded: &Recorded<P>);
}

impl<P, F: Fn(&Recorded<P>) + Send + Sync> RecordSink<P> for F {
    fn record(&self, recorded: &Recorded<P>) {
        self(recorded);
    }
}

struct State<P> {
    next: u64,
    entries: Vec<Recorded<P>>,
    decisions: Vec<ControlDecision>,
    seen: HashSet<EventId>,
    sinks: Vec<Box<dyn RecordSink<P>>>,
    telemetry: Vec<Arc<dyn TelemetrySink>>,
}

struct Shared<P> {
    policy_version: String,
    state: Mutex<State<P>>,
    changed: Condvar,
}

/// The recorder of one run's event path, under the policy version the run is driven under; see
/// the module documentation. Clones record into the same recording.
pub struct Recorder<P> {
    shared: Arc<Shared<P>>,
}

impl<P> Clone for Recorder<P> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<P: Clone + Send + 'static> Recorder<P> {
    /// A recorder of a run driven under `policy_version`, holding nothing yet.
    #[must_use]
    pub fn new(policy_version: impl Into<String>) -> Self {
        Self {
            shared: Arc::new(Shared {
                policy_version: policy_version.into(),
                state: Mutex::new(State {
                    next: 0,
                    entries: Vec::new(),
                    decisions: Vec::new(),
                    seen: HashSet::new(),
                    sinks: Vec::new(),
                    telemetry: Vec::new(),
                }),
                changed: Condvar::new(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State<P>> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The recorder, writing every entry it takes from now on to `sink` as well.
    #[must_use]
    pub fn with_sink(self, sink: impl RecordSink<P> + 'static) -> Self {
        self.state().sinks.push(Box::new(sink));
        self
    }

    /// The recorder, offering `sink` one [`Telemetry`] record for every envelope it takes from
    /// now on.
    #[must_use]
    pub fn with_telemetry(self, sink: Arc<dyn TelemetrySink>) -> Self {
        self.state().telemetry.push(sink);
        self
    }

    /// The policy version the run is driven under.
    #[must_use]
    pub fn policy_version(&self) -> &str {
        &self.shared.policy_version
    }

    /// `implementation`, the implementation of `node`, with every envelope it sends recorded as
    /// [`Entry::Emitted`] before it reaches the edge or egress.
    #[must_use]
    pub fn wrap(
        &self,
        node: &Node,
        implementation: Box<dyn NodeImplementation<P>>,
    ) -> Box<dyn NodeImplementation<P>> {
        Box::new(RecordingNode {
            node_id: node.node_id(),
            inner: implementation,
            recorder: self.clone(),
        })
    }

    /// `endpoint`, the caller's end of the ingress exposed under `name`, with every envelope sent
    /// on it recorded as [`Entry::Ingress`] before it reaches the ingress.
    #[must_use]
    pub fn ingress(
        &self,
        name: impl Into<String>,
        endpoint: Box<dyn OutputEndpoint<P>>,
    ) -> Box<dyn OutputEndpoint<P>> {
        Box::new(RecordingSender {
            inner: endpoint,
            tap: Tap::Ingress(name.into()),
            recorder: self.clone(),
        })
    }

    /// Records the host action `action`, before the caller does it.
    pub fn host(&self, action: impl Into<String>) {
        self.take(Entry::Host {
            action: action.into(),
        });
    }

    /// Everything recorded so far.
    #[must_use]
    pub fn recording(&self) -> Recording<P> {
        Recording {
            policy_version: self.shared.policy_version.clone(),
            entries: self.state().entries.clone(),
        }
    }

    /// The control decisions recorded so far, in merge order.
    #[must_use]
    pub fn decisions(&self) -> Vec<ControlDecision> {
        self.state().decisions.clone()
    }

    /// Waits until `ready` holds of the decisions recorded so far, or `timeout` passes: true when
    /// it held. `ready` is asked under the recorder's lock, so it must not call the recorder.
    pub fn wait_until(
        &self,
        timeout: Duration,
        mut ready: impl FnMut(&[ControlDecision]) -> bool,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.state();
        loop {
            if ready(&state.decisions) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Takes `entry` at the next merge order: keeps it, counts its decision, writes it to every
    /// sink, offers its telemetry and wakes every waiter.
    fn take(&self, entry: Entry<P>) {
        let mut guard = self.state();
        let state = &mut *guard;
        let recorded = Recorded {
            merge_order: state.next,
            entry,
        };
        state.next += 1;
        if let Some(decision) = decision(&recorded, &mut state.seen) {
            state.decisions.push(decision);
        }
        for sink in &state.sinks {
            sink.record(&recorded);
        }
        if let Some(envelope) = recorded.envelope()
            && !state.telemetry.is_empty()
        {
            let telemetry = Telemetry {
                merge_order: recorded.merge_order,
                run_id: envelope.run_id,
                node_id: match &recorded.entry {
                    Entry::Emitted { node_id, .. } => Some(*node_id),
                    Entry::Ingress { .. } | Entry::Host { .. } => None,
                },
                kind: envelope.kind.clone(),
                stream_id: envelope.stream_id,
                sequence: envelope.sequence,
            };
            for sink in &state.telemetry {
                sink.offer(telemetry.clone());
            }
        }
        state.entries.push(recorded);
        drop(guard);
        self.shared.changed.notify_all();
    }
}

impl<P> fmt::Debug for Recorder<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Recorder")
            .field("policy_version", &self.shared.policy_version)
            .finish_non_exhaustive()
    }
}

/// Where a recording sender sits on the event path.
enum Tap {
    Node { node_id: NodeId, port_id: PortId },
    Ingress(String),
}

/// An output endpoint that records every envelope before it hands it to the endpoint it wraps.
struct RecordingSender<P> {
    inner: Box<dyn OutputEndpoint<P>>,
    tap: Tap,
    recorder: Recorder<P>,
}

impl<P: Clone + Send + 'static> OutputEndpoint<P> for RecordingSender<P> {
    fn send(&self, envelope: EventEnvelope<P>) -> EndpointFuture<'_, Result<(), SendError<P>>> {
        let entry = match &self.tap {
            Tap::Node { node_id, port_id } => Entry::Emitted {
                node_id: *node_id,
                port_id: *port_id,
                envelope: envelope.clone(),
            },
            Tap::Ingress(name) => Entry::Ingress {
                name: name.clone(),
                envelope: envelope.clone(),
            },
        };
        self.recorder.take(entry);
        self.inner.send(envelope)
    }
}

/// A node implementation whose output endpoints record what it sends.
struct RecordingNode<P> {
    node_id: NodeId,
    inner: Box<dyn NodeImplementation<P>>,
    recorder: Recorder<P>,
}

impl<P: Clone + Send + 'static> NodeImplementation<P> for RecordingNode<P> {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.inner.describe(node)
    }

    fn described(&mut self, instance_id: NodeInstanceId) {
        self.inner.described(instance_id);
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.prepare()
    }

    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let NodeEndpoints { inputs, outputs } = endpoints;
        let outputs = outputs
            .into_iter()
            .map(|output| NodeOutput {
                port_id: output.port_id,
                endpoint: Box::new(RecordingSender {
                    inner: output.endpoint,
                    tap: Tap::Node {
                        node_id: self.node_id,
                        port_id: output.port_id,
                    },
                    recorder: self.recorder.clone(),
                }) as Box<dyn OutputEndpoint<P>>,
            })
            .collect();
        self.inner.start(NodeEndpoints { inputs, outputs })
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.drain()
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.inner.cancel()
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.inner.stop()
    }
}

#[cfg(test)]
mod tests {
    use std::future::{self, Future};
    use std::pin::pin;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    use inference_core::endpoint::{EndpointFuture, OutputEndpoint, SendError};
    use inference_core::envelope::{EventEnvelope, EventId, Timebase};
    use inference_core::id::{NodeId, RunId, StreamId};

    use super::{DecisionKind, Entry, Recorded, Recorder};

    /// An ingress that takes everything at once.
    struct Taking;

    impl OutputEndpoint<()> for Taking {
        fn send(&self, _: EventEnvelope<()>) -> EndpointFuture<'_, Result<(), SendError<()>>> {
            Box::pin(future::ready(Ok(())))
        }
    }

    fn sent(endpoint: &dyn OutputEndpoint<()>, envelope: EventEnvelope<()>) {
        let mut send = pin!(endpoint.send(envelope));
        assert!(matches!(
            send.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(()))
        ));
    }

    fn envelope(kind: &str, sequence: i64) -> EventEnvelope<()> {
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: RunId::generate(),
            session_id: None,
            stream_id: StreamId::generate(),
            sequence,
            producer: NodeId::generate(),
            port: "audio".to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: (),
        }
    }

    #[test]
    fn every_decision_kind_is_read_off_its_event_name_and_content_is_none() {
        for (kind, decision) in [
            ("commit", DecisionKind::Commit),
            ("inference.stream.TranscriptCommitted", DecisionKind::Commit),
            (
                "inference.stream.TextSegmentCommitted",
                DecisionKind::Commit,
            ),
            ("inference.stream.TurnCompleted", DecisionKind::Commit),
            (
                "inference.stream.GenerationSuperseded",
                DecisionKind::Supersede,
            ),
            ("cancel", DecisionKind::Cancel),
            ("inference.stream.GenerationCancelled", DecisionKind::Cancel),
            ("inference.stream.SegmentDiscarded", DecisionKind::Discard),
            (
                "inference.tools.ToolCallAuthorized",
                DecisionKind::ToolAuthorized,
            ),
            (
                "inference.tools.ToolCallRejected",
                DecisionKind::ToolRejected,
            ),
        ] {
            assert_eq!(DecisionKind::of(kind), Some(decision), "{kind}");
        }
        for content in [
            "stop",
            "start",
            "revise",
            "error",
            "inference.stream.TextDelta",
            "inference.audio.AudioFrame",
            "inference.tools.ToolCallProposed",
        ] {
            assert_eq!(DecisionKind::of(content), None, "{content}");
        }
    }

    #[test]
    fn the_ingress_and_host_actions_are_recorded_in_merge_order_and_written_to_the_sink() {
        let written: Arc<Mutex<Vec<u64>>> = Arc::default();
        let sink = Arc::clone(&written);
        let recorder = Recorder::new("policy 1").with_sink(move |recorded: &Recorded<()>| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(recorded.merge_order);
        });
        let ingress = recorder.ingress("audio", Box::new(Taking));
        let commit = envelope("commit", 1);
        sent(ingress.as_ref(), commit.clone());
        recorder.host("barge-in");
        sent(ingress.as_ref(), commit);
        let recording = recorder.recording();
        assert_eq!(recording.policy_version, "policy 1");
        assert_eq!(
            recording
                .entries
                .iter()
                .map(|recorded| recorded.merge_order)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert!(matches!(
            &recording.entries[1].entry,
            Entry::Host { action } if action == "barge-in"
        ));
        assert_eq!(recording.inputs().count(), 3);
        assert_eq!(
            *written.lock().unwrap_or_else(PoisonError::into_inner),
            [0, 1, 2]
        );
        assert_eq!(
            recording.decisions(),
            [],
            "what the caller sends is an input, not a decision of the run"
        );
        assert!(!recorder.wait_until(Duration::ZERO, |decisions| !decisions.is_empty()));
    }
}
