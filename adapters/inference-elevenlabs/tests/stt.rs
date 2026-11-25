//! The realtime speech-to-text binding against the local fixture server, without network: μ-law
//! in, hypotheses and committed spans with their timestamps out, one usage record of the audio
//! sent; manual and VAD commit; every documented error type as its own typed error; a stalled
//! server; the configuration and handshake refusals; and a cancel whose close frame cannot be
//! written, which ends its connection thread at the close bound.

mod stt_support;

use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use inference_elevenlabs::stt_protocol::ERROR_TYPES;
use inference_elevenlabs::{
    CommitStrategy, ConfigError, ElevenLabsError, ElevenLabsRecogniser, Endpoint, InputFormat,
    SpanTiming, TranscriptionConfig, VadSettings,
};
use inference_stt::inference_audio::AudioEncoding;
use inference_stt::inference_core::usage::QuantityKind;
use inference_stt::{Recogniser, RecognitionHandle, TranscriptEvent};
use stt_support::{
    Action, FixedKey, FixtureServer, KEY, Script, Step, Trigger, describe, manual, mulaw, partial,
    pcm16k, recognise, request, session_started, wait_for,
};

fn assert_clean(server: &FixtureServer) {
    assert_eq!(server.violations(), Vec::<String>::new());
    assert!(!server.seen().is_empty(), "the server saw no connection");
}

fn audio_ms(
    events: &[inference_stt::inference_core::envelope::EventEnvelope<TranscriptEvent>],
) -> (Option<i64>, bool) {
    match events.last().map(|event| &event.payload) {
        Some(TranscriptEvent::Usage(record)) => (
            record
                .quantities
                .iter()
                .find(|quantity| quantity.kind() == QuantityKind::InputAudioMs)
                .and_then(|quantity| quantity.value()),
            record.r#final,
        ),
        other => panic!("the last event is not the usage record: {other:?}"),
    }
}

#[test]
fn mulaw_frames_give_hypotheses_then_committed_spans_with_timestamps_then_one_usage_record() {
    let server = FixtureServer::start(Script::fixture("stt-ulaw.jsonl"));
    let config = TranscriptionConfig::new(InputFormat::Ulaw8000)
        .with_language("de")
        .with_commit_strategy(CommitStrategy::Vad(VadSettings::default()));
    let mut recogniser = server.recogniser(config);
    let (handle, events) = recognise(&mut recogniser, mulaw(120));
    assert_eq!(handle.failure(), None);
    assert_eq!(
        events.iter().map(describe).collect::<Vec<_>>(),
        [
            "hypothesis Guten",
            "revision Guten Morgen",
            "revision Guten Morgen.",
            "commit Guten Morgen.",
            "hypothesis Wie geht's",
            "revision Wie geht's?",
            "commit Wie geht's?",
            "usage",
        ]
    );
    let committed: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            TranscriptEvent::Committed(committed) => Some(committed.span_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        handle.span_timing(committed[0]),
        Some(SpanTiming {
            start: 0.12,
            end: 1.04
        })
    );
    assert_eq!(
        handle.span_timing(committed[1]),
        Some(SpanTiming {
            start: 1.3,
            end: 2.1
        })
    );
    assert_eq!(
        audio_ms(&events),
        (Some(2_400), true),
        "120 frames of 20 ms sent, one final record"
    );
    let usage = events
        .iter()
        .filter(|event| matches!(event.payload, TranscriptEvent::Usage(_)))
        .count();
    assert_eq!(usage, 1);
    let seen = &server.seen()[0];
    assert_eq!(seen.audio_chunks, 120);
    assert_eq!(seen.audio_bytes, 120 * 160);
    assert_eq!(seen.commits, 1, "one commit, at the end of input");
    assert!(
        seen.query.contains("audio_format=ulaw_8000"),
        "{}",
        seen.query
    );
    assert!(seen.query.contains("language_code=de"), "{}", seen.query);
    assert!(
        seen.query.contains("model_id=scribe_v2_realtime"),
        "{}",
        seen.query
    );
    assert_eq!(seen.key.as_deref(), Some(KEY));
    assert_clean(&server);
}

