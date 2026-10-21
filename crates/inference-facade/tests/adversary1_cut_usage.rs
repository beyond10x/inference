//! Adversary pass 1 on `story:llm-usage-on-cancel`: the model node of a facade session cut during
//! a turn, by a supersede or a cancel of its generation the host asks for.
//!
//! The node sends the cut turn's record right after the interruption, not at the drain, so a
//! cancel of the run after it loses nothing; it sends no text after the interruption; and a
//! model that never returns from the cancel is measured against the session's drain and cancel.

#![allow(dead_code)]

mod text;

use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use inference_core::usage::{QuantityKind, UsageRecorded};
use inference_facade::events::SessionEvent;
use inference_facade::live::{LiveSession, LiveSessions};
use inference_llm::Generator;
use inference_llm::llm_core::{
    BoxFuture, Cancel, Capabilities, Error, Id, Model, Protocol, Provenance,
    StreamEvent as LlmEvent, StreamSink, TurnOutcome, TurnRequest, Usage,
};
use inference_pipeline::InteractionMode;
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{ModelHandle, ModelNode};
use inference_testkit::stream::StreamEvent;

use text::{Application, block_on, generator, planned_with_usage, reply};

/// How long a case waits for what it expects.
const PATIENCE: Duration = Duration::from_secs(10);

/// How long a case gives a drain it expects to hang before it measures it as hung.
const DRAIN: Duration = Duration::from_secs(3);

fn count(events: &[SessionEvent], matches: fn(&SessionEvent) -> bool) -> usize {
    events.iter().filter(|event| matches(event)).count()
}

fn records(events: &[SessionEvent]) -> Vec<&UsageRecorded> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) => Some(recorded),
            _ => None,
        })
        .collect()
}

