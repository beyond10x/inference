//! Adversary pass on story:elevenlabs-tts-provider-cancel: a context the endpoint answers with
//! no audio, whatever the cause. The hosted endpoint ends a flushed context promptly only when
//! it is closed (a flush alone brought its final message only at the 20 s inactivity timeout).
//! A binding that closed a context only once a message of it had arrived would wait for the
//! read timeout on such a context, fail the synthesis and lose every later segment of the
//! utterance. The hosted endpoint did return audio for a text of punctuation only (`…`, one
//! audio message of 2973 B in the probe); this file's server answers it with none to stand for
//! any context that yields no audio.
//!
//! The server of this file follows that model: a flushed context with text other than the
//! ellipsis gets one audio message at once; every context gets its final message only on its
//! `close_context`.

mod support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_elevenlabs::{ElevenLabsSynthesiser, Endpoint, OutputFormat, VoiceConfig};
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use serde_json::{Value, json};
use support::{FixedKey, KEY, VOICE, committed, within};
use tokio_tungstenite::tungstenite::Message;

const FORMAT: OutputFormat = OutputFormat::Ulaw8000;
const SILENT_TEXT: &str = "\u{2026}";
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// A loopback server answering as described above; `closed` records each `close_context`.
fn start(closed: Arc<Mutex<Vec<String>>>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    listener.set_nonblocking(true).expect("non-blocking");
    let url = format!("ws://{}", listener.local_addr().expect("an address"));
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("a listener");
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, Arc::clone(&closed)));
            }
        });
    });
    url
}

async fn serve(stream: tokio::net::TcpStream, closed: Arc<Mutex<Vec<String>>>) {
    let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    while let Some(Ok(message)) = socket.next().await {
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };
        let Ok(Value::Object(object)) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        if object.contains_key("close_socket") {
            break;
        }
        let Some(Value::String(context_id)) = object.get("context_id") else {
            continue;
        };
        if object.get("close_context") == Some(&Value::Bool(true)) {
            closed.lock().unwrap().push(context_id.clone());
            let last = json!({"isFinal": true, "contextId": context_id}).to_string();
            if socket.send(Message::text(last)).await.is_err() {
                return;
            }
            continue;
        }
        if object.get("flush") != Some(&Value::Bool(true)) {
            continue;
        }
        let said = object
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if said.trim() == SILENT_TEXT {
            continue;
        }
        let audio = json!({
            "audio": STANDARD.encode([0xFF_u8; 400]),
            "alignment": null,
            "normalizedAlignment": null,
            "contextId": context_id,
        })
        .to_string();
        if socket.send(Message::text(audio)).await.is_err() {
            return;
        }
    }
    let _ = socket.close(None).await;
}

/// A segment the endpoint answers with no audio completes, and the next segment is spoken, well
/// within the read timeout.
#[test]
fn a_segment_the_endpoint_answers_with_no_audio_completes_and_the_next_one_speaks() {
    within(Duration::from_secs(20), || {
        let closed = Arc::new(Mutex::new(Vec::new()));
        let url = start(Arc::clone(&closed));
        let mut synthesiser = ElevenLabsSynthesiser::new(
            Endpoint::new()
                .with_url(&url)
                .with_read_timeout(READ_TIMEOUT)
                .with_connect_timeout(Duration::from_secs(5)),
            VoiceConfig::new(VOICE, FORMAT),
            Arc::new(FixedKey(KEY)),
            "elevenlabs-api-key",
        )
        .expect("a valid configuration");
        let mut handle = synthesiser.start().unwrap();
        let segments = committed(&[SILENT_TEXT, "After the pause."]);
        let ids: Vec<_> = segments
            .iter()
            .map(|segment| segment.segment_id())
            .collect();
        for segment in segments {
            handle.offer_segment(segment).unwrap();
        }
        handle.end_utterance().unwrap();
        let started = Instant::now();
        let events: Vec<SynthesisEvent> = handle.by_ref().collect();
        let took = started.elapsed();
        assert!(
            handle.failure().is_none(),
            "after {took:?}: {:?}; closes the server saw: {:?}",
            handle.failure(),
            closed.lock().unwrap()
        );
        assert_eq!(
            handle
                .output()
                .segment(ids[0])
                .map(|segment| segment.state()),
            Some(SegmentState::Synthesized),
            "the segment with no audio completes"
        );
        assert_eq!(
            handle
                .output()
                .segment(ids[1])
                .map(|segment| segment.state()),
            Some(SegmentState::Synthesized),
            "the segment after it is spoken"
        );
        assert!(
            took < READ_TIMEOUT / 2,
            "both segments took {took:?}, the read timeout is {READ_TIMEOUT:?}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SynthesisEvent::Audio(_)))
                .count(),
            1
        );
    });
}
