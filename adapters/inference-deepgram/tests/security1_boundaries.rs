//! Security review, boundaries of the binding's own contract: a key that cannot authenticate is
//! refused before any connection, a key that would split the header is refused typed, a
//! plaintext URL that only looks like loopback is refused, no reachable error renders the key, a
//! hostile server message is a typed outcome and never a panic, and a cancel during the opening
//! handshake closes the connection within the close bound.

mod support;

use std::io::Read;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use inference_deepgram::{
    CLOSE_BOUND, ConfigError, CredentialError, DeepgramError, DeepgramRecogniser, Endpoint,
    ListenConfig,
};
use inference_stt::{Recogniser, RecognitionHandle};
use support::{
    Action, FixedKey, FixtureServer, KEY, NoKey, Script, Step, Trigger, recognise, request, tone,
};

fn recogniser_at(endpoint: Endpoint, key: &'static str) -> DeepgramRecogniser {
    DeepgramRecogniser::new(
        endpoint,
        ListenConfig::new(),
        Arc::new(FixedKey(key)),
        "deepgram-api-key",
    )
    .expect("a valid configuration")
}

#[test]
fn an_empty_or_blank_key_is_refused_before_any_connection() {
    for key in ["", "   ", "\t"] {
        let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
        let mut recogniser = recogniser_at(server.endpoint(), key);
        let (handle, _) = recognise(&mut recogniser, tone(2));
        assert!(
            matches!(handle.failure(), Some(DeepgramError::Credential(_))),
            "key {key:?}: expected a typed credential refusal, got {:?}",
            handle.failure()
        );
        assert!(
            server.seen().is_empty(),
            "key {key:?}: a handshake was sent with an Authorization of {:?}",
            server
                .seen()
                .first()
                .and_then(|seen| seen.authorization.clone())
        );
    }
}

#[test]
fn a_key_that_would_split_the_header_is_refused_typed_before_any_connection() {
    for key in [
        "fixture-key-5f2b8c0e91d3a7c4\r\nx-injected: 1",
        "fixture-key-5f2b8c0e91d3a7c4\nx-injected: 1",
        "fixture-key-5f2b8c0e91d3a7c4\r",
        "fixture-key-5f2b8c0e91d3a7c4\0",
    ] {
        let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
        let mut recogniser = recogniser_at(server.endpoint(), key);
        let (handle, _) = recognise(&mut recogniser, tone(2));
        assert_eq!(
            handle.failure(),
            Some(&DeepgramError::Credential(CredentialError::Invalid)),
            "{key:?}"
        );
        assert!(server.seen().is_empty(), "{key:?}: a handshake was sent");
        let shown = format!("{} {:?}", handle.failure().unwrap(), handle);
        assert!(!shown.contains(KEY), "the key shows in {shown}");
    }
}

#[test]
fn plaintext_urls_that_only_look_loopback_are_refused_without_naming_the_url() {
    for url in [
        "ws://localhost.example.com",
        "ws://127.0.0.1.nip.io",
        "ws://[::ffff:127.0.0.1]",
        "ws://0.0.0.0",
        "ws://0.0.0.0:9",
        "ws://user@localhost",
        "ws://user:pw@127.0.0.1:9",
        "ws://127.0.0.1\\@example.com",
        "ws://localhost%2eexample.com",
        "ws://127.1",
        "ws://2130706433",
        "WS://example.com",
        "Ws://example.com",
        " ws://localhost",
        "wss:/example.com",
        "http://localhost",
    ] {
        match Endpoint::new().with_url(url).base() {
            Err(ConfigError::InvalidUrl(detail)) => {
                assert!(!detail.contains("example"), "{url}: {detail}");
                assert!(!detail.contains(KEY), "{url}: {detail}");
            }
            other => panic!("{url}: taken as {other:?}"),
        }
        assert!(
            DeepgramRecogniser::new(
                Endpoint::new().with_url(url),
                ListenConfig::new(),
                Arc::new(FixedKey(KEY)),
                "deepgram-api-key",
            )
            .is_err(),
            "{url}: a recogniser was built"
        );
    }
}

#[test]
fn the_region_switch_names_only_its_host_and_the_url_carries_no_key() {
    let format = tone(1).remove(0).format;
    for (endpoint, prefix) in [
        (Endpoint::new(), "wss://api.deepgram.com/v1/listen?"),
        (Endpoint::eu(), "wss://api.eu.deepgram.com/v1/listen?"),
    ] {
        let url = recogniser_at(endpoint, KEY).request_url(format).unwrap();
        assert!(url.starts_with(prefix), "{url}");
        assert!(!url.contains(KEY), "{url}");
        assert!(!url.contains('@'), "{url}");
    }
}

