#![cfg(feature = "testing")]
//! Second adversary pass on the one-call unit, after its correction: what a text agent's
//! `ToolCalled` carries, how far a chat's conversation goes with a tool, a tool that panics in
//! other ways than the first pass tried, the close and the drop of a run that holds or runs
//! calls, and a synthesiser that fails mid-way. Every test takes [`SESSIONS`] first and bounds
//! each wait with [`LIMIT`].

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use inference::audio::{AudioEncoding, AudioFormat};
use inference::audio_device::fake::{FakeDevice, FakeHost};
use inference::core::envelope::{EventEnvelope, EventId, Timebase};
use inference::core::id::{NodeId, StreamId};
use inference::core::model::{
    Adapter, AdapterDescriptor, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference::facade::live::{LiveSession, LiveSessions};
use inference::facade::session::{SessionState, SessionsRow};
use inference::llm::llm_core::{
    self, BoxFuture, CallId, Cancel, Capabilities, Id, Item, Protocol, Provenance, StopReason,
    StreamEvent as ModelEvent, StreamSink, ToolCall as ModelCall, TurnObservation, TurnOutcome,
    TurnRequest,
};
use inference::llm::tools::tool_name;
use inference::llm::{CONTRACT_REVISION, Generator};
use inference::pipeline::nodes::AuthorityHandle;
use inference::pipeline::nodes::log::StreamEvent;
use inference::text::{SpanId, TranscriptCommitted};
use inference::tools::{
    CancellationSignal, DeclareTool, ExecutionFuture, Json, SideEffectClass, Tool, ToolCall,
    ToolCallId, ToolExecutor, ToolId,
};
use inference::tts::inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued,
};
use inference::tts::{
    ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt, PlaybackRefused,
    SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};
use inference::{Agent, Binding, Chat, Env, Event, Payload, Recipe, Stack, Voice};
use inference_testkit::duplex::mock::{MockSpeechModel, Reply, ScriptedTurn, pcm, utterance};
use inference_testkit::llm::{self, MockGenerator, ToolProposal};
use inference_testkit::tts::{self, MockSynthesis, MockSynthesiser};
use serde_json::json;

/// Held by every test of this file while it runs.
static SESSIONS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// How long a case waits for a call before it fails rather than hangs.
const LIMIT: Duration = Duration::from_secs(3);

const PROMPT: &str = "the weather";

fn rows() -> Vec<SessionsRow> {
    inference::sessions().sessions_view()
}

/// The rows of the view that `before` does not hold.
fn opened_since(before: &[SessionsRow]) -> Vec<SessionsRow> {
    rows()
        .into_iter()
        .filter(|row| !before.iter().any(|old| old.session_id == row.session_id))
        .collect()
}

/// Waits until every row the view gained since `before` is `Closed`, at least `count` of them.
async fn closes_since(before: &[SessionsRow], count: usize, what: &str) {
    let started = Instant::now();
    loop {
        let new = opened_since(before);
        if new.len() >= count && new.iter().all(|row| row.state == SessionState::Closed) {
            return;
        }
        assert!(
            started.elapsed() < LIMIT,
            "{what}: not closed after {LIMIT:?}: {new:#?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn chatting(generator: impl Into<Generator>) -> Stack {
    Stack::new().bind(Binding::llm(generator))
}

fn city(name: &str) -> Json {
    json!({"city": name})
}

fn answer() -> Json {
    json!({"temperature": 21})
}

/// A read-only tool whose schema requires a string `city`, declared by hand.
fn lookup(requires_confirmation: bool) -> Tool {
    Tool::declare(DeclareTool {
        name: "lookup".to_owned(),
        description: "The weather of a city".to_owned(),
        schema: json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
            "additionalProperties": false
        }),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation,
    })
    .0
}

/// A mock generator with no script whose first model request of a conversation ends with one
/// call of `tool` for each of `cities`, the calls of one round.
fn calling_round(tool: &Tool, cities: &[&str]) -> MockGenerator {
    let round: Vec<ToolProposal> = cities
        .iter()
        .map(|name| ToolProposal {
            tool_id: tool.tool_id(),
            arguments: city(name),
        })
        .collect();
    MockGenerator::new(
        llm::MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .proposing_round(round, 1)
}

/// A tool executor that keeps the arguments of each call it is given and answers every call
/// with [`answer`].
struct Recording(Arc<Mutex<Vec<Json>>>);

impl ToolExecutor for Recording {
    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        self.0
            .lock()
            .expect("no test panicked holding it")
            .push(call.arguments().clone());
        Box::pin(std::future::ready(Ok(answer())))
    }
}

fn recording(calls: &Arc<Mutex<Vec<Json>>>) -> Box<dyn ToolExecutor> {
    Box::new(Recording(Arc::clone(calls)))
}

fn ran(calls: &Arc<Mutex<Vec<Json>>>) -> Vec<Json> {
    calls.lock().expect("no test panicked holding it").clone()
}

