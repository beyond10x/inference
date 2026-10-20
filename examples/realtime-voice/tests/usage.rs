//! Usage per response and per transcription, its totals and its estimated cost, with the
//! testkit's scripted mock model and the device adapter's fake host, as in `wiring.rs`.
//!
//! The mock reports, per response that completes, input 20 text, 50 audio and 0 cached tokens and
//! output 8 text and 30 audio tokens, and per transcribed turn 10 input and 3 output tokens. A
//! response it cancels reports no usage.

use inference_audio_device::fake::FakeHost;
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, Quantity, QuantityKind, QuantitySource, ResponseId, ResponseUsage,
    SessionConfig, SpeechSessionHandle, TokenCounts, TranscriptionUsage,
};
use inference_testkit::duplex::mock::{MockSpeechModel, Reply, ScriptedTurn};
use realtime_voice::{
    Conversation, Counts, Ending, Note, Options, Prices, Reported, Tally, Usage, nova_config,
    openai_config,
};
use serde_json::json;

/// Interleaved stereo samples of 10 ms at 48 kHz.
const TEN_MS: usize = 480 * 2;

/// Device frames of 10 ms at 48 kHz.
const TEN_MS_FRAMES: usize = 480;

/// The usage the mock reports for every response that completes.
const MOCK_RESPONSE: ResponseUsage = ResponseUsage {
    input: TokenCounts {
        text_tokens: 20,
        audio_tokens: 50,
        cached_tokens: 0,
    },
    output: TokenCounts {
        text_tokens: 8,
        audio_tokens: 30,
        cached_tokens: 0,
    },
};

/// The usage the mock reports for every transcribed turn.
const MOCK_TRANSCRIPTION: TranscriptionUsage = TranscriptionUsage {
    input_tokens: 10,
    output_tokens: 3,
};

/// A fixed table: text 4.00 in, 0.40 cached, 16.00 out; audio 32.00 in, 64.00 out, per million.
const TABLE: &str = r#"{
    "name": "test table",
    "date": "2026-10-20",
    "currency": "USD",
    "per_million_tokens": {
        "text_in": 4.0,
        "cached_in": 0.4,
        "text_out": 16,
        "audio_in": 32.0,
        "audio_out": 64.0
    }
}"#;

fn say(heard: &str, reply: &str) -> ScriptedTurn {
    ScriptedTurn {
        heard: heard.to_owned(),
        reply: Reply::Say(reply.to_owned()),
    }
}

fn clock() -> Box<dyn Fn() -> Json> {
    Box::new(|| json!({"local_time": "Tuesday 20 October 2026, 12:34"}))
}

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

/// Steps until a response completes, at most 50 steps, every note recorded in `tally`.
fn until_completed<S: SpeechSessionHandle>(
    conversation: &mut Conversation<S>,
    tally: &mut Tally,
) -> Vec<Note> {
    let mut notes = Vec::new();
    for _ in 0..50 {
        let more = conversation.step();
        for note in &more {
            tally.record(note);
        }
        let done = more.iter().any(|note| {
            matches!(
                note,
                Note::Ended {
                    how: Ending::Completed,
                    ..
                }
            )
        });
        notes.extend(more);
        if done {
            return notes;
        }
    }
    panic!("no response completed within 50 steps: {notes:#?}");
}

fn completed_response(notes: &[Note]) -> ResponseId {
    notes
        .iter()
        .find_map(|note| match note {
            Note::Ended {
                response_id,
                how: Ending::Completed,
            } => Some(*response_id),
            _ => None,
        })
        .expect("a response completed")
}

const fn known(
    input_text: u64,
    input_audio: u64,
    input_cached: u64,
    output_text: u64,
    output_audio: u64,
) -> Counts {
    Counts {
        input_text: Some(input_text),
        input_audio: Some(input_audio),
        input_cached: Some(input_cached),
        output_text: Some(output_text),
        output_audio: Some(output_audio),
    }
}

