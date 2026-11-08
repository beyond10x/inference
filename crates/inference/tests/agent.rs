//! The `Agent` over the testkit mocks: its builder, its event stream, its close and its end.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use inference::core::model::{
    Adapter, AdapterDescriptor as ModelDescriptor, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference::core::policy::ProfileKind;
use inference::duplex::{
    NativeSpeechModel, OpenError, SessionConfig, SpeechCallId, SpeechCapabilities,
};
use inference::facade::events::SessionEvent;
use inference::facade::history::HistoryKind;
use inference::facade::live::LiveSession;
use inference::facade::session::SessionState;
use inference::pipeline::PlannedPipeline;
use inference::tools::{
    CancellationSignal, DeclareTool, ExecutionFuture, SideEffectClass, Tool, ToolCall, ToolCallId,
    ToolExecutor,
};
use inference::{
    AdapterDescriptor as RegisteredAdapter, Agent, Binding, BindingSource, Description, Env, Event,
    Payload, Registry, Role, Stack,
};
use inference_testkit::duplex::mock::{
    MockHistory, MockSpeechModel, MockSpeechSession, Reply, ScriptedTurn, pcm, utterance,
};
use inference_testkit::llm::{MockConfiguration, MockGenerator};
use serde_json::json;

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

fn instructed_generator() -> MockGenerator {
    MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        ["fine"],
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_builders_instructions_reach_the_chat_generators_request() {
    let generator = instructed_generator();
    let stack = Stack::new().bind(Binding::llm(generator.clone()));
    let agent = Agent::text()
        .stack(stack)
        .instructions("answer briefly")
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    agent.send_text("hello").await.expect("sent");
    agent.commit().await.expect("committed");
    let seen = read_until(&mut events, "Said").await;
    let instructions = generator.instructions();
    assert!(!instructions.is_empty(), "{seen:#?}");
    assert!(
        instructions.iter().all(|given| given == "answer briefly"),
        "{instructions:?}"
    );
    agent.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_builders_instructions_reach_the_cascade_generators_request() {
    let generator = instructed_generator();
    let stack = Stack::new()
        .bind(Binding::named(Role::Stt, "mock", BTreeMap::new()))
        .bind(Binding::llm(generator.clone()))
        .bind(Binding::named(Role::Tts, "mock", BTreeMap::new()));
    let agent = Agent::voice()
        .stack(stack)
        .instructions("answer briefly")
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    for frame in inference_testkit::stt::synthetic_audio::tone_silence_tone_silence() {
        agent.send_audio(frame).await.expect("sent");
    }
    agent.commit().await.expect("committed");
    let seen = read_until(&mut events, "Said").await;
    let instructions = generator.instructions();
    assert!(!instructions.is_empty(), "{seen:#?}");
    assert!(
        instructions.iter().all(|given| given == "answer briefly"),
        "{instructions:?}"
    );
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

/// A speech model at `locality` whose `open` counts itself, then panics: a test over it asserts
/// that nothing opens it.
struct Unopened {
    adapter: Adapter,
    capabilities: SpeechCapabilities,
    opens: Arc<AtomicUsize>,
}

impl NativeSpeechModel for Unopened {
    type Session = MockSpeechSession;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, _: SessionConfig) -> Result<MockSpeechSession, OpenError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        panic!("the speech adapter is opened");
    }
}

/// Registers `name` at `locality` serving `speech` with an [`Unopened`] model; returns its opens.
fn register_unopened(name: &str, locality: Locality) -> Arc<AtomicUsize> {
    let opens = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&opens);
    Registry::register(
        RegisteredAdapter::new(name, locality).role(Role::Speech, move |_| {
            let mock = MockSpeechModel::native(Vec::new());
            let (adapter, _) = Adapter::declare(DeclareAdapter {
                model_id: ModelId::generate(),
                backend: "agent-test unopened speech".to_owned(),
                contract_revision: "1".to_owned(),
                execution_form: if locality == Locality::SameProcess {
                    ExecutionForm::InProcess
                } else {
                    ExecutionForm::HostedService
                },
                locality,
                capabilities: mock.adapter().capabilities().to_vec(),
                max_concurrency: 1,
                descriptor: ModelDescriptor::default(),
            })
            .expect("a concurrency of one is positive");
            let capabilities = mock.capabilities().clone();
            Ok(Binding::speech(Unopened {
                adapter,
                capabilities,
                opens: Arc::clone(&counted),
            })
            .into())
        }),
    )
    .unwrap_or_else(|error| panic!("{name} registers: {error}"));
    opens
}

