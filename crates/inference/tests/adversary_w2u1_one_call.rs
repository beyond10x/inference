#![cfg(feature = "testing")]
//! Adversary cases against the one-call functions and the handles `Chat`, `Transcriber` and
//! `Voice` (`docs/design/sdk.md` §3.1, §3.2), and against what the unit changed beneath them: the
//! tool loop of `chat` given tools, which a text agent now runs too. Every test takes
//! [`SESSIONS`] first, as `tests/oneshot.rs` does, and bounds each wait with [`LIMIT`].

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use inference::audio::{AudioEncoding, AudioFormat};
use inference::audio_device::fake::{FakeDevice, FakeHost};
use inference::core::model::ModelId;
use inference::facade::session::{SessionState, SessionsRow};
use inference::text::Span;
use inference::tools::{
    CancellationSignal, DeclareTool, ExecutionFuture, Json, SideEffectClass, Tool, ToolCall,
    ToolExecutor,
};
use inference::{Agent, Binding, Chat, ErrorKind, Event, Stack, Transcriber, Voice};
use inference_testkit::llm::{self, MockGenerator, ToolProposal};
use inference_testkit::stt::synthetic_audio::tone_silence_tone_silence;
use inference_testkit::stt::{self, Cue, MockRecogniser, ScriptStep};
use inference_testkit::tts::{self, MockSynthesiser};

/// Held by every test of this file while it runs.
static SESSIONS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// How long a case waits for a call before it fails rather than hangs.
const LIMIT: Duration = Duration::from_secs(3);

/// The texts of the two spans [`stt::script`] commits, in order.
const HEARD: [&str; 2] = ["hello there", "how are you"];

fn pcm16(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).expect("a rate"),
        channels: NonZeroU16::MIN,
    }
}

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

/// Asserts the view has exactly `count` rows more than `before`, each of them `Closed`.
fn closed_sessions_since(before: &[SessionsRow], count: usize) {
    let new = opened_since(before);
    assert_eq!(new.len(), count, "{new:#?}");
    assert!(
        new.iter().all(|row| row.state == SessionState::Closed),
        "{new:#?}"
    );
}

fn speaking() -> Stack {
    Stack::new().bind(Binding::tts(MockSynthesiser::new(
        tts::MockConfiguration::IncrementalStreaming,
        ModelId::generate(),
        pcm16(16_000),
    )))
}

fn hearing(configuration: stt::MockConfiguration, script: Vec<Cue>) -> Stack {
    Stack::new().bind(Binding::stt(MockRecogniser::new(
        configuration,
        ModelId::generate(),
        script,
    )))
}

fn chatting(generator: &MockGenerator) -> Stack {
    Stack::new().bind(Binding::llm(generator.clone()))
}

/// A mock generator that streams one scripted reply first and answers every later turn with the
/// user text of that turn.
fn scripted() -> MockGenerator {
    MockGenerator::new(
        llm::MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        ["Hello", " from", " the mock"],
    )
}

const REPLY: &str = "Hello from the mock";

/// A tool executor that keeps the arguments of each call it is given and answers every call
/// with `answer`.
struct Recording {
    answer: Json,
    calls: Arc<Mutex<Vec<Json>>>,
}

impl ToolExecutor for Recording {
    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        self.calls
            .lock()
            .expect("no test panicked holding it")
            .push(call.arguments().clone());
        Box::pin(std::future::ready(Ok(self.answer.clone())))
    }
}

/// A tool executor whose tool has a defect: it panics when it is called.
struct Broken;

impl ToolExecutor for Broken {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        panic!("the tool has a defect")
    }
}

fn berlin() -> Json {
    inference::__private::serde_json::json!({"city": "Berlin"})
}

fn answer() -> Json {
    inference::__private::serde_json::json!({"temperature": 21})
}

