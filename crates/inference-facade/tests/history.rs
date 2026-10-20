//! A session's history (`story:session-history`): the committed facts it accepted or delivered,
//! in order, each naming the run it came from, kept after the run completed and the session
//! closed. Two committed exchanges over the text pipeline of topology A record user turn,
//! assistant segment, user turn, assistant segment and no delta; a payload carrying a tool result
//! and a committed transcript on the egress records a tool result and a user turn.

#![allow(dead_code)]

mod text;

use std::future;
use std::thread::{self, JoinHandle};

use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::Capability;
use inference_facade::events::{SessionEvent, SessionPayload};
use inference_facade::history::HistoryKind;
use inference_facade::live::LiveSessions;
use inference_facade::session::SessionState;
use inference_pipeline::NodeImplementation;
use inference_pipeline::{InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture};
use inference_runtime::run::RunState;
use inference_text::{SpanId, TranscriptCommitted};
use inference_tools::{ToolCallId, ToolCallResult};

use text::{Application, INGRESS, block_on, echo_planned, generator, implement, planned};

#[test]
fn two_committed_exchanges_leave_user_assistant_user_assistant_after_the_run_and_the_session_end() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = planned(&generator, &[Capability::OutputStreaming]);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        implement(&generator),
    ))
    .expect("opens");
    let session_id = session.session_id();
    let run_id = session.run_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new(run_id);

    let mut deltas = 0;
    for input in ["1", "2"] {
        block_on(session.send(application.text(input))).expect("sent");
        block_on(session.commit_input()).expect("committed");
        loop {
            let event = block_on(session.receive()).expect("the reply arrives");
            if matches!(event, SessionEvent::TextDelta(_)) {
                deltas += 1;
            }
            if matches!(event, SessionEvent::TextSegmentCommitted(_)) {
                break;
            }
        }
    }
    assert!(deltas > 0, "the reply streamed deltas");
    block_on(session.close()).expect("drains");
    while block_on(session.receive()).is_some() {}
    let run = drive.join().expect("no panic").expect("the run ends");
    assert_eq!(run.state(), RunState::Completed);
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Closed)
    );

    let history = sessions.history(session_id);
    assert_eq!(
        history
            .iter()
            .map(|entry| (entry.position(), entry.kind(), entry.text(), entry.run_id()))
            .collect::<Vec<_>>(),
        [
            (1, HistoryKind::UserTurn, "1", run_id),
            (2, HistoryKind::AssistantSegment, "s1", run_id),
            (3, HistoryKind::UserTurn, "2", run_id),
            (4, HistoryKind::AssistantSegment, "s2", run_id),
        ],
        "every committed fact, in order, naming its run, and no delta; still there once the run \
         completed and the session closed"
    );
    assert!(history.iter().all(|entry| entry.session_id() == session_id));
    assert_eq!(sessions.history_view(), history);
    assert_eq!(
        sessions
            .events()
            .iter()
            .filter(|event| matches!(event, SessionEvent::HistoryRecorded(_)))
            .count(),
        4,
        "the state machine records each entry's event"
    );
}

#[test]
fn several_inputs_before_one_commit_are_one_user_turn() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = planned(&generator, &[Capability::OutputStreaming]);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        implement(&generator),
    ))
    .expect("opens");
    let session_id = session.session_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new(session.run_id());
    block_on(session.send(application.text("good"))).expect("sent");
    block_on(session.send(application.text("morning"))).expect("sent");
    block_on(session.close()).expect("drains");
    while block_on(session.receive()).is_some() {}
    drive.join().expect("no panic").expect("the run ends");
    let users: Vec<_> = sessions
        .history(session_id)
        .into_iter()
        .filter(|entry| entry.kind() == HistoryKind::UserTurn)
        .collect();
    assert!(
        users.is_empty(),
        "input never committed is no user turn: {users:#?}"
    );

    let (declaration, planned) = text::planned(&generator, &[Capability::OutputStreaming]);
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        implement(&generator),
    ))
    .expect("opens");
    let session_id = session.session_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new(session.run_id());
    block_on(session.send(application.text("good"))).expect("sent");
    block_on(session.send(application.text("morning"))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    block_on(session.close()).expect("drains");
    while block_on(session.receive()).is_some() {}
    drive.join().expect("no panic").expect("the run ends");
    let users: Vec<_> = sessions
        .history(session_id)
        .into_iter()
        .filter(|entry| entry.kind() == HistoryKind::UserTurn)
        .map(|entry| entry.text().to_owned())
        .collect();
    assert_eq!(users, ["good morning"], "one commit, one turn");
}