#[test]
fn usage_notes_are_produced_per_response_and_per_transcription() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::native(vec![say("what is the weather", "Sunny all day.")]);
    let mut conversation = open(&mut model, openai_config(), &host);
    let mut tally = Tally::new(Reported::OPENAI);

    speak(&host, 100);
    pause(&host, 100);
    let notes = until_completed(&mut conversation, &mut tally);
    let response = completed_response(&notes);

    assert!(
        notes.contains(&Note::Usage(Usage::Response {
            response_id: response,
            usage: MOCK_RESPONSE,
        })),
        "the response's usage is a note: {notes:#?}"
    );
    assert!(
        notes.contains(&Note::Usage(Usage::Transcription(MOCK_TRANSCRIPTION))),
        "the transcription's usage is a note: {notes:#?}"
    );
    let usage_at = notes
        .iter()
        .position(|note| matches!(note, Note::Usage(Usage::Response { .. })));
    let ended_at = notes
        .iter()
        .position(|note| matches!(note, Note::Ended { .. }));
    assert!(
        usage_at < ended_at,
        "the mock reports usage before the end, and the notes keep that order"
    );

    let counts = tally
        .response(response)
        .expect("the response reported usage");
    assert_eq!(counts, known(20, 50, 0, 8, 30));
    assert_eq!(
        counts.to_string(),
        "in text 20 audio 50 cached 0, out text 8 audio 30"
    );
    assert!(tally.ended(response));
}

#[test]
fn totals_equal_the_sum_of_the_responses_and_transcriptions() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::native(vec![
        say("what is the weather", "Sunny all day."),
        say("and tomorrow", "Rain."),
    ]);
    let mut conversation = open(&mut model, openai_config(), &host);
    let mut tally = Tally::new(Reported::OPENAI);

    speak(&host, 100);
    pause(&host, 100);
    let first = completed_response(&until_completed(&mut conversation, &mut tally));
    let _ = host.pull(10 * TEN_MS_FRAMES).expect("the speaker runs");
    speak(&host, 100);
    pause(&host, 100);
    let second = completed_response(&until_completed(&mut conversation, &mut tally));
    assert_ne!(first, second);

    let one = tally.response(first).expect("usage");
    let two = tally.response(second).expect("usage");
    let totals = tally.totals();
    assert_eq!(totals.counts, one.plus(two));
    assert_eq!(totals.counts, known(40, 100, 0, 16, 60));
    assert_eq!(totals.responses, 2);
    assert_eq!(totals.unreported, 0);
    assert_eq!(totals.heard, 2);
    assert_eq!(totals.transcriptions, 2);
    assert_eq!(totals.transcription_input, Some(20));
    assert_eq!(totals.transcription_output, Some(6));
    assert_eq!(
        totals.to_string(),
        "responses 2: in text 40 audio 100 cached 0, out text 16 audio 60; \
         transcription in 20 out 6 (2 of 2 turns reported)"
    );
}

#[test]
fn a_later_usage_of_a_response_replaces_the_earlier_one() {
    let response = ResponseId::generate();
    let early = ResponseUsage {
        input: TokenCounts {
            text_tokens: 5,
            audio_tokens: 7,
            cached_tokens: 0,
        },
        output: TokenCounts::default(),
    };
    let mut tally = Tally::new(Reported::ALL);
    for usage in [early, MOCK_RESPONSE] {
        tally.record(&Note::Usage(Usage::Response {
            response_id: response,
            usage,
        }));
    }
    assert_eq!(tally.response(response), Some(known(20, 50, 0, 8, 30)));
    assert_eq!(tally.totals().counts, known(20, 50, 0, 8, 30));
    assert_eq!(tally.totals().responses, 1);
    assert!(!tally.ended(response), "no end was noted");
}

#[test]
fn a_response_without_usage_stays_unknown_and_out_of_the_sum() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::native(vec![
        say("tell me a story", "Once upon a time."),
        say("stop", "Stopped."),
    ]);
    let mut conversation = open(&mut model, openai_config(), &host);
    let mut tally = Tally::new(Reported::OPENAI);

    speak(&host, 100);
    pause(&host, 100);
    let mut cancelled = None;
    for _ in 0..100 {
        let mut notes = conversation.pump_microphone();
        notes.extend(conversation.pump_events(1));
        conversation.feed_speaker(&mut notes);
        for note in &notes {
            tally.record(note);
        }
        if conversation.speaker().queued() > 0 {
            cancelled = conversation.session().ledger().in_progress();
            break;
        }
    }
    let cancelled = cancelled.expect("audio of a response in progress plays");
    let _ = host.pull(TEN_MS_FRAMES).expect("the speaker runs");
    speak(&host, 40);
    for note in conversation.step() {
        tally.record(&note);
    }
    pause(&host, 100);
    let completed = completed_response(&until_completed(&mut conversation, &mut tally));

    assert!(
        tally.ended(cancelled),
        "the barge-in ended the first response"
    );
    assert_eq!(
        tally.response(cancelled),
        None,
        "the cancelled response reported no usage"
    );
    assert_eq!(
        Counts::default().to_string(),
        "in text ? audio ? cached ?, out text ? audio ?",
        "what is not known prints as ?"
    );
    let totals = tally.totals();
    assert_eq!(totals.responses, 2);
    assert_eq!(totals.unreported, 1);
    assert_eq!(
        totals.counts,
        tally.response(completed).expect("usage"),
        "the sum holds the response that reported, nothing for the other"
    );
    assert!(
        totals
            .to_string()
            .starts_with("responses 2 (1 without usage): "),
        "{totals}"
    );
}