/// A read-only tool whose schema requires a string `city`, declared by hand.
fn lookup(requires_confirmation: bool) -> Tool {
    Tool::declare(DeclareTool {
        name: "lookup".to_owned(),
        description: "The weather of a city".to_owned(),
        schema: inference::__private::serde_json::json!({
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

/// A mock generator with no script, which answers each turn with the texts it was given since
/// its last answer and ends each of its first `proposals` model requests with a call of `tool`
/// with [`berlin`].
fn calling(tool: &Tool, proposals: usize) -> MockGenerator {
    MockGenerator::new(
        llm::MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .proposing(tool.tool_id(), berlin(), proposals)
}

fn recording(calls: &Arc<Mutex<Vec<Json>>>) -> Box<dyn ToolExecutor> {
    Box::new(Recording {
        answer: answer(),
        calls: Arc::clone(calls),
    })
}

const PROMPT: &str = "the weather in Berlin";

/// `Chat`'s documentation sends a caller who wants to confirm calls to an `Agent`
/// (`crates/inference/src/chat.rs`, "Build an `Agent` to confirm calls"), and the design holds a
/// call that requires confirmation "until `agent.confirm(id)` or `agent.reject(id)`", its id
/// carried by `Event::ConfirmationRequested` (`docs/design/sdk.md` §3.2). A text agent given such
/// a tool now runs the tool loop, so it has to tell the held call's id.
#[tokio::test(flavor = "multi_thread")]
async fn a_text_agent_requests_the_confirmation_of_the_call_it_holds() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(true);
    let generator = calling(&tool, 1);
    let calls = Arc::default();
    let agent = Agent::text()
        .stack(chatting(&generator))
        .tool((tool, recording(&calls)))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    agent.send_text(PROMPT).await.expect("taken");
    agent.commit().await.expect("committed");

    let mut seen = Vec::new();
    let requested = tokio::time::timeout(LIMIT, async {
        while let Some(event) = events.next().await {
            let event = event.expect("no terminal error");
            if let Event::ConfirmationRequested { call_id, name, .. } = &event {
                return Some((*call_id, name.clone()));
            }
            seen.push(event);
        }
        None
    })
    .await;

    let Ok(Some((call_id, name))) = requested else {
        panic!(
            "no ConfirmationRequested within {LIMIT:?}: the call is held and nothing tells its \
             id; the events were {seen:#?}"
        );
    };
    assert_eq!(name, "lookup");
    assert!(agent.confirm(call_id), "the call was held");
    let said = tokio::time::timeout(LIMIT, async {
        while let Some(event) = events.next().await {
            if let Ok(Event::Said { text, .. }) = event
                && text.contains(&answer().to_string())
            {
                return true;
            }
        }
        false
    })
    .await;
    assert_eq!(said, Ok(true), "the reply carries the executor's answer");
    assert_eq!(
        *calls.lock().expect("no test panicked holding it"),
        [berlin()]
    );
    agent.close().await.expect("closes");
}

/// `Event::ToolCalled` is "the model called a tool" (`crates/inference/src/event.rs`); a text
/// agent given a tool now has its model call it, so its stream tells the call before the reply
/// that carries the tool's answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_text_agent_tells_the_call_of_its_tool() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling(&tool, 1);
    let calls = Arc::default();
    let agent = Agent::text()
        .stack(chatting(&generator))
        .tool((tool, recording(&calls)))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    agent.send_text(PROMPT).await.expect("taken");
    agent.commit().await.expect("committed");

    let mut seen = Vec::new();
    let answered = tokio::time::timeout(LIMIT, async {
        while let Some(event) = events.next().await {
            let event = event.expect("no terminal error");
            let last =
                matches!(&event, Event::Said { text, .. } if text.contains(&answer().to_string()));
            seen.push(event);
            if last {
                return true;
            }
        }
        false
    })
    .await;

    assert_eq!(
        answered,
        Ok(true),
        "the reply carries the answer: {seen:#?}"
    );
    assert_eq!(
        *calls.lock().expect("no test panicked holding it"),
        [berlin()],
        "the executor ran the call"
    );
    assert!(
        seen.iter()
            .any(|event| matches!(event, Event::ToolCalled { name, .. } if name == "lookup")),
        "the executor ran a call the stream never told: {seen:#?}"
    );
    agent.close().await.expect("closes");
}

/// A tool with a defect panics. The chat's `send` must come back, with a reply or an error: a
/// caller awaiting it has nothing else to act on.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_whose_tool_panics_returns_from_its_send() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling(&tool, 1);
    let mut chat = Chat::new_with(&chatting(&generator))
        .expect("plans")
        .tool((tool, Box::new(Broken) as Box<dyn ToolExecutor>))
        .expect("plans with the tool");

    let sent = tokio::time::timeout(LIMIT, chat.send(PROMPT)).await;

    assert!(
        sent.is_ok(),
        "send did not return within {LIMIT:?} of its tool's panic"
    );
    let closed = tokio::time::timeout(LIMIT, chat.close()).await;
    assert!(closed.is_ok(), "close did not return within {LIMIT:?}");
}

/// The way out of a `send` that waits on a tool that panicked: the caller gives the wait up and
/// closes the chat, which has to return.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_whose_tool_panicked_closes_once_its_send_was_given_up() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling(&tool, 1);
    let before = rows();
    let mut chat = Chat::new_with(&chatting(&generator))
        .expect("plans")
        .tool((tool, Box::new(Broken) as Box<dyn ToolExecutor>))
        .expect("plans with the tool");

    let _ = tokio::time::timeout(Duration::from_millis(500), chat.send(PROMPT)).await;
    let closed = tokio::time::timeout(LIMIT, chat.close()).await;

    assert!(closed.is_ok(), "close did not return within {LIMIT:?}");
    let new = opened_since(&before);
    assert!(
        new.iter().all(|row| row.state != SessionState::Ready),
        "no session is left ready: {new:#?}"
    );
}

/// `chat` given tools "declares the same authority and executor around its model node" as
/// `native_voice` (`crates/inference/src/recipe.rs`), whose session, closed while a call is held
/// for confirmation, denies the call and ends with no host answering it
/// (`tests/recipes.rs`, `closing_with_a_held_call_unanswered_rejects_it_denied`). The same close
/// of a `chat` run opened one layer down, through `Instantiation::open`.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_run_closed_with_a_call_held_for_confirmation_ends() {
    use inference::core::envelope::{EventEnvelope, EventId, Timebase};
    use inference::core::id::{NodeId, StreamId};
    use inference::facade::live::LiveSessions;
    use inference::pipeline::nodes::log::StreamEvent;
    use inference::text::{SpanId, TranscriptCommitted};
    use inference::{Env, Payload, Recipe};

    let _held = SESSIONS.lock().await;
    let tool = lookup(true);
    let generator = calling(&tool, 1);
    let calls = Arc::default();
    let stack = chatting(&generator)
        .resolve_with(&Env::new())
        .expect("resolves");
    let mut instantiation = Recipe::chat()
        .tools([(tool, recording(&calls))])
        .instantiate(&stack)
        .expect("chat plans with the tool");
    let sessions = LiveSessions::new();
    let (session, driver) = instantiation
        .open(&stack, &sessions)
        .await
        .expect("the run opens");
    let driving = tokio::spawn(driver.drive());
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
    let started = Instant::now();
    while authority.held().is_empty() {
        assert!(started.elapsed() < LIMIT, "the authority holds the call");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let ended = tokio::time::timeout(LIMIT, async {
        let closed = session.close().await;
        while session.receive().await.is_some() {}
        closed
    })
    .await;

    assert!(
        ended.is_ok(),
        "closing a chat session with a call held unanswered does not end within {LIMIT:?}"
    );
    assert!(
        calls
            .lock()
            .expect("no test panicked holding it")
            .is_empty(),
        "a held call does not run"
    );
    driving.abort();
}

/// "A tool that requires confirmation is never confirmed, since a chat has no one to ask: its
/// call is answered as rejected" (`crates/inference/src/chat.rs`).
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_answers_a_call_held_for_confirmation_as_rejected() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(true);
    let generator = calling(&tool, 1);
    let calls = Arc::default();
    let before = rows();
    let mut chat = Chat::new_with(&chatting(&generator))
        .expect("plans")
        .tool((tool, recording(&calls)))
        .expect("plans with the tool");

    let reply = tokio::time::timeout(LIMIT, chat.send(PROMPT))
        .await
        .expect("send returns within the limit")
        .expect("answers");

    assert!(reply.contains("rejected"), "{reply}");
    assert!(
        calls
            .lock()
            .expect("no test panicked holding it")
            .is_empty(),
        "a call nobody confirmed does not run"
    );
    chat.close().await.expect("closes");
    closed_sessions_since(&before, 1);
}

/// "A reply is whole once the model has answered and awaits no call, so it holds the text of
/// every turn of the tool loop, in order": two rounds of one call each.
#[tokio::test(flavor = "multi_thread")]
async fn a_reply_holds_every_turn_of_a_tool_loop_of_two_rounds() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling(&tool, 2);
    let calls = Arc::default();
    let mut chat = Chat::new_with(&chatting(&generator))
        .expect("plans")
        .tool((tool, recording(&calls)))
        .expect("plans with the tool");

    let reply = tokio::time::timeout(LIMIT, chat.send(PROMPT))
        .await
        .expect("send returns within the limit")
        .expect("answers");

    let answer = answer().to_string();
    assert_eq!(reply, format!("{PROMPT}{answer}{answer}"));
    assert_eq!(calls.lock().expect("no test panicked holding it").len(), 2);
    assert_eq!(
        tokio::time::timeout(LIMIT, chat.send("and now"))
            .await
            .expect("send returns within the limit")
            .expect("answers"),
        "and now",
        "the next reply starts after the end of the first"
    );
    chat.close().await.expect("closes");
}