/// Reads `events` until a `Said` whose text holds `until`, within [`LIMIT`]: every event read,
/// and whether that `Said` came.
async fn read_to_said(events: &mut inference::Events, until: &str) -> (Vec<Event>, bool) {
    let mut seen = Vec::new();
    let came = tokio::time::timeout(LIMIT, async {
        while let Some(event) = events.next().await {
            let event = event.expect("no terminal error");
            let last = matches!(&event, Event::Said { text, .. } if text.contains(until));
            seen.push(event);
            if last {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    (seen, came)
}

/// The arguments of every `ToolCalled` of `events`, in order.
fn told(events: &[Event]) -> Vec<Json> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ToolCalled { arguments, .. } => Some(arguments.clone()),
            _ => None,
        })
        .collect()
}

/// `Event::ToolCalled` carries "the call's arguments" (`crates/inference/src/event.rs`). A text
/// agent's model proposes two calls in one round, each with a city of its own: every
/// `ToolCalled` carries the arguments of its call, in the order the model proposed them, in each
/// of 40 conversations.
#[tokio::test(flavor = "multi_thread")]
async fn every_tool_called_of_a_text_agent_carries_the_arguments_of_its_call() {
    let _held = SESSIONS.lock().await;
    let cities = ["Berlin", "Paris"];
    let expected: Vec<Json> = cities.iter().map(|name| city(name)).collect();
    let rounds = 40;
    let mut wrong = Vec::new();
    for round in 0..rounds {
        let tool = lookup(false);
        let generator = calling_round(&tool, &cities);
        let calls = Arc::default();
        let agent = Agent::text()
            .stack(chatting(generator))
            .tool((tool, recording(&calls)))
            .start()
            .await
            .expect("starts");
        let mut events = agent.events();
        agent.send_text(PROMPT).await.expect("taken");
        agent.commit().await.expect("committed");
        let (seen, came) = read_to_said(&mut events, &answer().to_string()).await;
        assert!(
            came,
            "round {round}: the reply carries the answers: {seen:#?}"
        );
        assert_eq!(ran(&calls), expected, "round {round}: the executor's calls");
        agent.close().await.expect("closes");
        let told = told(&seen);
        if told != expected {
            wrong.push(format!("round {round}: {}", Json::from(told)));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {rounds} conversations told a call without its arguments; each should tell {}:\n{}",
        wrong.len(),
        Json::from(expected),
        wrong.join("\n")
    );
}

/// How long a case that repeats a conversation waits for one of them.
const SHORT: Duration = Duration::from_secs(1);

/// What a chat whose model's first request ends with one round of `count` calls came to, in
/// each of `conversations` conversations that did not answer the round within [`SHORT`], up to
/// the third of them: the
/// tool loop "may return calls to the model for one input" four times (`MAX_TOOL_ITERATIONS`),
/// the calls of one reply counted once, and a round of two is answered
/// (`tests/adversary_w2u1_one_call.rs`).
async fn rounds_not_answered(count: usize, conversations: usize) -> Vec<String> {
    let cities = &["Berlin", "Paris", "Rome", "Oslo"][..count];
    let whole = format!("{PROMPT}{}", answer().to_string().repeat(count));
    let mut wrong = Vec::new();
    for conversation in 0..conversations {
        let tool = lookup(false);
        let generator = calling_round(&tool, cities);
        let calls = Arc::default();
        let mut chat = Chat::new_with(&chatting(generator))
            .expect("plans")
            .tool((tool, recording(&calls)))
            .expect("plans with the tool");
        let sent = tokio::time::timeout(SHORT, chat.send(PROMPT)).await;
        let ran = ran(&calls).len();
        match sent {
            Ok(Ok(reply)) if reply == whole => {}
            Ok(Ok(reply)) => wrong.push(format!("conversation {conversation}: replied {reply}")),
            Ok(Err(error)) => wrong.push(format!(
                "conversation {conversation}: {:?}: {error}; the executor ran {ran} of {count} \
                 calls",
                error.kind()
            )),
            Err(_) => {
                let closed = tokio::time::timeout(SHORT, chat.close()).await;
                wrong.push(format!(
                    "conversation {conversation}: send did not return within {SHORT:?}; the \
                     executor ran {ran} of {count} calls; a close then came to {closed:?}"
                ));
            }
        }
        drop(chat);
        if wrong.len() == 3 {
            break;
        }
    }
    wrong
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_answers_a_round_of_three_calls_in_each_of_thirty_conversations() {
    let _held = SESSIONS.lock().await;
    let wrong = rounds_not_answered(3, 30).await;
    assert!(
        wrong.is_empty(),
        "the first {} of at most 30 conversations that did not answer a round of three calls:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_answers_a_round_of_four_calls_in_each_of_thirty_conversations() {
    let _held = SESSIONS.lock().await;
    let wrong = rounds_not_answered(4, 30).await;
    assert!(
        wrong.is_empty(),
        "the first {} of at most 30 conversations that did not answer a round of four calls:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// One line per event, for a failure's message.
fn lines(events: &[Event]) -> String {
    events
        .iter()
        .map(|event| match event {
            Event::Said { text, .. } => format!("Said {text}"),
            Event::Error { error, .. } => format!("Error {error}"),
            Event::ToolCalled { arguments, .. } => format!("ToolCalled {arguments}"),
            Event::Started { .. } => "Started".to_owned(),
            Event::Usage(_) => "Usage".to_owned(),
            Event::Closed { .. } => "Closed".to_owned(),
            _ => "another event".to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The same round of four calls through a text agent, in each of at most 30 conversations: its
/// stream says the reply that carries the four answers, or tells an error; it does not go
/// silent. The first conversation that does ends the case.
#[tokio::test(flavor = "multi_thread")]
async fn a_text_agent_says_the_reply_to_a_round_of_four_calls_or_tells_an_error() {
    let _held = SESSIONS.lock().await;
    for conversation in 0..30 {
        said_or_told(conversation).await;
    }
}

/// One conversation of the case above.
async fn said_or_told(conversation: usize) {
    let cities = ["Berlin", "Paris", "Rome", "Oslo"];
    let tool = lookup(false);
    let generator = calling_round(&tool, &cities);
    let calls = Arc::default();
    let agent = Agent::text()
        .stack(chatting(generator))
        .tool((tool, recording(&calls)))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    agent.send_text(PROMPT).await.expect("taken");
    agent.commit().await.expect("committed");

    let (seen, came) = read_to_said(&mut events, &answer().to_string()).await;

    let told_an_error = seen
        .iter()
        .any(|event| matches!(event, Event::Error { .. }));
    let closed = tokio::time::timeout(SHORT, agent.close()).await;
    assert!(
        came || told_an_error,
        "conversation {conversation}: within {LIMIT:?} the stream neither said the reply nor \
         told an error; the executor ran {} of 4 calls; a close then came to {closed:?}; the \
         events were:\n{}",
        ran(&calls).len(),
        lines(&seen)
    );
    assert!(
        came,
        "conversation {conversation}: the stream told an error:\n{}",
        lines(&seen)
    );
}

/// The same over `native_voice`, whose speech node tells its calls on `events`: one call per
/// conversation, 20 conversations.
#[tokio::test(flavor = "multi_thread")]
async fn every_tool_called_of_a_native_voice_agent_carries_the_arguments_of_its_call() {
    let _held = SESSIONS.lock().await;
    let rounds = 20;
    let mut wrong = Vec::new();
    for round in 0..rounds {
        let model = MockSpeechModel::native(vec![ScriptedTurn {
            heard: "where is it".to_owned(),
            reply: Reply::Call {
                tool: "lookup".to_owned(),
                arguments: city("Berlin"),
                then: "It is {result}".to_owned(),
            },
        }]);
        let calls = Arc::default();
        let agent = Agent::voice_with(Stack::new().bind(Binding::speech(model)))
            .tool((lookup(false), recording(&calls)))
            .start()
            .await
            .expect("starts");
        let mut events = agent.events();
        for frame in utterance(pcm(24_000), 0).frames() {
            agent.send_audio(frame).await.expect("sent");
        }
        let (seen, came) = read_to_said(&mut events, "It is").await;
        assert!(came, "round {round}: the reply uses the result: {seen:#?}");
        agent.close().await.expect("closes");
        let told = told(&seen);
        if told != [city("Berlin")] {
            wrong.push(format!("round {round}: {}", Json::from(told)));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {rounds} conversations told a call without its arguments:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// A model that calls its one tool once in every turn of a conversation, with the user's text
/// as the city, and answers the tool's result with that result: what a model does that looks
/// each question up.
struct OneCallPerTurn {
    provenance: Provenance,
    capabilities: Capabilities,
    tool: ToolId,
}

impl llm_core::Model for OneCallPerTurn {
    fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn turn<'a>(
        &'a self,
        request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        _: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, llm_core::Error>> {
        Box::pin(async move {
            let called = request
                .items
                .iter()
                .filter(|item| matches!(item, Item::ToolCall(_)))
                .count();
            let (text, call) = match request.items.last() {
                Some(Item::UserText { text, .. }) => (
                    format!("looking {text} up: "),
                    Some(ModelCall {
                        call_id: CallId::new(format!("call-{called}"))
                            .expect("a printable identifier"),
                        name: tool_name(self.tool),
                        arguments: city(text),
                    }),
                ),
                Some(Item::ToolResult { output, .. }) => (output.to_string(), None),
                _ => ("nothing to answer".to_owned(), None),
            };
            sink.emit(ModelEvent::TextDelta { text: text.clone() })
                .await?;
            let stop_reason = if call.is_some() {
                StopReason::ToolCalls
            } else {
                StopReason::EndTurn
            };
            let mut items = vec![Item::assistant(text)];
            items.extend(call.map(Item::ToolCall));
            Ok(TurnOutcome {
                stop_reason,
                items,
                observation: TurnObservation {
                    usage: None,
                    final_usage: true,
                    ..TurnObservation::new(self.provenance.clone())
                },
            })
        })
    }
}

/// A generator over [`OneCallPerTurn`], claiming streaming, concurrent input and tool calling.
fn one_call_per_turn(tool: ToolId) -> Generator {
    let (adapter, _) = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "a model that looks each question up".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: vec![
            Capability::OutputStreaming,
            Capability::ConcurrentInputOutput,
            Capability::ToolCalling,
        ],
        max_concurrency: 1,
        descriptor: AdapterDescriptor::default(),
    })
    .expect("a maximum concurrency of one is positive");
    let id = |value: &str| Id::new(value).expect("a printable identifier");
    let model = OneCallPerTurn {
        provenance: Provenance {
            protocol: Protocol::Responses,
            provider: id("adversary"),
            account: id("test"),
            endpoint: id("in-process"),
            model: id("one-call-per-turn"),
            binding_revision: id("1"),
        },
        capabilities: Capabilities {
            tools: true,
            tool_choice: true,
            ..Capabilities::text(1 << 20, 1 << 16).with_output_streaming(true)
        },
        tool,
    };
    Generator::new(adapter, Arc::new(model) as Arc<dyn llm_core::Model>)
        .expect("the model declares what the adapter claims")
}

/// "A chat keeps its history: the first text opens one session ... and every later text is a
/// turn of the same conversation" (`crates/inference/src/chat.rs`). The model of this chat calls
/// its tool once in every turn. Six texts: each is answered with the tool's result, over one
/// session.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_whose_model_calls_its_tool_once_a_turn_answers_six_turns() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let calls = Arc::default();
    let before = rows();
    let mut chat = Chat::new_with(&chatting(one_call_per_turn(tool.tool_id())))
        .expect("plans")
        .tool((tool, recording(&calls)))
        .expect("plans with the tool");

    let cities = ["Berlin", "Paris", "Rome", "Oslo", "Wien", "Bern"];
    let mut answered = 0;
    let mut failed = None;
    for name in cities {
        let reply = tokio::time::timeout(LIMIT, chat.send(name))
            .await
            .expect("send returns within the limit");
        match reply {
            Ok(reply) => {
                assert_eq!(
                    reply,
                    format!("looking {name} up: {}", answer()),
                    "turn {}",
                    answered + 1
                );
                answered += 1;
            }
            Err(error) => {
                failed = Some(format!("{:?}: {error}", error.kind()));
                break;
            }
        }
    }

    assert_eq!(
        (answered, failed),
        (cities.len(), None),
        "the chat answered that many of {} turns of one call each before a send failed; the \
         executor ran {} calls and the view gained {} sessions",
        cities.len(),
        ran(&calls).len(),
        opened_since(&before).len()
    );
    chat.close().await.expect("closes");
}

/// The control of the case above: the same model and tool over four turns, which every reading
/// of the tool loop's bound admits.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_whose_model_calls_its_tool_once_a_turn_answers_four_turns() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let calls = Arc::default();
    let before = rows();
    let mut chat = Chat::new_with(&chatting(one_call_per_turn(tool.tool_id())))
        .expect("plans")
        .tool((tool, recording(&calls)))
        .expect("plans with the tool");

    for name in ["Berlin", "Paris", "Rome", "Oslo"] {
        let reply = tokio::time::timeout(LIMIT, chat.send(name))
            .await
            .expect("send returns within the limit")
            .expect("answers");
        assert_eq!(reply, format!("looking {name} up: {}", answer()));
    }

    assert_eq!(ran(&calls).len(), 4);
    assert_eq!(opened_since(&before).len(), 1, "one session took all four");
    chat.close().await.expect("closes");
}

/// The bound of `native_voice` under the counting per input: its speech node answers each user
/// turn in a response of its own, a generation of its own, and its proposals name no triggering
/// event, so each generation is counted as one input of its own. Six turns of one call each over
/// one session run all six calls, in order, and each is answered with the tool's result.
#[tokio::test(flavor = "multi_thread")]
async fn six_turns_of_one_call_each_over_native_voice_run_all_six_calls() {
    let _held = SESSIONS.lock().await;
    let cities = ["Berlin", "Paris", "Rome", "Oslo", "Wien", "Bern"];
    let script = cities
        .iter()
        .map(|name| ScriptedTurn {
            heard: format!("where is {name}"),
            reply: Reply::Call {
                tool: "lookup".to_owned(),
                arguments: city(name),
                then: "It is {result}".to_owned(),
            },
        })
        .collect();
    let calls = Arc::default();
    let agent =
        Agent::voice_with(Stack::new().bind(Binding::speech(MockSpeechModel::native(script))))
            .tool((lookup(false), recording(&calls)))
            .start()
            .await
            .expect("starts");
    let mut events = agent.events();
    let mut at = 0;
    for (answered, name) in cities.into_iter().enumerate() {
        let spoken = utterance(pcm(24_000), at);
        for frame in spoken.frames() {
            agent.send_audio(frame).await.expect("sent");
        }
        at = spoken.next();
        let (seen, came) = read_to_said(&mut events, "It is").await;
        assert!(
            came,
            "turn {} ({name}) was not answered after {answered} turns; the executor ran {} \
             calls: {}",
            answered + 1,
            ran(&calls).len(),
            lines(&seen)
        );
    }

    let expected: Vec<Json> = cities.iter().map(|name| city(name)).collect();
    assert_eq!(
        ran(&calls),
        expected,
        "the executor ran every call, in order"
    );
    agent.close().await.expect("closes");
}

/// What one invocation of [`Faulty`] does.
#[derive(Clone, Copy)]
enum Fault {
    /// Panics with a payload that is no string.
    NoMessage,
    /// Panics with a message, as the tool is called.
    Message,
    /// Awaits a timer of the async runtime the application runs on, then answers.
    Timer,
    /// Answers.
    None,
}

/// A tool executor that takes the next fault of its list at each invocation and answers once the
/// list is spent.
struct Faulty(Mutex<Vec<Fault>>);

fn faulty(faults: impl IntoIterator<Item = Fault>) -> Box<dyn ToolExecutor> {
    let mut faults: Vec<Fault> = faults.into_iter().collect();
    faults.reverse();
    Box::new(Faulty(Mutex::new(faults)))
}

impl ToolExecutor for Faulty {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        let fault = self
            .0
            .lock()
            .expect("no invocation panicked holding it")
            .pop()
            .unwrap_or(Fault::None);
        match fault {
            Fault::NoMessage => std::panic::panic_any(7_u32),
            Fault::Message => panic!("the tool has a defect"),
            Fault::Timer => Box::pin(async {
                tokio::time::sleep(Duration::from_millis(1)).await;
                Ok(answer())
            }),
            Fault::None => Box::pin(std::future::ready(Ok(answer()))),
        }
    }
}

/// A panic whose payload is no string: `send` returns the model's reply to the failed call and
/// the conversation goes on.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_whose_tool_panics_without_a_message_returns_the_reply_to_the_failed_call() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling_round(&tool, &["Berlin"]);
    let mut chat = Chat::new_with(&chatting(generator))
        .expect("plans")
        .tool((tool, faulty([Fault::NoMessage])))
        .expect("plans with the tool");

    let reply = tokio::time::timeout(LIMIT, chat.send(PROMPT))
        .await
        .expect("send returns within the limit")
        .expect("the model answers the failed call");

    assert!(reply.contains("a panic without a message"), "{reply}");
    let next = tokio::time::timeout(LIMIT, chat.send("and now"))
        .await
        .expect("send returns within the limit")
        .expect("answers");
    assert_eq!(next, "and now");
    chat.close().await.expect("closes");
}

/// One round of two calls whose first panics: the second still runs, and the reply holds the
/// failure of the first and the answer of the second.
#[tokio::test(flavor = "multi_thread")]
async fn a_round_whose_first_call_panics_still_runs_its_second() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling_round(&tool, &["Berlin", "Paris"]);
    let mut chat = Chat::new_with(&chatting(generator))
        .expect("plans")
        .tool((tool, faulty([Fault::Message, Fault::None])))
        .expect("plans with the tool");

    let reply = tokio::time::timeout(LIMIT, chat.send(PROMPT))
        .await
        .expect("send returns within the limit")
        .expect("the model answers both calls");

    assert!(reply.contains("the tool has a defect"), "{reply}");
    assert!(reply.contains(&answer().to_string()), "{reply}");
    chat.close().await.expect("closes");
}

/// A call that was held for confirmation, confirmed, and whose tool then panics: the agent's
/// stream says the model's reply to the failure, and the agent closes.
#[tokio::test(flavor = "multi_thread")]
async fn a_confirmed_call_whose_tool_panics_is_answered_as_failed() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(true);
    let generator = calling_round(&tool, &["Berlin"]);
    let agent = Agent::text()
        .stack(chatting(generator))
        .tool((tool, faulty([Fault::Message])))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    agent.send_text(PROMPT).await.expect("taken");
    agent.commit().await.expect("committed");
    let call_id = requested(&mut events, 1).await[0];

    assert!(agent.confirm(call_id), "the call was held");
    let (seen, came) = read_to_said(&mut events, "the tool has a defect").await;

    assert!(came, "the reply names the failure: {seen:#?}");
    tokio::time::timeout(LIMIT, agent.close())
        .await
        .expect("close returns within the limit")
        .expect("closes");
}

/// Reads `events` until `count` `ConfirmationRequested` came, within [`LIMIT`]: their call ids.
async fn requested(events: &mut inference::Events, count: usize) -> Vec<ToolCallId> {
    let mut ids = Vec::new();
    let mut seen = Vec::new();
    let read = tokio::time::timeout(LIMIT, async {
        while ids.len() < count {
            let Some(event) = events.next().await else {
                break;
            };
            let event = event.expect("no terminal error");
            if let Event::ConfirmationRequested { call_id, .. } = &event {
                ids.push(*call_id);
            }
            seen.push(event);
        }
    })
    .await;
    assert!(
        read.is_ok() && ids.len() == count,
        "{count} confirmations are requested within {LIMIT:?}: {seen:#?}"
    );
    ids
}

/// A tool executor that holds a handle of the runtime its application runs on: each call is
/// spawned onto it, awaits a timer there and answers, and the tool's future awaits the task.
struct Spawning(tokio::runtime::Handle);

impl ToolExecutor for Spawning {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        let answered = self.0.spawn(async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            answer()
        });
        Box::pin(async move { Ok(answered.await.expect("the spawned task answers")) })
    }
}

/// `Chat::tool`: "A tool's future is polled on a thread of the run, outside any Tokio runtime
/// ... A tool that needs one holds a `tokio::runtime::Handle`, spawns its work onto it and
/// awaits the join handle". A tool that awaits a timer of the runtime its application runs on
/// that way: its call is answered with the tool's answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_that_spawns_onto_a_runtime_handle_it_holds_answers_its_call() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling_round(&tool, &["Berlin"]);
    let spawning: Box<dyn ToolExecutor> = Box::new(Spawning(tokio::runtime::Handle::current()));
    let mut chat = Chat::new_with(&chatting(generator))
        .expect("plans")
        .tool((tool, spawning))
        .expect("plans with the tool");

    let reply = tokio::time::timeout(LIMIT, chat.send(PROMPT))
        .await
        .expect("send returns within the limit")
        .expect("answers");

    assert_eq!(
        reply,
        format!("{PROMPT}{}", answer()),
        "the reply carries the tool's answer"
    );
    chat.close().await.expect("closes");
}

/// `Chat::tool`: "a tool that awaits a Tokio timer or socket directly panics there, which fails
/// its call with the panic's message". The same timer awaited in the tool's own future: the
/// call fails, the model answers the failure, and the reply carries the panic's message.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_that_awaits_a_tokio_timer_directly_fails_its_call_with_the_panics_message() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling_round(&tool, &["Berlin"]);
    let mut chat = Chat::new_with(&chatting(generator))
        .expect("plans")
        .tool((tool, faulty([Fault::Timer])))
        .expect("plans with the tool");

    let reply = tokio::time::timeout(LIMIT, chat.send(PROMPT))
        .await
        .expect("send returns within the limit")
        .expect("the model answers the failed call");

    assert!(reply.contains("there is no reactor running"), "{reply}");
    chat.close().await.expect("closes");
}