/// The session's ledger records one `inference.usage` record per response, the one the barge-in
/// cancelled before the mock reported usage with every token count unknown, and one per
/// transcribed turn.
#[test]
fn the_session_records_one_usage_record_per_response_and_per_transcription() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::native(vec![
        say("tell me a story", "Once upon a time."),
        say("stop", "Stopped."),
    ]);
    let mut conversation = open(&mut model, openai_config(), &host);
    let mut tally = Tally::new(Reported::OPENAI);

    speak(&host, 100);
    pause(&host, 100);
    let mut cancelled = None;
    for _ in 0..100 {
        let mut notes = conversation.pump_microphone();
        notes.extend(conversation.pump_events(1));
        conversation.feed_speaker(&mut notes);
        if conversation.speaker().queued() > 0 {
            cancelled = conversation.session().ledger().in_progress();
            break;
        }
    }
    let cancelled = cancelled.expect("audio of a response in progress plays");
    let _ = host.pull(TEN_MS_FRAMES).expect("the speaker runs");
    speak(&host, 40);
    let _ = conversation.step();
    pause(&host, 100);
    let completed = completed_response(&until_completed(&mut conversation, &mut tally));
    let _ = conversation.close();

    let records = conversation.session().ledger().usage_records();
    let of = |response: ResponseId| {
        records
            .iter()
            .filter(|record| record.response_id() == Some(response))
            .collect::<Vec<_>>()
    };
    assert_eq!(of(cancelled).len(), 1, "{records:#?}");
    assert_eq!(of(completed).len(), 1, "{records:#?}");
    let responses = conversation.session().ledger().responses().len();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.response_id().is_some())
            .count(),
        responses,
        "one record per response"
    );
    assert!(
        of(cancelled)[0]
            .quantities()
            .iter()
            .filter(|quantity| quantity.kind() != QuantityKind::Requests)
            .all(|quantity| quantity.source() == QuantitySource::Unknown),
        "cancelled before any usage: {:?}",
        of(cancelled)[0]
    );
    assert_eq!(
        of(completed)[0].quantities(),
        [
            Quantity::provider_reported(QuantityKind::InputTextTokens, 20),
            Quantity::provider_reported(QuantityKind::InputAudioTokens, 50),
            Quantity::provider_reported(QuantityKind::CachedInputTokens, 0),
            Quantity::provider_reported(QuantityKind::OutputTextTokens, 8),
            Quantity::provider_reported(QuantityKind::OutputAudioTokens, 30),
            Quantity::measured(QuantityKind::Requests, 1),
        ]
    );
    let transcriptions: Vec<_> = records
        .iter()
        .filter(|record| record.response_id().is_none())
        .collect();
    assert_eq!(transcriptions.len(), 2, "one per transcribed turn");
    assert!(transcriptions.iter().all(|record| record.quantities()
        == [
            Quantity::provider_reported(QuantityKind::InputAudioTokens, 10),
            Quantity::provider_reported(QuantityKind::OutputTextTokens, 3),
        ]));
}

#[test]
fn a_count_the_provider_does_not_report_prints_as_unknown_never_zero() {
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::server_turns(vec![say("what is the weather", "Sunny.")]);
    let mut conversation = open(&mut model, nova_config(), &host);
    let mut tally = Tally::new(Reported::NOVA);

    speak(&host, 100);
    pause(&host, 100);
    let response = completed_response(&until_completed(&mut conversation, &mut tally));

    let counts = tally.response(response).expect("usage");
    assert_eq!(
        counts.input_cached, None,
        "Nova Sonic reports no cached tokens"
    );
    assert_eq!(
        counts.to_string(),
        "in text 20 audio 50 cached ?, out text 8 audio 30"
    );
    assert_eq!(tally.totals().counts.input_cached, None);
}

