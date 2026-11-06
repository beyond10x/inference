//! Adversary pass 1 on the Cartesia binding: a scripted loopback server that answers each client
//! message with exactly the messages a case names, to drive the binding against the documented
//! protocol where the shared fixture does not reach: a message type the API adds within a pinned
//! version, a cancelled context's late messages interleaved with the next context's, samples
//! split across chunks of one and two bytes, an idle gap past the read timeout between segments,
//! segments offered while another speaks, a server that closes between segments, and the key in
//! `Debug` and errors. Every case runs under the shared deadline.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_cartesia::{CartesiaSynthesiser, Endpoint, OutputFormat, VoiceConfig};
use inference_tts::inference_audio::AudioEncoding;
use inference_tts::inference_text::{SegmentId, SegmentState};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

use support::{FixedKey, committed, within};

const KEY: &str = "adversary-fixture-key-7d1c0e5a";

/// What the scripted server sends in answer to one client message.
enum Out {
    Text(String),
    Close(u16),
}

/// What a script is told of one client message, and what it returns to send.
type ScriptFn = dyn FnMut(&Value, &mut Vec<String>) -> Vec<Out> + Send;

struct Scripted {
    url: String,
    received: Arc<Mutex<Vec<Value>>>,
    _thread: std::thread::JoinHandle<()>,
}

impl Scripted {
    /// A server whose every connection hands each client message to `script`, together with the
    /// contexts the connection's generation requests opened so far, and sends what it returns.
    fn start(script: Box<ScriptFn>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let received = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&received);
        let script = Arc::new(Mutex::new(script));
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                while let Ok((stream, _)) = listener.accept().await {
                    let seen = Arc::clone(&seen);
                    let script = Arc::clone(&script);
                    tokio::spawn(async move {
                        let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
                            return;
                        };
                        let mut contexts = Vec::new();
                        while let Some(Ok(message)) = socket.next().await {
                            let Message::Text(text) = message else {
                                continue;
                            };
                            let value: Value =
                                serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                            seen.lock().unwrap().push(value.clone());
                            if value.get("transcript").is_some()
                                && let Some(context) = value["context_id"].as_str()
                            {
                                contexts.push(context.to_owned());
                            }
                            let out = (*script.lock().unwrap())(&value, &mut contexts);
                            for item in out {
                                match item {
                                    Out::Text(line) => {
                                        if socket.send(Message::text(line)).await.is_err() {
                                            return;
                                        }
                                    }
                                    Out::Close(code) => {
                                        let _ = socket
                                            .close(Some(CloseFrame {
                                                code: code.into(),
                                                reason: "scripted close".into(),
                                            }))
                                            .await;
                                        return;
                                    }
                                }
                            }
                        }
                    });
                }
            });
        });
        Self {
            url,
            received,
            _thread: thread,
        }
    }

    fn synthesiser(&self, format: OutputFormat, read_timeout: Duration) -> CartesiaSynthesiser {
        CartesiaSynthesiser::new(
            Endpoint::new()
                .with_url(&self.url)
                .with_read_timeout(read_timeout)
                .with_connect_timeout(Duration::from_secs(5)),
            VoiceConfig::new("adversary-voice", format),
            Arc::new(FixedKey(KEY)),
            "cartesia-api-key",
        )
        .unwrap()
    }

    fn requests(&self) -> Vec<Value> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .filter(|message| message.get("transcript").is_some())
            .cloned()
            .collect()
    }

    fn cancels(&self) -> Vec<Value> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .filter(|message| message.get("cancel").is_some())
            .cloned()
            .collect()
    }
}

fn chunk(context: &str, data: &[u8], done: bool) -> Out {
    Out::Text(
        json!({
            "type": "chunk",
            "data": STANDARD.encode(data),
            "done": done,
            "status_code": 206,
            "step_time": 3.0,
            "context_id": context,
        })
        .to_string(),
    )
}

fn done(context: &str) -> Out {
    Out::Text(
        json!({"type": "done", "done": true, "status_code": 206, "context_id": context})
            .to_string(),
    )
}

