//! The wiring of the demo without hardware or network: the testkit's scripted mock model behind
//! the contract, the device adapter's fake host for the microphone and the speaker, every device
//! callback made by the test itself.
//!
//! The fake host is a laptop: stereo input and output at 48 kHz. Speech is a constant sample of
//! -0.25, which the mock hears as speech its recogniser cannot read, so what it heard is the
//! script's; silence is zero.

use std::time::Duration;

use inference_audio_device::fake::FakeHost;
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, ResponseId, ResponseState, SessionConfig, SpeechCallId, SpeechSessionHandle,
    SpeechToolCallState,
};
use inference_testkit::duplex::mock::{AUDIO_FRAMES, MockSpeechModel, Reply, ScriptedTurn};
use realtime_voice::{Conversation, Ending, Note, Options, TIME_TOOL, nova_config, openai_config};
use serde_json::json;

/// Interleaved stereo samples of 10 ms at 48 kHz.
const TEN_MS: usize = 480 * 2;

/// Device frames of 10 ms at 48 kHz.
const TEN_MS_FRAMES: usize = 480;

fn say(heard: &str, reply: &str) -> ScriptedTurn {
    ScriptedTurn {
        heard: heard.to_owned(),
        reply: Reply::Say(reply.to_owned()),
    }
}

fn clock() -> Box<dyn Fn() -> Json> {
    Box::new(|| json!({"local_time": "Tuesday 20 October 2026, 12:34"}))
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

fn open(
    model: &mut MockSpeechModel,
    config: SessionConfig,
    host: &FakeHost,
) -> Conversation<<MockSpeechModel as NativeSpeechModel>::Session> {
    Conversation::open(model, config, host, Options::default(), clock())
        .expect("the fake devices and the mock session open")
}

/// Steps until `done` holds of the notes so far, at most 50 steps.
fn step_until<S: SpeechSessionHandle>(
    conversation: &mut Conversation<S>,
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

fn completed(notes: &[Note]) -> bool {
    notes.iter().any(|note| {
        matches!(
            note,
            Note::Ended {
                how: Ending::Completed,
                ..
            }
        )
    })
}

/// Handles one event at a time until the speaker holds the first audio of a response still in
/// progress, and answers that response.
fn until_audio_plays<S: SpeechSessionHandle>(conversation: &mut Conversation<S>) -> ResponseId {
    let mut notes = conversation.pump_microphone();
    for _ in 0..100 {
        notes.extend(conversation.pump_events(1));
        conversation.feed_speaker(&mut notes);
        if conversation.speaker().queued() > 0 {
            return conversation
                .session()
                .ledger()
                .in_progress()
                .expect("the response is still in progress");
        }
    }
    panic!("no audio reached the speaker: {notes:#?}");
}

#[test]
fn speech_reaches_the_session_and_model_audio_reaches_the_speaker() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::native(vec![say("what is the weather", "Sunny all day.")]);
    let mut conversation = open(&mut model, openai_config(), &host);

    speak(&host, 100);
    pause(&host, 100);
    let notes = step_until(&mut conversation, completed);

    assert!(
        conversation.frames_sent() >= 9,
        "200 ms of capture is at least 9 frames of 20 ms, sent {}",
        conversation.frames_sent()
    );
    assert!(
        notes.contains(&Note::Heard("what is the weather".to_owned())),
        "the user's transcript: {notes:#?}"
    );
    assert!(
        notes
            .iter()
            .any(|note| matches!(note, Note::Said { text, .. } if text == "Sunny all day.")),
        "the model's transcript: {notes:#?}"
    );
    let emitted = AUDIO_FRAMES as usize * 480;
    assert_eq!(
        conversation.speaker().queued(),
        emitted,
        "every sample of the response's 24 kHz audio is queued at the speaker"
    );

    let _ = host.pull(2 * TEN_MS_FRAMES).expect("the speaker runs");
    let played = conversation.speaker().played().expect("audio was pushed");
    assert!(played.get() > 0, "the device played the model's audio");
    assert!(conversation.speaker().queued() < emitted);
    assert!(
        !notes.iter().any(|note| matches!(note, Note::Refused(_))),
        "nothing refused: {notes:#?}"
    );
}

#[test]
fn a_barge_in_stops_playout_and_truncates_at_the_played_position() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::native(vec![
        say("tell me a story", "Once upon a time."),
        say("stop", "Stopped."),
    ]);
    let mut conversation = open(&mut model, openai_config(), &host);

    speak(&host, 100);
    pause(&host, 100);
    let response = until_audio_plays(&mut conversation);
    let _ = host.pull(TEN_MS_FRAMES).expect("the speaker runs");
    let heard = conversation.speaker().played().expect("audio was pushed");
    assert!(heard.get() > 0, "part of the response was heard");

    speak(&host, 40);
    let notes = conversation.step();

    assert!(
        notes.contains(&Note::BargeIn {
            response_id: Some(response),
            heard: Some(heard),
        }),
        "playout stopped at the played position: {notes:#?}"
    );
    assert_eq!(conversation.speaker().queued(), 0, "nothing more plays");
    assert!(
        notes.contains(&Note::Truncated {
            response_id: response,
            heard,
        }),
        "the response is truncated where the listener stopped hearing it: {notes:#?}"
    );
    assert!(
        notes.contains(&Note::Ended {
            response_id: response,
            how: Ending::Cancelled,
        }),
        "the caller cancelled the response in progress: {notes:#?}"
    );
    assert_eq!(
        conversation
            .session()
            .ledger()
            .response(response)
            .map(|r| r.state()),
        Some(ResponseState::Cancelled)
    );

    pause(&host, 100);
    let notes = step_until(&mut conversation, completed);
    assert!(
        notes
            .iter()
            .any(|note| matches!(note, Note::Said { text, .. } if text == "Stopped.")),
        "the next turn is answered: {notes:#?}"
    );
    assert!(conversation.speaker().queued() > 0, "its audio plays");
    assert!(
        !notes.iter().any(|note| matches!(note, Note::Refused(_))),
        "nothing refused: {notes:#?}"
    );
}

