//! The `Agent` over the testkit mocks: its builder, its event stream, its close and its end.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use inference::facade::events::SessionEvent;
use inference::facade::session::SessionState;
use inference::{Agent, Binding, BindingSource, Event, Role, Stack};
use inference_testkit::duplex::mock::{MockSpeechModel, Reply, ScriptedTurn, pcm, utterance};

const LIMIT: Duration = Duration::from_secs(5);

fn turn(text: &str) -> ScriptedTurn {
    ScriptedTurn {
        heard: "hello".to_owned(),
        reply: Reply::Say(text.to_owned()),
    }
}

fn speech(model: MockSpeechModel) -> Stack {
    Stack::new().bind(Binding::speech(model))
}

fn cascade() -> Stack {
    Stack::new()
        .bind(Binding::named(Role::Stt, "mock", BTreeMap::new()))
        .bind(Binding::named(Role::Llm, "mock", BTreeMap::new()))
        .bind(Binding::named(Role::Tts, "mock", BTreeMap::new()))
}

fn name(event: &Event) -> &'static str {
    match event {
        Event::Started { .. } => "Started",
        Event::Heard { .. } => "Heard",
        Event::Transcript { .. } => "Transcript",
        Event::Said { .. } => "Said",
        Event::Audio(_) => "Audio",
        Event::ToolCalled { .. } => "ToolCalled",
        Event::Usage(_) => "Usage",
        Event::Interrupted { .. } => "Interrupted",
        Event::Closed { .. } => "Closed",
        Event::Error { .. } => "Error",
        Event::Lagged(_) => "Lagged",
        Event::ConfirmationRequested { .. } => "ConfirmationRequested",
        _ => "other",
    }
}

/// Sends utterance 0 in the session's input format, 24 kHz PCM16.
async fn speak(agent: &Agent) {
    for frame in utterance(pcm(24_000), 0).frames() {
        agent.send_audio(frame).await.expect("sent");
    }
}

/// Reads `events` until it ends, within the limit.
async fn read_all(events: &mut inference::Events) -> Vec<Event> {
    tokio::time::timeout(LIMIT, async {
        let mut seen = Vec::new();
        while let Some(event) = events.next().await {
            seen.push(event.expect("no terminal error"));
        }
        seen
    })
    .await
    .expect("the stream ends within 5 s")
}

/// Reads `events` up to the first that `stop` answers, within the limit.
async fn read_until(events: &mut inference::Events, stop: &str) -> Vec<Event> {
    tokio::time::timeout(LIMIT, async {
        let mut seen = Vec::new();
        while let Some(event) = events.next().await {
            let event = event.expect("no terminal error");
            let done = name(&event) == stop;
            seen.push(event);
            if done {
                return seen;
            }
        }
        panic!("the stream ended before {stop}: {seen:#?}");
    })
    .await
    .expect("the event arrives within 5 s")
}