fn is_request(message: &Value) -> bool {
    message.get("transcript").is_some()
}

fn frames_of(events: &[SynthesisEvent], segment: SegmentId) -> Vec<Vec<u8>> {
    events
        .iter()
        .filter_map(|event| match event {
            SynthesisEvent::Audio(audio) if audio.segment_id() == segment => {
                Some(audio.frame().data.clone())
            }
            _ => None,
        })
        .collect()
}

fn state(handle: &impl SynthesisHandle, segment: SegmentId) -> Option<SegmentState> {
    handle
        .output()
        .segment(segment)
        .map(|segment| segment.state())
}

/// The conventions page lets the API add variants to enum-like output values within a pinned
/// `Cartesia-Version`, and `type` is one: a message of a type this binding does not know, sent
/// between the chunks of a segment, is to be passed over, not a reason to fail the synthesis.
#[test]
fn a_message_type_added_within_the_pinned_version_does_not_fail_the_segment() {
    within(|| {
        let server = Scripted::start(Box::new(|message: &Value, contexts: &mut Vec<String>| {
            if !is_request(message) {
                return Vec::new();
            }
            let context = contexts.last().unwrap().clone();
            vec![
                chunk(&context, &[0x7f; 80], false),
                Out::Text(
                    json!({
                        "type": "segment_metadata",
                        "done": false,
                        "status_code": 206,
                        "context_id": context,
                    })
                    .to_string(),
                ),
                chunk(&context, &[0x7e; 80], false),
                done(&context),
            ]
        }));
        let mut synthesiser = server.synthesiser(OutputFormat::MULAW_8000, Duration::from_secs(5));
        let mut handle = synthesiser.start().unwrap();
        let segment = committed(&["A new message type arrives mid-segment."]).remove(0);
        let id = segment.segment_id();
        handle.offer_segment(segment).unwrap();
        handle.end_utterance().unwrap();
        let events: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(
            handle.failure().is_none(),
            "an output type added within the pinned version failed the synthesis: {:?}",
            handle.failure()
        );
        assert_eq!(frames_of(&events, id).concat().len(), 160);
        assert_eq!(state(&handle, id), Some(SegmentState::Synthesized));
    });
}

/// A cancelled context goes on to its end, as the contexts guide says, and its late chunk, its
/// final chunk, its `done` and an `error` naming it arrive between the next context's chunks:
/// none of them reaches the caller or fails the synthesis, and the next segment is whole.
#[test]
fn a_cancelled_contexts_late_messages_interleaved_with_the_next_context_never_reach_the_caller() {
    within(|| {
        let server = Scripted::start(Box::new(|message: &Value, contexts: &mut Vec<String>| {
            if !is_request(message) {
                return Vec::new();
            }
            match contexts.as_slice() {
                [first] => vec![chunk(first, &[1; 40], false)],
                [first, second, ..] => vec![
                    chunk(second, &[2; 40], false),
                    chunk(first, &[1; 40], false),
                    chunk(second, &[2; 40], false),
                    chunk(first, &[1; 40], true),
                    done(first),
                    Out::Text(
                        json!({
                            "type": "error",
                            "done": true,
                            "status_code": 400,
                            "error_code": null,
                            "title": "Context cancelled",
                            "message": "late",
                            "request_id": "r",
                            "context_id": first,
                        })
                        .to_string(),
                    ),
                    chunk(second, &[2; 40], true),
                    done(second),
                ],
                [] => Vec::new(),
            }
        }));
        let mut synthesiser = server.synthesiser(OutputFormat::MULAW_8000, Duration::from_secs(5));
        let mut handle = synthesiser.start().unwrap();
        let mut segments = committed(&["Interrupt me.", "The answer after."]).into_iter();
        let cancelled = segments.next().unwrap();
        let cancelled_id = cancelled.segment_id();
        handle.offer_segment(cancelled).unwrap();
        assert!(matches!(handle.next(), Some(SynthesisEvent::Audio(_))));
        handle.discard(cancelled_id).unwrap();
        let next = segments.next().unwrap();
        let next_id = next.segment_id();
        handle.offer_segment(next).unwrap();
        handle.end_utterance().unwrap();
        let events: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(handle.failure().is_none(), "{:?}", handle.failure());
        assert!(frames_of(&events, cancelled_id).is_empty());
        assert_eq!(frames_of(&events, next_id).concat(), vec![2; 120]);
        assert_eq!(state(&handle, next_id), Some(SegmentState::Synthesized));
        assert_eq!(server.cancels().len(), 1);
        assert_eq!(server.requests().len(), 2);
    });
}