#[test]
fn a_barge_in_on_a_finished_response_still_playing_truncates_it_without_cancelling() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::native(vec![
        say("tell me a story", "Once upon a time."),
        say("stop", "Stopped."),
    ]);
    let mut conversation = open(&mut model, openai_config(), &host);

    speak(&host, 100);
    pause(&host, 100);
    let notes = step_until(&mut conversation, completed);
    let response = notes
        .iter()
        .find_map(|note| match note {
            Note::Ended { response_id, .. } => Some(*response_id),
            _ => None,
        })
        .expect("a response ended");
    let _ = host.pull(TEN_MS_FRAMES).expect("the speaker runs");
    let heard = conversation.speaker().played().expect("audio was pushed");
    assert!(conversation.speaker().queued() > 0, "the rest still plays");

    speak(&host, 40);
    let notes = conversation.step();

    assert!(
        notes.contains(&Note::BargeIn {
            response_id: Some(response),
            heard: Some(heard),
        }),
        "{notes:#?}"
    );
    assert!(
        notes.contains(&Note::Truncated {
            response_id: response,
            heard,
        }),
        "{notes:#?}"
    );
    assert_eq!(
        conversation
            .session()
            .ledger()
            .response(response)
            .map(|r| r.state()),
        Some(ResponseState::Completed),
        "a finished response is truncated, not cancelled"
    );
    assert!(
        !notes.iter().any(|note| matches!(note, Note::Refused(_))),
        "nothing refused: {notes:#?}"
    );
}

#[test]
fn a_provider_side_barge_in_stops_playout_without_truncating() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::server_turns(vec![
        say("tell me a story", "Once upon a time."),
        say("stop", "Stopped."),
    ]);
    let mut conversation = open(&mut model, nova_config(), &host);

    speak(&host, 100);
    pause(&host, 100);
    let response = until_audio_plays(&mut conversation);
    let _ = host.pull(TEN_MS_FRAMES).expect("the speaker runs");
    let heard = conversation.speaker().played().expect("audio was pushed");

    speak(&host, 40);
    let notes = conversation.step();

    assert!(
        notes.contains(&Note::BargeIn {
            response_id: Some(response),
            heard: Some(heard),
        }),
        "playout stopped at the played position: {notes:#?}"
    );
    assert_eq!(conversation.speaker().queued(), 0, "nothing more plays");
    assert!(
        notes.contains(&Note::Ended {
            response_id: response,
            how: Ending::Cancelled,
        }),
        "the provider cancelled the response itself: {notes:#?}"
    );
    assert!(
        !notes
            .iter()
            .any(|note| matches!(note, Note::Truncated { .. } | Note::Refused(_))),
        "a model that takes no truncation or cancellation is asked for neither: {notes:#?}"
    );
}

