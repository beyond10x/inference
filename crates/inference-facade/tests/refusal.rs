//! A model's refusal reaches the application of a live session (`story:facade-session-refusal-event`).
//!
//! Over the text-only topology A, the mock model refuses the input it is given: the wrapper
//! delivers `inference.stream.GenerationRefused` on the model node's answer port, and the session
//! delivers it as one [`SessionEvent::GenerationRefused`] naming the generation and carrying the
//! refusal text, with no text segment for that generation.

#![allow(dead_code)]

mod text;

use std::panic;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use inference_core::model::Capability;
use inference_facade::events::SessionEvent;
use inference_facade::live::LiveSessions;
use inference_pipeline::InteractionMode;
use inference_runtime::run::RunState;

use text::{Application, block_on, generator, implement, planned};

/// What the model says instead of an answer.
const REFUSAL: &str = "I cannot help with that request.";

/// How long the whole session may take on the wall clock.
const DEADLINE: Duration = Duration::from_secs(20);

/// Runs `body` on a thread of its own and fails when it has not finished within [`DEADLINE`].
fn within_deadline(body: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let worker = thread::spawn(move || {
        body();
        let _ = done.send(());
    });
    match finished.recv_timeout(DEADLINE) {
        Ok(()) | Err(RecvTimeoutError::Disconnected) => {
            if let Err(failure) = worker.join() {
                panic::resume_unwind(failure);
            }
        }
        Err(RecvTimeoutError::Timeout) => panic!("the session did not end within {DEADLINE:?}"),
    }
}

#[test]
fn a_refusal_is_delivered_as_one_session_event_naming_its_generation_and_no_text_segment() {
    within_deadline(|| {
        let generator = generator(Vec::<String>::new()).refusing(REFUSAL);
        let (declaration, planned) = planned(&generator, &[Capability::OutputStreaming]);
        let sessions = LiveSessions::new();
        let (mut session, driver) = block_on(sessions.open(
            &declaration,
            &planned,
            InteractionMode::TurnBased,
            implement(&generator),
        ))
        .expect("a session opens over a planned pipeline of its mode");
        let run_id = session.run_id();
        let drive = thread::spawn(move || block_on(driver.drive()));

        let mut application = Application::new(run_id);
        block_on(session.send(application.text("say something forbidden")))
            .expect("a Ready session takes input");
        block_on(session.commit_input()).expect("a Ready session takes a commit");
        block_on(session.close()).expect("a Ready session drains");

        let mut received = Vec::new();
        while let Some(event) = block_on(session.receive()) {
            received.push(event);
        }
        let run = drive
            .join()
            .expect("the driver does not panic")
            .expect("the run ends");
        assert_eq!(run.state(), RunState::Completed, "{received:#?}");

        let started: Vec<_> = received
            .iter()
            .filter_map(|event| match event {
                SessionEvent::GenerationStarted(started) => Some(started.generation_id),
                _ => None,
            })
            .collect();
        let [generation_id] = started[..] else {
            panic!("one generation answers the one input: {received:#?}");
        };
        let refusals: Vec<_> = received
            .iter()
            .filter_map(|event| match event {
                SessionEvent::GenerationRefused(refused) => Some(refused),
                _ => None,
            })
            .collect();
        assert_eq!(
            refusals.len(),
            1,
            "the refusal is delivered once: {received:#?}"
        );
        assert_eq!(
            refusals[0].generation_id, generation_id,
            "the refusal names the generation it refused"
        );
        assert_eq!(refusals[0].text, REFUSAL, "the refusal carries its text");
        assert!(
            !received.iter().any(|event| matches!(
                event,
                SessionEvent::TextDelta(_) | SessionEvent::TextSegmentCommitted(_)
            )),
            "a refusal makes no text segment: {received:#?}"
        );
        assert!(
            received.iter().any(|event| matches!(
                event,
                SessionEvent::GenerationCompleted(completed)
                    if completed.generation_id == generation_id
            )),
            "the refused generation completes as any other: {received:#?}"
        );
    });
}

/// The refusal is the reply to the user turn it answers: the history holds the turn and, after
/// it, the refusal with its text, as `llm`'s conversation keeps the refusal item.
#[test]
fn a_delivered_refusal_joins_the_history_after_the_turn_it_answers() {
    within_deadline(|| {
        let generator = generator(Vec::<String>::new()).refusing(REFUSAL);
        let (declaration, planned) = planned(&generator, &[Capability::OutputStreaming]);
        let sessions = LiveSessions::new();
        let (mut session, driver) = block_on(sessions.open(
            &declaration,
            &planned,
            InteractionMode::TurnBased,
            implement(&generator),
        ))
        .expect("a session opens over a planned pipeline of its mode");
        let session_id = session.session_id();
        let run_id = session.run_id();
        let drive = thread::spawn(move || block_on(driver.drive()));

        let mut application = Application::new(run_id);
        block_on(session.send(application.text("say something forbidden")))
            .expect("a Ready session takes input");
        block_on(session.commit_input()).expect("a Ready session takes a commit");
        block_on(session.close()).expect("a Ready session drains");
        while block_on(session.receive()).is_some() {}
        let run = drive
            .join()
            .expect("the driver does not panic")
            .expect("the run ends");
        assert_eq!(run.state(), RunState::Completed);

        let history: Vec<_> = sessions
            .history(session_id)
            .iter()
            .map(|entry| {
                (
                    entry.position(),
                    entry.kind().as_str(),
                    entry.text().to_owned(),
                    entry.run_id(),
                )
            })
            .collect();
        assert_eq!(
            history,
            [
                (1, "user_turn", "say something forbidden".to_owned(), run_id),
                (2, "assistant_refusal", REFUSAL.to_owned(), run_id),
            ],
            "the refusal is recorded after the turn it answers, with its text"
        );
    });
}