/// 16-bit and 32-bit samples cut into chunks of one, two and zero bytes come back byte for byte
/// as whole samples, each frame's sample count its whole samples.
#[test]
fn samples_split_into_one_and_two_byte_chunks_come_back_whole() {
    for (format, cuts) in [
        (
            OutputFormat::new(AudioEncoding::PcmS16le, 16_000).unwrap(),
            vec![1_usize, 1, 3, 0, 1],
        ),
        (
            OutputFormat::new(AudioEncoding::PcmF32le, 48_000).unwrap(),
            vec![1, 2, 0, 5, 3, 1],
        ),
    ] {
        let total: usize = cuts.iter().sum();
        let bytes: Vec<u8> = (0..total).map(|n| u8::try_from(n + 1).unwrap()).collect();
        let script_bytes = bytes.clone();
        let script_cuts = cuts.clone();
        let collected = within(move || {
            let server = Scripted::start(Box::new(
                move |message: &Value, contexts: &mut Vec<String>| {
                    if !is_request(message) {
                        return Vec::new();
                    }
                    let context = contexts.last().unwrap().clone();
                    let mut out = Vec::new();
                    let mut at = 0;
                    for cut in &script_cuts {
                        out.push(chunk(&context, &script_bytes[at..at + cut], false));
                        at += cut;
                    }
                    out.push(done(&context));
                    out
                },
            ));
            let mut synthesiser = server.synthesiser(format, Duration::from_secs(5));
            let mut handle = synthesiser.start().unwrap();
            handle
                .offer_segment(committed(&["Split samples."]).remove(0))
                .unwrap();
            handle.end_utterance().unwrap();
            let events: Vec<SynthesisEvent> = handle.by_ref().collect();
            assert!(
                handle.failure().is_none(),
                "{format}: {:?}",
                handle.failure()
            );
            let width = if format.encoding() == AudioEncoding::PcmF32le {
                4
            } else {
                2
            };
            let mut data = Vec::new();
            for event in &events {
                if let SynthesisEvent::Audio(audio) = event {
                    let frame = audio.frame();
                    assert_eq!(
                        frame.data.len(),
                        usize::try_from(frame.sample_count).unwrap() * width
                    );
                    data.extend_from_slice(&frame.data);
                }
            }
            data
        });
        assert_eq!(collected, bytes, "{format}");
    }
}

