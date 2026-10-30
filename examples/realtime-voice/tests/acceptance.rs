//! The story's acceptance without a headset: "On a laptop with a headset, the operator holds a
//! spoken conversation with each provider, interrupts it mid-answer and hears it stop, and asks
//! for the time and hears the tool's answer."
//!
//! One test per provider configuration of the demo, each one continuous conversation through
//! [`Conversation`]: (1) the user speaks and hears the answer, (2) speaks again mid-answer and
//! the answer stops, (3) asks for the time and hears the tool's answer.
//!
//! The model is the testkit's scripted mock behind a [`Provider`] that gives it the provider's
//! shape: what the conversation may ask of the session is what the provider's binding takes,
//! and where the mock would deliver an event the binding never delivers, the event is the one
//! the binding records instead. The session keeps a [`Log`] of what the conversation sent it.
//! The devices are the device adapter's fake laptop host, stereo at 48 kHz both ways, every
//! device callback made by the test. Speech is a constant sample of -0.25, which the mock hears
//! as speech it cannot read, so what it heard is the script's; silence is zero.

use std::time::Duration;

use inference_audio_device::fake::FakeHost;
use inference_duplex::inference_audio::{AudioFrame, SampleClockPosition};
use inference_duplex::inference_core::model::Adapter;
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseCompleted, ResponseId, ResponseState, SessionConfig,
    SessionError, SessionEvent, SessionLedger, SpeechCallId, SpeechCapabilities,
    SpeechSessionHandle, SpeechToolCallState,
};
use inference_testkit::duplex::mock::{MockSpeechModel, Reply, ScriptedTurn};
use realtime_voice::{
    Conversation, Ending, Note, Options, TIME_TOOL, nova_config, openai_config, openai_live_config,
};
use serde_json::json;

type MockSession = <MockSpeechModel as NativeSpeechModel>::Session;

/// The fake laptop's device rate.
const DEVICE_RATE: u64 = 48_000;

/// The output rate of every demo configuration.
const OUTPUT_RATE: u64 = 24_000;

/// Interleaved stereo samples of 10 ms at the device rate.
const TEN_MS: usize = 480 * 2;

/// Device frames of 10 ms.
const TEN_MS_FRAMES: usize = 480;

/// Samples of one 20 ms frame of the mock's output audio at the output rate.
const FRAME: usize = 480;

/// What the injected clock tells the time tool.
fn the_time() -> Json {
    json!({"local_time": "Tuesday 20 October 2026, 12:34"})
}

/// The conversation's three user turns.
fn script() -> Vec<ScriptedTurn> {
    vec![
        ScriptedTurn {
            heard: "tell me a story".to_owned(),
            reply: Reply::Say("Once upon a time.".to_owned()),
        },
        ScriptedTurn {
            heard: "wait".to_owned(),
            reply: Reply::Say("Sure.".to_owned()),
        },
        ScriptedTurn {
            heard: "what time is it".to_owned(),
            reply: Reply::Call {
                tool: TIME_TOOL.to_owned(),
                arguments: json!({}),
                then: "It is {result}.".to_owned(),
            },
        },
    ]
}

/// How the mock's events reach the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// As the mock delivers them: the binding delivers the same kinds of event.
    AsScripted,
    /// As the GPT-Live binding records them (`adapters/inference-openai/src/live_duplex.rs`):
    /// no speech start or stop and no commit (`live_duplex.rs:37`), a response the listener cut
    /// short completed, never cancelled (`live_duplex.rs:31-34`), and after a tool result the
    /// next response started by the provider (`live_duplex.rs:38-42`, `live_duplex.rs:980-992`).
    GptLive,
}

/// The mock in a provider's shape: the conversation reads `capabilities` for what it may ask.
struct Provider {
    mock: MockSpeechModel,
    capabilities: SpeechCapabilities,
    shape: Shape,
}

impl Provider {
    /// OpenAI Realtime: the native mock takes what the Realtime binding takes from the caller
    /// (`adapters/inference-openai/src/duplex.rs:146-159`): responses, cancellation and
    /// truncation. It reports the server's speech start (`duplex.rs:859-864`), and with
    /// `auto_interrupt` off neither it nor the provider cancels a response by itself.
    fn openai_realtime() -> Self {
        let mock = MockSpeechModel::native(script());
        let capabilities = mock.capabilities().clone();
        same_caller_operations(&capabilities, &inference_openai::duplex::capabilities());
        Self {
            mock,
            capabilities,
            shape: Shape::AsScripted,
        }
    }