#[test]
fn a_manual_commit_mid_stream_commits_what_was_heard_and_the_end_of_input_commits_the_rest() {
    let server = FixtureServer::start(Script::fixture("stt-manual.jsonl"));
    let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
    let mut handle = recogniser.start(request()).unwrap();
    let frames = mulaw(40);
    let mut events = Vec::new();
    for frame in frames[..20].iter().cloned() {
        handle.offer_frame(frame).unwrap();
        events.extend(handle.by_ref());
    }
    handle.commit();
    wait_for(
        &mut handle,
        &mut events,
        "commit eins",
        Duration::from_secs(3),
    );
    for frame in frames[20..].iter().cloned() {
        handle.offer_frame(frame).unwrap();
        events.extend(handle.by_ref());
    }
    handle.end_input().unwrap();
    events.extend(handle.by_ref());
    assert_eq!(handle.failure(), None);
    assert_eq!(
        events.iter().map(describe).collect::<Vec<_>>(),
        [
            "hypothesis eins",
            "commit eins",
            "hypothesis zwei",
            "commit zwei",
            "usage"
        ]
    );
    let seen = &server.seen()[0];
    assert!(
        seen.query.contains("commit_strategy=manual"),
        "{}",
        seen.query
    );
    assert_eq!(
        seen.commits_after,
        [20, 40],
        "the commit followed the 20 frames offered before it, and the end of input all 40"
    );
    assert_eq!(audio_ms(&events), (Some(800), true));
    assert_clean(&server);
}

#[test]
fn vad_commits_arrive_before_the_end_of_input_without_a_commit_from_the_binding() {
    let server = FixtureServer::start(Script::fixture("stt-vad.jsonl"));
    let vad = VadSettings {
        threshold: Some(0.4),
        silence_threshold_secs: Some(0.5),
        min_speech_duration_ms: Some(100),
        min_silence_duration_ms: Some(300),
    };
    let config = TranscriptionConfig::new(InputFormat::Ulaw8000)
        .with_commit_strategy(CommitStrategy::Vad(vad));
    let mut recogniser = server.recogniser(config);
    let mut handle = recogniser.start(request()).unwrap();
    let mut events = Vec::new();
    for frame in mulaw(40) {
        handle.offer_frame(frame).unwrap();
        events.extend(handle.by_ref());
    }
    wait_for(
        &mut handle,
        &mut events,
        "commit nein",
        Duration::from_secs(3),
    );
    assert_eq!(
        server.seen()[0].commits,
        0,
        "no commit before the end of input"
    );
    handle.end_input().unwrap();
    events.extend(handle.by_ref());
    assert_eq!(handle.failure(), None);
    assert_eq!(
        events.iter().map(describe).collect::<Vec<_>>(),
        [
            "hypothesis ja",
            "commit ja",
            "hypothesis nein",
            "commit nein",
            "usage"
        ]
    );
    assert_eq!(handle.warnings(), ["fixture warning"]);
    let seen = &server.seen()[0];
    for needed in [
        "commit_strategy=vad",
        "vad_threshold=0.4",
        "vad_silence_threshold_secs=0.5",
        "min_speech_duration_ms=100",
        "min_silence_duration_ms=300",
    ] {
        assert!(
            seen.query.split('&').any(|pair| pair == needed),
            "{needed} in {}",
            seen.query
        );
    }
    assert_eq!(seen.commits, 1, "the end of input commits what is left");
    assert_clean(&server);
}

#[test]
fn a_documented_error_type_fails_the_recognition_with_its_own_typed_error() {
    let server = FixtureServer::start(Script::sends_after_first(
        r#"{"message_type":"quota_exceeded","error":"Quota exceeded for this key"}"#,
    ));
    let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
    let (handle, events) = recognise(&mut recogniser, mulaw(10));
    assert_eq!(
        handle.failure(),
        Some(&ElevenLabsError::QuotaExceeded(
            "Quota exceeded for this key".to_owned()
        ))
    );
    let (_, is_final) = audio_ms(&events);
    assert!(!is_final, "a failed recognition's usage is not final");
    assert!(!handle.failure().unwrap().to_string().contains(KEY));
}