/// Segments offered while another is being spoken are each sent once the one before is done and
/// each spoken whole, in order, none lost; a gap longer than the read timeout between segments,
/// with nothing offered, is not a stall.
#[test]
fn segments_offered_while_one_speaks_and_after_an_idle_gap_are_all_spoken_in_order() {
    within(|| {
        let server = Scripted::start(Box::new(|message: &Value, contexts: &mut Vec<String>| {
            if !is_request(message) {
                return Vec::new();
            }
            let context = contexts.last().unwrap().clone();
            let fill = u8::try_from(contexts.len()).unwrap();
            vec![
                chunk(&context, &[fill; 20], false),
                chunk(&context, &[fill; 20], false),
                done(&context),
            ]
        }));
        let read_timeout = Duration::from_millis(300);
        let mut synthesiser = server.synthesiser(OutputFormat::ALAW_8000, read_timeout);
        let mut handle = synthesiser.start().unwrap();
        let segments = committed(&["One.", "Two.", "Three.", "Four, after a pause."]);
        let ids: Vec<SegmentId> = segments.iter().map(|s| s.segment_id()).collect();
        let mut segments = segments.into_iter();
        handle.offer_segment(segments.next().unwrap()).unwrap();
        let mut events = vec![handle.next().expect("a first frame")];
        handle.offer_segment(segments.next().unwrap()).unwrap();
        handle.offer_segment(segments.next().unwrap()).unwrap();
        events.extend(handle.by_ref());
        assert!(handle.failure().is_none(), "{:?}", handle.failure());
        std::thread::sleep(read_timeout * 3);
        handle.offer_segment(segments.next().unwrap()).unwrap();
        handle.end_utterance().unwrap();
        events.extend(handle.by_ref());
        assert!(
            handle.failure().is_none(),
            "an idle gap between segments failed the synthesis: {:?}",
            handle.failure()
        );
        for (index, id) in ids.iter().enumerate() {
            let fill = u8::try_from(index + 1).unwrap();
            assert_eq!(
                frames_of(&events, *id).concat(),
                vec![fill; 40],
                "segment {index}"
            );
            assert_eq!(state(&handle, *id), Some(SegmentState::Synthesized));
        }
        let order: Vec<SegmentId> = events
            .iter()
            .filter_map(|event| match event {
                SynthesisEvent::Audio(audio) => Some(audio.segment_id()),
                _ => None,
            })
            .collect();
        let mut sorted = order.clone();
        sorted.sort_by_key(|id| ids.iter().position(|known| known == id));
        assert_eq!(order, sorted, "frames of segments interleaved");
        assert_eq!(server.requests().len(), 4);
    });
}

/// A server that closes the connection between segments fails the next segment, as the
/// protocol summary says, promptly and without a panic; the segment spoken before stays whole.
#[test]
fn a_server_closing_between_segments_fails_the_next_segment_promptly() {
    within(|| {
        let server = Scripted::start(Box::new(|message: &Value, contexts: &mut Vec<String>| {
            if !is_request(message) {
                return Vec::new();
            }
            let context = contexts.last().unwrap().clone();
            vec![
                chunk(&context, &[9; 16], false),
                done(&context),
                Out::Close(1000),
            ]
        }));
        let mut synthesiser = server.synthesiser(OutputFormat::MULAW_8000, Duration::from_secs(5));
        let mut handle = synthesiser.start().unwrap();
        let segments = committed(&["Before the close.", "After the close."]);
        let first = segments[0].segment_id();
        let second = segments[1].segment_id();
        let mut segments = segments.into_iter();
        handle.offer_segment(segments.next().unwrap()).unwrap();
        let _: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert_eq!(state(&handle, first), Some(SegmentState::Synthesized));
        std::thread::sleep(Duration::from_millis(200));
        handle.offer_segment(segments.next().unwrap()).unwrap();
        handle.end_utterance().unwrap();
        let started = std::time::Instant::now();
        let after: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert!(frames_of(&after, second).is_empty());
        assert!(handle.failure().is_some(), "the close went unnoticed");
        assert_ne!(state(&handle, second), Some(SegmentState::Synthesized));
    });
}

/// The key appears in no `Debug` of the synthesiser or the handle, and in no error of a refused
/// handshake or a server error.
#[test]
fn the_key_is_in_no_debug_and_no_error() {
    within(|| {
        let server = Scripted::start(Box::new(|message: &Value, contexts: &mut Vec<String>| {
            if !is_request(message) {
                return Vec::new();
            }
            vec![Out::Text(
                json!({
                    "type": "error",
                    "done": true,
                    "status_code": 401,
                    "error_code": null,
                    "title": "Unauthorized",
                    "message": "bad key",
                    "request_id": "r",
                    "context_id": contexts.last().unwrap(),
                })
                .to_string(),
            )]
        }));
        let mut synthesiser = server.synthesiser(OutputFormat::MULAW_8000, Duration::from_secs(5));
        assert!(!format!("{synthesiser:?}").contains(KEY));
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Fail me."]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let _: Vec<SynthesisEvent> = handle.by_ref().collect();
        let failure = handle.failure().cloned().expect("a server error");
        assert!(!format!("{failure:?} {failure}").contains(KEY));
        assert!(!format!("{handle:?}").contains(KEY));
    });
}
