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

/// Three sentences: the mock speaks three, two and one frames of them, one frame per word.
const SENTENCES: [&str; 3] = ["One two three.", "Four five!!", "Six?"];

/// The text of [`SENTENCES`], with the spacing a caller might leave.
fn three_sentences() -> String {
    format!("  {}\n{}   {} ", SENTENCES[0], SENTENCES[1], SENTENCES[2])
}

/// The time `samples` samples take at `rate` hertz, truncated to whole nanoseconds.
fn samples_at(samples: u64, rate: u32) -> std::time::Duration {
    let nanos = u128::from(samples) * 1_000_000_000 / u128::from(rate);
    std::time::Duration::from_nanos(u64::try_from(nanos).expect("fits"))
}

/// The samples per channel of `frames`, which share one sample rate, and that rate.
fn samples_and_rate(frames: &[inference::audio::AudioFrame]) -> (u64, u32) {
    let rate = frames[0].format.sample_rate.get();
    assert!(
        frames
            .iter()
            .all(|frame| frame.format.sample_rate.get() == rate)
    );
    let samples = frames
        .iter()
        .map(|frame| u64::from(frame.sample_count))
        .sum();
    (samples, rate)
}

/// The names in `dir`, sorted.
fn names_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("the directory is read")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .into_string()
                .expect("a UTF-8 name")
        })
        .collect();
    names.sort();
    names
}

