//! Adversary: the session usage view (`story:usage-session-totals`) under an interrupt, a cancel,
//! a run that records no usage and a session that holds many runs.

#![allow(dead_code)]

mod text;

use std::thread;
use std::time::{Duration, Instant};

use inference_core::id::RunId;
use inference_core::model::Capability;
use inference_core::usage::{QuantityKind, UsageRecorded, UsageTotal};
use inference_facade::events::SessionEvent;
use inference_facade::live::{LiveSession, LiveSessions};
use inference_pipeline::{InteractionMode, Node, NodeImplementation, PortId};
use inference_runtime::record::Recorder;
use inference_runtime::run::RunState;
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{MockGenerator, ModelNode};

use text::{Application, block_on, generator, implement, planned, planned_with_usage, reply};

/// The text pipeline's model node given its usage port.
fn with_usage(
    generator: &MockGenerator,
    usage: PortId,
) -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<PipelinePayload>> + use<> {
    let generator = generator.clone();
    move |run_id, _| {
        Box::new(ModelNode::new(run_id, generator.clone(), reply, RunLog::new()).with_usage(usage))
    }
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

fn total_of<'a>(records: impl IntoIterator<Item = &'a UsageRecorded>) -> UsageTotal {
    let mut total = UsageTotal::new();
    for recorded in records {
        total.add(&recorded.quantities, recorded.r#final);
    }
    total
}

/// Sends `text` and commits it, then receives until `turns` segments were committed in all.
fn turn(
    session: &mut LiveSession<PipelinePayload>,
    application: &mut Application,
    text: &str,
    turns: usize,
    received: &mut Vec<SessionEvent>,
) {
    block_on(session.send(application.text(text))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    while received
        .iter()
        .filter(|event| matches!(event, SessionEvent::TextSegmentCommitted(_)))
        .count()
        < turns
    {
        received.push(block_on(session.receive()).expect("the reply streams"));
    }
}

#[test]
fn an_interrupt_never_cuts_the_usage_record_of_the_generation_it_cut() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned, usage) = planned_with_usage(&generator);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        with_usage(&generator, usage),
    ))
    .expect("opens");
    let run_id = session.run_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new(run_id);
    block_on(session.send(application.text("1"))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    let mut before = Vec::new();
    loop {
        let event = block_on(session.receive()).expect("the reply streams");
        let delta = matches!(event, SessionEvent::TextDelta(_));
        before.push(event);
        if delta {
            break;
        }
    }
    assert!(records(&before).is_empty(), "{before:#?}");

    block_on(session.interrupt()).expect("a Ready session is interrupted");
    block_on(session.close()).expect("drains");
    let mut after = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        after.push(event);
    }
    drive.join().expect("no panic").expect("the run ends");

    let delivered = records(&after);
    assert_eq!(
        delivered.len(),
        1,
        "the usage of the cut generation is delivered after the interrupt: {after:#?}"
    );
    let usage = session.usage();
    assert_eq!(usage.session, total_of(delivered.iter().copied()));
    assert_eq!(usage.run(run_id), Some(&usage.session));
    assert_eq!(usage.session.records(), 1);
}

#[test]
fn a_cancelled_session_delivers_no_usage_record_but_counts_every_one_it_reads() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned, usage) = planned_with_usage(&generator);
    let recorder = Recorder::<PipelinePayload>::new("adversary");
    let wrapping = recorder.clone();
    let mut nodes = with_usage(&generator, usage);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        move |run_id, node| wrapping.wrap(node, nodes(run_id, node)),
    ))
    .expect("opens");
    let run_id = session.run_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new(run_id);
    block_on(session.send(application.text("1"))).expect("sent");
    block_on(session.commit_input()).expect("committed");

    // The node sent its usage record onto the egress; the application has received nothing.
    let deadline = Instant::now() + Duration::from_secs(10);
    while recorder.recording().usage().is_empty() {
        assert!(Instant::now() < deadline, "the model node sent no usage");
        thread::sleep(Duration::from_millis(5));
    }
    thread::sleep(Duration::from_millis(50));
    assert_eq!(session.usage().session.records(), 0, "nothing read yet");

    session.cancel().expect("a Ready session is cancelled");
    let mut events = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        events.push(event);
    }
    let run = drive.join().expect("no panic").expect("the run ends");
    assert_eq!(run.state(), RunState::Cancelled);

    let delivered: Vec<_> = records(&events)
        .into_iter()
        .filter(|recorded| recorded.run_id == Some(run_id))
        .collect();
    let sent_before_cancel = recorder.recording().usage().len();
    let usage = session.usage();
    assert_eq!(
        usage.session.records(),
        u64::try_from(sent_before_cancel).expect("small"),
        "every usage record the run sent before the cancel is counted, delivered {}: {events:#?}",
        delivered.len()
    );
    assert!(
        delivered.is_empty(),
        "a cancelled session delivers nothing more, usage among it: {delivered:#?}"
    );
    assert_eq!(usage.run(run_id), Some(&usage.session));
}