/// A payload carrying a tool result or a committed transcript, or a control event.
#[derive(Debug, Clone)]
enum Carried {
    Result(ToolCallResult),
    Transcript(TranscriptCommitted),
    Control(ControlKind),
}

impl SessionPayload for Carried {
    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }

    fn session_event(&self) -> Option<SessionEvent> {
        match self {
            Self::Result(result) => Some(SessionEvent::ToolCallResult(result.clone())),
            Self::Transcript(committed) => {
                Some(SessionEvent::TranscriptCommitted(committed.clone()))
            }
            Self::Control(_) => None,
        }
    }
}

/// A node passing on every envelope its one input delivers to its one output, until the end of
/// input, which it passes on too.
#[derive(Default)]
struct Echo {
    exchange: Option<JoinHandle<()>>,
}

impl Echo {
    fn join(&mut self) {
        if let Some(exchange) = self.exchange.take() {
            exchange.join().expect("the echo does not panic");
        }
    }
}

impl NodeImplementation<Carried> for Echo {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<Carried>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let NodeEndpoints { inputs, outputs } = endpoints;
        let (Some(mut input), Some(output)) =
            (inputs.into_iter().next(), outputs.into_iter().next())
        else {
            return Box::pin(future::ready(Err(NodeError::new(
                "one input and one output",
            ))));
        };
        self.exchange = Some(thread::spawn(move || {
            while let Some(envelope) = block_on(input.endpoint.recv()) {
                let stop = matches!(envelope.payload, Carried::Control(ControlKind::Stop));
                if block_on(output.endpoint.send(envelope)).is_err() || stop {
                    break;
                }
            }
        }));
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.join();
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.join();
        Box::pin(future::ready(()))
    }
}

fn carried(run_id: RunId, sequence: i64, payload: Carried) -> EventEnvelope<Carried> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id,
        session_id: None,
        stream_id: StreamId::generate(),
        sequence,
        producer: NodeId::generate(),
        port: INGRESS.to_owned(),
        kind: "carried".to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

#[test]
fn a_delivered_tool_result_and_a_delivered_committed_transcript_join_the_history() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = echo_planned(&generator);
    let sessions = LiveSessions::new();
    let mut echo = Some(Echo::default());
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        |_, _| Box::new(echo.take().expect("one node")) as Box<dyn NodeImplementation<Carried>>,
    ))
    .expect("opens");
    let session_id = session.session_id();
    let run_id = session.run_id();
    let drive = thread::spawn(move || block_on(driver.drive()));

    let call_id = ToolCallId::generate();
    let result = ToolCallResult {
        call_id,
        result: serde_json::json!({"temperature": 21}),
    };
    block_on(session.send(carried(run_id, 1, Carried::Result(result))))
        .expect("the result goes in");
    block_on(session.send(carried(
        run_id,
        1,
        Carried::Transcript(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: "what about tomorrow".to_owned(),
        }),
    )))
    .expect("the transcript goes in");
    block_on(session.close()).expect("drains");
    let mut delivered = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        delivered.push(event);
    }
    assert!(
        delivered
            .iter()
            .any(|event| matches!(event, SessionEvent::ToolCallResult(_))),
        "the tool result is delivered: {delivered:#?}"
    );
    let run = drive.join().expect("no panic").expect("the run ends");
    assert_eq!(run.state(), RunState::Completed);

    let history = sessions.history(session_id);
    assert_eq!(
        history
            .iter()
            .map(|entry| (
                entry.position(),
                entry.kind(),
                entry.text(),
                entry.call_id()
            ))
            .collect::<Vec<_>>(),
        [
            (
                1,
                HistoryKind::ToolResult,
                "{\"temperature\":21}",
                Some(call_id)
            ),
            (2, HistoryKind::UserTurn, "what about tomorrow", None),
        ],
        "the tool result with its call, then the voice turn's committed transcript"
    );
    assert!(history.iter().all(|entry| entry.run_id() == run_id));
}
