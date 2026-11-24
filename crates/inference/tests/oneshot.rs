#![cfg(feature = "testing")]
//! The one-call functions and the handles `Chat`, `Transcriber` and `Voice` over the testkit
//! mocks and the fake audio host (`docs/design/sdk.md` §3.1, §3.2, §3.5). Every test takes
//! [`SESSIONS`] first, so no two of them open sessions at once and the row counts are exact; each
//! builds its stack from typed mock bindings and calls a `_with` form or `new_with`, so none
//! reads the process environment.

use std::num::{NonZeroU16, NonZeroU32};

use inference::audio::{AudioEncoding, AudioFormat};
use inference::audio_device::fake::{FakeDevice, FakeHost};
use inference::core::model::ModelId;
use inference::facade::session::{SessionState, SessionsRow};
use inference::text::{Span, SpanState};
use inference::{Binding, Chat, Stack, Transcriber, Voice};
use inference_testkit::llm::{self, MockGenerator};
use inference_testkit::mark::{self, Mark, MockId};
use inference_testkit::stt::synthetic_audio::tone_silence_tone_silence;
use inference_testkit::stt::{self, MockRecogniser};
use inference_testkit::tts::{self, MockSynthesiser};

/// Held by every test of this file while it runs.
static SESSIONS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What the mock synthesiser is asked to say: three words, so three frames.
const WORDS: &str = "one two three";

fn pcm16(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).expect("a rate"),
        channels: NonZeroU16::MIN,
    }
}

/// A stack binding `tts` to a mock synthesiser of 16 kHz mono frames.
fn speaking() -> Stack {
    Stack::new().bind(Binding::tts(MockSynthesiser::new(
        tts::MockConfiguration::IncrementalStreaming,
        ModelId::generate(),
        pcm16(16_000),
    )))
}

/// A fake host with one device, a mono 16 kHz speaker: a microphone does not open on it.
fn speaker_only() -> FakeHost {
    let rate = NonZeroU32::new(16_000).expect("a rate");
    FakeHost::new(vec![FakeDevice::output(
        "test speaker",
        rate,
        NonZeroU16::MIN,
        true,
    )])
}

/// The marks of the mock synthesiser's reply to [`WORDS`]: reply 0, frames 0, 1 and 2.
fn reply_marks() -> Vec<Option<Mark>> {
    (0..3)
        .map(|frame| {
            Some(Mark {
                mock: MockId::Synthesiser,
                reply: 0,
                frame,
            })
        })
        .collect()
}

fn rows() -> Vec<SessionsRow> {
    inference::sessions().sessions_view()
}

/// Asserts the view has exactly one row more than `before`, and that this row is `Closed`.
fn one_closed_session_since(before: &[SessionsRow]) {
    let after = rows();
    let new: Vec<&SessionsRow> = after
        .iter()
        .filter(|row| !before.iter().any(|old| old.session_id == row.session_id))
        .collect();
    assert_eq!(after.len(), before.len() + 1, "one row more: {new:#?}");
    assert_eq!(new.len(), 1, "{new:#?}");
    assert_eq!(new[0].state, SessionState::Closed, "{new:#?}");
}

/// A stack binding `stt` to a mock recogniser replaying the fixture's script: two spans.
fn hearing() -> Stack {
    Stack::new().bind(Binding::stt(MockRecogniser::new(
        stt::MockConfiguration::Incremental,
        ModelId::generate(),
        stt::script(),
    )))
}

/// The texts of the two spans [`stt::script`] commits, in order.
const HEARD: [&str; 2] = ["hello there", "how are you"];