#[test]
fn every_documented_error_type_is_a_typed_error_of_its_own() {
    assert_eq!(ERROR_TYPES.len(), 14, "the documentation lists fourteen");
    let mut seen = Vec::new();
    for error_type in ERROR_TYPES {
        let message =
            serde_json::json!({"message_type": error_type, "error": "fixture"}).to_string();
        let server = FixtureServer::start(Script::sends_after_first(&message));
        let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
        let (handle, _) = recognise(&mut recogniser, mulaw(3));
        let failure = handle.failure().cloned().expect("the recognition failed");
        assert_eq!(failure.scribe_error_type(), Some(error_type), "{failure:?}");
        assert!(
            !matches!(
                failure,
                ElevenLabsError::Server(_) | ElevenLabsError::Protocol(_)
            ),
            "{error_type} is not typed: {failure:?}"
        );
        assert!(failure.to_string().contains("fixture"), "{failure}");
        seen.push(std::mem::discriminant(&failure));
    }
    seen.sort_by_key(|discriminant| format!("{discriminant:?}"));
    seen.dedup();
    assert_eq!(seen.len(), ERROR_TYPES.len(), "each type its own variant");
}

#[test]
fn a_server_silent_after_the_end_of_input_with_a_span_open_fails_as_stalled_within_the_bound() {
    let server = FixtureServer::start(Script::Steps(vec![
        stt_support::Step {
            on: stt_support::Trigger::Open,
            action: stt_support::Action::Send(stt_support::session_started()),
        },
        stt_support::Step {
            on: stt_support::Trigger::Audio(1),
            action: stt_support::Action::Send(stt_support::partial("hallo")),
        },
    ]));
    let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
    let mut handle = recogniser.start(request()).unwrap();
    let mut events = Vec::new();
    for frame in mulaw(5) {
        handle.offer_frame(frame).unwrap();
        events.extend(handle.by_ref());
    }
    wait_for(
        &mut handle,
        &mut events,
        "hypothesis hallo",
        Duration::from_secs(3),
    );
    handle.end_input().unwrap();
    let ended = Instant::now();
    events.extend(handle.by_ref());
    let waited = ended.elapsed();
    assert!(
        matches!(handle.failure(), Some(ElevenLabsError::Stalled(_))),
        "{:?}",
        handle.failure()
    );
    assert!(
        waited < Duration::from_secs(4),
        "the read timeout of 2 s and the close bound, not {waited:?}"
    );
    assert!(waited >= Duration::from_secs(2), "{waited:?}");
    let (_, is_final) = audio_ms(&events);
    assert!(!is_final);
    assert!(handle.next().is_none(), "nothing after the usage record");
}

#[test]
fn an_end_of_input_commit_answered_with_nothing_heard_ends_the_recognition_cleanly() {
    let server = FixtureServer::start(Script::Steps(vec![
        stt_support::Step {
            on: stt_support::Trigger::Open,
            action: stt_support::Action::Send(stt_support::session_started()),
        },
        stt_support::Step {
            on: stt_support::Trigger::Commit(1),
            action: stt_support::Action::Send(
                r#"{"message_type":"committed_transcript_with_timestamps","text":"","words":[]}"#
                    .to_owned(),
            ),
        },
    ]));
    let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
    let (handle, events) = recognise(&mut recogniser, mulaw(5));
    assert_eq!(handle.failure(), None);
    assert_eq!(events.iter().map(describe).collect::<Vec<_>>(), ["usage"]);
    assert_eq!(audio_ms(&events), (Some(100), true));
}

#[test]
fn an_end_of_input_commit_never_answered_fails_as_stalled_though_nothing_was_heard() {
    let server = FixtureServer::start(Script::Steps(vec![stt_support::Step {
        on: stt_support::Trigger::Open,
        action: stt_support::Action::Send(stt_support::session_started()),
    }]));
    let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
    let (handle, events) = recognise(&mut recogniser, mulaw(5));
    assert!(
        matches!(handle.failure(), Some(ElevenLabsError::Stalled(_))),
        "{:?}",
        handle.failure()
    );
    assert_eq!(events.iter().map(describe).collect::<Vec<_>>(), ["usage"]);
    assert_eq!(audio_ms(&events), (Some(100), false));
}