    /// GPT-Live: the native mock, so that it opens the 24 kHz `semantic_vad` session, under the
    /// GPT-Live binding's own capabilities (`live_duplex.rs:153-166`): no response creation,
    /// cancellation or truncation by the caller. Its events are translated as [`Shape::GptLive`].
    fn gpt_live() -> Self {
        Self {
            mock: MockSpeechModel::native(script()),
            capabilities: inference_openai::live_duplex::capabilities(),
            shape: Shape::GptLive,
        }
    }

    /// Nova Sonic: the `server_turns` mock, which takes what the Nova Sonic binding takes
    /// (`adapters/inference-aws/src/duplex.rs:118-138`): no response creation, cancellation or
    /// truncation by the caller (`aws/src/duplex.rs:1061-1079`). It cancels a response on the
    /// provider's interruption notice and reports speech there (`aws/src/duplex.rs:751-760`), and
    /// starts the response after a tool result itself (`aws/src/duplex.rs:1083-1101`, `:820`).
    fn nova_sonic() -> Self {
        let mock = MockSpeechModel::server_turns(script());
        let capabilities = mock.capabilities().clone();
        same_caller_operations(
            &capabilities,
            &inference_aws::duplex::capabilities(inference_aws::nova_sonic::DEFAULT_MODEL_ID),
        );
        Self {
            mock,
            capabilities,
            shape: Shape::AsScripted,
        }
    }
}

/// The mock asks and takes from the caller what the binding does.
fn same_caller_operations(mock: &SpeechCapabilities, binding: &SpeechCapabilities) {
    let operations = |c: &SpeechCapabilities| {
        [
            c.manual_commit,
            c.manual_response,
            c.manual_interrupt,
            c.client_cancel,
            c.truncation,
            c.tool_calling,
        ]
    };
    assert_eq!(
        operations(mock),
        operations(binding),
        "commit, response, interrupt, cancel, truncation and tools as the binding takes them"
    );
}

impl NativeSpeechModel for Provider {
    type Session = Session;

    fn adapter(&self) -> &Adapter {
        self.mock.adapter()
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SessionConfig) -> Result<Session, OpenError> {
        self.mock.open(config).map(|inner| Session {
            inner,
            shape: self.shape,
            log: Log::default(),
            owed: false,
        })
    }
}

/// What the conversation sent the session.
#[derive(Debug, Default)]
struct Log {
    /// Samples of input audio accepted.
    input_samples: u64,
    /// Of those, samples that are not silence.
    speech_samples: u64,
    response_requests: usize,
    cancels: Vec<ResponseId>,
    truncations: Vec<(ResponseId, SampleClockPosition)>,
    results: Vec<(SpeechCallId, Json)>,
}

struct Session {
    inner: MockSession,
    shape: Shape,
    log: Log,
    /// A tool result was sent that the provider answers in a response it starts itself.
    owed: bool,
}

impl SpeechSessionHandle for Session {
    fn ledger(&self) -> &SessionLedger {
        self.inner.ledger()
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        let silence = frame.format.encoding.silence();
        let width = usize::try_from(frame.format.data_len(1))
            .unwrap_or(1)
            .max(1);
        let speech = frame
            .data
            .chunks(width)
            .filter(|sample| sample.iter().any(|byte| *byte != silence))
            .count();
        let samples = u64::from(frame.sample_count);
        self.inner.append_audio(frame)?;
        self.log.input_samples += samples;
        self.log.speech_samples += speech as u64;
        Ok(())
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.inner.commit_input()
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.log.response_requests += 1;
        self.inner.create_response()
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.log.cancels.push(response_id);
        self.inner.cancel_response(response_id)
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.log.truncations.push((response_id, heard));
        self.inner.truncate_output(response_id, heard)
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        self.log.results.push((call_id.clone(), result.clone()));
        self.inner.submit_tool_result(call_id, result)?;
        self.owed = self.shape == Shape::GptLive;
        Ok(())
    }

    fn close(&mut self) -> Result<(), SessionError> {
        self.inner.close()
    }

    fn next_event(&mut self, wait: Duration) -> Option<SessionEvent> {
        loop {
            if self.owed && self.inner.ledger().in_progress().is_none() {
                self.owed = false;
                self.inner
                    .create_response()
                    .expect("the provider continues after the tool result");
            }
            let event = self.inner.next_event(wait)?;
            if self.shape == Shape::AsScripted {
                return Some(event);
            }
            match event {
                SessionEvent::SpeechStarted(_)
                | SessionEvent::SpeechStopped(_)
                | SessionEvent::InputCommitted(_) => {}
                SessionEvent::ResponseCancelled(cancelled) => {
                    return Some(SessionEvent::ResponseCompleted(ResponseCompleted {
                        response_id: cancelled.response_id,
                    }));
                }
                event => return Some(event),
            }
        }
    }
}

