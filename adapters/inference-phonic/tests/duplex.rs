//! The Phonic binding against the local fixture server of `support/mod.rs`, which speaks the
//! documented conversation protocol: the opening order and `config`, input chunking, a response
//! from start to finish, barge-in, a closed session, server errors and closes, a handshake never
//! answered, the refusals before connecting, liveness, tools, and the duplex conformance
//! procedure.

mod support;

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use inference_phonic::inference_duplex::conformance::{
    NativeDuplexProbe, NoNativeDuplex, native_duplex_evidence,
};
use inference_phonic::inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_phonic::inference_duplex::inference_core::model::Capability;
use inference_phonic::inference_duplex::inference_tools::{
    DeclareTool, Json, SideEffectClass, Tool,
};
use inference_phonic::inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionFailureKind, SpeechSessionHandle, TurnDetection, TurnTuning, Unsupported,
};
use inference_phonic::protocol::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_PING_INTERVAL, DEFAULT_READ_TIMEOUT, DEFAULT_READY_TIMEOUT,
};
use inference_phonic::{
    CLOSE_BOUND, Credential, CredentialError, CredentialFuture, CredentialSource, PhonicConfig,
    PhonicEndpoint, PhonicError, PhonicSession, PhonicSpeech, claims, native_duplex_verdict,
};
use support::{Handshake, KEY, PhonicServer, Script, Step, bounded};

const WAIT: Duration = Duration::from_millis(20);
const KEY_NAME: &str = "phonic-api-key";

/// The connect timeout given to the open whose handshake is never answered.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

/// That open must end before twice its connect timeout: a bound doubled or more fails the test,
/// a bound multiplied by less than two still passes. The 500 ms past the bound are room for a
/// loaded machine: over repeated runs on one, such an open ended at most 51 ms after its bound,
/// and tests that measure stalls saw one of 204 ms there.
const LATEST: Duration = CONNECT_TIMEOUT.saturating_mul(2);

/// The test's own deadline for an open that nothing ends: past it the test fails instead of
/// waiting.
const DEADLINE: Duration = Duration::from_secs(4);

/// How many connections wait in the queue of `listener`, which nothing accepted before. An
/// attempt is there from the moment its TCP connect completed, so the count needs no wait.
fn attempts(listener: &std::net::TcpListener) -> usize {
    listener.set_nonblocking(true).unwrap();
    let mut count = 0;
    loop {
        match listener.accept() {
            Ok(_) => count += 1,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return count,
            Err(error) => panic!("the listener failed: {error}"),
        }
    }
}

struct FixedKey(&'static str);

impl CredentialSource for FixedKey {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        let key = Credential::new(self.0.as_bytes().to_vec());
        Box::pin(std::future::ready(Ok(key)))
    }
}

struct NoKey;

impl CredentialSource for NoKey {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::ready(Err(CredentialError::Missing)))
    }
}

fn pcm(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        voice: "sabrina".to_owned(),
        instructions: "Answer in one sentence.".to_owned(),
        input_format: pcm(16_000),
        output_format: pcm(16_000),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

/// 20 ms of silence at 16 kHz.
fn frame(position: u64) -> AudioFrame {
    AudioFrame {
        format: pcm(16_000),
        position: SampleClockPosition::new(position).unwrap(),
        sample_count: 320,
        data: vec![0; 640],
    }
}

fn endpoint(server: &PhonicServer) -> PhonicEndpoint {
    PhonicEndpoint::new()
        .with_url(&server.url)
        .with_connect_timeout(Duration::from_secs(3))
        .with_ready_timeout(Duration::from_secs(3))
        .with_read_timeout(Duration::from_secs(3))
}

fn model(server: &PhonicServer) -> PhonicSpeech {
    PhonicSpeech::new(
        endpoint(server),
        PhonicConfig::new(),
        Arc::new(FixedKey(KEY)),
        KEY_NAME,
    )
    .expect("a valid endpoint")
}

fn open(server: &PhonicServer) -> PhonicSession {
    model(server).open(config()).expect("the session opens")
}

/// Every event until `stop` holds for one, that one included, or five seconds pass.
fn events_until(
    session: &mut PhonicSession,
    stop: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while Instant::now() < deadline {
        if let Some(event) = session.next_event(WAIT) {
            let done = stop(&event);
            events.push(event);
            if done {
                break;
            }
        }
    }
    events
}

fn ended(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::SpeechSessionClosed(_) | SessionEvent::SpeechSessionFailed(_)
    )
}