#[test]
fn a_run_that_records_no_usage_still_has_a_total_of_its_own_in_the_order_held() {
    let silent = generator(Vec::<String>::new());
    let (silent_declaration, silent_planned) = planned(&silent, &[Capability::OutputStreaming]);
    let counted = generator(Vec::<String>::new());
    let (counted_declaration, counted_planned, usage) = planned_with_usage(&counted);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &silent_declaration,
        &silent_planned,
        InteractionMode::TurnBased,
        implement(&silent),
    ))
    .expect("opens");
    let silent_run = session.run_id();
    let silent_drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new(silent_run);
    let mut received = Vec::new();
    turn(&mut session, &mut application, "1", 1, &mut received);
    assert_eq!(
        session.usage().runs,
        [(silent_run, UsageTotal::new())],
        "the run the session holds has a total of no record"
    );

    let driver = block_on(session.switch(
        &counted_declaration,
        &counted_planned,
        InteractionMode::TurnBased,
        with_usage(&counted, usage),
    ))
    .expect("switches");
    let counted_run = session.run_id();
    silent_drive.join().expect("no panic").expect("ends");
    let counted_drive = thread::spawn(move || block_on(driver.drive()));
    turn(&mut session, &mut application, "2", 2, &mut received);
    block_on(session.close()).expect("drains");
    while let Some(event) = block_on(session.receive()) {
        received.push(event);
    }
    counted_drive.join().expect("no panic").expect("ends");

    let usage = session.usage();
    let delivered = records(&received);
    assert_eq!(delivered.len(), 1, "{received:#?}");
    assert_eq!(
        usage.runs,
        [
            (silent_run, UsageTotal::new()),
            (counted_run, total_of(delivered.iter().copied())),
        ]
    );
    assert_eq!(usage.session, total_of(delivered.iter().copied()));
}

#[test]
fn many_runs_keep_one_total_each_their_usage_before_their_end_and_a_session_total_of_all() {
    const RUNS: usize = 4;
    const TURNS: usize = 2;
    let generator = generator(Vec::<String>::new());
    let (declaration, planned, usage) = planned_with_usage(&generator);
    let sessions = LiveSessions::new();
    let (mut session, mut driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        with_usage(&generator, usage),
    ))
    .expect("opens");
    let mut application = Application::new(session.run_id());
    let mut received = Vec::new();
    let mut runs = Vec::new();
    let mut drives = Vec::new();
    for run in 0..RUNS {
        runs.push(session.run_id());
        drives.push(thread::spawn(move || block_on(driver.drive())));
        for at in 0..TURNS {
            let done = run * TURNS + at + 1;
            turn(
                &mut session,
                &mut application,
                &format!("{run}.{at}"),
                done,
                &mut received,
            );
        }
        if run + 1 < RUNS {
            driver = block_on(session.switch(
                &declaration,
                &planned,
                InteractionMode::TurnBased,
                with_usage(&generator, usage),
            ))
            .expect("switches to the same pipeline, a new run");
        } else {
            block_on(session.close()).expect("drains");
            while let Some(event) = block_on(session.receive()) {
                received.push(event);
            }
            break;
        }
    }
    for drive in drives {
        let run = drive.join().expect("no panic").expect("ends");
        assert_eq!(run.state(), RunState::Completed);
    }

    let usage = session.usage();
    assert_eq!(
        usage
            .runs
            .iter()
            .map(|(run_id, _)| *run_id)
            .collect::<Vec<_>>(),
        runs
    );
    let mut sum = UsageTotal::new();
    for (at, run_id) in runs.iter().enumerate() {
        let of_run: Vec<_> = records(&received)
            .into_iter()
            .filter(|recorded| recorded.run_id == Some(*run_id))
            .collect();
        assert_eq!(of_run.len(), TURNS, "run {at}: one record per turn");
        assert_eq!(usage.run(*run_id), Some(&total_of(of_run.iter().copied())));
        for recorded in &of_run {
            sum.add(&recorded.quantities, recorded.r#final);
        }
        let ended = received
            .iter()
            .position(
                |event| matches!(event, SessionEvent::RunCompleted(done) if done.run_id == *run_id),
            )
            .expect("the run's end is delivered");
        let last_usage = received
            .iter()
            .rposition(|event| {
                matches!(event, SessionEvent::UsageRecorded(r) if r.run_id == Some(*run_id))
            })
            .expect("usage delivered");
        assert!(
            last_usage < ended,
            "run {at}: no usage record arrives after its run's end"
        );
    }
    assert_eq!(usage.session, sum);
    let requests = usage
        .session
        .kind(QuantityKind::Requests)
        .expect("requests counted");
    assert_eq!(
        (requests.known, requests.unknown, requests.records),
        (8, 0, 8)
    );
    let tokens = usage
        .session
        .kind(QuantityKind::InputTextTokens)
        .expect("tokens carried");
    assert_eq!(
        (tokens.known, tokens.unknown, tokens.complete()),
        (0, 8, false),
        "unknown tokens over many runs never become zero"
    );
}