/// The recipe `transcribe` opens over an `stt` binding, takes audio and nothing else, and commits
/// the spans of that audio: its STT node is given no turn by any other node.
#[tokio::test(flavor = "multi_thread")]
async fn the_transcribe_recipe_opens_and_commits_the_spans_of_its_audio() {
    use inference::core::envelope::{EventEnvelope, EventId, Timebase};
    use inference::core::id::{NodeId, StreamId};
    use inference::facade::events::SessionEvent;
    use inference::facade::live::LiveSessions;
    use inference::runtime::run::RunState;
    use inference::{Env, Payload, Recipe};

    let _held = SESSIONS.lock().await;
    let stack = hearing().resolve_with(&Env::new()).expect("resolves");
    let mut instantiation = Recipe::transcribe()
        .instantiate(&stack)
        .expect("transcribe plans");
    let sessions = LiveSessions::new();
    let (session, driver) = instantiation
        .open(&stack, &sessions)
        .await
        .expect("the run opens");
    let driving = tokio::spawn(driver.drive());
    let stream_id = StreamId::generate();
    for (sequence, frame) in (1..).zip(tone_silence_tone_silence()) {
        let envelope = EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: session.run_id(),
            session_id: None,
            stream_id,
            sequence,
            producer: NodeId::generate(),
            port: "audio".to_owned(),
            kind: "inference.audio.AudioFrame".to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: Payload::Audio(frame),
        };
        session.send(envelope).await.expect("sent");
    }
    session.close().await.expect("a Ready session drains");
    let mut committed = Vec::new();
    while let Some(event) = session.receive().await {
        if let SessionEvent::TranscriptCommitted(span) = event {
            committed.push(span.text);
        }
    }
    let run = driving.await.expect("joins").expect("the run ends");
    assert_eq!(run.state(), RunState::Completed);
    assert_eq!(committed, HEARD);
}