/// The response ids of the `ResponseCreated` events, in order.
fn created(events: &[SessionEvent]) -> Vec<ResponseId> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ResponseCreated(created) => Some(created.response_id),
            _ => None,
        })
        .collect()
}

fn audio_of(events: &[SessionEvent], response_id: ResponseId) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, SessionEvent::OutputAudio(audio) if audio.response_id == response_id))
        .count()
}

fn position_of(events: &[SessionEvent], wanted: impl Fn(&SessionEvent) -> bool) -> usize {
    events
        .iter()
        .position(wanted)
        .unwrap_or_else(|| panic!("{events:#?}"))
}

#[test]
fn config_goes_first_with_the_bearer_key_and_no_audio_precedes_ready() {
    bounded(|| {
        let mut script = Script::new(vec![Step::AwaitAudio(2)]);
        script.ready_delay = Duration::from_millis(300);
        let server = PhonicServer::start(script);
        let started = Instant::now();
        let mut session = open(&server);
        assert!(
            started.elapsed() >= Duration::from_millis(250),
            "the open returned before ready_to_start_conversation"
        );
        assert_eq!(session.conversation_id(), Some("conv_fixture"));
        session.append_audio(frame(0)).unwrap();
        session.append_audio(frame(320)).unwrap();
        let seen = server.wait(0, Duration::from_secs(3), |seen| {
            seen.of_type("audio_chunk").len() == 2
        });
        assert!(seen.violations.is_empty(), "{:?}", seen.violations);
        assert_eq!(seen.path, "/v1/sts/ws");
        assert_eq!(seen.query, "");
        assert_eq!(
            seen.authorization.as_deref(),
            Some(&*format!("Bearer {KEY}"))
        );
        let config = &seen.messages[0].1;
        assert_eq!(config["type"], "config");
        assert_eq!(config["model"], "phonic-1");
        assert_eq!(config["input_format"], "pcm_16000");
        assert_eq!(config["output_format"], "pcm_16000");
        assert_eq!(config["voice_id"], "sabrina");
        assert_eq!(config["system_prompt"], "Answer in one sentence.");
        assert!(config.get("tools").is_none(), "{config}");
        assert_eq!(seen.audio(), vec![vec![0_u8; 640], vec![0_u8; 640]]);
    });
}

#[test]
fn a_frame_longer_than_twenty_milliseconds_goes_out_in_chunks_of_at_most_twenty() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![Step::AwaitAudio(6)]));
        let mut session = open(&server);
        let long = AudioFrame {
            format: pcm(16_000),
            position: SampleClockPosition::new(0).unwrap(),
            sample_count: 1_650,
            data: vec![1; 3_300],
        };
        session.append_audio(long).unwrap();
        let seen = server.wait(0, Duration::from_secs(3), |seen| {
            seen.of_type("audio_chunk").len() == 6
        });
        let sizes: Vec<_> = seen.audio().iter().map(Vec::len).collect();
        assert_eq!(sizes, [640, 640, 640, 640, 640, 100]);
    });
}

#[test]
fn a_response_runs_from_assistant_started_to_finished_with_its_audio_and_text() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::bare("user_started_speaking")),
            Step::Send(support::bare("user_finished_speaking")),
            Step::Send(support::input_text("what is the capital of France")),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "Paris ")),
            Step::Send(support::audio(641, "is the capital.")),
            Step::Send(support::audio(641, "")),
            Step::Send(support::bare("assistant_finished_speaking")),
        ]));
        let mut session = open(&server);
        let events = events_until(&mut session, |event| {
            matches!(event, SessionEvent::ResponseCompleted(_))
        });
        let ids = created(&events);
        assert_eq!(ids.len(), 1, "{events:#?}");
        let id = ids[0];
        let heard = position_of(
            &events,
            |event| matches!(event, SessionEvent::InputTranscribed(heard) if heard.text == "what is the capital of France"),
        );
        let committed = position_of(&events, |event| {
            matches!(event, SessionEvent::InputCommitted(_))
        });
        let started = position_of(&events, |event| {
            matches!(event, SessionEvent::SpeechStarted(_))
        });
        let stopped = position_of(&events, |event| {
            matches!(event, SessionEvent::SpeechStopped(_))
        });
        assert!(started < stopped && committed < heard, "{events:#?}");
        let text: String = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::OutputTranscriptDelta(delta) if delta.response_id == id => {
                    Some(delta.text.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(text, "Paris is the capital.");
        let samples: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::OutputAudio(audio) if audio.response_id == id => {
                    Some((audio.frame.position.get(), audio.frame.sample_count))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            samples,
            vec![(0, 320), (320, 320), (640, 321)],
            "a half sample was not carried over"
        );
        assert!(matches!(
            events.last(),
            Some(SessionEvent::ResponseCompleted(done)) if done.response_id == id
        ));
        assert_eq!(session.final_transcript(id), Some("Paris is the capital."));
    });
}