/// A `chat` run opened one layer down, and a text committed on its `user_input`.
async fn opened(
    recipe: Recipe,
    stack: Stack,
) -> (
    LiveSession<Payload>,
    tokio::task::JoinHandle<String>,
    AuthorityHandle,
) {
    let stack = stack.resolve_with(&Env::new()).expect("resolves");
    let mut instantiation = recipe
        .instantiate(&stack)
        .expect("chat plans with the tool");
    let sessions = LiveSessions::new();
    let (session, driver) = instantiation
        .open(&stack, &sessions)
        .await
        .expect("the run opens");
    let driving = tokio::spawn(async move { format!("{:?}", driver.drive().await) });
    let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
        span_id: SpanId::generate(),
        text: PROMPT.to_owned(),
    });
    let envelope = EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: session.run_id(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: NodeId::generate(),
        port: "user_input".to_owned(),
        kind: event.name().to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: Payload::Stream(event),
    };
    session.send(envelope).await.expect("sent");
    let authority = instantiation
        .authority()
        .expect("chat given tools has an authority")
        .clone();
    (session, driving, authority)
}

/// A `chat` run opened through `Instantiation::open` whose session is never read: the four calls
/// of its round still run, as the egress `tool_events` holds them at its default capacity.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_run_whose_session_is_not_read_runs_the_calls_of_its_round() {
    let _held = SESSIONS.lock().await;
    let cities = ["Berlin", "Paris", "Rome", "Oslo"];
    let tool = lookup(false);
    let generator = calling_round(&tool, &cities);
    let calls = Arc::default();
    let recipe = Recipe::chat().tools([(tool, recording(&calls))]);
    let (session, driving, _) = opened(recipe, chatting(generator)).await;

    let started = Instant::now();
    while ran(&calls).len() < cities.len() {
        assert!(
            started.elapsed() < LIMIT,
            "an unread session kept its calls from their tool: {:?} ran",
            ran(&calls)
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let ended = tokio::time::timeout(LIMIT, async {
        let closed = session.close().await;
        while session.receive().await.is_some() {}
        closed
    })
    .await;
    assert!(ended.is_ok(), "the run closes within {LIMIT:?}");
    driving.abort();
}

/// A `chat` run opened through `Instantiation::open` over a model whose first request ends with a
/// round of three calls, its session read: all three calls run, in each of at most 30 runs. A
/// run that left a call out is closed and tells what its driver returned.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_run_runs_every_call_of_a_round_of_three_in_each_of_thirty_runs() {
    let _held = SESSIONS.lock().await;
    let cities = ["Berlin", "Paris", "Rome"];
    let mut wrong = Vec::new();
    for run in 0..30 {
        let tool = lookup(false);
        let generator = calling_round(&tool, &cities);
        let calls = Arc::default();
        let recipe = Recipe::chat().tools([(tool, recording(&calls))]);
        let (session, driving, _) = opened(recipe, chatting(generator)).await;

        let started = Instant::now();
        let mut read = 0_usize;
        while ran(&calls).len() < cities.len() && started.elapsed() < SHORT {
            let next = tokio::time::timeout(Duration::from_millis(5), session.receive());
            if matches!(next.await, Ok(Some(_))) {
                read += 1;
            }
        }
        let count = ran(&calls).len();
        let closed = tokio::time::timeout(SHORT, session.close())
            .await
            .map(|closed| closed.map_err(|error| error.to_string()));
        if count < cities.len() {
            let driven = tokio::time::timeout(SHORT, driving).await;
            wrong.push(format!(
                "run {run}: the executor ran {count} of 3 calls within {SHORT:?}, {read} events \
                 read; close came to {closed:?}; the driver came to {driven:?}"
            ));
        } else {
            driving.abort();
        }
        if wrong.len() == 3 {
            break;
        }
    }
    assert!(
        wrong.is_empty(),
        "the first {} of at most 30 runs that left a call of the round out:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// The close of a `chat` run that holds two calls of one round for confirmation: both are
/// denied, neither runs, and the session ends.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_run_closed_with_two_calls_held_denies_both_and_ends() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(true);
    let generator = calling_round(&tool, &["Berlin", "Paris"]);
    let calls = Arc::default();
    let recipe = Recipe::chat().tools([(tool, recording(&calls))]);
    let (session, driving, authority) = opened(recipe, chatting(generator)).await;
    let started = Instant::now();
    while authority.held().len() < 2 {
        assert!(started.elapsed() < LIMIT, "the authority holds both calls");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let ended = tokio::time::timeout(LIMIT, async {
        let closed = session.close().await;
        while session.receive().await.is_some() {}
        closed
    })
    .await;

    assert!(ended.is_ok(), "the close ends within {LIMIT:?}");
    assert!(authority.held().is_empty(), "no call is left held");
    assert!(ran(&calls).is_empty(), "a held call does not run");
    driving.abort();
}

/// Two closes at once of a text agent that holds two calls for confirmation: both return, no
/// call runs and the session is closed.
#[tokio::test(flavor = "multi_thread")]
async fn a_text_agent_holding_two_calls_closed_twice_at_once_closes() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(true);
    let generator = calling_round(&tool, &["Berlin", "Paris"]);
    let calls = Arc::default();
    let before = rows();
    let agent = Agent::text()
        .stack(chatting(generator))
        .tool((tool, recording(&calls)))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    agent.send_text(PROMPT).await.expect("taken");
    agent.commit().await.expect("committed");
    requested(&mut events, 2).await;

    let closed = tokio::time::timeout(LIMIT, async { tokio::join!(agent.close(), agent.close()) })
        .await
        .expect("both closes return within the limit");

    assert!(closed.0.is_ok() && closed.1.is_ok(), "{closed:?}");
    assert!(ran(&calls).is_empty(), "a held call does not run");
    closes_since(&before, 1, "the agent's session").await;
}

/// A text agent that holds a call for confirmation and is dropped without a close: its session
/// is closed and the call does not run.
#[tokio::test(flavor = "multi_thread")]
async fn a_text_agent_dropped_with_a_call_held_closes_its_session() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(true);
    let generator = calling_round(&tool, &["Berlin"]);
    let calls = Arc::default();
    let before = rows();
    let agent = Agent::text()
        .stack(chatting(generator))
        .tool((tool, recording(&calls)))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    agent.send_text(PROMPT).await.expect("taken");
    agent.commit().await.expect("committed");
    requested(&mut events, 1).await;

    drop(events);
    drop(agent);

    closes_since(&before, 1, "the dropped agent's session").await;
    assert!(ran(&calls).is_empty(), "a held call does not run");
}