#[test]
fn the_documented_input_formats_are_taken_and_every_other_is_refused_at_configuration() {
    for (name, encoding, rate) in [
        ("ulaw_8000", AudioEncoding::PcmMulaw, 8_000),
        ("pcm_8000", AudioEncoding::PcmS16le, 8_000),
        ("pcm_16000", AudioEncoding::PcmS16le, 16_000),
        ("pcm_22050", AudioEncoding::PcmS16le, 22_050),
        ("pcm_24000", AudioEncoding::PcmS16le, 24_000),
        ("pcm_44100", AudioEncoding::PcmS16le, 44_100),
        ("pcm_48000", AudioEncoding::PcmS16le, 48_000),
    ] {
        let format = InputFormat::from_str(name).unwrap();
        assert_eq!(format.as_str(), name);
        assert_eq!(format.audio_format().encoding, encoding);
        assert_eq!(format.audio_format().sample_rate.get(), rate);
        assert_eq!(format.audio_format().channels.get(), 1);
    }
    assert_eq!(InputFormat::ALL.len(), 7);
    for refused in [
        "alaw_8000",
        "pcm_32000",
        "mp3_44100",
        "opus_48000",
        "wav",
        "",
    ] {
        assert!(
            matches!(
                InputFormat::from_str(refused),
                Err(ConfigError::UnsupportedInputFormat { .. })
            ),
            "{refused}"
        );
    }
}

#[test]
fn a_frame_in_another_format_than_configured_fails_the_recognition_before_anything_connects() {
    let server = FixtureServer::start(Script::fixture("stt-ulaw.jsonl"));
    let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
    let (handle, events) = recognise(&mut recogniser, pcm16k(3));
    assert!(
        matches!(handle.failure(), Some(ElevenLabsError::UnsupportedAudio(_))),
        "{:?}",
        handle.failure()
    );
    assert_eq!(audio_ms(&events), (Some(0), false), "nothing was sent");
    assert!(server.seen().is_empty(), "nothing connected");
}

#[test]
fn plaintext_is_refused_to_any_host_but_loopback_and_bad_options_at_configuration() {
    let new = |endpoint: Endpoint, config: TranscriptionConfig| {
        ElevenLabsRecogniser::new(
            endpoint,
            config,
            Arc::new(FixedKey(KEY)),
            "elevenlabs-api-key",
        )
    };
    let config = || manual(InputFormat::Ulaw8000);
    assert!(matches!(
        new(Endpoint::new().with_url("ws://api.elevenlabs.io"), config()),
        Err(ConfigError::InvalidUrl(_))
    ));
    assert!(new(Endpoint::new().with_url("ws://127.0.0.1:9"), config()).is_ok());
    assert!(new(Endpoint::new(), config()).is_ok());
    assert!(matches!(
        new(Endpoint::new(), config().with_model("scribe v2")),
        Err(ConfigError::InvalidModelId)
    ));
    assert!(matches!(
        new(Endpoint::new(), config().with_language("deutsch")),
        Err(ConfigError::InvalidLanguageCode)
    ));
    let mut blank = config();
    blank.keyterms = vec!["  ".to_owned()];
    assert!(matches!(
        new(Endpoint::new(), blank),
        Err(ConfigError::InvalidKeyterm)
    ));
    let vad = VadSettings {
        threshold: Some(f64::NAN),
        ..VadSettings::default()
    };
    assert!(matches!(
        new(
            Endpoint::new(),
            config().with_commit_strategy(CommitStrategy::Vad(vad))
        ),
        Err(ConfigError::VadSetting { .. })
    ));
}

#[test]
fn keyterms_are_sent_encoded_one_query_parameter_each() {
    let mut config = manual(InputFormat::Pcm16000);
    config.keyterms = vec!["Müller & Söhne".to_owned(), "Rechnung".to_owned()];
    let recogniser = ElevenLabsRecogniser::new(
        Endpoint::new(),
        config,
        Arc::new(FixedKey(KEY)),
        "elevenlabs-api-key",
    )
    .unwrap();
    let url = recogniser.request_url();
    assert!(
        url.starts_with("wss://api.elevenlabs.io/v1/speech-to-text/realtime?"),
        "{url}"
    );
    assert!(
        url.contains("&keyterms=M%C3%BCller%20%26%20S%C3%B6hne&keyterms=Rechnung"),
        "{url}"
    );
    assert!(!url.contains(KEY));
}