#[test]
fn barge_in_cancels_the_response_in_progress_and_drops_its_late_audio() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "It is ")),
            Step::Send(support::audio(640, "mild ")),
            Step::Send(support::bare("user_started_speaking")),
            Step::Send(support::interrupted("It is mild")),
            Step::Send(support::audio(640, "and sunny")),
            Step::Send(support::input_text("in Lisbon")),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "Lisbon is warm.")),
            Step::Send(support::bare("assistant_finished_speaking")),
        ]));
        let mut session = open(&server);
        let mut events = events_until(&mut session, |event| {
            matches!(event, SessionEvent::ResponseCancelled(_))
        });
        events.extend(events_until(&mut session, |event| {
            matches!(event, SessionEvent::ResponseCompleted(_))
        }));
        let ids = created(&events);
        assert_eq!(ids.len(), 2, "{events:#?}");
        let (cut, next) = (ids[0], ids[1]);
        assert_eq!(
            audio_of(&events, cut),
            2,
            "late audio of the cut response was delivered"
        );
        assert_eq!(audio_of(&events, next), 1);
        let speech = position_of(&events, |event| {
            matches!(event, SessionEvent::SpeechStarted(_))
        });
        let cancelled = position_of(
            &events,
            |event| matches!(event, SessionEvent::ResponseCancelled(gone) if gone.response_id == cut),
        );
        assert!(speech < cancelled);
        assert!(matches!(
            events.last(),
            Some(SessionEvent::ResponseCompleted(done)) if done.response_id == next
        ));
        assert_eq!(session.late_audio_dropped(), 1);
        assert_eq!(session.final_transcript(cut), Some("It is mild"));
        assert_eq!(session.final_transcript(next), Some("Lisbon is warm."));
    });
}

#[test]
fn a_closed_session_takes_nothing_more_and_closes_its_connection() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "Hello")),
            Step::Pause(Duration::from_secs(10)),
        ]));
        let mut session = open(&server);
        events_until(&mut session, |event| {
            matches!(event, SessionEvent::OutputAudio(_))
        });
        let closing = Instant::now();
        session.close().unwrap();
        assert!(
            closing.elapsed() < CLOSE_BOUND * 2,
            "the close outlived its deadline"
        );
        let events = events_until(&mut session, ended);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, SessionEvent::ResponseCancelled(_))),
            "{events:#?}"
        );
        assert!(matches!(
            events.last(),
            Some(SessionEvent::SpeechSessionClosed(_))
        ));
        assert!(matches!(
            session.append_audio(frame(0)),
            Err(SessionError::NotOpen(_))
        ));
        assert!(session.close().is_err());
        let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
        assert!(
            seen.closed.is_some(),
            "the closed session left its connection open"
        );
        assert!(seen.audio().is_empty());
    });
}

#[test]
fn dropping_a_session_mid_stream_closes_the_connection_within_the_close_bound() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::audio(640, "Hello")),
            Step::Pause(Duration::from_secs(10)),
        ]));
        let mut session = open(&server);
        let events = events_until(&mut session, |event| {
            matches!(event, SessionEvent::OutputAudio(_))
        });
        assert!(
            !created(&events).is_empty(),
            "audio with no response in progress opened none"
        );
        let dropped_at = Instant::now();
        drop(session);
        assert!(dropped_at.elapsed() < CLOSE_BOUND);
        let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
        let closed = seen.closed.expect("the connection was closed");
        assert!(closed.saturating_duration_since(dropped_at) < CLOSE_BOUND);
    });
}