#[tokio::test(flavor = "multi_thread")]
async fn no_switch_to_the_cascade_across_a_profile_boundary() {
    let opens = register_unopened("agent-hosted-switch", Locality::ExternalProvider);
    let stack = || {
        cascade()
            .bind(Binding::named(
                Role::Speech,
                "agent-hosted-switch",
                BTreeMap::new(),
            ))
            .profile(ProfileKind::OfflineStrict)
    };
    let described = Agent::voice().stack(stack()).describe();
    let error = described.expect_err("the profile denies the hosted speech adapter");
    assert_eq!(error.kind(), inference::ErrorKind::ProfileDenies, "{error}");
    assert!(
        matches!(error, inference::Error::ProfileDenies { ref role, .. } if role == "speech"),
        "{error}"
    );
    let Err(error) = Agent::voice().stack(stack()).start().await else {
        panic!("a voice_cascade was started in place of the denied native_voice");
    };
    assert_eq!(error.kind(), inference::ErrorKind::ProfileDenies, "{error}");
    assert_eq!(opens.load(Ordering::SeqCst), 0, "the speech adapter opened");
}

#[test]
fn a_first_hosted_stack_is_told_the_fix() {
    let opens = register_unopened("agent-hosted-first", Locality::ExternalProvider);
    let stack = Stack::new()
        .bind(Binding::named(
            Role::Speech,
            "agent-hosted-first",
            BTreeMap::new(),
        ))
        .resolve_with(&Env::new())
        .expect("resolves");
    let error = Agent::voice()
        .stack(stack)
        .describe()
        .expect_err("the default profile denies a hosted adapter");
    assert_eq!(error.kind(), inference::ErrorKind::ProfileDenies, "{error}");
    let text = error.to_string();
    let fix = text
        .find("profile = \"hybrid_opt_in\"")
        .unwrap_or_else(|| panic!("names the stack-file fix: {text}"));
    assert!(text.contains("INFERENCE_PROFILE"), "{text}");
    for named in ["speech", "agent-hosted-first", "external_provider"] {
        let at = text
            .find(named)
            .unwrap_or_else(|| panic!("names {named}: {text}"));
        assert!(at < fix, "{named} comes after the fix: {text}");
    }
    assert_eq!(opens.load(Ordering::SeqCst), 0, "the speech adapter opened");
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

#[test]
fn the_builders_describe_opens_nothing() {
    let opens = register_unopened("agent-describe-no-open", Locality::SameProcess);
    let stack = Stack::new().bind(Binding::named(
        Role::Speech,
        "agent-describe-no-open",
        BTreeMap::new(),
    ));
    let described = Agent::voice()
        .stack(stack)
        .instructions("Answer briefly.")
        .describe()
        .expect("plans");
    let config = described.session_config.expect("a speech config");
    assert_eq!(config.instructions, "Answer briefly.");
    assert_eq!(
        opens.load(Ordering::SeqCst),
        0,
        "describe opened the speech adapter"
    );
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

/// Waits until the mock completed `count` responses.
async fn completed(history: &MockHistory, count: usize) {
    let started = Instant::now();
    while history.completed() < count {
        assert!(
            started.elapsed() < LIMIT,
            "{} of {count}",
            history.completed()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Answers every call with "Berlin" and counts its runs.
struct Berlin(Arc<AtomicUsize>);

impl ToolExecutor for Berlin {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Ok(json!("Berlin"))))
    }
}

/// The hand-built "lookup" pair and its executor's run count.
fn lookup(requires_confirmation: bool) -> ((Tool, Box<dyn ToolExecutor>), Arc<AtomicUsize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let tool = Tool::declare(DeclareTool {
        name: "lookup".to_owned(),
        description: "looks a city up".to_owned(),
        schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation,
    })
    .0;
    let executor: Box<dyn ToolExecutor> = Box::new(Berlin(Arc::clone(&runs)));
    ((tool, executor), runs)
}

fn call_turn() -> ScriptedTurn {
    ScriptedTurn {
        heard: "where is it".to_owned(),
        reply: Reply::Call {
            tool: "lookup".to_owned(),
            arguments: json!({"city": "Berlin"}),
            then: "It is {result}".to_owned(),
        },
    }
}

fn said(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Said { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_call_over_native_voice_is_called_then_answered() {
    let model = MockSpeechModel::native(vec![call_turn()]);
    let (pair, runs) = lookup(false);
    let agent = Agent::voice_with(speech(model))
        .tool(pair)
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    speak(&agent).await;
    let seen = read_until(&mut events, "Said").await;
    let called = seen
        .iter()
        .position(|event| {
            matches!(event, Event::ToolCalled { name, speech_call_id, .. }
                if name == "lookup" && *speech_call_id == Some(SpeechCallId::new("call_1")))
        })
        .unwrap_or_else(|| panic!("ToolCalled for call_1: {seen:#?}"));
    assert!(called < seen.len() - 1, "{seen:#?}");
    assert!(said(&seen)[0].contains("Berlin"), "{seen:#?}");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    agent.close().await.expect("closes");
}

/// What `held` answers: the agent, its stream, the events read, the runs and the call.
type Held = (
    Agent,
    inference::Events,
    Vec<Event>,
    Arc<AtomicUsize>,
    ToolCallId,
);

/// Starts an agent whose "lookup" requires confirmation, speaks, and reads until its
/// `ConfirmationRequested`.
async fn held() -> Held {
    let model = MockSpeechModel::native(vec![call_turn()]);
    let (pair, runs) = lookup(true);
    let agent = Agent::voice_with(speech(model))
        .tool(pair)
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    speak(&agent).await;
    let seen = read_until(&mut events, "ConfirmationRequested").await;
    let Some(Event::ConfirmationRequested { call_id, name, .. }) = seen.last() else {
        unreachable!()
    };
    assert_eq!(name, "lookup");
    let call_id = *call_id;
    (agent, events, seen, runs, call_id)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confirmation_is_requested_after_its_call() {
    let (agent, _events, seen, runs, call_id) = held().await;
    let called = seen
        .iter()
        .position(|event| matches!(event, Event::ToolCalled { call_id: id, .. } if *id == call_id))
        .unwrap_or_else(|| panic!("ToolCalled before ConfirmationRequested: {seen:#?}"));
    assert!(called < seen.len() - 1, "{seen:#?}");
    assert_eq!(
        names(&seen)
            .iter()
            .filter(|name| **name == "ConfirmationRequested")
            .count(),
        1
    );
    assert_eq!(runs.load(Ordering::SeqCst), 0, "a held call does not run");
    agent.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_call_answers_the_model_with_a_rejection() {
    let (agent, mut events, _, runs, call_id) = held().await;
    assert!(agent.reject(call_id));
    let seen = read_until(&mut events, "Said").await;
    assert!(said(&seen)[0].contains("rejected"), "{seen:#?}");
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    agent.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confirmed_call_runs_the_executor() {
    let (agent, mut events, _, runs, call_id) = held().await;
    assert!(agent.confirm(call_id));
    let seen = read_until(&mut events, "Said").await;
    assert!(said(&seen)[0].contains("Berlin"), "{seen:#?}");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    agent.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_with_a_confirmation_held_ends_with_closed() {
    let (agent, mut events, _, runs, _) = held().await;
    agent.close().await.expect("closes");
    let seen = read_all(&mut events).await;
    assert_eq!(names(&seen).last(), Some(&"Closed"), "{seen:#?}");
    assert_eq!(runs.load(Ordering::SeqCst), 0);
}

/// The call id of `event` when it is the `ToolCalled` of the model's call `speech`.
fn called(event: &Event, speech: &str) -> Option<ToolCallId> {
    match event {
        Event::ToolCalled {
            call_id,
            speech_call_id: Some(id),
            ..
        } if *id == SpeechCallId::new(speech) => Some(*call_id),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_rejects_a_held_call_and_one_called_after_it() {
    let model = MockSpeechModel::native(vec![call_turn(), call_turn()]);
    let history = model.history();
    let (pair, runs) = lookup(true);
    let agent = Agent::voice_with(speech(model))
        .tool(pair)
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    let at = say(&agent, 0).await;
    let seen = read_until(&mut events, "ConfirmationRequested").await;
    let first = seen
        .iter()
        .find_map(|event| called(event, "call_1"))
        .unwrap_or_else(|| panic!("ToolCalled for call_1: {seen:#?}"));
    assert!(
        matches!(seen.last(), Some(Event::ConfirmationRequested { call_id, .. }) if *call_id == first),
        "{seen:#?}"
    );
    completed(&history, 1).await;
    say(&agent, at).await;
    let seen = tokio::time::timeout(LIMIT, async {
        let mut seen = Vec::new();
        while !seen.iter().any(|event| called(event, "call_2").is_some()) {
            let event = events.next().await.expect("open").expect("no error");
            seen.push(event);
        }
        seen
    })
    .await
    .expect("ToolCalled for call_2 within 5 s");
    let second = seen
        .iter()
        .find_map(|event| called(event, "call_2"))
        .expect("call_2");
    agent.close().await.expect("closes");
    let seen = [seen, read_all(&mut events).await].concat();
    assert!(
        seen.iter()
            .filter(|event| matches!(
                event,
                Event::ConfirmationRequested { call_id, .. } if *call_id == second
            ))
            .count()
            <= 1,
        "{seen:#?}"
    );
    assert_eq!(names(&seen).last(), Some(&"Closed"), "{seen:#?}");
    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "a rejected call does not run"
    );
    let answered = history.answered();
    for speech in ["call_1", "call_2"] {
        let answer = answered
            .iter()
            .find(|(id, _)| *id == SpeechCallId::new(speech))
            .unwrap_or_else(|| panic!("no answer under {speech}: {answered:?}"));
        assert!(
            answer.1["error"]
                .as_str()
                .is_some_and(|error| error.starts_with("rejected")),
            "{answered:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_is_yielded_in_emission_order() {
    let model = MockSpeechModel::native(vec![turn("hi there")]);
    let history = model.history();
    let agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    speak(&agent).await;
    completed(&history, 1).await;
    let mut seen = read_until(&mut events, "Said").await;
    seen.extend(read_until(&mut events, "Usage").await);
    assert_eq!(
        names(&seen),
        [
            "Started", "Heard", "Usage", "Said", "Audio", "Audio", "Usage"
        ],
        "{seen:#?}"
    );
    agent.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn reading_and_sending_run_at_once() {
    let model = MockSpeechModel::native(vec![turn("reply 0"), turn("reply 1")]);
    let history = model.history();
    let agent = Arc::new(
        Agent::voice_with(speech(model))
            .start()
            .await
            .expect("starts"),
    );
    let mut first = agent.events();
    let reader = tokio::spawn(async move {
        let mut seen = read_until(&mut first, "Said").await;
        seen.extend(read_until(&mut first, "Usage").await);
        seen
    });
    let (next, go) = tokio::sync::oneshot::channel::<()>();
    let sender = tokio::spawn({
        let agent = Arc::clone(&agent);
        async move {
            let at = say(&agent, 0).await;
            go.await.expect("told to send utterance 1");
            say(&agent, at).await;
        }
    });
    let seen = reader.await.expect("the reader ends");
    assert_eq!(
        names(&seen),
        [
            "Started", "Heard", "Usage", "Said", "Audio", "Audio", "Usage"
        ],
        "{seen:#?}"
    );
    assert_eq!(said(&seen), ["reply 0"]);
    let mut second = agent.events();
    completed(&history, 1).await;
    next.send(()).expect("the sender waits");
    let mut seen = read_until(&mut second, "Said").await;
    seen.extend(read_until(&mut second, "Usage").await);
    assert_eq!(
        names(&seen),
        ["Heard", "Usage", "Said", "Audio", "Audio", "Usage"],
        "{seen:#?}"
    );
    assert_eq!(said(&seen), ["reply 1"]);
    sender.await.expect("the sender ends");
    agent.close().await.expect("closes");
}

/// A clock that always answers the instant it was made with.
#[derive(Clone, Copy)]
struct Fixed(Instant);

impl inference::Clock for Fixed {
    fn now(&self) -> Instant {
        self.0
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_clock_sets_the_started_instant() {
    let at = Instant::now() - Duration::from_secs(3600);
    let model = MockSpeechModel::native(vec![turn("hi")]);
    let agent = Agent::voice_with(speech(model))
        .clock(Fixed(at))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    let seen = read_until(&mut events, "Started").await;
    let Some(Event::Started { at: started, .. }) = seen.last() else {
        unreachable!()
    };
    assert_eq!(*started, at);
    agent.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn each_escape_method_returns_the_type_the_design_names() {
    let model = MockSpeechModel::native(vec![turn("hi")]);
    let agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    let s: &LiveSession<Payload> = agent.session();
    let _ = s.session_id();
    let p: &PlannedPipeline = agent.pipeline();
    let described: Description = agent.describe();
    let pipeline: &inference::facade::describe::Description =
        described.pipeline.as_ref().expect("described");
    assert_eq!(pipeline.pipeline_id, p.pipeline_id());
    let mut events = agent.events();
    let seen = read_until(&mut events, "Started").await;
    let raw: Option<SessionEvent> = seen[0].raw();
    assert!(matches!(raw, Some(SessionEvent::SessionReady(_))));
    agent.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reader_behind_by_four_is_told_lagged_four() {
    const TURNS: usize = 171;
    let script = (0..TURNS).map(|i| turn(&format!("reply {i}"))).collect();
    let model = MockSpeechModel::native(script).closes();
    let history = model.history();
    let agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    let mut at = 0;
    for i in 0..TURNS {
        completed(&history, i).await;
        let spoken = utterance(pcm(24_000), at);
        for frame in spoken.frames() {
            agent.send_audio(frame).await.expect("sent");
        }
        at = spoken.next();
    }
    tokio::time::timeout(LIMIT, agent.closed())
        .await
        .expect("the agent ends within 5 s");
    let seen = read_all(&mut events).await;
    assert!(matches!(seen[0], Event::Lagged(4)), "{:?}", names(&seen));
    let mut expected = vec!["Audio", "Audio", "Usage"];
    for _ in 1..TURNS {
        expected.extend(["Heard", "Usage", "Said", "Audio", "Audio", "Usage"]);
    }
    expected.push("Closed");
    assert_eq!(names(&seen[1..]), expected);
    let texts = said(&seen);
    assert_eq!(texts.len(), TURNS - 1);
    for (i, text) in texts.iter().enumerate() {
        assert_eq!(*text, format!("reply {}", i + 1));
    }
}

/// Sends the utterance starting at `at`; where it ends.
async fn say(agent: &Agent, at: u64) -> u64 {
    let spoken = utterance(pcm(24_000), at);
    for frame in spoken.frames() {
        agent.send_audio(frame).await.expect("sent");
    }
    spoken.next()
}

/// The state of the agent's session row.
fn row_state(agent: &Agent) -> SessionState {
    inference::sessions()
        .sessions_view()
        .into_iter()
        .find(|row| row.session_id == agent.session().session_id())
        .expect("the session's row")
        .state
}

#[tokio::test(flavor = "multi_thread")]
async fn a_native_switch_keeps_the_stream_and_merges_the_config() {
    let model = MockSpeechModel::native(vec![turn("one"), turn("two")]);
    let history = model.history();
    let mut agent = Agent::voice_with(speech(model))
        .voice("verse")
        .start()
        .await
        .expect("starts");
    let opened = agent.describe().session_config.expect("a native config");
    let mut events = agent.events();
    let at = say(&agent, 0).await;
    let before = read_until(&mut events, "Said").await;
    completed(&history, 1).await;
    let old_run = agent.session().run_id();
    let old_pipeline = agent.pipeline().pipeline_id();
    agent
        .switch(inference::Recipe::native_voice())
        .await
        .expect("switches");
    assert_ne!(agent.session().run_id(), old_run);
    assert_ne!(agent.pipeline().pipeline_id(), old_pipeline);
    say(&agent, at).await;
    let after = read_until(&mut events, "Heard").await;
    let after = [after, read_until(&mut events, "Said").await].concat();
    assert!(!names(&before).contains(&"Closed"), "{before:#?}");
    assert!(!names(&after).contains(&"Closed"), "{after:#?}");
    let config = agent.describe().session_config.expect("a native config");
    assert_eq!(config.voice, "verse");
    assert_eq!(config.auto_interrupt, opened.auto_interrupt);
    let session_id = agent.session().session_id();
    agent.close().await.expect("closes");
    let entries = inference::sessions().history(session_id);
    let first_new = entries
        .iter()
        .position(|entry| entry.run_id() != old_run)
        .unwrap_or_else(|| panic!("an entry of the new run: {entries:#?}"));
    assert!(first_new > 0, "{entries:#?}");
    assert!(
        entries[first_new..]
            .iter()
            .all(|entry| entry.run_id() != old_run),
        "{entries:#?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_switch_keeps_the_confirmations_and_refuses_after_close() {
    let (mut agent, mut events, _, _, call_id) = held().await;
    agent
        .switch(inference::Recipe::native_voice())
        .await
        .expect("switches with a call held");
    agent.close().await.expect("closes");
    let rest = read_all(&mut events).await;
    assert!(
        !rest.iter().any(|event| matches!(
            event,
            Event::ConfirmationRequested { call_id: id, .. } if *id == call_id
        )),
        "{rest:#?}"
    );
    assert_eq!(names(&rest).last(), Some(&"Closed"), "{rest:#?}");
    let refused = agent.switch(inference::Recipe::native_voice()).await;
    assert!(
        matches!(refused, Err(inference::Error::Closed { .. })),
        "{refused:?}"
    );
    assert!(events.next().await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_switch_right_after_the_models_close_is_refused() {
    let model = MockSpeechModel::native(vec![turn("hi")]).closes();
    let mut agent = Agent::voice_with(speech(model))
        .start()
        .await
        .expect("starts");
    let pipeline = agent.pipeline().pipeline_id();
    let mut events = agent.events();
    speak(&agent).await;
    let started = Instant::now();
    while row_state(&agent) == SessionState::Ready {
        assert!(started.elapsed() < LIMIT, "the session stays Ready");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let refused = agent.switch(inference::Recipe::native_voice()).await;
    assert!(
        matches!(refused, Err(inference::Error::Closed { .. })),
        "{refused:?}"
    );
    assert_eq!(agent.pipeline().pipeline_id(), pipeline);
    let seen = read_all(&mut events).await;
    assert_eq!(
        names(&seen)
            .iter()
            .filter(|name| **name == "Closed")
            .count(),
        1,
        "{seen:#?}"
    );
    assert_eq!(names(&seen).last(), Some(&"Closed"), "{seen:#?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_switch_whose_resolve_fails_leaves_a_working_agent() {
    let model = MockSpeechModel::native(vec![turn("one"), turn("two"), turn("three")]).closes();
    let history = model.history();
    let mut config = Agent::voice_with(speech(MockSpeechModel::native(vec![turn("hi")])))
        .describe()
        .expect("plans")
        .session_config
        .expect("a speech config");
    config.auto_interrupt = true;
    let mut agent = Agent::voice_with(speech(model))
        .session_config(config)
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    let at = say(&agent, 0).await;
    read_until(&mut events, "Said").await;
    let refused = agent
        .switch(inference::Recipe::voice_cascade())
        .await
        .expect_err("no stt is bound");
    assert_eq!(refused.kind(), inference::ErrorKind::RoleUnbound);
    assert!(refused.to_string().contains("stt"), "{refused}");
    completed(&history, 1).await;
    say(&agent, at).await;
    let seen = read_until(&mut events, "Heard").await;
    let seen = [seen, read_until(&mut events, "Said").await].concat();
    assert!(!names(&seen).contains(&"Closed"), "{seen:#?}");
    assert_eq!(said(&seen).last().map(String::as_str), Some("two"));
    agent.close().await.expect("closes");
}

/// The commit goes into the cascade's audio ingress, so the fan-out hands it to the STT node's
/// audio port ahead of the close's end of input: the run takes it and drains, and no error is
/// yielded however the commit and the turns interleave.
#[tokio::test(flavor = "multi_thread")]
async fn a_committed_cascade_turn_drains_without_an_error() {
    let agent = Agent::voice_with(cascade()).start().await.expect("starts");
    let mut events = agent.events();
    for frame in inference_testkit::stt::synthetic_audio::tone_silence_tone_silence() {
        agent.send_audio(frame).await.expect("sent");
    }
    agent.commit().await.expect("committed");
    let seen = read_until(&mut events, "Said").await;
    agent.close().await.expect("closes");
    let seen = [seen, read_all(&mut events).await].concat();
    assert!(!names(&seen).contains(&"Error"), "{seen:#?}");
    assert_eq!(names(&seen).last(), Some(&"Closed"), "{seen:#?}");
}

/// The interrupt's `cancel` takes the same way into the cascade as a commit: the run takes it
/// and drains, and no error is yielded.
#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_cascade_turn_drains_without_an_error() {
    let agent = Agent::voice_with(cascade()).start().await.expect("starts");
    let mut events = agent.events();
    for frame in inference_testkit::stt::synthetic_audio::tone_silence_tone_silence() {
        agent.send_audio(frame).await.expect("sent");
    }
    let seen = read_until(&mut events, "Said").await;
    agent.interrupt().await.expect("interrupts");
    agent.close().await.expect("closes");
    let seen = [seen, read_all(&mut events).await].concat();
    assert!(!names(&seen).contains(&"Error"), "{seen:#?}");
    assert_eq!(names(&seen).last(), Some(&"Closed"), "{seen:#?}");
}

/// An application starts and switches an agent inside spawned tasks: both futures are `Send`, or
/// this case does not compile.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_starts_and_switches_inside_spawned_tasks() {
    let agent = tokio::spawn(Agent::voice_with(cascade()).start())
        .await
        .expect("joins")
        .expect("starts");
    let agent = tokio::spawn(async move {
        let mut agent = agent;
        agent
            .switch(inference::Recipe::voice_cascade())
            .await
            .map(|()| agent)
    })
    .await
    .expect("joins")
    .expect("switches");
    agent.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cascade_switch_keeps_the_session_and_answers_the_next_utterance() {
    let mut agent = Agent::from_recipe(inference::Recipe::voice_cascade())
        .stack(cascade())
        .start()
        .await
        .expect("starts");
    let session_id = agent.session().session_id();
    let old_pipeline = agent.pipeline().pipeline_id();
    let mut events = agent.events();
    agent
        .switch(inference::Recipe::voice_cascade())
        .await
        .expect("switches");
    let pipeline_id = agent.pipeline().pipeline_id();
    assert_ne!(pipeline_id, old_pipeline);
    let row = inference::sessions()
        .sessions_view()
        .into_iter()
        .find(|row| row.session_id == session_id)
        .expect("the session's row stays");
    assert_eq!(row.pipeline_id, pipeline_id);
    for frame in inference_testkit::stt::synthetic_audio::tone_silence_tone_silence() {
        agent.send_audio(frame).await.expect("sent");
    }
    agent.commit().await.expect("committed");
    let seen = tokio::time::timeout(LIMIT, async {
        let mut seen = Vec::new();
        while !(names(&seen).contains(&"Heard") && names(&seen).contains(&"Said")) {
            let event = events.next().await.expect("open").expect("no error");
            seen.push(event);
        }
        seen
    })
    .await
    .expect("a Heard and a Said within 5 s");
    assert!(!names(&seen).contains(&"Closed"), "{seen:#?}");
    agent.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_close_while_the_egress_is_full_completes() {
    let model = MockSpeechModel::native(vec![turn("hi")]).closes();
    let agent =
        Agent::from_recipe(inference::Recipe::native_voice().exposure_capacity(NonZeroUsize::MIN))
            .stack(speech(model))
            .start()
            .await
            .expect("starts");
    let mut events = agent.events();
    speak(&agent).await;
    tokio::time::timeout(LIMIT, agent.closed())
        .await
        .expect("the close completes within 5 s");
    let seen = read_all(&mut events).await;
    assert_eq!(
        names(&seen),
        [
            "Started", "Heard", "Usage", "Said", "Audio", "Audio", "Usage", "Closed"
        ],
        "{seen:#?}"
    );
    let entries = inference::sessions().history(agent.session().session_id());
    let facts: Vec<_> = entries
        .iter()
        .map(|entry| (entry.kind(), entry.text()))
        .collect();
    assert_eq!(
        facts,
        [
            (HistoryKind::UserTurn, "hello"),
            (HistoryKind::AssistantSegment, "hi")
        ],
        "{entries:#?}"
    );
}

#[cfg(feature = "aws")]
#[test]
fn an_aws_bound_stack_describes_nova_sonics_default_without_credentials() {
    let root = tempfile::tempdir().expect("a temporary directory");
    let file = root.path().join("stack.toml");
    std::fs::write(
        &file,
        "profile = \"hybrid_opt_in\"\n\n[speech]\nadapter = \"aws\"\n",
    )
    .expect("the stack file is written");
    let env = Env::new()
        .working_dir(root.path().join("work"))
        .config_home(root.path().join("config"))
        .var("INFERENCE_STACK", file.to_str().expect("a UTF-8 path"));
    let stack = Stack::new().resolve_with(&env).expect("resolves");
    let config = Agent::voice()
        .stack(stack)
        .describe()
        .expect("plans with no AWS configuration")
        .session_config
        .expect("a speech config");
    assert_eq!(config, inference_aws::duplex::default_session_config());
}