fn open(provider: &mut Provider, config: SessionConfig, host: &FakeHost) -> Conversation<Session> {
    provider
        .capabilities
        .admit(&config)
        .expect("the provider takes the demo's configuration");
    assert_eq!(
        u64::from(config.output_format.sample_rate.get()),
        OUTPUT_RATE
    );
    Conversation::open(
        provider,
        config,
        host,
        Options::default(),
        Box::new(the_time),
    )
    .expect("the fake devices and the mock session open")
}

/// Feeds `millis` of a constant `sample` to the fake microphone.
fn capture(host: &FakeHost, millis: usize, sample: f32) {
    for _ in 0..millis / 10 {
        assert!(host.feed(&[sample; TEN_MS]), "the microphone runs");
    }
}

fn speak(host: &FakeHost, millis: usize) {
    capture(host, millis, -0.25);
}

fn pause(host: &FakeHost, millis: usize) {
    capture(host, millis, 0.0);
}

/// Pulls 10 ms from the fake speaker.
fn pull_ten_ms(host: &FakeHost) {
    let _ = host.pull(TEN_MS_FRAMES).expect("the speaker runs");
}

/// The position on the output clock the device reached after `device_frames` frames of it.
fn output_position(device_frames: usize) -> SampleClockPosition {
    let samples = device_frames as u64 * OUTPUT_RATE / DEVICE_RATE;
    SampleClockPosition::new(samples).expect("a position")
}

/// Handles one event at a time until the speaker holds the first audio of a response; that
/// response and the notes so far.
fn until_audio_plays(conversation: &mut Conversation<Session>) -> (ResponseId, Vec<Note>) {
    let mut notes = conversation.pump_microphone();
    for _ in 0..100 {
        notes.extend(conversation.pump_events(1));
        conversation.feed_speaker(&mut notes);
        if conversation.speaker().queued() > 0 {
            let response = conversation
                .session()
                .ledger()
                .in_progress()
                .expect("the answer is still in progress");
            return (response, notes);
        }
    }
    panic!("no audio reached the speaker: {notes:#?}");
}

/// Steps until `done` holds of the notes so far, at most 50 steps.
fn step_until(
    conversation: &mut Conversation<Session>,
    done: impl Fn(&[Note]) -> bool,
) -> Vec<Note> {
    let mut notes = Vec::new();
    for _ in 0..50 {
        notes.extend(conversation.step());
        if done(&notes) {
            return notes;
        }
    }
    panic!("not done within 50 steps: {notes:#?}");
}

/// The response that said `text`, once the notes say it completed.
fn completed_saying(notes: &[Note], text: &str) -> Option<ResponseId> {
    let response = notes.iter().find_map(|note| match note {
        Note::Said {
            response_id,
            text: said,
        } if said.contains(text) => Some(*response_id),
        _ => None,
    })?;
    notes
        .contains(&Note::Ended {
            response_id: response,
            how: Ending::Completed,
        })
        .then_some(response)
}

/// Pulls 10 ms and steps until the speaker has played everything queued and waiting.
fn play_out(host: &FakeHost, conversation: &mut Conversation<Session>) -> Vec<Note> {
    let mut notes = Vec::new();
    for _ in 0..50 {
        pull_ten_ms(host);
        notes.extend(conversation.step());
        if conversation.speaker().queued() == 0 {
            return notes;
        }
    }
    panic!("the speaker did not run dry: {notes:#?}");
}

fn refused(notes: &[Note]) -> bool {
    notes.iter().any(|note| matches!(note, Note::Refused(_)))
}