/// The pings of the first flood of the backed-up close. The session answers each with a pong of
/// 131 bytes; 36,000 of them are 4.7 MB, more than the socket buffers hold where the kernel's
/// largest send buffer is 4 MiB.
const FLOOD: usize = 36_000;

/// The largest flood: a host whose buffers took every pong gets the flood doubled up to here.
const LARGEST_FLOOD: usize = 16 * FLOOD;

/// The test's own deadline for a close that nothing ends: past it the test lets the server read
/// again, which ends the close, and fails.
const CLOSE_DEADLINE: Duration = Duration::from_secs(4);

/// The server reads nothing, sends a flood of pings and then one audio chunk. The session
/// answers each ping with a pong as it reads; the pongs fill the socket buffers and then wait in
/// the session's WebSocket buffer, in front of the close frame, so that frame cannot be written
/// and only the close bound ends the close. Whether the connection was backed up is the
/// server's to tell: reading again after the close, it counts fewer pongs than it sent pings. A
/// flood whose pongs all arrived is doubled and the session run again.
///
/// The close must hold its caller no shorter than one second and less than two. Both numbers
/// are written out and not taken from `CLOSE_BOUND`: limits that moved with the constant would
/// pass with any value of it. The bound is applied twice here, on the write of the close frame
/// and on the caller's wait for the connection task, both against the same blocked write; this
/// test does not tell the two apart.
#[test]
fn a_close_on_a_backed_up_connection_ends_at_the_close_bound() {
    bounded(|| {
        let mut pings = FLOOD;
        loop {
            let mut script = Script::new(vec![
                Step::Pings(pings),
                Step::Send(support::audio(640, "Hello")),
            ]);
            script.deaf = true;
            let server = PhonicServer::start(script);
            let patient = endpoint(&server)
                .with_read_timeout(Duration::from_secs(30))
                .with_ping_interval(Duration::from_secs(30));
            let mut model = PhonicSpeech::new(
                patient,
                PhonicConfig::new(),
                Arc::new(FixedKey(KEY)),
                KEY_NAME,
            )
            .unwrap();
            let opened = Instant::now();
            let mut session = model.open(config()).expect("the session opens");
            let events = events_until(&mut session, |event| {
                matches!(event, SessionEvent::OutputAudio(_))
            });
            assert!(
                matches!(events.last(), Some(SessionEvent::OutputAudio(_))),
                "the audio behind a flood of {pings} pings did not arrive: {events:#?}"
            );
            let flooded = opened.elapsed();
            let (done, finished) = mpsc::channel();
            let closer = std::thread::spawn(move || {
                let closing = Instant::now();
                let closed = session.close();
                let _ = done.send((closing.elapsed(), closed));
            });
            let outcome = finished.recv_timeout(CLOSE_DEADLINE);
            server.read_again(0);
            closer.join().expect("the close returned");
            let (held, closed) = outcome.unwrap_or_else(|error| {
                panic!("the close did not end within {CLOSE_DEADLINE:?}: {error}")
            });
            closed.expect("an open session closes");
            let seen = server.wait(0, Duration::from_secs(5), |seen| seen.closed.is_some());
            assert!(
                seen.closed.is_some(),
                "the server read no end of the stream"
            );
            println!(
                "a flood of {pings} pings, read within {flooded:?}: the close held {held:?}; the \
                 server then read {} pongs, close frame: {}",
                seen.pongs, seen.close_frame
            );
            if seen.pongs < pings {
                assert!(
                    !seen.close_frame,
                    "a close frame behind pongs never written"
                );
                assert!(
                    held >= Duration::from_secs(1),
                    "the close on a backed-up connection returned after {held:?}, before its bound"
                );
                assert!(
                    held < Duration::from_secs(2),
                    "the close on a backed-up connection held {held:?}, twice its bound or longer"
                );
                return;
            }
            assert!(
                pings < LARGEST_FLOOD,
                "the socket buffers of this host took every pong of a flood of {pings} pings, \
                 so no close was backed up and nothing was asserted of its bound"
            );
            pings *= 2;
        }
    });
}