#[test]
fn transcription_usage_never_reported_stays_unknown() {
    let mut tally = Tally::new(Reported::OPENAI);
    tally.record(&Note::Heard("hello".to_owned()));
    let totals = tally.totals();
    assert_eq!(totals.transcription_input, None);
    assert_eq!(totals.transcription_output, None);
    assert_eq!(
        totals.to_string(),
        "responses 0: in text ? audio ? cached ?, out text ? audio ?; \
         transcription in ? out ? (0 of 1 turns reported)"
    );
}

#[test]
fn the_cost_of_a_fixed_table_is_exact() {
    let prices = Prices::parse(TABLE).expect("the table reads");
    assert_eq!(prices.to_string(), "test table (2026-10-20)");
    assert_eq!(prices.currency, "USD");

    let counts = known(20, 50, 10, 8, 30);
    let cost = prices.cost(&counts).expect("every count is known");
    let picos = 10 * 4_000_000 + 50 * 32_000_000 + 10 * 400_000 + 8 * 16_000_000 + 30 * 64_000_000;
    assert_eq!(
        cost.picos(),
        picos,
        "cached tokens are priced at the cached rate and taken from the text input first"
    );
    assert_eq!(cost.to_string(), "0.003692");

    let unknown_cached = Counts {
        input_cached: None,
        ..counts
    };
    assert_eq!(
        prices.cost(&unknown_cached),
        None,
        "a table with a cached price needs the cached count"
    );
    let without_cached: Prices = Prices {
        cached_in: None,
        ..prices.clone()
    };
    assert_eq!(
        without_cached
            .cost(&unknown_cached)
            .map(realtime_voice::Cost::picos),
        Some(20 * 4_000_000 + 50 * 32_000_000 + 8 * 16_000_000 + 30 * 64_000_000),
        "a table without one prices every input token at its own rate"
    );
}

#[test]
fn the_cost_of_a_conversation_is_the_sum_of_its_responses() {
    let prices = Prices::parse(TABLE).expect("the table reads");
    let host = FakeHost::laptop();
    let mut model = MockSpeechModel::native(vec![
        say("what is the weather", "Sunny all day."),
        say("and tomorrow", "Rain."),
    ]);
    let mut conversation = open(&mut model, openai_config(), &host);
    let mut tally = Tally::new(Reported::OPENAI);
    assert_eq!(tally.cost(&prices), None, "nothing reported, nothing known");

    speak(&host, 100);
    pause(&host, 100);
    let first = completed_response(&until_completed(&mut conversation, &mut tally));
    let _ = host.pull(10 * TEN_MS_FRAMES).expect("the speaker runs");
    speak(&host, 100);
    pause(&host, 100);
    let _ = until_completed(&mut conversation, &mut tally);

    let one = prices
        .cost(&tally.response(first).expect("usage"))
        .expect("known");
    assert_eq!(
        one.picos(),
        20 * 4_000_000 + 50 * 32_000_000 + 8 * 16_000_000 + 30 * 64_000_000
    );
    assert_eq!(
        tally.cost(&prices).map(realtime_voice::Cost::picos),
        Some(2 * one.picos())
    );
}

#[test]
fn a_price_table_is_refused_when_it_is_not_one() {
    let altered = |from: &str, to: &str| {
        assert!(TABLE.contains(from), "the table holds {from}");
        TABLE.replace(from, to)
    };
    let refused = [
        (
            r#"{"name": "t", "date": "d", "currency": "USD"}"#.to_owned(),
            "no prices",
        ),
        (
            altered("\"text_in\": 4.0", "\"text_in\": 4.0000001"),
            "more than six decimals",
        ),
        (
            altered("\"text_in\": 4.0", "\"text_in\": -4.0"),
            "a negative price",
        ),
        (
            altered(
                "\"audio_out\": 64.0",
                "\"audio_out\": 64.0, \"image_in\": 1",
            ),
            "an unknown price",
        ),
        (
            altered("\"currency\"", "\"note\": \"x\", \"currency\""),
            "an unknown field",
        ),
        (altered("\"name\": \"test table\",", ""), "no name"),
        (altered("\"date\": \"2026-10-20\",", ""), "no date"),
        ("not json".to_owned(), "not JSON"),
    ];
    for (table, why) in refused {
        assert!(Prices::parse(&table).is_err(), "refused: {why}");
    }
    assert!(Prices::parse(TABLE).is_ok(), "the unaltered table reads");
}