/// Step 1: the user speaks, the session receives the speech, both transcripts are noted and the
/// answer's audio reaches the speaker and plays. Returns the answer, still in progress, with
/// 10 ms of it played.
fn first_answer_plays(host: &FakeHost, conversation: &mut Conversation<Session>) -> ResponseId {
    let input_rate = u64::from(
        conversation
            .session()
            .ledger()
            .session()
            .input_format()
            .sample_rate
            .get(),
    );
    speak(host, 100);
    pause(host, 100);
    let (answer, notes) = until_audio_plays(conversation);

    let log = &conversation.session().log;
    assert!(
        log.speech_samples >= input_rate * 80 / 1_000 && log.speech_samples <= input_rate / 10,
        "the session received the 100 ms of speech at its own {input_rate} Hz, {} samples",
        log.speech_samples
    );
    assert!(
        log.input_samples >= log.speech_samples + input_rate * 60 / 1_000,
        "and the pause after it, {} samples in all",
        log.input_samples
    );
    assert!(
        notes.contains(&Note::Heard("tell me a story".to_owned())),
        "the user's transcript: {notes:#?}"
    );
    assert!(
        notes.contains(&Note::Said {
            response_id: answer,
            text: "Once upon a time.".to_owned(),
        }),
        "the transcript of the answer playing: {notes:#?}"
    );
    assert_eq!(
        conversation.speaker().queued(),
        FRAME,
        "the answer's first 20 ms frame is queued at the speaker"
    );

    pull_ten_ms(host);
    assert_eq!(
        conversation.speaker().played(),
        Some(output_position(TEN_MS_FRAMES)),
        "the device played 10 ms of the answer"
    );
    assert_eq!(conversation.speaker().queued(), FRAME - FRAME / 2);
    assert!(!refused(&notes), "nothing refused: {notes:#?}");
    answer
}

/// After step 2: the user stops speaking, the answer to the interruption is completed and
/// played out, so the speaker is idle before the time is asked.
fn interruption_answered(host: &FakeHost, conversation: &mut Conversation<Session>) {
    pause(host, 100);
    let mut notes = step_until(conversation, |notes| {
        completed_saying(notes, "Sure.").is_some()
    });
    notes.extend(play_out(host, conversation));
    assert_eq!(conversation.speaker().queued(), 0);
    assert!(!refused(&notes), "nothing refused: {notes:#?}");
}

/// Step 3: the user asks for the time; the model proposes the time tool, the conversation
/// answers it by call id with the injected clock's value, and the answer that follows plays.
/// `requests` is how many responses the conversation asks for after the call.
fn time_answer_plays(host: &FakeHost, conversation: &mut Conversation<Session>, requests: usize) {
    speak(host, 100);
    pause(host, 100);
    let notes = step_until(conversation, |notes| {
        completed_saying(notes, "12:34").is_some()
    });

    let call = SpeechCallId::new("call_1");
    assert!(
        notes.contains(&Note::Answered {
            call_id: call.clone(),
            result: the_time(),
        }),
        "the call is answered with the clock's value: {notes:#?}"
    );
    let ledger = conversation.session().ledger();
    let proposed = ledger.call(&call).expect("the model proposed the call");
    assert_eq!(proposed.state(), SpeechToolCallState::Answered);
    let offered = ledger
        .tools()
        .iter()
        .find(|tool| tool.tool_id() == proposed.tool_id())
        .map(|tool| tool.name().to_owned());
    assert_eq!(offered.as_deref(), Some(TIME_TOOL));
    assert_eq!(
        conversation.session().log.results,
        vec![(call.clone(), the_time())],
        "the session got the clock's value under the call's id, once"
    );
    assert_eq!(
        conversation.session().log.response_requests,
        requests,
        "responses the conversation asked for"
    );
    let spoken = completed_saying(&notes, "12:34").expect("the answer was said");
    assert_ne!(
        spoken,
        proposed.response_id(),
        "the answer is a response after the one that proposed the call"
    );
    assert_eq!(
        conversation.speaker().queued(),
        2 * FRAME,
        "both 20 ms frames of the answer are queued at the speaker"
    );

    pull_ten_ms(host);
    assert_eq!(
        conversation.speaker().played(),
        Some(output_position(TEN_MS_FRAMES)),
        "the device played 10 ms of the tool's answer"
    );
    assert!(
        !notes
            .iter()
            .any(|note| matches!(note, Note::BargeIn { .. } | Note::Refused(_))),
        "{notes:#?}"
    );
}

/// OpenAI Realtime: barge-in is the caller's rule (`auto_interrupt` off), so the conversation
/// stops playout, cancels the answer and truncates it where the listener stopped hearing it, and
/// asks for the response that speaks the tool's answer (the binding sends the result only,
/// `adapters/inference-openai/src/duplex.rs:1109-1120`).
#[test]
fn openai_realtime_conversation_is_interrupted_and_answers_the_time() {
    let host = FakeHost::laptop();
    let mut provider = Provider::openai_realtime();
    let mut conversation = open(&mut provider, openai_config(), &host);

    let answer = first_answer_plays(&host, &mut conversation);
    let heard = output_position(TEN_MS_FRAMES);

    speak(&host, 40);
    let mut notes = conversation.step();
    assert!(
        notes.contains(&Note::BargeIn {
            response_id: Some(answer),
            heard: Some(heard),
        }),
        "playout stopped where the device was: {notes:#?}"
    );
    assert_eq!(conversation.speaker().queued(), 0, "nothing more plays");
    assert_eq!(conversation.session().log.cancels, vec![answer]);
    assert_eq!(
        conversation.session().log.truncations,
        vec![(answer, heard)],
        "truncated at the 10 ms of the answer the device pulled"
    );
    assert!(notes.contains(&Note::Truncated {
        response_id: answer,
        heard,
    }));
    for _ in 0..5 {
        notes.extend(conversation.step());
    }
    assert_eq!(
        conversation.speaker().played(),
        None,
        "nothing of the answer reached the speaker after the stop: {notes:#?}"
    );
    assert!(notes.contains(&Note::Ended {
        response_id: answer,
        how: Ending::Cancelled,
    }));
    assert!(!refused(&notes), "nothing refused: {notes:#?}");

    interruption_answered(&host, &mut conversation);
    time_answer_plays(&host, &mut conversation, 1);
}