fn names(events: &[Event]) -> Vec<&'static str> {
    events.iter().map(name).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn one_say_turn_yields_started_six_events_and_closed() {
    let model = MockSpeechModel::native(vec![turn("hi there")]).closes();
    let agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    speak(&agent).await;
    let seen = read_all(&mut events).await;
    assert_eq!(
        names(&seen),
        [
            "Started", "Heard", "Usage", "Said", "Audio", "Audio", "Usage", "Closed"
        ],
        "{seen:#?}"
    );
    let Event::Usage(first) = &seen[2] else {
        unreachable!()
    };
    let Event::Usage(second) = &seen[6] else {
        unreachable!()
    };
    assert!(first.response_id.is_none(), "{first:?}");
    assert!(second.response_id.is_some(), "{second:?}");
    assert!(events.next().await.is_none());
    assert!(matches!(
        seen.last().and_then(Event::raw),
        Some(SessionEvent::SessionClosed(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_is_idempotent_model_first() {
    let model = MockSpeechModel::native(vec![turn("hi")]).closes();
    let agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    speak(&agent).await;
    let seen = read_all(&mut events).await;
    agent.close().await.expect("a second close is Ok");
    assert_eq!(names(&seen).iter().filter(|n| **n == "Closed").count(), 1);
    assert_eq!(names(&seen).last(), Some(&"Closed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_is_idempotent_application_first() {
    let model = MockSpeechModel::native(vec![turn("hi")]);
    let agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    speak(&agent).await;
    let before = read_until(&mut events, "Said").await;
    agent.close().await.expect("closes");
    agent.close().await.expect("closes again");
    let after = read_all(&mut events).await;
    assert_eq!(names(&before).last(), Some(&"Said"));
    assert_eq!(names(&after).iter().filter(|n| **n == "Closed").count(), 1);
    assert_eq!(names(&after).last(), Some(&"Closed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_interrupt_yields_one_interrupted_after_said() {
    let model = MockSpeechModel::native(vec![turn("hi")]);
    let agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    speak(&agent).await;
    let mut seen = read_until(&mut events, "Said").await;
    agent.interrupt().await.expect("interrupts");
    agent.close().await.expect("closes");
    seen.extend(read_all(&mut events).await);
    let names = names(&seen);
    assert_eq!(names.iter().filter(|n| **n == "Interrupted").count(), 1);
    let said = names.iter().position(|n| *n == "Said");
    let interrupted = names.iter().position(|n| *n == "Interrupted");
    assert!(said < interrupted, "{names:?}");
    assert_eq!(names.last(), Some(&"Closed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_response_yields_a_provider_error_and_no_interruption() {
    let model = MockSpeechModel::native(vec![turn("hi")])
        .fails_responses()
        .closes();
    let agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    speak(&agent).await;
    let seen = read_all(&mut events).await;
    let errors: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            Event::Error { error, .. } => Some(error.kind()),
            _ => None,
        })
        .collect();
    assert_eq!(errors, [inference::ErrorKind::Provider], "{seen:#?}");
    assert!(!names(&seen).contains(&"Interrupted"));
    assert_eq!(names(&seen).last(), Some(&"Closed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn every_stream_ends_after_closed() {
    let model = MockSpeechModel::native(vec![turn("hi")]).closes();
    let agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    let mut second = {
        let first = agent.events();
        let second = agent.events();
        speak(&agent).await;
        agent.closed().await;
        let mut first = first;
        let all = read_all(&mut first).await;
        assert_eq!(all.first().map(name), Some("Started"));
        assert_eq!(all.last().map(name), Some("Closed"));
        second
    };
    let seen = read_all(&mut second).await;
    assert_eq!(seen.last().map(name), Some("Closed"));
    assert!(agent.events().next().await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_events_call_after_the_end_yields_the_buffered_events() {
    let model = MockSpeechModel::native(vec![turn("hi")]).closes();
    let agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    speak(&agent).await;
    tokio::time::timeout(LIMIT, agent.closed())
        .await
        .expect("closes within 5 s");
    let mut first = agent.events();
    let seen = read_all(&mut first).await;
    assert_eq!(seen.first().map(name), Some("Started"));
    assert_eq!(seen.last().map(name), Some("Closed"));
    assert!(agent.events().next().await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_model_session_closes_the_agent() {
    let model = MockSpeechModel::native(vec![turn("hi")]).with_faults(
        inference_testkit::duplex::mock::Faults {
            close_after: Some(3),
            ..Default::default()
        },
    );
    let agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    speak(&agent).await;
    let seen = read_all(&mut events).await;
    let errors: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            Event::Error { error, .. } => Some(error.kind()),
            _ => None,
        })
        .collect();
    assert_eq!(
        errors,
        [inference::ErrorKind::Provider, inference::ErrorKind::Layer],
        "{seen:#?}"
    );
    assert_eq!(names(&seen).last(), Some(&"Closed"));
    let row = inference::sessions()
        .sessions_view()
        .into_iter()
        .find(|row| row.session_id == agent.session().session_id())
        .expect("the session's row");
    assert_eq!(row.state, SessionState::Closed);
    agent.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_holds_the_agents_row() {
    let agent = Agent::voice_with(cascade()).start().await.expect("starts");
    assert!(
        inference::sessions()
            .sessions_view()
            .iter()
            .any(|row| row.session_id == agent.session().session_id())
    );
    let _: &inference::pipeline::PlannedPipeline = agent.pipeline();
    agent.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_text_agent_says_the_generators_text() {
    let stack = Stack::new().bind(Binding::named(Role::Llm, "mock", BTreeMap::new()));
    let builder = Agent::text().stack(stack);
    assert_eq!(builder.describe().expect("plans").recipe, "chat");
    let agent = builder.start().await.expect("starts");
    let mut events = agent.events();
    agent.send_text("hello").await.expect("sent");
    agent.commit().await.expect("committed");
    let seen = read_until(&mut events, "Said").await;
    assert!(names(&seen).contains(&"Said"), "{seen:#?}");
    agent.close().await.expect("closes");
}

#[test]
fn voice_plans_native_with_speech_and_the_cascade_without() {
    let model = MockSpeechModel::native(vec![turn("hi")]);
    let native = Agent::voice_with(speech(model)).describe().expect("plans");
    assert_eq!(native.recipe, "native_voice");
    let cascade = Agent::voice_with(cascade()).describe().expect("plans");
    assert_eq!(cascade.recipe, "voice_cascade");
    assert!(cascade.session_config.is_none());
}

#[test]
fn voice_with_and_voice_stack_describe_alike() {
    let with = Agent::voice_with(cascade()).describe().expect("plans");
    let stacked = Agent::voice().stack(cascade()).describe().expect("plans");
    assert_eq!(
        without_ids(&format!("{with:?}")),
        without_ids(&format!("{stacked:?}"))
    );
}

/// `debug` with every UUID masked: each plan generates its own pipeline and node ids.
fn without_ids(debug: &str) -> String {
    let uuid = |s: &str| {
        s.len() == 36
            && s.char_indices().all(|(i, c)| match i {
                8 | 13 | 18 | 23 => c == '-',
                _ => c.is_ascii_hexdigit(),
            })
    };
    let mut out = String::with_capacity(debug.len());
    let mut i = 0;
    while i < debug.len() {
        if debug.get(i..i + 36).is_some_and(uuid) {
            out.push_str("<id>");
            i += 36;
        } else {
            let c = debug[i..].chars().next().expect("in range");
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

#[test]
fn typed_binders_bind_their_roles_from_the_builder() {
    let model = MockSpeechModel::native(vec![turn("hi")]);
    let described = Agent::voice()
        .stack(Stack::new())
        .speech(Binding::speech(model))
        .describe()
        .expect("plans");
    assert_eq!(described.recipe, "native_voice");
    assert!(
        described
            .bindings
            .iter()
            .any(|(role, _, source)| *role == Role::Speech && *source == BindingSource::Builder)
    );
}

#[test]
fn a_voice_agent_short_of_tts_is_told() {
    let stack = Stack::new()
        .bind(Binding::named(Role::Stt, "mock", BTreeMap::new()))
        .bind(Binding::named(Role::Llm, "mock", BTreeMap::new()));
    let error = Agent::voice_with(stack)
        .describe()
        .expect_err("tts unbound");
    assert_eq!(error.kind(), inference::ErrorKind::RoleUnbound);
    assert!(error.to_string().contains("tts"), "{error}");
}

#[test]
fn the_merged_session_config_takes_each_later_step() {
    let model = || MockSpeechModel::native(vec![turn("hi")]);
    let default = Agent::voice_with(speech(model()))
        .describe()
        .expect("plans")
        .session_config
        .expect("a speech config");
    let voiced = Agent::voice_with(speech(model()))
        .voice("verse")
        .instructions("Answer briefly.")
        .describe()
        .expect("plans")
        .session_config
        .expect("a speech config");
    assert_eq!(voiced.voice, "verse");
    assert_eq!(voiced.instructions, "Answer briefly.");
    assert_eq!(voiced.input_format, default.input_format);
    assert_eq!(voiced.auto_interrupt, default.auto_interrupt);
    let mut given = default.clone();
    given.input_format = pcm(16_000);
    given.auto_interrupt = true;
    given.voice = "matthew".to_owned();
    let replaced = Agent::voice_with(speech(model()))
        .session_config(given.clone())
        .describe()
        .expect("plans")
        .session_config
        .expect("a speech config");
    assert_eq!(replaced.input_format, given.input_format);
    assert!(replaced.auto_interrupt);
    assert_eq!(replaced.voice, "matthew");
    let both = Agent::voice_with(speech(model()))
        .session_config(given)
        .voice("verse")
        .describe()
        .expect("plans")
        .session_config
        .expect("a speech config");
    assert_eq!(both.voice, "verse");
    assert_eq!(both.input_format, pcm(16_000));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_started_agent_describes_the_config_the_builder_did() {
    let model = MockSpeechModel::native(vec![turn("hi")]);
    let builder = Agent::voice_with(speech(model)).voice("verse");
    let planned = builder.describe().expect("plans").session_config;
    let agent = builder.start().await.expect("starts");
    assert_eq!(agent.describe().session_config, planned);
    agent.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_an_unclosed_agent_closes_its_session() {
    let model = MockSpeechModel::native(vec![turn("hi")]);
    let agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    let session_id = agent.session().session_id();
    drop(agent);
    let started = Instant::now();
    loop {
        let row = inference::sessions()
            .sessions_view()
            .into_iter()
            .find(|row| row.session_id == session_id)
            .expect("the session's row");
        if row.state == SessionState::Closed {
            break;
        }
        assert!(started.elapsed() < LIMIT, "{row:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