#[test]
fn a_server_close_mid_response_fails_the_response_and_the_session_with_its_code() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "Hel")),
            Step::Close(4505),
        ]));
        let mut session = open(&server);
        let events = events_until(&mut session, ended);
        let ids = created(&events);
        assert_eq!(ids.len(), 1);
        assert!(
            events.iter().any(
                |event| matches!(event, SessionEvent::ResponseFailed(failed) if failed.response_id == ids[0])
            ),
            "{events:#?}"
        );
        match events.last() {
            Some(SessionEvent::SpeechSessionFailed(failed)) => {
                assert_eq!(failed.failure.kind, SessionFailureKind::TransportClosed);
                assert!(failed.failure.detail.contains("4505"), "{failed:?}");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            session.failure_cause(),
            Some(PhonicError::Closed {
                code: Some(4505),
                ..
            })
        ));
    });
}

#[test]
fn an_error_fails_the_session_as_a_provider_error_and_a_warning_does_not() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::warning("the voice is deprecated")),
            Step::Send(support::error("audio chunk too long", "chunk_too_long")),
        ]));
        let mut session = open(&server);
        let events = events_until(&mut session, ended);
        match events.last() {
            Some(SessionEvent::SpeechSessionFailed(failed)) => {
                assert_eq!(failed.failure.kind, SessionFailureKind::ProviderError);
                assert!(failed.failure.detail.contains("chunk_too_long"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(session.warnings(), 1);
        assert_eq!(
            session.failure_cause(),
            Some(&PhonicError::Server {
                code: Some("chunk_too_long".to_owned()),
                message: "audio chunk too long".to_owned(),
            })
        );
    });
}

#[test]
fn no_ready_within_the_deadline_fails_the_open_and_closes_the_connection() {
    bounded(|| {
        let mut script = Script::new(Vec::new());
        script.ready = false;
        let server = PhonicServer::start(script);
        let mut model = PhonicSpeech::new(
            endpoint(&server).with_ready_timeout(Duration::from_millis(200)),
            PhonicConfig::new(),
            Arc::new(FixedKey(KEY)),
            KEY_NAME,
        )
        .unwrap();
        let started = Instant::now();
        let outcome = model.open(config());
        match &outcome {
            Err(OpenError::Failed(failure)) => {
                assert_eq!(failure.kind, SessionFailureKind::Stalled)
            }
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() < Duration::from_millis(200) + CLOSE_BOUND);
        assert!(matches!(model.open_error(), Some(PhonicError::Stalled(_))));
        let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
        assert!(
            seen.closed.is_some(),
            "the failed open left its connection open"
        );
    });
}

/// The bounds of the endpoint a caller gets from `PhonicEndpoint::new()`, their constants in
/// `inference_phonic::protocol` and the close bound, each compared with its number written out:
/// a changed default fails here. Outside the source, `quality/adapters.md`, entry
/// `phonic-time-bounds/held`, states the bound on the wait for the ready message (10 s), the
/// read timeout (30 s), the ping interval (10 s) and the close bound (1 s). No document states
/// the connect timeout: for it the number below is the reference.
#[test]
fn the_time_bound_defaults_are_the_stated_numbers() {
    let endpoint = PhonicEndpoint::new();
    let defaults = [
        (
            "protocol::DEFAULT_CONNECT_TIMEOUT",
            DEFAULT_CONNECT_TIMEOUT,
            Duration::from_secs(10),
        ),
        (
            "PhonicEndpoint::new().connect_timeout",
            endpoint.connect_timeout,
            Duration::from_secs(10),
        ),
        (
            "protocol::DEFAULT_READY_TIMEOUT",
            DEFAULT_READY_TIMEOUT,
            Duration::from_secs(10),
        ),
        (
            "PhonicEndpoint::new().ready_timeout",
            endpoint.ready_timeout,
            Duration::from_secs(10),
        ),
        (
            "protocol::DEFAULT_READ_TIMEOUT",
            DEFAULT_READ_TIMEOUT,
            Duration::from_secs(30),
        ),
        (
            "PhonicEndpoint::new().read_timeout",
            endpoint.read_timeout,
            Duration::from_secs(30),
        ),
        (
            "protocol::DEFAULT_PING_INTERVAL",
            DEFAULT_PING_INTERVAL,
            Duration::from_secs(10),
        ),
        (
            "PhonicEndpoint::new().ping_interval",
            endpoint.ping_interval,
            Duration::from_secs(10),
        ),
        ("CLOSE_BOUND", CLOSE_BOUND, Duration::from_secs(1)),
    ];
    let changed: Vec<_> = defaults
        .iter()
        .filter(|(_, default, stated)| default != stated)
        .collect();
    assert!(
        changed.is_empty(),
        "each default that is not its stated number, as (name, default, stated): {changed:?}"
    );
}

/// The listener takes the TCP connection into its queue and nothing reads it or answers the
/// upgrade, so only the connect timeout ends the open. A refused connection is the same variant
/// with no status: the detail and the time tell the two apart. The open connects once: a second
/// attempt after the timeout is a second connection in the listener's queue. The wait is the
/// test's own, so an open that no timeout ends fails here at [`DEADLINE`] instead of hanging.
#[test]
fn a_handshake_never_answered_fails_the_open_at_the_connect_timeout() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = PhonicEndpoint::new()
        .with_url(format!("ws://{}", listener.local_addr().unwrap()))
        .with_connect_timeout(CONNECT_TIMEOUT);
    let (done, waited) = mpsc::channel();
    std::thread::spawn(move || {
        let mut model = PhonicSpeech::new(
            endpoint,
            PhonicConfig::new(),
            Arc::new(FixedKey(KEY)),
            KEY_NAME,
        )
        .unwrap();
        let started = Instant::now();
        let outcome = model.open(config()).err();
        let took = started.elapsed();
        let _ = done.send((outcome, model.open_error().cloned(), took));
    });
    let (outcome, error, took) = waited
        .recv_timeout(DEADLINE)
        .unwrap_or_else(|error| panic!("the open did not end within {DEADLINE:?}: {error}"));
    match outcome {
        Some(OpenError::Failed(failure)) => {
            assert_eq!(failure.kind, SessionFailureKind::TransportClosed)
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        error,
        Some(PhonicError::Connect {
            status: None,
            detail: format!("no handshake within {CONNECT_TIMEOUT:?}"),
        })
    );
    assert_eq!(attempts(&listener), 1, "the connections the open made");
    assert!(
        took >= CONNECT_TIMEOUT,
        "the open failed after {took:?}, before its connect timeout"
    );
    assert!(
        took < LATEST,
        "the open failed after {took:?}, at twice its connect timeout or later"
    );
}

#[test]
fn a_refused_handshake_or_a_missing_key_fails_the_open_and_the_key_is_in_no_error() {
    bounded(|| {
        let mut script = Script::new(Vec::new());
        script.handshake = Handshake::Refuse(401);
        let server = PhonicServer::start(script);
        let mut model = model(&server);
        let outcome = model.open(config());
        match &outcome {
            Err(OpenError::Failed(failure)) => {
                assert_eq!(failure.kind, SessionFailureKind::Rejected)
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            model.open_error(),
            Some(PhonicError::Connect {
                status: Some(401),
                ..
            })
        ));
        for rendered in [
            format!("{outcome:?}"),
            format!("{:?}", model.open_error()),
            format!("{model:?}"),
        ] {
            assert!(!rendered.contains(KEY), "{rendered}");
        }

        let mut keyless = PhonicSpeech::new(
            endpoint(&server),
            PhonicConfig::new(),
            Arc::new(NoKey),
            KEY_NAME,
        )
        .unwrap();
        assert!(matches!(keyless.open(config()), Err(OpenError::Failed(_))));
        assert_eq!(
            keyless.open_error(),
            Some(&PhonicError::Credential(CredentialError::Missing))
        );
        assert_eq!(server.connections(), 1, "a session without a key connected");
    });
}

#[test]
fn a_format_phonic_does_not_take_and_a_plaintext_remote_host_are_refused_before_connecting() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(Vec::new()));
        let mut model = model(&server);
        let mut wide = config();
        wide.output_format = pcm(48_000);
        assert_eq!(
            model.open(wide).err(),
            Some(OpenError::Unsupported(Unsupported::OutputFormat(pcm(
                48_000
            ))))
        );
        let mut manual = config();
        manual.turn_detection = TurnDetection::Disabled;
        assert!(matches!(model.open(manual), Err(OpenError::Unsupported(_))));
        for url in [
            "ws://api.phonic.ai",
            "ws://10.0.0.1:9",
            "http://127.0.0.1:9",
        ] {
            assert!(
                matches!(
                    PhonicSpeech::new(
                        PhonicEndpoint::new().with_url(url),
                        PhonicConfig::new(),
                        Arc::new(FixedKey(KEY)),
                        KEY_NAME,
                    ),
                    Err(PhonicError::InvalidUrl(_))
                ),
                "{url}"
            );
        }
        assert!(matches!(
            PhonicSpeech::new(
                endpoint(&server),
                PhonicConfig::new().with_agent("  "),
                Arc::new(FixedKey(KEY)),
                KEY_NAME,
            ),
            Err(PhonicError::InvalidConfig("agent"))
        ));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(server.connections(), 0);
    });
}