/// Writes the fixture's audio, 16 kHz mono PCM16, as a WAV file in `dir` and returns its path.
fn fixture_wav(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("fixture.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("the file is created");
    for frame in tone_silence_tone_silence() {
        for sample in frame.data.as_chunks::<2>().0 {
            writer
                .write_sample(i16::from_le_bytes(*sample))
                .expect("a sample is written");
        }
    }
    writer.finalize().expect("the file is finished");
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn transcribe_with_returns_the_committed_text_of_a_wav_file() {
    let _held = SESSIONS.lock().await;
    let dir = tempfile::tempdir().expect("a directory");
    let path = fixture_wav(dir.path());
    let stack = hearing();
    let before = rows();

    let text = inference::transcribe_with(&stack, &path)
        .await
        .expect("transcribes");

    assert_eq!(text, HEARD.join(" "));
    one_closed_session_since(&before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transcriber_returns_the_scripted_spans_committed_and_in_order() {
    let _held = SESSIONS.lock().await;
    let stack = hearing();
    let mut transcriber = Transcriber::new_with(&stack).expect("plans");

    for _ in 0..2 {
        let before = rows();
        let spans = transcriber
            .transcribe_frames(tone_silence_tone_silence())
            .await
            .expect("transcribes");
        let texts: Vec<&str> = spans.iter().map(Span::text).collect();
        assert_eq!(texts, HEARD, "{spans:#?}");
        assert!(
            spans
                .iter()
                .all(|span| span.state() == SpanState::Committed),
            "{spans:#?}"
        );
        let session = transcriber.session().expect("the last call's session");
        assert!(
            spans.iter().all(|span| span.run_id() == session.run_id()),
            "the spans are the session's run's: {spans:#?}"
        );
        one_closed_session_since(&before);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transcriber_describes_transcribe_and_reaches_its_session_and_pipeline() {
    let _held = SESSIONS.lock().await;
    let stack = hearing();
    let mut transcriber = Transcriber::new_with(&stack).expect("plans");

    assert_eq!(transcriber.describe().recipe, "transcribe");
    let planned = transcriber.pipeline().pipeline_id();
    assert!(
        transcriber.describe().pipeline.is_some(),
        "the pipeline {planned} is described"
    );
    assert!(
        transcriber.session().is_none(),
        "no call opened a session yet"
    );

    transcriber
        .transcribe_frames(tone_silence_tone_silence())
        .await
        .expect("transcribes");
    let session = transcriber.session().expect("the last call's session");
    assert!(
        rows()
            .iter()
            .any(|row| row.session_id == session.session_id() && row.state == SessionState::Closed),
        "the session of the call is a closed row of the view"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_that_is_not_wav_is_refused_by_name_and_opens_no_session() {
    let _held = SESSIONS.lock().await;
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "not audio").expect("written");
    let stack = hearing();
    let before = rows();

    let error = inference::transcribe_with(&stack, &path)
        .await
        .expect_err("a text file is no recording");

    assert_eq!(error.kind(), inference::ErrorKind::Layer, "{error}");
    assert!(error.to_string().contains("notes.txt"), "{error}");
    assert_eq!(rows().len(), before.len(), "nothing opened");
}

#[tokio::test(flavor = "multi_thread")]
async fn speak_with_plays_every_frame_of_the_reply_on_the_host_with_no_pull() {
    let _held = SESSIONS.lock().await;
    let stack = speaking();
    let host = speaker_only();
    let before = rows();

    inference::speak_with(&stack, WORDS, host.clone())
        .await
        .expect("speaks");

    let played: Vec<Option<Mark>> = host.played_frames().iter().map(mark::decode).collect();
    assert_eq!(played, reply_marks(), "{:#?}", host.played());
    assert!(!host.output_open(), "the speaker is let go");
    one_closed_session_since(&before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_returns_the_frames_of_the_text_and_plays_the_same_frames() {
    let _held = SESSIONS.lock().await;
    let stack = speaking();
    let mut voice = Voice::new_with(&stack).expect("plans");
    let before = rows();

    let frames = voice.synthesise(WORDS).await.expect("synthesises");

    let marks: Vec<Option<Mark>> = frames.iter().map(mark::decode).collect();
    assert_eq!(marks, reply_marks(), "{frames:#?}");
    one_closed_session_since(&before);

    let host = speaker_only();
    let before = rows();
    voice.play(WORDS, host.clone()).await.expect("plays");
    assert_eq!(host.played_frames(), frames);
    one_closed_session_since(&before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_describes_speak_and_reaches_its_session_and_pipeline() {
    let _held = SESSIONS.lock().await;
    let stack = speaking();
    let mut voice = Voice::new_with(&stack).expect("plans");

    assert_eq!(voice.describe().recipe, "speak");
    let planned = voice.pipeline().pipeline_id();
    assert!(
        voice.describe().pipeline.is_some(),
        "the pipeline {planned} is described"
    );
    assert!(voice.session().is_none(), "no call opened a session yet");

    voice.synthesise(WORDS).await.expect("synthesises");
    let session = voice.session().expect("the last call's session");
    assert!(
        rows()
            .iter()
            .any(|row| row.session_id == session.session_id() && row.state == SessionState::Closed),
        "the session of the call is a closed row of the view"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_without_a_synthesiser_is_refused_when_it_is_built() {
    let _held = SESSIONS.lock().await;
    let before = rows();
    let error = Voice::new_with(&Stack::new()).expect_err("no tts is bound");
    assert_eq!(error.kind(), inference::ErrorKind::RoleUnbound, "{error}");
    assert_eq!(rows().len(), before.len(), "nothing opened");
}

/// The pieces the mock generator streams as the first reply of a conversation.
const PIECES: [&str; 3] = ["Hello", " from", " the mock"];

/// The first reply of a conversation with [`scripted`]: its pieces, joined.
const REPLY: &str = "Hello from the mock";

/// A mock generator that streams [`PIECES`] as the first reply of each conversation and answers
/// every later turn with the user text of that turn.
fn scripted() -> MockGenerator {
    MockGenerator::new(
        llm::MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        PIECES,
    )
}

/// A stack binding `llm` to a clone of `generator`, which records what both are asked.
fn chatting(generator: &MockGenerator) -> Stack {
    Stack::new().bind(Binding::llm(generator.clone()))
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_with_returns_the_scripted_reply_over_one_session() {
    let _held = SESSIONS.lock().await;
    let generator = scripted();
    let stack = chatting(&generator);
    let before = rows();

    let reply = inference::chat_with(&stack, "say hello")
        .await
        .expect("answers");

    assert_eq!(reply, REPLY);
    assert_eq!(generator.asked(), [["say hello"]]);
    one_closed_session_since(&before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_streams_the_deltas_whose_concatenation_is_the_reply() {
    use futures_util::StreamExt;

    let _held = SESSIONS.lock().await;
    let stack = chatting(&scripted());
    let mut chat = Chat::new_with(&stack).expect("plans");

    let deltas: Vec<String> = chat
        .stream("say hello")
        .map(|delta| delta.expect("a delta"))
        .collect()
        .await;

    assert_eq!(deltas, PIECES, "one delta per piece the model streamed");
    assert_eq!(deltas.concat(), REPLY);
    chat.close().await.expect("closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_keeps_history_so_a_second_send_carries_the_first_turn() {
    let _held = SESSIONS.lock().await;
    let generator = scripted();
    let stack = chatting(&generator);
    let mut chat = Chat::new_with(&stack).expect("plans");
    let before = rows();

    assert_eq!(chat.send("first").await.expect("answers"), REPLY);
    assert_eq!(chat.send("second").await.expect("answers"), "second");

    assert_eq!(
        generator.asked(),
        [vec!["first"], vec!["first", "second"]],
        "the second request carries the first turn's user text"
    );
    let open = rows();
    assert_eq!(
        open.len(),
        before.len() + 1,
        "both sends ran on one session"
    );
    chat.close().await.expect("closes");
    one_closed_session_since(&before);
}

/// A tool executor that keeps the arguments of each call it is given and answers every call
/// with `answer`.
struct Recording {
    answer: inference::tools::Json,
    calls: std::sync::Arc<std::sync::Mutex<Vec<inference::tools::Json>>>,
}

impl inference::tools::ToolExecutor for Recording {
    fn execute<'a>(
        &'a self,
        call: &'a inference::tools::ToolCall,
        _: std::time::Instant,
        _: &'a inference::tools::CancellationSignal,
    ) -> inference::tools::ExecutionFuture<'a> {
        self.calls
            .lock()
            .expect("no test panicked holding it")
            .push(call.arguments().clone());
        Box::pin(std::future::ready(Ok(self.answer.clone())))
    }
}

/// What the scripted generator of [`calling`] calls [`lookup`] with.
fn berlin() -> inference::tools::Json {
    inference::__private::serde_json::json!({"city": "Berlin"})
}

/// A read-only tool whose schema requires a string `city`, declared by hand.
fn lookup() -> inference::tools::Tool {
    use inference::tools::{DeclareTool, SideEffectClass, Tool};

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
        requires_confirmation: false,
    })
    .0
}

/// A mock generator with no script, which answers each turn with the texts it was given since
/// its last answer and ends each of its first `proposals` model requests with a call of `tool`
/// with [`berlin`].
fn calling(tool: &inference::tools::Tool, proposals: usize) -> MockGenerator {
    MockGenerator::new(
        llm::MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .proposing(tool.tool_id(), berlin(), proposals)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_tool_is_given_the_scripted_call_and_the_reply_carries_its_answer() {
    use inference::__private::serde_json::json;
    use inference::tools::ToolExecutor;

    let _held = SESSIONS.lock().await;
    let tool = lookup();
    let answer = json!({"temperature": 21});
    let generator = calling(&tool, 1);
    let stack = chatting(&generator);
    let calls = std::sync::Arc::default();
    let executor: Box<dyn ToolExecutor> = Box::new(Recording {
        answer: answer.clone(),
        calls: std::sync::Arc::clone(&calls),
    });
    let before = rows();
    let mut chat = Chat::new_with(&stack)
        .expect("plans")
        .tool((tool, executor))
        .expect("plans with the tool");

    let reply = chat.send("the weather in Berlin").await.expect("answers");

    assert_eq!(
        *calls.lock().expect("no test panicked holding it"),
        [berlin()],
        "the executor was given the scripted call once"
    );
    assert!(
        reply.contains(&answer.to_string()),
        "the reply carries the executor's answer: {reply}"
    );
    assert_eq!(
        generator.offered().len(),
        1,
        "the model was offered the tool"
    );
    chat.close().await.expect("closes");
    one_closed_session_since(&before);
}

/// The tool loop of `chat` returns a call to the model four times. A model that calls a fifth
/// time is answered as exhausted, which ends its generation: the reply never becomes whole.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_whose_tool_calls_reach_the_loops_bound_ends_its_session_with_an_error() {
    use inference::__private::serde_json::json;
    use inference::tools::ToolExecutor;

    let _held = SESSIONS.lock().await;
    let tool = lookup();
    let stack = chatting(&calling(&tool, 5));
    let calls = std::sync::Arc::default();
    let executor: Box<dyn ToolExecutor> = Box::new(Recording {
        answer: json!({"temperature": 21}),
        calls: std::sync::Arc::clone(&calls),
    });
    let before = rows();
    let mut chat = Chat::new_with(&stack)
        .expect("plans")
        .tool((tool, executor))
        .expect("plans with the tool");

    let error = chat
        .send("the weather in Berlin")
        .await
        .expect_err("the fifth call is past the bound");

    assert_eq!(error.kind(), inference::ErrorKind::Closed, "{error}");
    assert_eq!(
        calls.lock().expect("no test panicked holding it").len(),
        4,
        "the executor ran the calls within the bound"
    );
    one_closed_session_since(&before);
    chat.close().await.expect("nothing is left to close");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_is_an_error_of_its_send_and_the_conversation_goes_on() {
    let _held = SESSIONS.lock().await;
    let generator = scripted().refusing("I will not say that");
    let stack = chatting(&generator);
    let mut chat = Chat::new_with(&stack).expect("plans");
    let before = rows();

    let error = chat.send("say it").await.expect_err("the model refuses");

    assert_eq!(error.kind(), inference::ErrorKind::Provider, "{error}");
    assert!(error.to_string().contains("I will not say that"), "{error}");
    assert_eq!(chat.send("say hello").await.expect("answers"), REPLY);
    assert_eq!(rows().len(), before.len() + 1, "one session took both");
    chat.close().await.expect("closes");
    one_closed_session_since(&before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_describes_chat_and_reaches_its_session_and_pipeline() {
    let _held = SESSIONS.lock().await;
    let stack = chatting(&scripted());
    let mut chat = Chat::new_with(&stack).expect("plans");

    assert_eq!(chat.describe().recipe, "chat");
    let planned = chat.pipeline().pipeline_id();
    assert!(
        chat.describe().pipeline.is_some(),
        "the pipeline {planned} is described"
    );
    assert!(chat.session().is_none(), "no send opened a session yet");

    chat.send("say hello").await.expect("answers");
    let session_id = chat.session().expect("the open session").session_id();
    assert!(
        rows()
            .iter()
            .any(|row| row.session_id == session_id && row.state == SessionState::Ready),
        "the chat's session stays open between sends"
    );
    chat.close().await.expect("closes");
    assert!(
        rows()
            .iter()
            .any(|row| row.session_id == session_id && row.state == SessionState::Closed),
        "the closed session is still the chat's"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_without_a_generator_is_refused_when_it_is_built() {
    let _held = SESSIONS.lock().await;
    let before = rows();
    let error = Chat::new_with(&Stack::new()).expect_err("no llm is bound");
    assert_eq!(error.kind(), inference::ErrorKind::RoleUnbound, "{error}");
    assert_eq!(rows().len(), before.len(), "nothing opened");
}