#[tokio::test(flavor = "multi_thread")]
async fn a_text_of_three_sentences_becomes_three_clips_of_their_own_frames_in_order() {
    let _held = SESSIONS.lock().await;
    let stack = speaking();
    let mut voice = Voice::new_with(&stack).expect("plans");
    let before = rows();

    let clips = voice
        .synthesise_clips(three_sentences())
        .await
        .expect("synthesises");

    let texts: Vec<&str> = clips.iter().map(|clip| clip.text.as_str()).collect();
    assert_eq!(texts, SENTENCES);
    let counts: Vec<usize> = clips.iter().map(|clip| clip.frames.len()).collect();
    assert_eq!(counts, [3, 2, 1], "{clips:#?}");
    for clip in &clips {
        let (samples, rate) = samples_and_rate(&clip.frames);
        assert_eq!(clip.duration, samples_at(samples, rate), "{}", clip.text);
        let alone = voice.synthesise(clip.text.clone()).await.expect("alone");
        assert_eq!(clip.frames, alone, "{}", clip.text);
    }
    let after = rows();
    assert_eq!(
        after.len(),
        before.len() + 6,
        "one session per sentence and call"
    );
    assert!(
        after
            .iter()
            .filter(|row| !before.iter().any(|old| old.session_id == row.session_id))
            .all(|row| row.state == SessionState::Closed),
        "every session of the clips is closed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_text_with_no_sentence_end_is_one_clip_and_an_empty_text_none_with_no_session() {
    let _held = SESSIONS.lock().await;
    let stack = speaking();
    let mut voice = Voice::new_with(&stack).expect("plans");
    let before = rows();

    for empty in ["", "   ", " \n\t "] {
        let clips = voice
            .synthesise_clips(empty)
            .await
            .expect("an empty text is no error");
        assert!(clips.is_empty(), "{empty:?}: {clips:#?}");
    }
    assert_eq!(rows().len(), before.len(), "no session opened");
    assert!(voice.session().is_none(), "no call opened a session");

    let clips = voice
        .synthesise_clips(" version 3.5 is out ")
        .await
        .expect("synthesises");
    assert_eq!(clips.len(), 1, "{clips:#?}");
    assert_eq!(clips[0].text, "version 3.5 is out");
    assert_eq!(clips[0].frames.len(), 4);
    one_closed_session_since(&before);
}

#[tokio::test(flavor = "multi_thread")]
async fn clips_written_to_a_directory_are_numbered_files_as_long_as_their_durations() {
    let _held = SESSIONS.lock().await;
    let stack = speaking();
    let mut voice = Voice::new_with(&stack).expect("plans");
    let dir = tempfile::tempdir().expect("a directory");

    let written = voice
        .synthesise_clips_to(three_sentences(), dir.path())
        .await
        .expect("writes");

    let names = ["clip-001.wav", "clip-002.wav", "clip-003.wav"];
    assert_eq!(names_in(dir.path()), names);
    let paths: Vec<std::path::PathBuf> = written.iter().map(|file| file.path.clone()).collect();
    let expected: Vec<std::path::PathBuf> =
        names.iter().map(|name| dir.path().join(name)).collect();
    assert_eq!(paths, expected);

    let mut read_back = std::time::Duration::ZERO;
    for file in &written {
        let reader = hound::WavReader::open(&file.path).expect("a WAV file");
        let spec = reader.spec();
        assert_eq!(
            (spec.channels, spec.bits_per_sample, spec.sample_format),
            (1, 16, hound::SampleFormat::Int)
        );
        let length = samples_at(u64::from(reader.duration()), spec.sample_rate);
        assert_eq!(length, file.duration, "{}", file.path.display());
        read_back += length;
    }
    let returned: std::time::Duration = written.iter().map(|file| file.duration).sum();
    assert_eq!(returned, read_back);
    assert_eq!(
        returned,
        samples_at(6 * 160, 16_000),
        "six frames of 160 samples"
    );

    let clips = voice
        .synthesise_clips(three_sentences())
        .await
        .expect("synthesises");
    for (file, clip) in written.iter().zip(&clips) {
        let mut reader = hound::WavReader::open(&file.path).expect("a WAV file");
        let samples: Vec<i16> = reader
            .samples::<i16>()
            .map(|s| s.expect("a sample"))
            .collect();
        let frames: Vec<i16> = clip
            .frames
            .iter()
            .flat_map(|frame| {
                frame
                    .data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| i16::from_le_bytes(*b))
            })
            .collect();
        assert_eq!(samples, frames, "the file holds the clip's samples");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_directory_or_an_existing_first_clip_is_refused_and_changes_nothing() {
    let _held = SESSIONS.lock().await;
    let stack = speaking();
    let mut voice = Voice::new_with(&stack).expect("plans");
    let dir = tempfile::tempdir().expect("a directory");
    let before = rows();

    let absent = dir.path().join("absent");
    let error = voice
        .synthesise_clips_to(three_sentences(), &absent)
        .await
        .expect_err("a missing directory is refused");
    assert!(error.to_string().contains("absent"), "{error}");
    assert!(!absent.exists(), "the directory is not made");
    assert!(names_in(dir.path()).is_empty());

    let first = dir.path().join("clip-001.wav");
    std::fs::write(&first, b"kept").expect("written");
    let error = voice
        .synthesise_clips_to(three_sentences(), dir.path())
        .await
        .expect_err("an existing clip-001.wav is refused");
    assert!(error.to_string().contains("clip-001.wav"), "{error}");
    assert_eq!(names_in(dir.path()), ["clip-001.wav"]);
    assert_eq!(
        std::fs::read(&first).expect("read"),
        b"kept",
        "not overwritten"
    );
    assert_eq!(rows().len(), before.len(), "no session opened");
}

/// The file names have three digits, so a text of a thousand sentences is refused before any
/// session opens.
#[tokio::test(flavor = "multi_thread")]
async fn a_text_of_more_than_999_sentences_is_refused_naming_the_limit_and_writes_nothing() {
    let _held = SESSIONS.lock().await;
    let stack = speaking();
    let mut voice = Voice::new_with(&stack).expect("plans");
    let dir = tempfile::tempdir().expect("a directory");
    let before = rows();

    let error = voice
        .synthesise_clips_to("Go. ".repeat(1000), dir.path())
        .await
        .expect_err("a thousand sentences are refused");

    assert!(error.to_string().contains("999"), "{error}");
    assert!(error.to_string().contains("1000"), "{error}");
    assert!(names_in(dir.path()).is_empty(), "nothing written");
    assert_eq!(rows().len(), before.len(), "no session opened");
}

#[tokio::test(flavor = "multi_thread")]
async fn clips_in_mu_law_or_a_law_are_refused_naming_the_encoding_and_write_nothing() {
    let _held = SESSIONS.lock().await;
    for (encoding, name) in [
        (AudioEncoding::PcmMulaw, "pcm_mulaw"),
        (AudioEncoding::PcmAlaw, "pcm_alaw"),
    ] {
        let format = AudioFormat {
            encoding,
            sample_rate: NonZeroU32::new(8_000).expect("a rate"),
            channels: NonZeroU16::MIN,
        };
        let stack = Stack::new().bind(Binding::tts(MockSynthesiser::new(
            tts::MockConfiguration::IncrementalStreaming,
            ModelId::generate(),
            format,
        )));
        let mut voice = Voice::new_with(&stack).expect("plans");
        let dir = tempfile::tempdir().expect("a directory");

        let error = voice
            .synthesise_clips_to(three_sentences(), dir.path())
            .await
            .expect_err("companded frames are refused");

        assert!(error.to_string().contains(name), "{error}");
        assert!(names_in(dir.path()).is_empty(), "{name}: nothing written");
    }
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

/// A tool executor whose tool has a defect: every call of it panics.
struct Defective;

impl inference::tools::ToolExecutor for Defective {
    fn execute<'a>(
        &'a self,
        _: &'a inference::tools::ToolCall,
        _: std::time::Instant,
        _: &'a inference::tools::CancellationSignal,
    ) -> inference::tools::ExecutionFuture<'a> {
        panic!("the tool has a defect")
    }
}

/// A tool that panics fails its call and nothing else: the model is given the failure as the
/// call's result and answers it, `send` returns that reply, and the conversation goes on over
/// the same session.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_whose_tool_panics_returns_the_models_reply_to_the_failed_call() {
    use inference::tools::ToolExecutor;

    let _held = SESSIONS.lock().await;
    let tool = lookup();
    let stack = chatting(&calling(&tool, 1));
    let before = rows();
    let mut chat = Chat::new_with(&stack)
        .expect("plans")
        .tool((tool, Box::new(Defective) as Box<dyn ToolExecutor>))
        .expect("plans with the tool");

    let reply = chat
        .send("the weather in Berlin")
        .await
        .expect("the model answers the failed call");

    assert!(
        reply.contains("the tool has a defect"),
        "the reply carries the failure the model was given: {reply}"
    );
    assert_eq!(chat.send("and now").await.expect("answers"), "and now");
    assert_eq!(rows().len(), before.len() + 1, "one session took both");
    chat.close().await.expect("closes");
    one_closed_session_since(&before);
}