#[test]
fn a_quiet_server_that_answers_pings_stays_open_and_a_deaf_one_stalls() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![Step::Pause(Duration::from_secs(10))]));
        let quiet = endpoint(&server)
            .with_read_timeout(Duration::from_millis(400))
            .with_ping_interval(Duration::from_millis(100));
        let mut model = PhonicSpeech::new(
            quiet.clone(),
            PhonicConfig::new(),
            Arc::new(FixedKey(KEY)),
            KEY_NAME,
        )
        .unwrap();
        let mut session = model.open(config()).unwrap();
        let mut quiet_session = Vec::new();
        let until = Instant::now() + Duration::from_millis(1_200);
        while Instant::now() < until {
            quiet_session.extend(session.next_event(WAIT));
        }
        assert!(!quiet_session.iter().any(ended), "{quiet_session:#?}");
        assert!(server.seen(0).pings >= 2, "{:?}", server.seen(0).pings);
        drop(session);

        let mut script = Script::new(Vec::new());
        script.deaf = true;
        let deaf = PhonicServer::start(script);
        let mut model = PhonicSpeech::new(
            quiet.with_url(&deaf.url),
            PhonicConfig::new(),
            Arc::new(FixedKey(KEY)),
            KEY_NAME,
        )
        .unwrap();
        let mut session = model.open(config()).unwrap();
        let started = Instant::now();
        let events = events_until(&mut session, ended);
        assert!(started.elapsed() < Duration::from_secs(2));
        match events.last() {
            Some(SessionEvent::SpeechSessionFailed(failed)) => {
                assert_eq!(failed.failure.kind, SessionFailureKind::Stalled);
            }
            other => panic!("{other:?}"),
        }
    });
}