/// A tool executor whose call is running until `release` is set: its future is pending and
/// polled again every 5 ms by a thread of its own, and answers once released.
struct Slow {
    running: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
}

impl ToolExecutor for Slow {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        self.running.store(true, Ordering::SeqCst);
        let release = Arc::clone(&self.release);
        Box::pin(std::future::poll_fn(move |cx| {
            if release.load(Ordering::SeqCst) {
                return std::task::Poll::Ready(Ok(answer()));
            }
            let waker = cx.waker().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(5));
                waker.wake();
            });
            std::task::Poll::Pending
        }))
    }
}

/// A chat closed while its tool runs a call: the close drains, so it returns once the tool
/// answered, here 200 ms later, and the session is closed.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_closed_while_its_tool_runs_returns_once_the_tool_answered() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling_round(&tool, &["Berlin"]);
    let (running, release) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let slow: Box<dyn ToolExecutor> = Box::new(Slow {
        running: Arc::clone(&running),
        release: Arc::clone(&release),
    });
    let before = rows();
    let mut chat = Chat::new_with(&chatting(generator))
        .expect("plans")
        .tool((tool, slow))
        .expect("plans with the tool");

    let given_up = tokio::time::timeout(Duration::from_millis(300), chat.send(PROMPT)).await;
    assert!(given_up.is_err(), "the tool has not answered: {given_up:?}");
    assert!(running.load(Ordering::SeqCst), "the tool runs the call");
    let releasing = Arc::clone(&release);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        releasing.store(true, Ordering::SeqCst);
    });
    let closed = tokio::time::timeout(LIMIT, chat.close()).await;

    assert!(
        closed.is_ok(),
        "close did not return within {LIMIT:?} of the tool's answer"
    );
    closes_since(&before, 1, "the chat's session").await;
}