/// `chat` given tools has one exposure more than without them, the egress `tool_events`, on
/// which its session is told each call the model proposes.
#[tokio::test]
async fn chat_given_tools_exposes_the_egress_tool_events() {
    use inference::__private::serde_json::json;
    use inference::Recipe;
    use inference::tools::ToolExecutor;

    let _held = SESSIONS.lock().await;
    let exposures = |recipe: Recipe| -> Vec<String> {
        let declaration = recipe.declaration();
        let mut names: Vec<String> = declaration
            .exposures()
            .map(|exposure| exposure.name().to_owned())
            .collect();
        names.sort();
        names
    };
    let executor: Box<dyn ToolExecutor> = Box::new(Recording {
        answer: json!({}),
        calls: std::sync::Arc::default(),
    });

    assert_eq!(
        exposures(Recipe::chat()),
        ["assistant_text", "usage", "user_input"]
    );
    assert_eq!(
        exposures(Recipe::chat().tools([(lookup(), executor)])),
        ["assistant_text", "tool_events", "usage", "user_input"]
    );
}

/// The tool loop of `chat` given tools and of `native_voice`, edge by edge: the three edges the
/// authority and the executor send on hold 4 envelopes and pause their producer when they are
/// full; the feedback edge holds 8 and is the one edge that fails on overload.
#[tokio::test]
async fn the_tool_loop_fails_on_its_feedback_edge_alone_and_pauses_on_the_others() {
    use inference::__private::serde_json::json;
    use inference::Recipe;
    use inference::graph::OverloadPolicy;
    use inference::tools::ToolExecutor;

    let _held = SESSIONS.lock().await;
    let edges = |recipe: Recipe| -> Vec<(String, String, usize, OverloadPolicy)> {
        let declaration = recipe.declaration();
        let port = |port_id| {
            let port = declaration
                .ports()
                .find(|port| port.port_id() == port_id)
                .expect("declared");
            let node = declaration.node(port.node_id()).expect("declared");
            format!("{}.{}", node.name(), port.name())
        };
        let mut edges: Vec<_> = declaration
            .edges()
            .map(|edge| {
                (
                    port(edge.from_port()),
                    port(edge.to_port()),
                    edge.capacity().get(),
                    edge.overload(),
                )
            })
            .collect();
        edges.sort_by(|left, right| left.0.cmp(&right.0));
        edges
    };
    let wired = |model: &str| -> Vec<(String, String, usize, OverloadPolicy)> {
        let mut edges = vec![
            (
                "authority.authorized".to_owned(),
                "executor.authorized".to_owned(),
                4,
                OverloadPolicy::Pause,
            ),
            (
                "authority.rejected".to_owned(),
                format!("{model}.tool_result"),
                4,
                OverloadPolicy::Pause,
            ),
            (
                "executor.outcomes".to_owned(),
                format!("{model}.tool_result"),
                4,
                OverloadPolicy::Pause,
            ),
            (
                format!("{model}.tool_call"),
                "authority.proposed".to_owned(),
                8,
                OverloadPolicy::Fail,
            ),
        ];
        edges.sort_by(|left, right| left.0.cmp(&right.0));
        edges
    };
    let executor: Box<dyn ToolExecutor> = Box::new(Recording {
        answer: json!({}),
        calls: std::sync::Arc::default(),
    });

    assert_eq!(
        edges(Recipe::chat().tools([(lookup(), executor)])),
        wired("agent")
    );
    assert_eq!(edges(Recipe::native_voice()), wired("speech"));
}