fn time_tool() -> Tool {
    Tool::declare(DeclareTool {
        name: "local_time".to_owned(),
        description: "The local time.".to_owned(),
        schema: serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

#[test]
fn a_tool_the_session_offers_is_declared_inline_proposed_and_answered() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::tool_call("local_time", "call_1")),
            Step::AwaitType("tool_call_output"),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "It is noon.")),
            Step::Send(support::bare("assistant_finished_speaking")),
        ]));
        let mut model = model(&server);
        let mut session_config = config();
        session_config.tools = vec![time_tool()];
        let mut session = model.open(session_config).unwrap();
        let events = events_until(&mut session, |event| {
            matches!(event, SessionEvent::SpeechToolCallProposed(_))
        });
        let Some(SessionEvent::SpeechToolCallProposed(proposed)) = events.last() else {
            panic!("{events:#?}");
        };
        let call_id = proposed.call_id.clone();
        let calling = proposed.response_id;
        assert_eq!(call_id.as_str(), "call_1");
        let events = events_until(&mut session, |event| {
            matches!(event, SessionEvent::ResponseCompleted(_))
        });
        assert!(
            matches!(
                events.last(),
                Some(SessionEvent::ResponseCompleted(done)) if done.response_id == calling
            ),
            "story:phonic-realtime-adapter: the tool-call response completes once the call is \
             proposed, before the answer: {events:#?}"
        );
        session
            .submit_tool_result(&call_id, serde_json::json!({"time": "12:00"}))
            .unwrap();
        let events = events_until(&mut session, |event| {
            matches!(event, SessionEvent::ResponseCompleted(_))
        });
        assert!(
            matches!(
                events.last(),
                Some(SessionEvent::ResponseCompleted(done)) if done.response_id != calling
            ),
            "story:phonic-realtime-adapter: the answer opens the next response: {events:#?}"
        );
        let seen = server.seen(0);
        let tools = &seen.messages[0].1["tools"];
        assert_eq!(tools[0]["type"], "custom_websocket");
        assert_eq!(tools[0]["tool_schema"]["function"]["name"], "local_time");
        assert_eq!(tools[0]["tool_schema"]["function"]["strict"], true);
        let outputs = seen.of_type("tool_call_output");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0]["tool_call_id"], "call_1");
        assert_eq!(outputs[0]["output"], serde_json::json!({"time": "12:00"}));
        let _: Json = outputs[0]["output"].clone();
    });
}