/// A chat dropped while its tool runs a call that never answers: the drop cancels the session,
/// which is closed without the tool's answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_dropped_while_its_tool_runs_closes_its_session() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling_round(&tool, &["Berlin"]);
    let (running, release) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let slow: Box<dyn ToolExecutor> = Box::new(Slow {
        running: Arc::clone(&running),
        release: Arc::clone(&release),
    });
    let before = rows();
    let mut chat = Chat::new_with(&chatting(generator))
        .expect("plans")
        .tool((tool, slow))
        .expect("plans with the tool");

    let given_up = tokio::time::timeout(Duration::from_millis(300), chat.send(PROMPT)).await;
    assert!(given_up.is_err(), "the tool has not answered: {given_up:?}");
    assert!(running.load(Ordering::SeqCst), "the tool runs the call");
    drop(chat);

    closes_since(&before, 1, "the dropped chat's session").await;
    release.store(true, Ordering::SeqCst);
}

fn pcm16(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).expect("a rate"),
        channels: NonZeroU16::MIN,
    }
}

/// A synthesiser with a defect: the mock, whose every synthesis panics as it yields its third
/// audio frame.
struct Defective(MockSynthesiser);

struct DefectiveSynthesis {
    inner: MockSynthesis,
    frames: usize,
}