/// One round of two calls: the reply is whole only once both were answered.
#[tokio::test(flavor = "multi_thread")]
async fn a_reply_waits_for_both_calls_of_one_round() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let round = [
        ToolProposal {
            tool_id: tool.tool_id(),
            arguments: berlin(),
        },
        ToolProposal {
            tool_id: tool.tool_id(),
            arguments: berlin(),
        },
    ];
    let generator = MockGenerator::new(
        llm::MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .proposing_round(round, 1);
    let calls = Arc::default();
    let mut chat = Chat::new_with(&chatting(&generator))
        .expect("plans")
        .tool((tool, recording(&calls)))
        .expect("plans with the tool");

    let reply = tokio::time::timeout(LIMIT, chat.send(PROMPT))
        .await
        .expect("send returns within the limit")
        .expect("answers");

    let answer = answer().to_string();
    assert_eq!(reply, format!("{PROMPT}{answer}{answer}"));
    assert_eq!(calls.lock().expect("no test panicked holding it").len(), 2);
    chat.close().await.expect("closes");
}

/// "A later text opens a new session and starts a new conversation" (`Chat::close`).
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_closed_and_sent_to_again_starts_a_new_conversation() {
    let _held = SESSIONS.lock().await;
    let generator = scripted();
    let before = rows();
    let mut chat = Chat::new_with(&chatting(&generator)).expect("plans");

    assert_eq!(chat.send("first").await.expect("answers"), REPLY);
    chat.close().await.expect("closes");
    chat.close()
        .await
        .expect("a second close finds nothing open");
    let again = tokio::time::timeout(LIMIT, chat.send("second"))
        .await
        .expect("send returns within the limit")
        .expect("answers");

    assert_eq!(again, REPLY, "the first reply of a new conversation");
    assert_eq!(generator.asked(), [["first"], ["second"]]);
    chat.close().await.expect("closes");
    closed_sessions_since(&before, 2);
}