#[test]
fn a_tool_the_session_does_not_offer_fails_the_session_naming_it() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![Step::Send(support::tool_call(
            "open_door",
            "call_2",
        ))]));
        let mut session = open(&server);
        let events = events_until(&mut session, ended);
        match events.last() {
            Some(SessionEvent::SpeechSessionFailed(failed)) => {
                assert!(failed.failure.detail.contains("open_door"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            session.failure_cause(),
            Some(&PhonicError::ToolNotOffered {
                name: "open_door".to_owned()
            })
        );
    });
}

#[test]
fn a_tool_of_the_named_agent_is_answered_with_an_error_and_noted_and_the_session_goes_on() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::tool_call("open_door", "call_3")),
            Step::AwaitType("tool_call_output"),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "I cannot open it.")),
            Step::Send(support::bare("assistant_finished_speaking")),
        ]));
        let mut model = PhonicSpeech::new(
            endpoint(&server),
            PhonicConfig::new().with_agent("front-desk"),
            Arc::new(FixedKey(KEY)),
            KEY_NAME,
        )
        .unwrap();
        let mut session = model.open(config()).unwrap();
        let events = events_until(&mut session, |event| {
            ended(event) || matches!(event, SessionEvent::ResponseCompleted(_))
        });
        assert!(
            matches!(events.last(), Some(SessionEvent::ResponseCompleted(_))),
            "{events:#?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SessionEvent::SpeechToolCallProposed(_))),
            "{events:#?}"
        );
        assert!(
            session
                .ledger()
                .dropped_events()
                .contains(&SessionError::ToolNotOffered {
                    name: "open_door".to_owned()
                }),
            "{:?}",
            session.ledger().dropped_events()
        );
        let outputs = server.seen(0).of_type("tool_call_output");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0]["tool_call_id"], "call_3");
        let error = outputs[0]["output"]["error"].as_str().unwrap_or_default();
        assert!(error.contains("open_door"), "{outputs:?}");
    });
}

/// The probe: a question, then during the answer a place; the fixture interrupts the answer as
/// the documentation describes, and answers in a new response.
fn probe() -> NativeDuplexProbe {
    NativeDuplexProbe {
        config: config(),
        opening: vec![frame(0), frame(320)],
        during: vec![frame(640), frame(960)],
        marker: "lisbon".to_owned(),
        wait: WAIT,
        patience: 100,
    }
}

#[test]
fn the_duplex_conformance_procedure_runs_over_the_fixture_with_barge_in_and_agrees_with_the_claims()
{
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::AwaitAudio(2),
            Step::Send(support::input_text("what is the weather")),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "The weather ")),
            Step::Send(support::audio(640, "is ")),
            Step::AwaitAudio(4),
            Step::Send(support::bare("user_started_speaking")),
            Step::Send(support::interrupted("The weather is")),
            Step::Send(support::input_text("in Lisbon")),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "It is mild in Lisbon.")),
            Step::Send(support::bare("assistant_finished_speaking")),
            Step::Pause(Duration::from_secs(10)),
        ]));
        let mut model = model(&server);
        let outcome = native_duplex_evidence(&mut model, &probe());
        assert!(
            matches!(outcome, Err(NoNativeDuplex::Restarted { first, second }) if first != second),
            "{outcome:?}"
        );
        assert_eq!(native_duplex_verdict(&outcome), Ok(true));
        assert!(!claims().contains(&Capability::NativeDuplex));
        let seen = server.wait(0, CLOSE_BOUND * 2, |seen| seen.closed.is_some());
        assert!(
            seen.closed.is_some(),
            "the procedure's close left the connection open"
        );
        assert!(seen.violations.is_empty(), "{:?}", seen.violations);
    });
}