impl Synthesiser for Defective {
    type Handle = DefectiveSynthesis;

    fn adapter(&self) -> &Adapter {
        self.0.adapter()
    }

    fn start(&mut self) -> Result<Self::Handle, ConcurrencyLimitReached> {
        self.0
            .start()
            .map(|inner| DefectiveSynthesis { inner, frames: 0 })
    }
}

impl Iterator for DefectiveSynthesis {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        let event = self.inner.next()?;
        if matches!(event, SynthesisEvent::Audio(_)) {
            self.frames += 1;
            assert!(self.frames < 3, "the synthesiser has a defect");
        }
        Some(event)
    }
}

impl SynthesisHandle for DefectiveSynthesis {
    fn output(&self) -> &SynthesisOutput {
        self.inner.output()
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        self.inner.offer_segment(segment)
    }

    fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        self.inner.end_utterance()
    }

    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        self.inner.acknowledge_playback(segment_id, receipt)
    }

    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        self.inner.discard(segment_id)
    }
}

fn defective() -> Stack {
    Stack::new().bind(Binding::tts(Defective(MockSynthesiser::new(
        tts::MockConfiguration::IncrementalStreaming,
        ModelId::generate(),
        pcm16(16_000),
    ))))
}

const WORDS: &str = "one two three four five six";

