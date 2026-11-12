//! Conformance checks of the opening of a recognition after the correction round: "the opening,
//! the credential included, is bounded by the connect timeout", and a handshake still pending
//! when input ends waits for its outcome instead of the read timeout.
#![cfg(feature = "stt")]

mod stt_support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_elevenlabs::{
    Credential, CredentialFuture, CredentialSource, ElevenLabsError, ElevenLabsRecogniser,
    Endpoint, InputFormat,
};
use inference_stt::{Recogniser, RecognitionHandle, TranscriptEvent};
use stt_support::{FixtureServer, Script, describe, manual, mulaw, request, session_started};
use tokio_tungstenite::tungstenite::Message;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const READ_TIMEOUT: Duration = Duration::from_millis(300);
const SLACK: Duration = Duration::from_secs(2);

/// A source whose future never resolves.
struct NeverResolves;

impl CredentialSource for NeverResolves {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::pending())
    }
}

/// A source whose future does its lookup on the polling thread, as a source over a synchronous
/// secret store client does, taking longer than the connect timeout.
struct BlocksItsThread(Duration);

impl CredentialSource for BlocksItsThread {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        let pause = self.0;
        Box::pin(async move {
            std::thread::sleep(pause);
            Ok(Credential::new(stt_support::KEY.as_bytes().to_vec()))
        })
    }
}

/// Offers three frames, ends input and iterates to the end; the time from the first frame (when
/// the connection starts) to the end, the failure and the events.
fn run(recogniser: &mut ElevenLabsRecogniser) -> (Duration, Option<ElevenLabsError>, Vec<String>) {
    let mut handle = recogniser.start(request()).unwrap();
    let started = Instant::now();
    let mut events = Vec::new();
    for frame in mulaw(3) {
        handle.offer_frame(frame).unwrap();
        events.extend(handle.by_ref());
    }
    handle.end_input().unwrap();
    events.extend(handle.by_ref().take(1_000));
    (
        started.elapsed(),
        handle.failure().cloned(),
        events.iter().map(describe).collect(),
    )
}

fn recogniser(url: &str, source: Arc<dyn CredentialSource>) -> ElevenLabsRecogniser {
    ElevenLabsRecogniser::new(
        Endpoint::new()
            .with_url(url)
            .with_connect_timeout(CONNECT_TIMEOUT)
            .with_read_timeout(READ_TIMEOUT),
        manual(InputFormat::Ulaw8000),
        source,
        "elevenlabs-api-key",
    )
    .unwrap()
}

#[test]
fn a_credential_that_never_resolves_fails_as_connect_within_the_connect_timeout() {
    let server = FixtureServer::start(Script::Steps(Vec::new()));
    let mut recogniser = recogniser(&server.url, Arc::new(NeverResolves));
    let (took, failure, events) = run(&mut recogniser);
    assert!(
        matches!(failure, Some(ElevenLabsError::Connect(_))),
        "{failure:?}"
    );
    assert!(took <= CONNECT_TIMEOUT + SLACK, "{took:?}");
    assert_eq!(events, ["usage"]);
    assert!(server.seen().is_empty(), "no connection without a key");
}

#[test]
fn a_credential_lookup_that_blocks_its_thread_is_still_bounded_by_the_connect_timeout() {
    let server = FixtureServer::start(Script::Steps(Vec::new()));
    let mut recogniser = recogniser(
        &server.url,
        Arc::new(BlocksItsThread(Duration::from_secs(6))),
    );
    let (took, failure, _) = run(&mut recogniser);
    assert!(
        took <= CONNECT_TIMEOUT + SLACK,
        "the opening, the credential included, is documented as bounded by the connect timeout \
         ({CONNECT_TIMEOUT:?}), yet the recognition took {took:?} to end, failure {failure:?}"
    );
    assert!(
        matches!(failure, Some(ElevenLabsError::Connect(_))),
        "{failure:?}"
    );
}

/// A server that answers the upgrade only after `delay`, then sends `session_started` and
/// answers the first commit with a timed committed transcript.
fn slow_upgrade_server(delay: Duration) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let Ok(Ok((stream, _))) =
                tokio::time::timeout(Duration::from_secs(10), listener.accept()).await
            else {
                return;
            };
            tokio::time::sleep(delay).await;
            let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
                return;
            };
            let _ = socket.send(Message::text(session_started())).await;
            while let Ok(Some(Ok(message))) =
                tokio::time::timeout(Duration::from_secs(10), socket.next()).await
            {
                let Message::Text(text) = message else {
                    continue;
                };
                let value: serde_json::Value = serde_json::from_str(text.as_str()).unwrap();
                if value["commit"] == true {
                    let answer = serde_json::json!({
                        "message_type": "committed_transcript_with_timestamps",
                        "text": "eins",
                        "words": [{"text": "eins", "start": 0.0, "end": 0.04, "type": "word"}]
                    });
                    let _ = socket.send(Message::text(answer.to_string())).await;
                }
            }
        });
    });
    (url, thread)
}

#[test]
fn a_handshake_answered_after_the_read_timeout_past_the_end_of_input_still_delivers() {
    let (url, server) = slow_upgrade_server(Duration::from_millis(900));
    let mut recogniser = ElevenLabsRecogniser::new(
        Endpoint::new()
            .with_url(&url)
            .with_connect_timeout(Duration::from_secs(5))
            .with_read_timeout(READ_TIMEOUT),
        manual(InputFormat::Ulaw8000),
        Arc::new(stt_support::FixedKey(stt_support::KEY)),
        "elevenlabs-api-key",
    )
    .unwrap();
    let mut handle = recogniser.start(request()).unwrap();
    let mut events = Vec::new();
    for frame in mulaw(3) {
        handle.offer_frame(frame).unwrap();
        events.extend(handle.by_ref());
    }
    handle.end_input().unwrap();
    events.extend(handle.by_ref().take(1_000));
    let failure = handle.failure().cloned();
    let final_record = matches!(
        events.last().map(|event| &event.payload),
        Some(TranscriptEvent::Usage(record)) if record.r#final
    );
    let described: Vec<String> = events.iter().map(describe).collect();
    drop(handle);
    server.join().unwrap();
    assert_eq!(failure, None, "{described:?}");
    assert_eq!(described, ["hypothesis eins", "commit eins", "usage"]);
    assert!(final_record);
}