#[test]
fn no_reachable_failure_renders_the_key() {
    let closed_port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("ws://{}", listener.local_addr().unwrap())
    };
    let mut outcomes = Vec::new();
    for script in [
        Script::Refuse(401),
        Script::Refuse(403),
        Script::Refuse(500),
        Script::fixture("error.jsonl"),
        Script::fixture("malformed.jsonl"),
        Script::fixture("no-metadata.jsonl"),
        Script::closes_after_first(1008, "DATA-0000"),
        Script::closes_after_first(1011, "NET-0001"),
    ] {
        let server = FixtureServer::start(script.clone());
        let mut recogniser = server.recogniser();
        let (handle, _) = recognise(&mut recogniser, tone(4));
        outcomes.push((format!("{script:?}"), handle, recogniser));
    }
    let endpoint = Endpoint::new()
        .with_url(&closed_port)
        .with_connect_timeout(Duration::from_secs(2));
    let mut recogniser = recogniser_at(endpoint.clone(), KEY);
    let (handle, _) = recognise(&mut recogniser, tone(2));
    outcomes.push(("closed port".to_owned(), handle, recogniser));
    let mut recogniser = DeepgramRecogniser::new(
        endpoint.clone(),
        ListenConfig::new(),
        Arc::new(NoKey),
        "deepgram-api-key",
    )
    .unwrap();
    let (handle, _) = recognise(&mut recogniser, tone(2));
    outcomes.push(("no key".to_owned(), handle, recogniser));
    let mut recogniser = recogniser_at(endpoint, "fixture-key-5f2b8c0e91d3a7c4\r\n");
    let (handle, _) = recognise(&mut recogniser, tone(2));
    outcomes.push(("split key".to_owned(), handle, recogniser));

    for (case, handle, recogniser) in &outcomes {
        let error = handle
            .failure()
            .unwrap_or_else(|| panic!("{case}: no failure"));
        for shown in [
            error.to_string(),
            format!("{error:?}"),
            format!("{handle:?}"),
            format!("{recogniser:?}"),
        ] {
            assert!(!shown.contains(KEY), "{case}: the key shows in {shown}");
            assert!(
                !shown.contains("5f2b8c0e"),
                "{case}: part of the key shows in {shown}"
            );
        }
    }
}

fn after_first_audio(text: String) -> Step {
    Step {
        on: Trigger::Audio(1),
        action: Action::Send(text),
    }
}

fn on_close_stream(text: &str) -> Step {
    Step {
        on: Trigger::CloseStream,
        action: Action::Send(text.to_owned()),
    }
}

#[test]
fn a_hostile_server_message_is_a_typed_outcome_never_a_panic() {
    let deep = format!(
        "{{\"type\":\"Results\",\"channel\":{}{}}}",
        "[".repeat(100_000),
        "]".repeat(100_000)
    );
    let metadata = r#"{"type":"Metadata","duration":1.0}"#;
    let cases: Vec<(&str, Vec<Step>, bool)> = vec![
        ("deeply nested JSON", vec![after_first_audio(deep)], false),
        (
            "a number past f64",
            vec![
                after_first_audio(support::results("hi", false, false)),
                on_close_stream(r#"{"type":"Metadata","duration":1e400}"#),
                on_close_stream(metadata),
            ],
            false,
        ),
        (
            "a negative duration",
            vec![on_close_stream(r#"{"type":"Metadata","duration":-5}"#)],
            true,
        ),
        (
            "a huge last_word_end and an out-of-range channel_index",
            vec![
                after_first_audio(
                    r#"{"type":"Results","channel_index":[99999999999,-1],"start":-1e300,"duration":1e300,"is_final":false,"channel":{"alternatives":[{"transcript":"hi"}]}}"#
                        .to_owned(),
                ),
                after_first_audio(
                    r#"{"type":"UtteranceEnd","channel":[-1],"last_word_end":1.7e308}"#.to_owned(),
                ),
                on_close_stream(metadata),
            ],
            true,
        ),
        (
            "an empty object",
            vec![after_first_audio("{}".to_owned())],
            false,
        ),
        (
            "a non-numeric last_word_end",
            vec![after_first_audio(
                r#"{"type":"UtteranceEnd","last_word_end":"-1"}"#.to_owned(),
            )],
            false,
        ),
        (
            "a one-megabyte transcript",
            vec![
                after_first_audio(support::results(&"a".repeat(1 << 20), false, false)),
                on_close_stream(metadata),
            ],
            true,
        ),
    ];
    for (case, steps, completes) in cases {
        let server = FixtureServer::start(Script::Steps(steps));
        let started = Instant::now();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (handle, _) = recognise(&mut server.recogniser(), tone(2));
            (handle.failure().cloned(), handle.provider_duration())
        }));
        let (failure, duration) = outcome.unwrap_or_else(|_| panic!("{case}: panicked"));
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "{case}: took {:?}",
            started.elapsed()
        );
        if completes {
            assert_eq!(failure, None, "{case}");
        } else {
            assert!(
                matches!(failure, Some(DeepgramError::Protocol(_))),
                "{case}: {failure:?}"
            );
        }
        eprintln!("{case}: failure {failure:?}, provider_duration {duration:?}");
    }
}

#[test]
fn a_cancel_during_the_opening_handshake_closes_the_connection_within_the_close_bound() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = Endpoint::new()
        .with_url(format!("ws://{}", listener.local_addr().unwrap()))
        .with_connect_timeout(Duration::from_secs(8))
        .with_read_timeout(Duration::from_secs(3));
    let mut recogniser = recogniser_at(endpoint, KEY);
    let mut handle = recogniser.start(request()).unwrap();
    handle.offer_frame(tone(1).remove(0)).unwrap();
    let _ = handle.next();
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(12)))
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(stream.read(&mut byte).unwrap(), 1, "the request head ends");
        head.push(byte[0]);
    }
    handle.cancel();
    let cancelled = Instant::now();
    drop(handle);
    let next = recogniser.start(request());
    assert!(next.is_ok(), "the slot is free once the handle is dropped");
    let mut rest = [0u8; 64];
    let closed_after = loop {
        match stream.read(&mut rest) {
            Ok(0) | Err(_) => break cancelled.elapsed(),
            Ok(_) => {}
        }
    };
    assert!(
        closed_after <= CLOSE_BOUND + Duration::from_millis(500),
        "the connection stayed open {closed_after:?} after the cancel, while the slot was already \
         free; the handshake runs to its connect timeout before the cancel is read"
    );
}