/// A mock generator with no script whose first model request of a conversation ends with `count`
/// calls of `tool` with [`berlin`], the calls of one round.
fn calling_round(tool: &inference::tools::Tool, count: usize) -> MockGenerator {
    let round = (0..count).map(|_| llm::ToolProposal {
        tool_id: tool.tool_id(),
        arguments: berlin(),
    });
    MockGenerator::new(
        llm::MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .proposing_round(round, 1)
}

/// An executor of [`lookup`] that answers every call at once.
fn answering() -> Box<dyn inference::tools::ToolExecutor> {
    Box::new(Recording {
        answer: inference::__private::serde_json::json!({"temperature": 21}),
        calls: std::sync::Arc::default(),
    })
}

/// How long a case of the tool loop waits for a call before it fails rather than hangs.
const LIMIT: std::time::Duration = std::time::Duration::from_secs(3);

/// A reply that ends with eight calls fills no edge of the tool loop, in each of 30
/// conversations: all eight reach the authority, which admits four calls of a generation and
/// answers the fifth as exhausted, so `send` returns the error of the loop's bound, never that of
/// a failed run, and never hangs.
#[tokio::test(flavor = "multi_thread")]
async fn a_round_of_eight_calls_fills_no_edge_of_the_tool_loop() {
    let _held = SESSIONS.lock().await;
    let mut wrong = Vec::new();
    for conversation in 0..30 {
        let tool = lookup();
        let stack = chatting(&calling_round(&tool, 8));
        let mut chat = Chat::new_with(&stack)
            .expect("plans")
            .tool((tool, answering()))
            .expect("plans with the tool");
        match tokio::time::timeout(LIMIT, chat.send("the weather in Berlin")).await {
            Ok(Err(error)) if error.kind() == inference::ErrorKind::Closed => {}
            Ok(sent) => wrong.push(format!("conversation {conversation}: {sent:?}")),
            Err(_) => wrong.push(format!("conversation {conversation}: send did not return")),
        }
        let closed = tokio::time::timeout(LIMIT, chat.close()).await;
        assert!(closed.is_ok(), "conversation {conversation}: close returns");
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// How many calls the reply of the cases below ends with: more than the tool loop can hold
/// between its model and its authority, so the feedback edge is full before the round is sent
/// and fails the model's node.
const OVERFLOWING: usize = 24;

/// What the error of a run failed by the full feedback edge says.
const FULL: &str = "the edge is full and fails on overload";

/// A chat whose tool loop failed an edge: `send` returns the error of the failed run, and
/// `close` returns.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_whose_tool_loop_fails_an_edge_returns_the_error_and_closes() {
    let _held = SESSIONS.lock().await;
    let tool = lookup();
    let stack = chatting(&calling_round(&tool, OVERFLOWING));
    let before = rows();
    let mut chat = Chat::new_with(&stack)
        .expect("plans")
        .tool((tool, answering()))
        .expect("plans with the tool");

    let error = tokio::time::timeout(LIMIT, chat.send("the weather in Berlin"))
        .await
        .expect("send returns within the limit")
        .expect_err("the run failed");

    assert!(error.to_string().contains(FULL), "{error}");
    tokio::time::timeout(LIMIT, chat.close())
        .await
        .expect("close returns within the limit")
        .expect("nothing is left to close");
    one_closed_session_since(&before);
}

/// A text agent whose tool loop failed an edge: its stream yields `Event::Error` naming the
/// failed run, and `close` returns.
#[tokio::test(flavor = "multi_thread")]
async fn a_text_agent_whose_tool_loop_fails_an_edge_tells_the_error_and_closes() {
    use futures_util::StreamExt;
    use inference::{Agent, Event};

    let _held = SESSIONS.lock().await;
    let tool = lookup();
    let stack = chatting(&calling_round(&tool, OVERFLOWING));
    let agent = Agent::text()
        .stack(stack)
        .tool((tool, answering()))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    agent
        .send_text("the weather in Berlin")
        .await
        .expect("taken");
    // The run may fail between the text and its commit, which the commit then reports: the
    // stream tells the error either way.
    let _ = agent.commit().await;

    let told = tokio::time::timeout(LIMIT, async {
        while let Some(event) = events.next().await {
            if let Ok(Event::Error { error, .. }) = event {
                return Some(error.to_string());
            }
        }
        None
    })
    .await
    .expect("the stream tells an error or ends within the limit");

    let told = told.expect("the stream yields an error before it ends");
    assert!(told.contains(FULL), "{told}");
    let _ = tokio::time::timeout(LIMIT, agent.close())
        .await
        .expect("close returns within the limit");
}

/// A `chat` run opened one layer down whose tool loop failed an edge: its driver ends with the
/// failed run, with no close asked for, and the session's stream ends.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_run_whose_tool_loop_fails_an_edge_ends_its_driver() {
    use inference::core::envelope::{EventEnvelope, EventId, Timebase};
    use inference::core::id::{NodeId, StreamId};
    use inference::facade::live::LiveSessions;
    use inference::pipeline::nodes::log::StreamEvent;
    use inference::text::{SpanId, TranscriptCommitted};
    use inference::{Env, Payload, Recipe};

    let _held = SESSIONS.lock().await;
    let tool = lookup();
    let stack = chatting(&calling_round(&tool, OVERFLOWING))
        .resolve_with(&Env::new())
        .expect("resolves");
    let mut instantiation = Recipe::chat()
        .tools([(tool, answering())])
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
        text: "the weather in Berlin".to_owned(),
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

    let driven = tokio::time::timeout(LIMIT, driving)
        .await
        .expect("the driver ends within the limit")
        .expect("the driver does not panic");

    assert!(driven.contains(FULL), "{driven}");
    tokio::time::timeout(LIMIT, async { while session.receive().await.is_some() {} })
        .await
        .expect("the session's stream ends within the limit");
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

/// `talk_with` over a native speech model scripted with one turn, which closes the session once
/// its script is spent: the microphone hears one utterance, the speaker plays the reply, and the
/// call returns with both devices let go and its one session closed.
#[tokio::test(flavor = "multi_thread")]
async fn talk_with_returns_once_the_model_closed_and_lets_go_of_both_devices() {
    use std::time::{Duration, Instant};

    use inference::audio_device::convert::decode;
    use inference_testkit::duplex::mock::{
        AUDIO_FRAMES, MockSpeechModel, Reply, ScriptedTurn, pcm, utterance,
    };

    const LIMIT: Duration = Duration::from_secs(5);

    let _held = SESSIONS.lock().await;
    let model = MockSpeechModel::native(vec![ScriptedTurn {
        heard: "hello".to_owned(),
        reply: Reply::Say("hi there".to_owned()),
    }])
    .closes();
    let stack = Stack::new().bind(Binding::speech(model));
    let rate = NonZeroU32::new(24_000).expect("a rate");
    let host = FakeHost::new(vec![
        FakeDevice::input("test microphone", rate, NonZeroU16::MIN, true),
        FakeDevice::output("test speaker", rate, NonZeroU16::MIN, true),
    ]);
    let before = rows();

    let speaking = async {
        let started = Instant::now();
        while !host.input_open() {
            assert!(started.elapsed() < LIMIT, "the call opens the microphone");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        for frame in utterance(pcm(24_000), 0).frames() {
            assert!(host.feed(&decode(&frame).expect("a well-formed frame")));
        }
    };
    let (talked, ()) = tokio::time::timeout(LIMIT, async {
        tokio::join!(inference::talk_with(&stack, host.clone()), speaking)
    })
    .await
    .expect("the call returns within 5 s");

    talked.expect("the conversation ran to the model's close");
    assert!(!host.input_open(), "the microphone is let go");
    assert!(!host.output_open(), "the speaker is let go");
    let played: Vec<Option<Mark>> = host.played_frames().iter().map(mark::decode).collect();
    let reply: Vec<Option<Mark>> = (0..AUDIO_FRAMES)
        .map(|frame| {
            Some(Mark {
                mock: MockId::Speech,
                reply: 0,
                frame,
            })
        })
        .collect();
    assert_eq!(played, reply, "{:#?}", host.played());
    one_closed_session_since(&before);
}

/// Adversary cases of story:sentence-clips-with-durations: the clip durations at a rate that does
/// not divide a frame and in stereo, the `pcm_f32le` file form, the splitter on whitespace,
/// marks and multi-byte text, the name check on links and later names, and the removal of the
/// files already written when a later file cannot be created.
mod adversary {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;

    /// A stack binding `tts` to a mock synthesiser of frames in `format`.
    fn speaking_in(format: AudioFormat) -> Stack {
        Stack::new().bind(Binding::tts(MockSynthesiser::new(
            tts::MockConfiguration::IncrementalStreaming,
            ModelId::generate(),
            format,
        )))
    }

    fn format(encoding: AudioEncoding, rate: u32, channels: u16) -> AudioFormat {
        AudioFormat {
            encoding,
            sample_rate: NonZeroU32::new(rate).expect("a rate"),
            channels: NonZeroU16::new(channels).expect("channels"),
        }
    }

    /// 44.1 kHz stereo: 160 samples per channel do not divide a second into whole nanoseconds,
    /// so the clip's duration truncates once over the clip, and counts samples per channel.
    #[tokio::test(flavor = "multi_thread")]
    async fn adversary_stereo_clips_at_44100_hz_last_their_samples_per_channel_truncated_once() {
        let _held = SESSIONS.lock().await;
        let stack = speaking_in(format(AudioEncoding::PcmS16le, 44_100, 2));
        let mut voice = Voice::new_with(&stack).expect("plans");
        let dir = tempfile::tempdir().expect("a directory");

        let clips = voice
            .synthesise_clips(three_sentences())
            .await
            .expect("synthesises");
        let durations: Vec<Duration> = clips.iter().map(|clip| clip.duration).collect();
        // 480, 320 and 160 samples per channel at 44 100 Hz, truncated to whole nanoseconds.
        let expected = [
            Duration::from_nanos(10_884_353),
            Duration::from_nanos(7_256_235),
            Duration::from_nanos(3_628_117),
        ];
        assert_eq!(durations, expected);

        let written = voice
            .synthesise_clips_to(three_sentences(), dir.path())
            .await
            .expect("writes");
        let returned: Vec<Duration> = written.iter().map(|file| file.duration).collect();
        assert_eq!(returned, expected);
        for file in &written {
            let reader = hound::WavReader::open(&file.path).expect("a WAV file");
            let spec = reader.spec();
            assert_eq!((spec.channels, spec.sample_rate), (2, 44_100));
            let nanos = u64::from(reader.duration()) * 1_000_000_000 / 44_100;
            assert_eq!(
                Duration::from_nanos(nanos),
                file.duration,
                "{}",
                file.path.display()
            );
        }
    }

    /// `pcm_f32le` audio is written as 32-bit float samples, every sample of every frame.
    #[tokio::test(flavor = "multi_thread")]
    async fn adversary_f32_clips_are_written_as_32_bit_float_files_of_their_samples() {
        let _held = SESSIONS.lock().await;
        let stack = speaking_in(format(AudioEncoding::PcmF32le, 22_050, 2));
        let mut voice = Voice::new_with(&stack).expect("plans");
        let dir = tempfile::tempdir().expect("a directory");

        let written = voice
            .synthesise_clips_to(three_sentences(), dir.path())
            .await
            .expect("f32 audio is written");
        let clips = voice
            .synthesise_clips(three_sentences())
            .await
            .expect("synthesises");
        assert_eq!(written.len(), 3);
        for (file, clip) in written.iter().zip(&clips) {
            let mut reader = hound::WavReader::open(&file.path).expect("a WAV file");
            let spec = reader.spec();
            assert_eq!(
                (
                    spec.channels,
                    spec.sample_rate,
                    spec.bits_per_sample,
                    spec.sample_format
                ),
                (2, 22_050, 32, hound::SampleFormat::Float)
            );
            let samples: Vec<u32> = reader
                .samples::<f32>()
                .map(|sample| sample.expect("a sample").to_bits())
                .collect();
            let frames: Vec<u32> = clip
                .frames
                .iter()
                .flat_map(|frame| {
                    frame
                        .data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| f32::from_le_bytes(*b).to_bits())
                        .collect::<Vec<u32>>()
                })
                .collect();
            assert_eq!(samples, frames, "{}", file.path.display());
            assert_eq!(file.duration, clip.duration);
        }
    }

    /// The splitter's rule over whitespace that is not a space, marks inside words, marks before
    /// a quote, a text of marks alone and multi-byte characters beside a mark.
    #[tokio::test(flavor = "multi_thread")]
    async fn adversary_the_splitter_follows_its_rule_on_unicode_whitespace_and_marks() {
        let _held = SESSIONS.lock().await;
        let stack = speaking();
        let mut voice = Voice::new_with(&stack).expect("plans");
        let cases: [(&str, &[&str]); 6] = [
            (
                "Ça va?\u{a0}Oui.\u{3000}3.14 est π.\nFin",
                &["Ça va?", "Oui.", "3.14 est π.", "Fin"],
            ),
            ("e.g.x is one", &["e.g.x is one"]),
            ("\"Hi.\" She left.", &["\"Hi.\" She left."]),
            ("Wait...\r\nWhat?!", &["Wait...", "What?!"]),
            ("...", &["..."]),
            ("Grüß dich!Ü ö.ä", &["Grüß dich!Ü ö.ä"]),
        ];
        for (text, expected) in cases {
            let clips = voice.synthesise_clips(text).await.expect("synthesises");
            let texts: Vec<&str> = clips.iter().map(|clip| clip.text.as_str()).collect();
            assert_eq!(texts, expected, "{text:?}");
        }
    }

    /// A name the text needs that is taken by a dangling link, or a later name taken while the
    /// first is free, is refused before any session opens, and the directory is unchanged.
    #[tokio::test(flavor = "multi_thread")]
    async fn adversary_a_later_name_or_a_dangling_link_is_refused_before_any_session() {
        let _held = SESSIONS.lock().await;
        let stack = speaking();
        let mut voice = Voice::new_with(&stack).expect("plans");
        let before = rows();

        let dir = tempfile::tempdir().expect("a directory");
        std::fs::write(dir.path().join("clip-003.wav"), b"kept").expect("written");
        let error = voice
            .synthesise_clips_to(three_sentences(), dir.path())
            .await
            .expect_err("an existing clip-003.wav is refused");
        assert!(error.to_string().contains("clip-003.wav"), "{error}");
        assert_eq!(names_in(dir.path()), ["clip-003.wav"]);

        let dir = tempfile::tempdir().expect("a directory");
        std::os::unix::fs::symlink(dir.path().join("nowhere"), dir.path().join("clip-002.wav"))
            .expect("linked");
        let error = voice
            .synthesise_clips_to(three_sentences(), dir.path())
            .await
            .expect_err("a dangling clip-002.wav is refused");
        assert!(error.to_string().contains("clip-002.wav"), "{error}");
        assert_eq!(names_in(dir.path()), ["clip-002.wav"]);

        let file = dir.path().join("plain");
        std::fs::write(&file, b"x").expect("written");
        let error = voice
            .synthesise_clips_to(three_sentences(), &file)
            .await
            .expect_err("a file is no directory");
        assert!(error.to_string().contains("plain"), "{error}");
        assert_eq!(rows().len(), before.len(), "no session opened");
    }

    /// `clip-002.wav` appears after the names were checked and before the files are written:
    /// the call fails naming it, removes `clip-001.wav` it wrote, and leaves the other file.
    /// The file is made by a thread once the call's first session is a row of the view; a try
    /// whose thread came after the call wrote `clip-002.wav` itself proves nothing and repeats.
    #[tokio::test(flavor = "multi_thread")]
    async fn adversary_a_name_taken_during_synthesis_fails_the_call_and_removes_its_files() {
        let _held = SESSIONS.lock().await;
        let stack = speaking();
        let mut voice = Voice::new_with(&stack).expect("plans");
        for _ in 0..20 {
            let dir = tempfile::tempdir().expect("a directory");
            let second = dir.path().join("clip-002.wav");
            let before = rows().len();
            let stop = Arc::new(AtomicBool::new(false));
            let taker = {
                let stop = Arc::clone(&stop);
                let second = second.clone();
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        if inference::sessions().sessions_view().len() > before {
                            return std::fs::OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(&second)
                                .is_ok();
                        }
                        std::thread::yield_now();
                    }
                    false
                })
            };
            let result = voice
                .synthesise_clips_to(three_sentences(), dir.path())
                .await;
            stop.store(true, Ordering::Release);
            let took = taker.join().expect("the thread ends");
            if !took {
                continue;
            }
            let error = result.expect_err("a taken clip-002.wav fails the call");
            assert!(error.to_string().contains("clip-002.wav"), "{error}");
            assert_eq!(
                names_in(dir.path()),
                ["clip-002.wav"],
                "the call removed the file it wrote and kept the other"
            );
            assert!(
                std::fs::read(&second).expect("read").is_empty(),
                "not overwritten"
            );
            return;
        }
        panic!("the thread never took clip-002.wav before the call wrote it");
    }

    /// The file form of an empty or whitespace-only text: no file, no error, no session.
    #[tokio::test(flavor = "multi_thread")]
    async fn adversary_an_empty_text_written_to_a_directory_is_no_file_no_error_no_session() {
        let _held = SESSIONS.lock().await;
        let stack = speaking();
        let mut voice = Voice::new_with(&stack).expect("plans");
        let dir = tempfile::tempdir().expect("a directory");
        let before = rows();

        for empty in ["", " \n\t "] {
            let written = voice
                .synthesise_clips_to(empty, dir.path())
                .await
                .expect("an empty text is no error");
            assert!(written.is_empty(), "{empty:?}: {written:#?}");
        }
        assert!(names_in(dir.path()).is_empty(), "nothing written");
        assert_eq!(rows().len(), before.len(), "no session opened");
    }
}