/// "After either of those two the session is closed, and the next text starts a new
/// conversation" (`Chat::stream`), here after the tool loop reached its bound.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_past_the_tool_loops_bound_starts_a_new_conversation_with_the_next_text() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling(&tool, 5);
    let calls = Arc::default();
    let before = rows();
    let mut chat = Chat::new_with(&chatting(&generator))
        .expect("plans")
        .tool((tool, recording(&calls)))
        .expect("plans with the tool");

    let first = tokio::time::timeout(LIMIT, chat.send(PROMPT))
        .await
        .expect("send returns within the limit")
        .expect_err("the fifth call is past the bound");
    assert_eq!(first.kind(), ErrorKind::Closed, "{first}");
    let second = tokio::time::timeout(LIMIT, chat.send("again"))
        .await
        .expect("send returns within the limit")
        .expect_err("the new conversation reaches the bound as the first did");

    assert_eq!(second.kind(), ErrorKind::Closed, "{second}");
    let asked = generator.asked();
    assert!(
        asked
            .iter()
            .any(|request| request.as_slice() == ["again".to_owned()]),
        "a request of a new conversation carries the new text alone: {asked:#?}"
    );
    closed_sessions_since(&before, 2);
}

/// "A stream dropped before its end leaves the rest of its reply unread, and the next call reads
/// past it before it sends its text" (`Chat::stream`).
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_dropped_half_way_is_read_past_by_the_next_send() {
    let _held = SESSIONS.lock().await;
    let generator = scripted();
    let mut chat = Chat::new_with(&chatting(&generator)).expect("plans");

    {
        let mut deltas = std::pin::pin!(chat.stream("first"));
        let delta = deltas.next().await.expect("a delta").expect("no error");
        assert_eq!(delta, "Hello");
    }
    let second = tokio::time::timeout(LIMIT, chat.send("second"))
        .await
        .expect("send returns within the limit")
        .expect("answers");

    assert_eq!(second, "second", "none of the first reply's deltas");
    assert_eq!(generator.asked(), [vec!["first"], vec!["first", "second"]]);
    chat.close().await.expect("closes");
}