#[test]
fn a_refused_handshake_fails_the_recognition_with_its_status() {
    let server = FixtureServer::start(Script::Refuse(401));
    let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
    let (handle, events) = recognise(&mut recogniser, mulaw(3));
    match handle.failure() {
        Some(ElevenLabsError::Connect(error)) => {
            assert_eq!(error.status, Some(401));
            assert!(error.is_refusal());
            assert!(!error.detail.contains(KEY));
        }
        other => panic!("{other:?}"),
    }
    assert!(!audio_ms(&events).1);
}

#[test]
fn a_handshake_never_answered_fails_within_the_connect_timeout() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let held = std::thread::spawn(move || {
        let accepted = listener.accept();
        std::thread::sleep(Duration::from_secs(3));
        drop(accepted);
    });
    let mut recogniser = ElevenLabsRecogniser::new(
        Endpoint::new()
            .with_url(url)
            .with_connect_timeout(Duration::from_millis(500))
            .with_read_timeout(Duration::from_secs(1)),
        manual(InputFormat::Ulaw8000),
        Arc::new(FixedKey(KEY)),
        "elevenlabs-api-key",
    )
    .unwrap();
    let started = Instant::now();
    let (handle, _) = recognise(&mut recogniser, mulaw(3));
    assert!(
        matches!(handle.failure(), Some(ElevenLabsError::Connect(error)) if error.status.is_none()),
        "{:?}",
        handle.failure()
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    held.join().unwrap();
}

/// The pings of the first flood of the backed-up cancel. The connection answers each with a
/// pong of 131 bytes; 36,000 of them are 4.7 MB, more than the socket buffers hold where the
/// kernel's largest send buffer is 4 MiB.
const FLOOD: usize = 36_000;

/// The largest flood: a host whose buffers took every pong gets the flood doubled up to here.
/// It is one the connection reads within the test's wait for the hypothesis behind it, so a
/// host that took this one too fails on that and not on the wait.
const LARGEST_FLOOD: usize = 8 * FLOOD;

/// The test's own deadline for a connection thread that nothing ends: past it the test lets the
/// server read again, which ends the thread, and fails.
const THREAD_DEADLINE: Duration = Duration::from_secs(4);

/// The kernel's name of a recognition's connection thread, the first 15 bytes of
/// `inference-elevenlabs-stt`. The kernel keeps no more of a name, so two other threads of the
/// crate are listed under the same 15 bytes: the one a synthesis runs on,
/// `inference-elevenlabs`, and the crate's clock thread, `inference-elevenlabs-clock`.
const THREAD: &str = "inference-eleve";

/// How many threads of this process the kernel lists under a name that starts with `name`. It
/// reads `/proc/self/task`, which Linux has; the gate runs on Linux.
fn threads_named(name: &str) -> usize {
    std::fs::read_dir("/proc/self/task")
        .expect("the kernel lists the threads of this process")
        .flatten()
        .filter(|task| {
            std::fs::read_to_string(task.path().join("comm"))
                .is_ok_and(|comm| comm.starts_with(name))
        })
        .count()
}

