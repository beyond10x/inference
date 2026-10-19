//! The endpoint traits are dyn-compatible, so a node holds an edge end as a trait object, and a
//! refused send hands the producer back the envelope it sent.

use std::future::{self, Future};
use std::pin::pin;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};

/// An edge of one slot that refuses a second envelope as full.
#[derive(Default)]
struct Slot(Mutex<Option<EventEnvelope<u8>>>);

impl OutputEndpoint<u8> for Slot {
    fn send(&self, envelope: EventEnvelope<u8>) -> EndpointFuture<'_, Result<(), SendError<u8>>> {
        let mut slot = self.0.lock().expect("unpoisoned");
        let outcome = if slot.is_some() {
            Err(SendError::Full(envelope))
        } else {
            *slot = Some(envelope);
            Ok(())
        };
        Box::pin(future::ready(outcome))
    }
}

impl InputEndpoint<u8> for Slot {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<u8>>> {
        let taken = self.0.lock().expect("unpoisoned").take();
        Box::pin(future::ready(taken))
    }
}

fn now<T>(future: impl Future<Output = T>) -> T {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the future did not complete at once"),
    }
}

fn envelope(payload: u8) -> EventEnvelope<u8> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: NodeId::generate(),
        port: "out".to_owned(),
        kind: "inference.stream.TextDelta".to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

#[test]
fn an_edge_end_is_a_trait_object() {
    let output: Box<dyn OutputEndpoint<u8>> = Box::new(Slot::default());
    let first = envelope(1);
    now(output.send(first.clone())).expect("an empty slot");
    let refused = now(output.send(envelope(2))).expect_err("a full slot");
    assert!(matches!(refused, SendError::Full(_)));
    assert_eq!(refused.envelope().payload, 2);
    assert_eq!(refused.into_envelope().payload, 2);

    let mut input: Box<dyn InputEndpoint<u8>> = Box::new(Slot(Mutex::new(Some(first.clone()))));
    assert_eq!(now(input.recv()), Some(first));
    assert_eq!(now(input.recv()), None);
}

#[test]
fn a_refusal_names_the_event_it_returns() {
    let sent = envelope(7);
    let full = SendError::Full(sent.clone());
    let closed = SendError::Closed(sent.clone());
    assert!(full.to_string().contains(&sent.event_id.to_string()));
    assert!(closed.to_string().contains(&sent.event_id.to_string()));
    assert_ne!(full.to_string(), closed.to_string());
    assert_eq!(closed.into_envelope(), sent);
}