/// Sends `text` and commits it, then receives until `segments` segments were committed and
/// `usage` usage records delivered in all.
fn turn(
    session: &mut LiveSession<PipelinePayload>,
    application: &mut Application,
    text: &str,
    (segments, usage): (usize, usize),
    received: &mut Vec<SessionEvent>,
) {
    block_on(session.send(application.text(text))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    while count(received, |event| {
        matches!(event, SessionEvent::TextSegmentCommitted(_))
    }) < segments
        || count(received, |event| {
            matches!(event, SessionEvent::UsageRecorded(_))
        }) < usage
    {
        received.push(block_on(session.receive()).expect("the reply streams"));
    }
}

/// A model that streams one delta, then waits for the cancel of its turn and never returns.
#[derive(Debug, Clone)]
struct Stubborn {
    provenance: Provenance,
    capabilities: Capabilities,
}

impl Stubborn {
    fn new() -> Self {
        let id = |value: &str| Id::new(value).expect("a printable identifier");
        Self {
            provenance: Provenance {
                protocol: Protocol::Messages,
                provider: id("test"),
                account: id("test"),
                endpoint: id("in-process"),
                model: id("stubborn"),
                binding_revision: id("1"),
            },
            capabilities: Capabilities::text(1024, 256),
        }
    }
}

impl Model for Stubborn {
    fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn turn<'a>(
        &'a self,
        _request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, Error>> {
        Box::pin(async move {
            sink.emit(LlmEvent::TextDelta {
                text: "partial".to_owned(),
            })
            .await?;
            cancel.cancelled().await;
            std::future::pending::<()>().await;
            unreachable!("a stubborn turn never returns")
        })
    }
}

/// An opened session over the text pipeline whose model node answers on `answering`, given the
/// usage port; the session, its driver's end, and the node's handle and log.
struct Opened {
    session: LiveSession<PipelinePayload>,
    application: Application,
    driven: mpsc::Receiver<bool>,
    node: ModelHandle,
    log: RunLog,
}

fn open(declared_by: &inference_testkit::llm::MockGenerator, answering: Generator) -> Opened {
    let (declaration, planned, usage) = planned_with_usage(declared_by);
    let handles: Arc<Mutex<Vec<ModelHandle>>> = Arc::default();
    let log = RunLog::new();
    let nodes = {
        let handles = Arc::clone(&handles);
        let log = log.clone();
        move |run_id, _: &inference_pipeline::Node| {
            let node =
                ModelNode::new(run_id, answering.clone(), reply, log.clone()).with_usage(usage);
            handles
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(node.handle());
            Box::new(node) as Box<dyn inference_pipeline::NodeImplementation<PipelinePayload>>
        }
    };
    let sessions = LiveSessions::new();
    let (session, driver) =
        block_on(sessions.open(&declaration, &planned, InteractionMode::TurnBased, nodes))
            .expect("opens");
    let application = Application::new(session.run_id());
    let (ended, driven) = mpsc::channel();
    thread::spawn(move || {
        let ran = block_on(driver.drive());
        let _ = ended.send(ran.is_ok());
    });
    let node = handles
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .first()
        .cloned()
        .expect("the run made its model node");
    Opened {
        session,
        application,
        driven,
        node,
        log,
    }
}

/// Receives on another thread until `wanted` holds of what was received or the session ends;
/// the session and what it received, or none when that took longer than [`PATIENCE`].
fn receive_until(
    mut session: LiveSession<PipelinePayload>,
    mut received: Vec<SessionEvent>,
    wanted: fn(&[SessionEvent]) -> bool,
) -> Option<(LiveSession<PipelinePayload>, Vec<SessionEvent>)> {
    let (sent, back) = mpsc::channel();
    thread::spawn(move || {
        while !wanted(&received) {
            let Some(event) = block_on(session.receive()) else {
                break;
            };
            received.push(event);
        }
        let _ = sent.send((session, received));
    });
    back.recv_timeout(PATIENCE).ok()
}

/// What the node's log holds after its interruption, `interrupted` naming it: every text event
/// of the generation logged after it.
fn text_after(log: &RunLog, interrupted: fn(&StreamEvent) -> bool) -> Vec<StreamEvent> {
    log.envelopes()
        .into_iter()
        .map(|envelope| envelope.payload)
        .skip_while(|event| !interrupted(event))
        .filter(|event| {
            matches!(
                event,
                StreamEvent::TextDelta(_)
                    | StreamEvent::TextSegmentCommitted(_)
                    | StreamEvent::GenerationCompleted(_)
            )
        })
        .collect()
}

/// Guards the node's `deliver_turn` after the interruption: the cut turn's record reaches the
/// session before the session is closed, so the run's cancel after it, which drops what the
/// generation still holds (`Step::Cancel`), loses nothing; no text follows the interruption, and
/// the session's total counts two records, not three.
#[test]
fn the_cut_turns_record_arrives_before_the_close_and_survives_a_cancel_of_the_run() {
    for supersede in [true, false] {
        let first = Usage {
            input_tokens: Some(12),
            output_tokens: Some(3),
            ..Usage::default()
        };
        let cut = Usage {
            input_tokens: Some(20),
            output_tokens: Some(1),
            ..Usage::default()
        };
        let mock = generator(Vec::<String>::new())
            .reporting([Some(first), Some(cut)])
            .holding(1);
        let Opened {
            mut session,
            mut application,
            driven,
            node,
            log,
        } = open(&mock, mock.clone().into());
        let mut received = Vec::new();
        turn(&mut session, &mut application, "1", (1, 1), &mut received);
        turn(&mut session, &mut application, "2", (2, 1), &mut received);
        if supersede {
            assert!(node.supersede());
        } else {
            assert!(node.cancel_generation());
        }
        let (session, received) =
            receive_until(session, received, |received| records(received).len() >= 2)
                .unwrap_or_else(|| {
                    panic!("supersede {supersede}: the cut turn's record arrives before any close")
                });
        session.cancel().expect("an open session cancels");
        let (session, received) = receive_until(session, received, |_| false)
            .unwrap_or_else(|| panic!("supersede {supersede}: the cancelled session ends"));
        assert!(
            driven.recv_timeout(PATIENCE).is_ok(),
            "supersede {supersede}: the run ends"
        );

        let delivered = records(&received);
        let [ended, partial] = delivered.as_slice() else {
            panic!("supersede {supersede}: two records: {received:#?}")
        };
        assert!(ended.r#final && !partial.r#final);
        assert_ne!(ended.usage_record_id, partial.usage_record_id);
        let usage = session.usage();
        assert_eq!(usage.session.records(), 2, "supersede {supersede}");
        let requests = usage
            .session
            .kind(QuantityKind::Requests)
            .expect("both records carry a request");
        assert_eq!((requests.known, requests.partial), (2, 1));
        let interrupted: fn(&StreamEvent) -> bool = if supersede {
            |event| matches!(event, StreamEvent::GenerationSuperseded(_))
        } else {
            |event| matches!(event, StreamEvent::GenerationCancelled(_))
        };
        assert_eq!(
            text_after(&log, interrupted),
            Vec::new(),
            "supersede {supersede}: no text and no completion after the interruption"
        );
    }
}

/// Guards the node's interruptible wait: a model that never returns from the cancel holds the
/// node after a supersede, and the session's cancel (the runtime's `cancel` of the node, which
/// the wait watches) still ends the run.
#[test]
fn a_cancel_ends_a_session_whose_superseded_turn_never_returns() {
    let mock = generator(Vec::<String>::new());
    let answering = Generator::new(mock.adapter().clone(), Arc::new(Stubborn::new()))
        .expect("the stubborn model declares what the mock's adapter claims");
    let Opened {
        mut session,
        mut application,
        driven,
        node,
        ..
    } = open(&mock, answering);
    let mut received = Vec::new();
    turn(&mut session, &mut application, "1", (1, 0), &mut received);
    assert!(node.supersede());
    block_on(session.close()).expect("drains");
    session.cancel().expect("a draining session cancels");
    assert!(
        driven.recv_timeout(PATIENCE).is_ok(),
        "the cancel ends the run though the cut turn never returns"
    );
}

/// RED, pre-existing in kind (the node's wait on a turn never watched the drain). A model that
/// never returns from the cancel holds the node after a supersede, and the run's drain, which
/// the node's wait does not watch, never ends: the session's close does not finish within
/// [`DRAIN`].
#[test]
fn a_drain_ends_a_session_whose_superseded_turn_never_returns() {
    let mock = generator(Vec::<String>::new());
    let answering = Generator::new(mock.adapter().clone(), Arc::new(Stubborn::new()))
        .expect("the stubborn model declares what the mock's adapter claims");
    let Opened {
        mut session,
        mut application,
        driven,
        node,
        ..
    } = open(&mock, answering);
    let mut received = Vec::new();
    turn(&mut session, &mut application, "1", (1, 0), &mut received);
    assert!(node.supersede());
    block_on(session.close()).expect("drains");
    let drained = driven.recv_timeout(DRAIN).is_ok();
    session
        .cancel()
        .expect("the session is still open, so it cancels");
    assert!(
        drained,
        "the drain did not end within {DRAIN:?} while the cut turn held the node"
    );
}