/// `cancel` returns at once and the close runs on the recognition's connection thread, so what
/// this test times is the end of that thread. After the first chunk the server sends a flood of
/// pings and one partial transcript, and reads nothing more. The connection answers each ping
/// with a pong as it reads; the pongs fill the socket buffers and then wait in its WebSocket
/// buffer, in front of the close frame, so that frame cannot be written and only the close
/// bound ends the thread. Whether the connection was backed up is the server's to tell: it had
/// read no pong when the test told it to read again, and reading again once the thread is gone,
/// it finds the stream ended with no close frame and fewer pongs than it sent pings. A server
/// that read on and was only slow would count fewer pongs too, which is why the first count is
/// asserted. A flood whose pongs all arrived is doubled and the recognition run again.
///
/// The thread must be gone no sooner than one second after the cancel and before two. Both
/// numbers are written out and not taken from `CLOSE_BOUND`: limits that moved with the
/// constant would pass with any value of it.
///
/// The thread is found by its name, so the test must see no other recognition's thread: it
/// relies on cargo-nextest, which runs each test in a process of its own, and fails where it
/// finds more than its one thread. The name it matches is also the start of the names of a
/// synthesis's thread and of the crate's clock thread, which starts when an async handle waits
/// for a deadline and never ends. This test starts neither: it starts no synthesis, and it
/// takes its events with the blocking iterator. If either ran, the count before the cancel
/// would be two and the test would fail there.
#[test]
fn a_cancel_on_a_backed_up_connection_ends_its_thread_at_the_close_bound() {
    let mut pings = FLOOD;
    loop {
        let after_first = |action| Step {
            on: Trigger::Audio(1),
            action,
        };
        let server = FixtureServer::start(Script::Steps(vec![
            Step {
                on: Trigger::Open,
                action: Action::Send(session_started()),
            },
            after_first(Action::Pings(pings)),
            after_first(Action::Send(partial("hallo"))),
            after_first(Action::Hold),
        ]));
        let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
        let started = Instant::now();
        let mut handle = recogniser.start(request()).unwrap();
        handle.offer_frame(mulaw(1).remove(0)).unwrap();
        let mut events = Vec::new();
        wait_for(
            &mut handle,
            &mut events,
            "hypothesis hallo",
            Duration::from_secs(10),
        );
        let flooded = started.elapsed();
        assert_eq!(
            threads_named(THREAD),
            1,
            "the connection threads of this process; another test's would be counted with it"
        );
        let cancelled = Instant::now();
        handle.cancel();
        let returned = cancelled.elapsed();
        let gone = loop {
            if threads_named(THREAD) == 0 {
                break Some(cancelled.elapsed());
            }
            if cancelled.elapsed() >= THREAD_DEADLINE {
                break None;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        server.read_again();
        let ended = server.next_client_close(Duration::from_secs(5));
        while threads_named(THREAD) != 0 {
            assert!(
                cancelled.elapsed() < THREAD_DEADLINE + Duration::from_secs(5),
                "the connection thread outlived the server's reading again"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let seen = server.seen().remove(0);
        println!(
            "a flood of {pings} pings, read within {flooded:?}: the cancel returned after \
             {returned:?} and the connection thread was gone after {gone:?}; the server had \
             read {} pongs when it was told to read again and {} at the end, close frame: {}, \
             end of the stream: {ended:?}",
            seen.early_pongs, seen.pongs, seen.close_frame
        );
        let gone = gone.unwrap_or_else(|| {
            panic!("the connection thread did not end within {THREAD_DEADLINE:?} of the cancel")
        });
        assert_eq!(ended, Some(0), "the server read no end of the stream");
        if seen.pongs < pings {
            assert_eq!(
                seen.early_pongs, 0,
                "the server read pongs before it was told to read again, so fewer pongs than \
                 pings does not say that the connection was backed up"
            );
            assert!(
                !seen.close_frame,
                "a close frame behind pongs never written"
            );
            assert!(
                gone >= Duration::from_secs(1),
                "the thread of a backed-up connection was gone {gone:?} after the cancel, \
                 before the close bound"
            );
            assert!(
                gone < Duration::from_secs(2),
                "the thread of a backed-up connection was gone {gone:?} after the cancel, at \
                 twice the close bound or later"
            );
            return;
        }
        assert!(
            pings < LARGEST_FLOOD,
            "the socket buffers of this host took every pong of a flood of {pings} pings, {} of \
             them before the server was told to read again, so no close was backed up and \
             nothing was asserted of its bound",
            seen.early_pongs
        );
        pings *= 2;
    }
}

#[test]
fn the_recogniser_is_declared_at_external_provider_with_its_claims() {
    use inference_stt::inference_core::model::Locality;
    let recogniser = ElevenLabsRecogniser::new(
        Endpoint::new(),
        manual(InputFormat::Ulaw8000),
        Arc::new(FixedKey(KEY)),
        "elevenlabs-api-key",
    )
    .unwrap();
    assert_eq!(recogniser.adapter().locality(), Locality::ExternalProvider);
    assert_eq!(
        recogniser.adapter().capabilities(),
        inference_elevenlabs::stt_claims().as_slice()
    );
    assert_eq!(recogniser.adapter().max_concurrency(), 1);
    assert!(
        !format!("{recogniser:?}").contains(KEY),
        "the key is never printed"
    );
}