/// A synthesiser that fails mid-way through a text: `Voice::synthesise` returns an error, and
/// the session it opened is closed.
#[tokio::test(flavor = "multi_thread")]
async fn a_voice_whose_synthesiser_fails_mid_way_returns_an_error_and_closes_its_session() {
    let _held = SESSIONS.lock().await;
    let before = rows();
    let mut voice = Voice::new_with(&defective()).expect("plans");

    let synthesised = tokio::time::timeout(LIMIT, voice.synthesise(WORDS)).await;

    let Ok(synthesised) = synthesised else {
        panic!("synthesise did not return within {LIMIT:?} of its synthesiser's failure");
    };
    assert!(
        synthesised.is_err(),
        "a text cut at its third frame is no success: {} frames",
        synthesised.map_or(0, |frames| frames.len())
    );
    closes_since(&before, 1, "the voice's session").await;
}

/// The same through `speak_with`: it returns an error, its session is closed and the speaker it
/// opened is let go.
#[tokio::test(flavor = "multi_thread")]
async fn speak_with_over_a_synthesiser_that_fails_mid_way_returns_and_lets_go_of_the_speaker() {
    let _held = SESSIONS.lock().await;
    let rate = NonZeroU32::new(16_000).expect("a rate");
    let host = FakeHost::new(vec![FakeDevice::output(
        "test speaker",
        rate,
        NonZeroU16::MIN,
        true,
    )]);
    let before = rows();

    let spoken = tokio::time::timeout(
        LIMIT,
        inference::speak_with(&defective(), WORDS, host.clone()),
    )
    .await;

    let Ok(spoken) = spoken else {
        panic!("speak_with did not return within {LIMIT:?} of its synthesiser's failure");
    };
    assert!(
        spoken.is_err(),
        "a text cut at its third frame is no success"
    );
    let started = Instant::now();
    while host.output_open() {
        assert!(started.elapsed() < LIMIT, "the speaker is let go");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    closes_since(&before, 1, "the session speak_with opened").await;
}

// Adversary pass on story:tool-loop-bound-counts-rounds-per-input: the bound counted per input
// across several turns, at the bound, with several calls per reply; a reply that names a tool
// the model node was not given.

/// A model that reads each text of a conversation as `<replies>,<calls>`: it answers the text
/// with `replies` replies, one after the results of the one before, each ending with `calls`
/// calls of its one tool, and then with the text `<replies> replies of <calls>`. Each reply that
/// ends with calls says `.`.
struct RoundsPerText {
    provenance: Provenance,
    capabilities: Capabilities,
    tool: ToolId,
}

impl llm_core::Model for RoundsPerText {
    fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn turn<'a>(
        &'a self,
        request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        _: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, llm_core::Error>> {
        Box::pin(async move {
            let called = request
                .items
                .iter()
                .filter(|item| matches!(item, Item::ToolCall(_)))
                .count();
            let asked = request
                .items
                .iter()
                .enumerate()
                .rev()
                .find_map(|(at, item)| match item {
                    Item::UserText { text, .. } => Some((at, text.clone())),
                    _ => None,
                });
            let (at, text) = asked.unwrap_or((0, String::new()));
            let (replies, per_reply) = text
                .split_once(',')
                .and_then(|(replies, calls)| Some((replies.parse().ok()?, calls.parse().ok()?)))
                .unwrap_or((0_usize, 1_usize));
            let since = request.items[at..]
                .iter()
                .filter(|item| matches!(item, Item::ToolCall(_)))
                .count();
            let done = since / per_reply.max(1);
            let calls: Vec<ModelCall> = if done < replies {
                (0..per_reply)
                    .map(|index| ModelCall {
                        call_id: CallId::new(format!("call-{}", called + index))
                            .expect("a printable identifier"),
                        name: tool_name(self.tool),
                        arguments: city("Berlin"),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let text = if calls.is_empty() {
                format!("{replies} replies of {per_reply}")
            } else {
                ".".to_owned()
            };
            sink.emit(ModelEvent::TextDelta { text: text.clone() })
                .await?;
            let stop_reason = if calls.is_empty() {
                StopReason::EndTurn
            } else {
                StopReason::ToolCalls
            };
            let mut items = vec![Item::assistant(text)];
            items.extend(calls.into_iter().map(Item::ToolCall));
            Ok(TurnOutcome {
                stop_reason,
                items,
                observation: TurnObservation {
                    usage: None,
                    final_usage: true,
                    ..TurnObservation::new(self.provenance.clone())
                },
            })
        })
    }
}

/// A generator over [`RoundsPerText`], claiming streaming, concurrent input and tool calling.
fn rounds_per_text(tool: ToolId) -> Generator {
    let (adapter, _) = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "a model that answers each text with replies of calls".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: vec![
            Capability::OutputStreaming,
            Capability::ConcurrentInputOutput,
            Capability::ToolCalling,
        ],
        max_concurrency: 1,
        descriptor: AdapterDescriptor::default(),
    })
    .expect("a maximum concurrency of one is positive");
    let id = |value: &str| Id::new(value).expect("a printable identifier");
    let model = RoundsPerText {
        provenance: Provenance {
            protocol: Protocol::Responses,
            provider: id("adversary"),
            account: id("test"),
            endpoint: id("in-process"),
            model: id("rounds-per-text"),
            binding_revision: id("1"),
        },
        capabilities: Capabilities {
            tools: true,
            tool_choice: true,
            ..Capabilities::text(1 << 20, 1 << 16).with_output_streaming(true)
        },
        tool,
    };
    Generator::new(adapter, Arc::new(model) as Arc<dyn llm_core::Model>)
        .expect("the model declares what the adapter claims")
}

/// The bound counts "replies with calls per input" (`ess/domains/graph.yaml`, the feedback loop
/// declaration; `MAX_TOOL_ITERATIONS`, 4). Three texts, each answered with four replies of two
/// calls: every text is answered over one session, 24 calls. A fourth text whose model returns
/// calls a fifth time, three calls a reply, ends the session with the bound's error: the four
/// replies within the bound ran, and none of the three calls of the fifth.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_answers_each_text_with_four_replies_of_calls_and_ends_at_a_fifth() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let calls = Arc::default();
    let before = rows();
    let mut chat = Chat::new_with(&chatting(rounds_per_text(tool.tool_id())))
        .expect("plans")
        .tool((tool, recording(&calls)))
        .expect("plans with the tool");

    for turn in 1..=3 {
        let reply = tokio::time::timeout(LIMIT, chat.send("4,2"))
            .await
            .expect("send returns within the limit");
        match reply {
            Ok(reply) => assert_eq!(reply, "....4 replies of 2", "turn {turn}"),
            Err(error) => panic!(
                "turn {turn} of four replies of two calls failed: {:?}: {error}; the executor \
                 ran {} calls",
                error.kind(),
                ran(&calls).len()
            ),
        }
    }
    assert_eq!(ran(&calls).len(), 24, "three texts of four replies of two");
    assert_eq!(opened_since(&before).len(), 1, "one session took all three");

    let past = tokio::time::timeout(LIMIT, chat.send("5,3"))
        .await
        .expect("send returns within the limit");
    let error = past.expect_err("the fifth reply with calls to one text is past the bound");
    assert_eq!(error.kind(), inference::ErrorKind::Closed, "{error}");
    assert_eq!(
        ran(&calls).len(),
        24 + 12,
        "the four replies within the bound ran, and none of the three calls of the fifth"
    );
    let _ = tokio::time::timeout(LIMIT, chat.close()).await;
}

/// A mock generator with no script whose first model request of a conversation ends with
/// `round`, the calls of one reply.
fn proposing(round: Vec<ToolProposal>) -> MockGenerator {
    MockGenerator::new(
        llm::MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .proposing_round(round, 1)
}

/// The round bound of `chat` counts "the calls to send" (`crates/inference-pipeline/src/nodes/
/// llm.rs`): a call naming a tool the node was not given is answered `denied` at once and never
/// sent. A reply of eight calls of the given tool and one of a tool not given holds no more than
/// the feedback edge's 8: the executor runs the eight and `send` returns the model's reply, in
/// each of 10 conversations.
#[tokio::test(flavor = "multi_thread")]
async fn a_reply_of_eight_calls_and_one_to_a_tool_not_given_runs_the_eight() {
    let _held = SESSIONS.lock().await;
    let mut wrong = Vec::new();
    for conversation in 0..10 {
        let tool = lookup(false);
        let mut round: Vec<ToolProposal> = (0..8)
            .map(|_| ToolProposal {
                tool_id: tool.tool_id(),
                arguments: city("Berlin"),
            })
            .collect();
        round.push(ToolProposal {
            tool_id: ToolId::generate(),
            arguments: city("Paris"),
        });
        let calls = Arc::default();
        let mut chat = Chat::new_with(&chatting(proposing(round)))
            .expect("plans")
            .tool((tool, recording(&calls)))
            .expect("plans with the tool");
        let sent = tokio::time::timeout(LIMIT, chat.send(PROMPT)).await;
        let ran = ran(&calls).len();
        match sent {
            Ok(Ok(_)) if ran == 8 => {}
            Ok(sent) => wrong.push(format!(
                "conversation {conversation}: {sent:?}; the executor ran {ran} of 8 calls"
            )),
            Err(_) => wrong.push(format!(
                "conversation {conversation}: send did not return within {LIMIT:?}; the \
                 executor ran {ran} of 8 calls"
            )),
        }
        let _ = tokio::time::timeout(LIMIT, chat.close()).await;
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// "A call naming a tool the node was not given is dropped: the node answers it at once as
/// rejected `denied` ... and the generation continues without it" (`crates/inference-pipeline/
/// src/nodes/llm.rs`, module documentation). A reply whose one call names a tool the chat was
/// not given: `send` returns the model's reply to the denial, within the limit.
#[tokio::test(flavor = "multi_thread")]
async fn a_reply_whose_one_call_names_a_tool_not_given_is_answered() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let calls = Arc::default();
    let mut chat = Chat::new_with(&chatting(proposing(vec![ToolProposal {
        tool_id: ToolId::generate(),
        arguments: city("Paris"),
    }])))
    .expect("plans")
    .tool((tool, recording(&calls)))
    .expect("plans with the tool");

    let sent = tokio::time::timeout(LIMIT, chat.send(PROMPT)).await;

    let closed = tokio::time::timeout(LIMIT, chat.close()).await;
    let Ok(sent) = sent else {
        panic!(
            "send did not return within {LIMIT:?} of a reply whose one call names a tool not \
             given; the executor ran {} calls; a close then came to {closed:?}",
            ran(&calls).len()
        );
    };
    let reply = sent.expect("the model answers the denied call");
    assert!(reply.starts_with(PROMPT), "{reply}");
    assert!(ran(&calls).is_empty(), "the executor ran no call");
}