/// `speak_with` on a host with no speaker: the device error, and the session it opened closed.
#[tokio::test(flavor = "multi_thread")]
async fn speak_with_on_a_host_without_a_speaker_closes_what_it_opened() {
    let _held = SESSIONS.lock().await;
    let rate = NonZeroU32::new(16_000).expect("a rate");
    let host = FakeHost::new(vec![FakeDevice::input(
        "test microphone",
        rate,
        NonZeroU16::MIN,
        true,
    )]);
    let before = rows();

    let error = tokio::time::timeout(
        LIMIT,
        inference::speak_with(&speaking(), "one two three", host.clone()),
    )
    .await
    .expect("the call returns within the limit")
    .expect_err("no speaker opens");

    assert_eq!(error.kind(), ErrorKind::Device, "{error}");
    let new = opened_since(&before);
    assert!(
        new.iter().all(|row| row.state == SessionState::Closed),
        "{new:#?}"
    );
}

/// `talk_with` on a host with no microphone: the device error, its session closed and the
/// speaker it opened let go.
#[tokio::test(flavor = "multi_thread")]
async fn talk_with_on_a_host_without_a_microphone_lets_go_of_the_speaker() {
    use inference_testkit::duplex::mock::MockSpeechModel;

    let _held = SESSIONS.lock().await;
    let rate = NonZeroU32::new(24_000).expect("a rate");
    let host = FakeHost::new(vec![FakeDevice::output(
        "test speaker",
        rate,
        NonZeroU16::MIN,
        true,
    )]);
    let stack = Stack::new().bind(Binding::speech(MockSpeechModel::native(Vec::new())));
    let before = rows();

    let error = tokio::time::timeout(LIMIT, inference::talk_with(&stack, host.clone()))
        .await
        .expect("the call returns within the limit")
        .expect_err("no microphone opens");

    assert_eq!(error.kind(), ErrorKind::Device, "{error}");
    let started = Instant::now();
    while host.output_open() {
        assert!(started.elapsed() < LIMIT, "the speaker is let go");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    closed_sessions_since(&before, 1);
}

/// The empty text: `synthesise` returns, with frames or an error, and leaves no session open.
#[tokio::test(flavor = "multi_thread")]
async fn a_voice_given_the_empty_text_returns_and_leaves_no_session_open() {
    let _held = SESSIONS.lock().await;
    let before = rows();
    let mut voice = Voice::new_with(&speaking()).expect("plans");

    let _ = tokio::time::timeout(LIMIT, voice.synthesise(""))
        .await
        .expect("the call returns within the limit");

    let new = opened_since(&before);
    assert!(
        new.iter().all(|row| row.state == SessionState::Closed),
        "{new:#?}"
    );
}

/// No frame at all: `transcribe_frames` returns and its one session is closed.
#[tokio::test(flavor = "multi_thread")]
async fn a_transcriber_given_no_frame_returns_and_closes_its_session() {
    let _held = SESSIONS.lock().await;
    let before = rows();
    let stack = hearing(stt::MockConfiguration::Incremental, stt::script());
    let mut transcriber = Transcriber::new_with(&stack).expect("plans");

    tokio::time::timeout(LIMIT, transcriber.transcribe_frames(Vec::new()))
        .await
        .expect("the call returns within the limit")
        .expect("no audio is no failure");

    closed_sessions_since(&before, 1);
}

/// A WAV file with a header and no sample.
#[tokio::test(flavor = "multi_thread")]
async fn transcribe_with_over_a_wav_file_of_no_sample_returns() {
    let _held = SESSIONS.lock().await;
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join("empty.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    hound::WavWriter::create(&path, spec)
        .expect("the file is created")
        .finalize()
        .expect("the file is finished");
    let before = rows();
    let stack = hearing(stt::MockConfiguration::Incremental, Vec::new());

    let text = tokio::time::timeout(LIMIT, inference::transcribe_with(&stack, &path))
        .await
        .expect("the call returns within the limit")
        .expect("an empty recording is no failure");

    assert_eq!(text, "");
    closed_sessions_since(&before, 1);
}

/// A batch recogniser, which answers only after the end of its input, as the whisper.cpp
/// adapter does: `transcribe`'s node is given one turn that no node completes, so the end of the
/// audio has to end the recognition.
#[tokio::test(flavor = "multi_thread")]
async fn a_transcriber_over_a_batch_recogniser_returns_its_spans() {
    let _held = SESSIONS.lock().await;
    let before = rows();
    let stack = hearing(stt::MockConfiguration::Batch, stt::script());
    let mut transcriber = Transcriber::new_with(&stack).expect("plans");

    let spans = tokio::time::timeout(
        LIMIT,
        transcriber.transcribe_frames(tone_silence_tone_silence()),
    )
    .await
    .expect("the call returns within the limit")
    .expect("transcribes");

    let texts: Vec<&str> = spans.iter().map(Span::text).collect();
    assert_eq!(texts, HEARD, "{spans:#?}");
    closed_sessions_since(&before, 1);
}

/// A recogniser that revises its hypothesis often: the spans come from the run's log, so a
/// reader of the session's events that fell behind takes nothing from the result.
#[tokio::test(flavor = "multi_thread")]
async fn a_transcriber_returns_its_spans_however_many_events_the_session_sent() {
    let _held = SESSIONS.lock().await;
    let mut script = vec![Cue {
        after_frames: 1,
        step: ScriptStep::Hypothesis("draft 0".to_owned()),
    }];
    script.extend((1..=6000).map(|revision| Cue {
        after_frames: 1,
        step: ScriptStep::Revise {
            span: 0,
            text: format!("draft {revision}"),
        },
    }));
    script.push(Cue {
        after_frames: 2,
        step: ScriptStep::Commit { span: 0 },
    });
    let stack = hearing(stt::MockConfiguration::Incremental, script);
    let mut transcriber = Transcriber::new_with(&stack).expect("plans");

    let spans = tokio::time::timeout(
        Duration::from_secs(20),
        transcriber.transcribe_frames(tone_silence_tone_silence()),
    )
    .await
    .expect("the call returns within 20 s")
    .expect("transcribes");

    let texts: Vec<&str> = spans.iter().map(Span::text).collect();
    assert_eq!(texts, ["draft 6000"]);
}

/// A long text on a current-thread runtime, as `#[tokio::test]` and
/// `#[tokio::main(flavor = "current_thread")]` start: every frame of the reply is returned.
#[tokio::test]
async fn a_voice_returns_every_frame_of_a_long_text_on_a_current_thread_runtime() {
    let _held = SESSIONS.lock().await;
    let words = vec!["word"; 6000].join(" ");
    let mut voice = Voice::new_with(&speaking()).expect("plans");

    let frames = tokio::time::timeout(Duration::from_secs(20), voice.synthesise(words))
        .await
        .expect("the call returns within 20 s")
        .expect("synthesises");

    assert_eq!(frames.len(), 6000);
}

/// The control of [`a_chat_run_closed_with_a_call_held_for_confirmation_ends`]: the same run, the
/// same close, a tool that needs no confirmation. Its call runs and the close ends.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_run_closed_after_its_call_ran_ends() {
    use inference::core::envelope::{EventEnvelope, EventId, Timebase};
    use inference::core::id::{NodeId, StreamId};
    use inference::facade::live::LiveSessions;
    use inference::pipeline::nodes::log::StreamEvent;
    use inference::text::{SpanId, TranscriptCommitted};
    use inference::{Env, Payload, Recipe};

    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling(&tool, 1);
    let calls: Arc<Mutex<Vec<Json>>> = Arc::default();
    let stack = chatting(&generator)
        .resolve_with(&Env::new())
        .expect("resolves");
    let mut instantiation = Recipe::chat()
        .tools([(tool, recording(&calls))])
        .instantiate(&stack)
        .expect("chat plans with the tool");
    let sessions = LiveSessions::new();
    let (session, driver) = instantiation
        .open(&stack, &sessions)
        .await
        .expect("the run opens");
    let driving = tokio::spawn(driver.drive());
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
    let started = Instant::now();
    while calls
        .lock()
        .expect("no test panicked holding it")
        .is_empty()
    {
        assert!(started.elapsed() < LIMIT, "the executor runs the call");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let ended = tokio::time::timeout(LIMIT, async {
        let closed = session.close().await;
        while session.receive().await.is_some() {}
        closed
    })
    .await;

    assert!(
        ended.is_ok(),
        "closing a chat session whose call ran does not end within {LIMIT:?}"
    );
    driving.abort();
}

/// "A chat dropped without it cancels its session" (`crates/inference/src/chat.rs`): the last way
/// out of a chat whose tool panicked. Its session's row in the view becomes `Closed`.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_dropped_after_its_tool_panicked_closes_its_session() {
    let _held = SESSIONS.lock().await;
    let tool = lookup(false);
    let generator = calling(&tool, 1);
    let before = rows();
    let mut chat = Chat::new_with(&chatting(&generator))
        .expect("plans")
        .tool((tool, Box::new(Broken) as Box<dyn ToolExecutor>))
        .expect("plans with the tool");

    let _ = tokio::time::timeout(Duration::from_millis(500), chat.send(PROMPT)).await;
    drop(chat);

    let started = Instant::now();
    loop {
        let new = opened_since(&before);
        if new.iter().all(|row| row.state == SessionState::Closed) {
            break;
        }
        assert!(
            started.elapsed() < LIMIT,
            "the dropped chat's session is not closed after {LIMIT:?}: {new:#?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The same on the multi-thread runtime `#[inference::main]` starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_voice_returns_every_frame_of_a_long_text() {
    let _held = SESSIONS.lock().await;
    let words = vec!["word"; 6000].join(" ");
    let mut voice = Voice::new_with(&speaking()).expect("plans");

    let frames = tokio::time::timeout(Duration::from_secs(20), voice.synthesise(words))
        .await
        .expect("the call returns within 20 s")
        .expect("synthesises");

    assert_eq!(frames.len(), 6000);
}