/// GPT-Live: the model takes its turns, its interruptions and its responses itself and reports
/// no speech, so the conversation gets no sign of the interruption. The model stops speaking;
/// the conversation neither cancels nor truncates, and what the speaker had queued still
/// plays. It asks for no response after the call: the binding asks the backend to continue.
#[test]
fn gpt_live_conversation_is_interrupted_and_answers_the_time() {
    let host = FakeHost::laptop();
    let mut provider = Provider::gpt_live();
    let config = openai_live_config();
    inference_openai::live_duplex::capabilities()
        .admit(&config)
        .expect("GPT-Live takes the demo's configuration");
    let mut conversation = open(&mut provider, config, &host);

    let answer = first_answer_plays(&host, &mut conversation);
    let queued = conversation.speaker().queued();

    speak(&host, 40);
    let mut notes = Vec::new();
    for _ in 0..5 {
        notes.extend(conversation.step());
    }
    assert!(
        notes.contains(&Note::Ended {
            response_id: answer,
            how: Ending::Completed,
        }),
        "the model stopped the answer; the binding records it completed: {notes:#?}"
    );
    assert_eq!(
        conversation.speaker().queued(),
        queued,
        "no further audio of the answer reached the speaker, and the conversation, given no \
         sign of the interruption, left what was queued to play: {notes:#?}"
    );
    assert!(
        !notes
            .iter()
            .any(|note| matches!(note, Note::BargeIn { .. } | Note::Truncated { .. })),
        "{notes:#?}"
    );
    assert!(conversation.session().log.cancels.is_empty());
    assert!(conversation.session().log.truncations.is_empty());
    assert!(!refused(&notes), "nothing refused: {notes:#?}");

    interruption_answered(&host, &mut conversation);
    time_answer_plays(&host, &mut conversation, 0);
}

/// Nova Sonic: the provider interrupts the answer itself and reports the user's speech at its
/// interruption notice, so the conversation stops playout at once, and asks for neither a
/// cancellation nor a truncation, which Nova Sonic takes neither of; it starts the response
/// after the tool result itself.
#[test]
fn nova_sonic_conversation_is_interrupted_and_answers_the_time() {
    let host = FakeHost::laptop();
    let mut provider = Provider::nova_sonic();
    let mut conversation = open(&mut provider, nova_config(), &host);

    let answer = first_answer_plays(&host, &mut conversation);
    let heard = output_position(TEN_MS_FRAMES);

    speak(&host, 40);
    let mut notes = conversation.step();
    assert!(
        notes.contains(&Note::BargeIn {
            response_id: Some(answer),
            heard: Some(heard),
        }),
        "playout stopped where the device was: {notes:#?}"
    );
    assert_eq!(conversation.speaker().queued(), 0, "nothing more plays");
    for _ in 0..5 {
        notes.extend(conversation.step());
    }
    assert_eq!(
        conversation.speaker().played(),
        None,
        "nothing of the answer reached the speaker after the stop: {notes:#?}"
    );
    assert!(
        notes.contains(&Note::Ended {
            response_id: answer,
            how: Ending::Cancelled,
        }),
        "the provider cancelled the answer itself: {notes:#?}"
    );
    assert_eq!(
        conversation
            .session()
            .ledger()
            .response(answer)
            .map(|r| r.state()),
        Some(ResponseState::Cancelled)
    );
    assert!(conversation.session().log.cancels.is_empty());
    assert!(conversation.session().log.truncations.is_empty());
    assert!(
        !notes
            .iter()
            .any(|note| matches!(note, Note::Truncated { .. } | Note::Refused(_))),
        "{notes:#?}"
    );

    interruption_answered(&host, &mut conversation);
    time_answer_plays(&host, &mut conversation, 0);
}
