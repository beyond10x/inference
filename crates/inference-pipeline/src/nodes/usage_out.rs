//! The `usage` port of a started cascade node, [`AgentNode`](super::agent::AgentNode) or
//! [`SpeechNode`](super::speech::SpeechNode), given one: the edge or exposure leaving it, on which
//! the node sends the usage record of each model call as one stream of its own, numbered from 1.
//! The record is placed on the node's run and the node itself, the session its envelopes name,
//! none. It is sent whether or not the node was cancelled, waiting for room as long as the edge
//! takes it, so a cancel never loses the usage of a call the node already holds; only an edge that
//! is closed once the node was cancelled takes it no more. The port convention is the model
//! node's (`docs/design/inference.md` 684-688): the edge or exposure leaving it uses an overload
//! policy that never drops (`pause` or `fail`).

use std::time::{SystemTime, UNIX_EPOCH};

use inference_core::endpoint::{OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::usage::UsageRecorded;

use super::Watched;
use super::tool_authority::send_unless;
use crate::NodeOutput;

/// The `usage` port of a started node; see the module documentation.
pub(crate) struct UsageOut<P> {
    output: Box<dyn OutputEndpoint<P>>,
    run_id: RunId,
    producer: NodeId,
    port: String,
    stream_id: StreamId,
    sequence: i64,
    /// The payload carrying a record; none for a payload that carries no usage.
    carry: fn(UsageRecorded) -> Option<P>,
}

impl<P: From<ControlKind>> UsageOut<P> {
    /// The port of the node `producer` of the run `run_id` over `output`, carrying each record
    /// as `carry` makes it.
    pub(crate) fn new(
        output: NodeOutput<P>,
        run_id: RunId,
        producer: NodeId,
        carry: fn(UsageRecorded) -> Option<P>,
    ) -> Self {
        Self {
            port: output.port_id.to_string(),
            output: output.endpoint,
            run_id,
            producer,
            stream_id: StreamId::generate(),
            sequence: 0,
            carry,
        }
    }

    /// Sends `recorded`, of a call of the generation `generation_id` if it has one, placed on the
    /// run and the node; a payload that carries no usage sends nothing. Cancelled, as `watched`
    /// says through `cancelled`, an edge that is closed is not an error.
    pub(crate) async fn send<S>(
        &mut self,
        mut recorded: UsageRecorded,
        generation_id: Option<GenerationId>,
        watched: &Watched<S>,
        cancelled: fn(&S) -> bool,
    ) -> Result<(), String> {
        recorded.run_id = Some(self.run_id);
        recorded.node_id = Some(self.producer);
        recorded.session_id = None;
        let Some(payload) = (self.carry)(recorded) else {
            return Ok(());
        };
        let envelope = self.next(UsageRecorded::KIND, generation_id, payload);
        match self.output.send(envelope).await {
            Ok(()) => Ok(()),
            Err(SendError::Closed(_)) if watched.read(cancelled) => Ok(()),
            Err(refused) => Err(refused.to_string()),
        }
    }

    /// Passes the end of input on, unless the node is cancelled first or while the send waits
    /// for room.
    pub(crate) async fn end<S>(
        &mut self,
        watched: &Watched<S>,
        cancelled: fn(&S) -> bool,
    ) -> Result<(), String> {
        let end = self.next(ControlKind::Stop.as_str(), None, P::from(ControlKind::Stop));
        send_unless(watched, cancelled, self.output.as_ref(), end).await
    }

    /// The next envelope on the port's stream, stamped on the wall clock.
    fn next(
        &mut self,
        kind: &str,
        generation_id: Option<GenerationId>,
        payload: P,
    ) -> EventEnvelope<P> {
        self.sequence += 1;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_micros()).unwrap_or(i64::MAX)
            });
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: self.port.clone(),
            kind: kind.to_owned(),
            generation_id,
            timestamp,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }
}
