//! Adversary cases, pass 2, for story:elevenlabs-realtime-stt-adapter: the opening of the
//! connection once input has ended, which the correction after pass 1 bounds by the connect
//! timeout and starts the read timeout from. Each case pins one line of that correction that no
//! other case of the suite reaches.
#![cfg(feature = "stt")]

mod stt_support;

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use inference_elevenlabs::{
    CredentialFuture, CredentialSource, ElevenLabsError, ElevenLabsRecogniser, Endpoint,
    InputFormat,
};
use inference_stt::TranscriptEvent;
use inference_stt::inference_core::envelope::EventEnvelope;
use serde_json::Value;
use stt_support::{FixedKey, KEY, describe, manual, mulaw, partial, recognise, session_started};
use tokio_tungstenite::tungstenite::{self, Message};

fn usage_is_final(events: &[EventEnvelope<TranscriptEvent>]) -> Option<bool> {
    match events.last().map(|event| &event.payload) {
        Some(TranscriptEvent::Usage(record)) => Some(record.r#final),
        _ => None,
    }
}

/// The crate documentation: "after the end of input the read timeout runs from the later of the
/// end of input, the opening of the connection and the last server message, and does not run
/// while the handshake is pending". Pass 1 (security finding 1) named the second half of the
/// defect: "a handshake that is slow but succeeds loses its transcript the same way". Here the
/// server answers the upgrade 1.2 s after the client connected, four times the 300 ms read
/// timeout, and then answers the end-of-input commit at once. The recognition must deliver the
/// transcript. It fails as `Stalled` if the read timeout is not restarted when the connection
/// opens (`recogniser.rs:466`, `Inbound::Opened`).
#[test]
fn a_handshake_slower_than_the_read_timeout_that_succeeds_after_the_end_of_input_keeps_its_transcript()
 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (accepted_at, accepted) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let _ = accepted_at.send(Instant::now());
        std::thread::sleep(Duration::from_millis(1_200));
        let Ok(mut socket) = tungstenite::accept(stream) else {
            return;
        };
        let _ = socket.send(Message::text(session_started()));
        let mut heard = false;
        loop {
            match socket.read() {
                Ok(Message::Text(text)) => {
                    let chunk: Value = serde_json::from_str(text.as_str()).unwrap();
                    let audio = chunk["audio_base_64"]
                        .as_str()
                        .is_some_and(|a| !a.is_empty());
                    if audio && !heard {
                        heard = true;
                        let _ = socket.send(Message::text(partial("eins")));
                    }
                    if chunk["commit"] == true {
                        let committed = serde_json::json!({
                            "message_type": "committed_transcript_with_timestamps",
                            "text": "eins",
                            "words": [{"text": "eins", "start": 0.0, "end": 0.1, "type": "word"}]
                        });
                        let _ = socket.send(Message::text(committed.to_string()));
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });
    let mut recogniser = ElevenLabsRecogniser::new(
        Endpoint::new()
            .with_url(url)
            .with_connect_timeout(Duration::from_secs(5))
            .with_read_timeout(Duration::from_millis(300)),
        manual(InputFormat::Ulaw8000),
        Arc::new(FixedKey(KEY)),
        "elevenlabs-api-key",
    )
    .unwrap();
    let (handle, events) = recognise(&mut recogniser, mulaw(5));
    let ended = Instant::now();
    let accepted_at = accepted.recv_timeout(Duration::from_secs(1)).unwrap();
    let failure = handle.failure().cloned();
    let described: Vec<String> = events.iter().map(describe).collect();
    let is_final = usage_is_final(&events);
    drop(handle);
    let _ = server.join();
    assert!(
        ended.duration_since(accepted_at) >= Duration::from_millis(1_200),
        "the scenario needs the handshake to finish after the end of input"
    );
    assert_eq!(
        failure, None,
        "the connection opened 1.2 s after the client connected and the commit was answered at \
         once, yet the recognition failed; events {described:?}"
    );
    assert_eq!(described, ["hypothesis eins", "commit eins", "usage"]);
    assert_eq!(is_final, Some(true));
}

/// A credential source whose future never resolves, as a keychain waiting on an unlock prompt
/// nobody answers.
struct NeverResolves;

impl CredentialSource for NeverResolves {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::pending())
    }
}

/// The crate documentation: "The opening, the credential included, is bounded by the connect
/// timeout", and once input has ended the handle waits on the opening with no deadline of its
/// own. A credential that never resolves must fail the recognition as `Connect` within about the
/// connect timeout, not hold the caller's thread. Only the connect timeout that `drive` wraps
/// around `Connection::open` (`recogniser.rs:718`) bounds the credential; the handshake's own
/// timeout inside `Connection::open` starts after it.
#[test]
fn a_credential_that_never_resolves_fails_as_connect_within_the_connect_timeout() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (done, outcome) = mpsc::channel();
    std::thread::spawn(move || {
        let mut recogniser = ElevenLabsRecogniser::new(
            Endpoint::new()
                .with_url(url)
                .with_connect_timeout(Duration::from_millis(500))
                .with_read_timeout(Duration::from_millis(300)),
            manual(InputFormat::Ulaw8000),
            Arc::new(NeverResolves),
            "elevenlabs-api-key",
        )
        .unwrap();
        let started = Instant::now();
        let (handle, events) = recognise(&mut recogniser, mulaw(3));
        let _ = done.send((
            started.elapsed(),
            handle.failure().cloned(),
            usage_is_final(&events),
        ));
        drop(handle);
    });
    let (elapsed, failure, is_final) = outcome
        .recv_timeout(Duration::from_secs(5))
        .expect("a credential that never resolves held the recognition for 5 s, ten times the 500 ms connect timeout");
    drop(listener);
    assert!(
        matches!(failure, Some(ElevenLabsError::Connect(_))),
        "{failure:?}"
    );
    assert_eq!(is_final, Some(false));
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
}