#[test]
fn the_time_tool_is_answered_by_call_id_and_its_answer_spoken() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::native(vec![ScriptedTurn {
        heard: "what time is it".to_owned(),
        reply: Reply::Call {
            tool: TIME_TOOL.to_owned(),
            arguments: json!({}),
            then: "It is {result}.".to_owned(),
        },
    }]);
    let mut conversation = open(&mut model, openai_config(), &host);

    speak(&host, 100);
    pause(&host, 100);
    let notes = step_until(&mut conversation, |notes| {
        notes
            .iter()
            .any(|note| matches!(note, Note::Said { text, .. } if text.contains("12:34")))
    });

    let call = SpeechCallId::new("call_1");
    assert!(
        notes.contains(&Note::Answered {
            call_id: call.clone(),
            result: json!({"local_time": "Tuesday 20 October 2026, 12:34"}),
        }),
        "the call is answered with the clock's time: {notes:#?}"
    );
    assert_eq!(
        conversation
            .session()
            .ledger()
            .call(&call)
            .map(|c| c.state()),
        Some(SpeechToolCallState::Answered)
    );
    assert_eq!(
        conversation.session().ledger().responses().len(),
        2,
        "the answer is spoken in the response the caller asked for after the call"
    );
    assert!(
        !notes.iter().any(|note| matches!(note, Note::Refused(_))),
        "nothing refused: {notes:#?}"
    );
}

#[test]
fn a_model_that_creates_its_own_responses_is_not_asked_for_one_after_a_call() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::server_turns(vec![ScriptedTurn {
        heard: "what time is it".to_owned(),
        reply: Reply::Call {
            tool: TIME_TOOL.to_owned(),
            arguments: json!({}),
            then: "It is {result}.".to_owned(),
        },
    }]);
    let mut conversation = open(&mut model, nova_config(), &host);

    speak(&host, 100);
    pause(&host, 100);
    let notes = step_until(&mut conversation, completed);
    for _ in 0..5 {
        let more = conversation.step();
        assert!(
            !more.iter().any(|note| matches!(note, Note::Refused(_))),
            "{more:#?}"
        );
    }

    assert!(
        notes.iter().any(
            |note| matches!(note, Note::Answered { call_id, .. } if call_id.as_str() == "call_1")
        ),
        "the call is answered: {notes:#?}"
    );
    assert!(
        !notes.iter().any(|note| matches!(note, Note::Refused(_))),
        "no response is asked of a model that creates every response: {notes:#?}"
    );
}

#[test]
fn closing_ends_the_session() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::native(vec![]);
    let mut conversation = open(&mut model, openai_config(), &host);
    assert!(conversation.is_open());
    let notes = conversation.close();
    assert!(notes.contains(&Note::Closed), "{notes:#?}");
    assert!(!conversation.is_open());
}

#[test]
fn the_demo_sessions_are_what_each_provider_takes() {
    let openai = openai_config();
    inference_openai::duplex::capabilities()
        .admit(&openai)
        .expect("the Realtime API takes the OpenAI session");
    let binding = inference_openai::duplex::OpenAiRealtime::new(
        inference_openai::Endpoint::new(inference_openai::DEFAULT_MODEL),
        Box::new(inference_openai::EnvironmentSource::openai()),
        inference_openai::OPENAI_KEY_NAME,
    );
    let wire = binding
        .client_config(&openai)
        .expect("the binding maps the session");
    assert_eq!(wire.input_format, inference_openai::WireFormat::Pcm16);
    assert_eq!(wire.output_format, inference_openai::WireFormat::Pcm16);
    assert_eq!(openai.input_format.sample_rate.get(), 24_000);
    assert!(!openai.auto_interrupt, "barge-in is the caller's rule");
    assert_eq!(wire.tools.len(), 1);
    assert_eq!(wire.tools[0].name, TIME_TOOL);

    let nova = nova_config();
    inference_aws::duplex::capabilities(inference_aws::nova_sonic::DEFAULT_MODEL_ID)
        .admit(&nova)
        .expect("Nova Sonic takes the Nova session");
    assert_eq!(nova.input_format.sample_rate.get(), 16_000);
    assert_eq!(nova.tools.len(), 1);
}

#[test]
fn the_local_time_answers_a_time_and_an_offset() {
    let answer = realtime_voice::local_time();
    assert!(
        answer["local_time"]
            .as_str()
            .is_some_and(|t| t.contains(':'))
    );
    assert!(answer["utc_offset"].as_str().is_some_and(|o| o.len() == 6));
}

#[test]
fn a_session_the_model_refuses_opens_nothing() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::server_turns(vec![]);
    let refused = Conversation::open(
        &mut model,
        openai_config(),
        &host,
        Options {
            wait: Duration::from_millis(1),
            ..Options::default()
        },
        clock(),
    );
    assert!(refused.is_err(), "server turns take no 24 kHz input");
    assert!(
        !host.input_open() && !host.output_open(),
        "the devices are released"
    );
}
