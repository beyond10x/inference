//! Adversary pass 1 on story:elevenlabs-multi-context-tts-connection. Each case attacks the
//! multi-stream binding against a loopback server of this file that answers each context the
//! client closes with a script, and may send messages of any other context at any time. Every
//! case runs under a wall-clock deadline under 30 s.

mod support;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_elevenlabs::{
    ElevenLabsError, ElevenLabsSynthesiser, Endpoint, OutputFormat, VoiceConfig,
};
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixedKey, KEY, VOICE, committed};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

const FORMAT: OutputFormat = OutputFormat::Ulaw8000;

/// One step the server takes for a context the client closed.
#[derive(Clone)]
enum Act {
    /// This text, naming the context in `contextId` when it is an object that names none.
    Send(String),
    /// This text exactly as it is.
    Raw(String),
    /// This text every 5 ms, naming the context, until the send fails.
    Stream(String),
    /// A close frame with this code, then the connection is dropped, as an idle timeout would.
    Close(u16),
}

/// What the server saw of one connection.
#[derive(Debug, Clone, Default)]
struct Seen {
    messages: Vec<String>,
    close_socket: bool,
}

struct Server {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    ends: Mutex<mpsc::Receiver<usize>>,
}

type Scripts = Arc<(Vec<Vec<Act>>, Mutex<usize>)>;
type Sink = Arc<
    tokio::sync::Mutex<
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
            Message,
        >,
    >,
>;

impl Server {
    fn start(scripts: Vec<Vec<Act>>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener.set_nonblocking(true).expect("non-blocking");
        let url = format!("ws://{}", listener.local_addr().expect("an address"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let thread_seen = Arc::clone(&seen);
        let (end_tx, end_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("a listener");
                let scripts: Scripts = Arc::new((scripts, Mutex::new(0)));
                let mut index = 0;
                while let Ok((stream, _)) = listener.accept().await {
                    thread_seen.lock().unwrap().push(Seen::default());
                    tokio::spawn(serve(
                        stream,
                        index,
                        Arc::clone(&scripts),
                        Arc::clone(&thread_seen),
                        end_tx.clone(),
                    ));
                    index += 1;
                }
            });
        });
        Self {
            url,
            seen,
            ends: Mutex::new(end_rx),
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn next_end(&self, bound: Duration) -> Option<usize> {
        self.ends.lock().unwrap().recv_timeout(bound).ok()
    }

    fn synthesiser(&self, read_timeout: Duration) -> ElevenLabsSynthesiser {
        ElevenLabsSynthesiser::new(
            Endpoint::new()
                .with_url(&self.url)
                .with_read_timeout(read_timeout)
                .with_connect_timeout(Duration::from_secs(5)),
            VoiceConfig::new(VOICE, FORMAT),
            Arc::new(FixedKey(KEY)),
            "elevenlabs-api-key",
        )
        .expect("a valid configuration")
    }
}

async fn serve(
    stream: tokio::net::TcpStream,
    index: usize,
    scripts: Scripts,
    seen: Arc<Mutex<Vec<Seen>>>,
    ends: mpsc::Sender<usize>,
) {
    let Ok(socket) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (sink, mut stream) = socket.split();
    let sink: Sink = Arc::new(tokio::sync::Mutex::new(sink));
    let hang_up = Arc::new(tokio::sync::Notify::new());
    let mut acting = Vec::new();
    loop {
        let message = tokio::select! {
            () = hang_up.notified() => break,
            message = stream.next() => message,
        };
        let Some(Ok(message)) = message else { break };
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };
        let text = text.as_str().to_owned();
        seen.lock().unwrap()[index].messages.push(text.clone());
        let Ok(serde_json::Value::Object(object)) =
            serde_json::from_str::<serde_json::Value>(&text)
        else {
            continue;
        };
        if object.contains_key("close_socket") {
            seen.lock().unwrap()[index].close_socket = true;
            continue;
        }
        if object.get("close_context") != Some(&serde_json::Value::Bool(true)) {
            continue;
        }
        let Some(serde_json::Value::String(context_id)) = object.get("context_id") else {
            continue;
        };
        let acts = {
            let mut answered = scripts.1.lock().unwrap();
            let acts = scripts.0[(*answered).min(scripts.0.len() - 1)].clone();
            *answered += 1;
            acts
        };
        acting.push(tokio::spawn(act(
            acts,
            context_id.clone(),
            Arc::clone(&sink),
            Arc::clone(&hang_up),
        )));
    }
    for task in acting {
        task.abort();
    }
    let _ = sink.lock().await.close().await;
    let _ = ends.send(index);
}

async fn act(acts: Vec<Act>, context_id: String, sink: Sink, hang_up: Arc<tokio::sync::Notify>) {
    let named = |text: &str| match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(mut object)) => {
            object
                .entry("contextId")
                .or_insert_with(|| serde_json::Value::String(context_id.clone()));
            serde_json::Value::Object(object).to_string()
        }
        _ => text.to_owned(),
    };
    for act in acts {
        let sent = match act {
            Act::Send(text) => sink.lock().await.send(Message::text(named(&text))).await,
            Act::Raw(text) => sink.lock().await.send(Message::text(text)).await,
            Act::Stream(text) => loop {
                if sink
                    .lock()
                    .await
                    .send(Message::text(named(&text)))
                    .await
                    .is_err()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            },
            Act::Close(code) => {
                let _ = sink
                    .lock()
                    .await
                    .send(Message::Close(Some(CloseFrame {
                        code: code.into(),
                        reason: "idle".into(),
                    })))
                    .await;
                hang_up.notify_one();
                return;
            }
        };
        if sent.is_err() {
            return;
        }
    }
}

fn audio(byte: u8) -> Act {
    Act::Send(format!(r#"{{"audio":"{}"}}"#, STANDARD.encode([byte; 80])))
}

fn audio_of(context_id: &str, byte: u8) -> Act {
    Act::Raw(format!(
        r#"{{"audio":"{}","contextId":"{context_id}"}}"#,
        STANDARD.encode([byte; 80])
    ))
}

fn final_message() -> Act {
    Act::Send(r#"{"isFinal":true}"#.to_owned())
}

fn within<T: Send + 'static>(bound: Duration, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(bound)
        .unwrap_or_else(|_| panic!("the case did not finish within {bound:?}"))
}

/// Every frame as (segment, position, first byte, sample count).
fn frames(
    events: &[SynthesisEvent],
) -> Vec<(inference_tts::inference_text::SegmentId, u64, u8, u32)> {
    events
        .iter()
        .filter_map(|event| match event {
            SynthesisEvent::Audio(audio) => Some((
                audio.segment_id(),
                audio.frame().position.get(),
                audio.frame().data[0],
                audio.frame().sample_count,
            )),
            _ => None,
        })
        .collect()
}

/// The cancelled context's late audio and its final message, interleaved with the next
/// context's audio, never reach the next segment's frames, and the clock goes on from the frame
/// emitted before the cancel.
#[test]
fn adversary_rf1_late_audio_of_a_cancelled_context_interleaved_with_the_next_never_leaks() {
    let server = Server::start(vec![
        vec![audio(0x11)],
        vec![
            audio_of("s0", 0x22),
            audio(0x33),
            audio_of("s0", 0x22),
            Act::Raw(r#"{"isFinal":true,"contextId":"s0"}"#.to_owned()),
            audio(0x33),
            final_message(),
        ],
    ]);
    let mut synthesiser = server.synthesiser(Duration::from_secs(5));
    let (failure, after, next_id, state) = within(Duration::from_secs(20), move || {
        let mut handle = synthesiser.start().unwrap();
        let mut segments = committed(&["Cut me.", "Then me."]).into_iter();
        let cut = segments.next().unwrap();
        let cut_id = cut.segment_id();
        handle.offer_segment(cut).unwrap();
        assert!(matches!(handle.next(), Some(SynthesisEvent::Audio(_))));
        handle.discard(cut_id).unwrap();
        let next = segments.next().unwrap();
        let next_id = next.segment_id();
        handle.offer_segment(next).unwrap();
        handle.end_utterance().unwrap();
        let after: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        let state = handle.output().segment(next_id).map(|s| s.state());
        (handle.failure().cloned(), after, next_id, state)
    });
    assert_eq!(failure, None);
    assert_eq!(
        frames(&after),
        vec![(next_id, 80, 0x33, 80), (next_id, 160, 0x33, 80)],
        "only the next context's audio, on the clock after the cancelled frame"
    );
    assert_eq!(state, Some(SegmentState::Synthesized));
    assert!(matches!(after.last(), Some(SynthesisEvent::Usage(record)) if record.r#final));
    assert_eq!(server.seen().len(), 1);
}

/// The vendor's multi-context guide reads the final message as `is_final`, in its Python and
/// its JavaScript example alike; the AsyncAPI page says `isFinal`. A final message spelled as
/// the guide reads it ends the segment instead of failing the synthesis.
#[test]
fn adversary_rf1_a_final_message_spelled_as_the_guide_reads_it_ends_the_segment() {
    let server = Server::start(vec![vec![
        audio(0x44),
        Act::Send(r#"{"is_final":true}"#.to_owned()),
    ]]);
    let mut synthesiser = server.synthesiser(Duration::from_secs(5));
    let (failure, events, state) = within(Duration::from_secs(20), move || {
        let mut handle = synthesiser.start().unwrap();
        let segment = committed(&["Guide spelling."]).remove(0);
        let id = segment.segment_id();
        handle.offer_segment(segment).unwrap();
        handle.end_utterance().unwrap();
        let events: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        let state = handle.output().segment(id).map(|s| s.state());
        (handle.failure().cloned(), events.len(), state)
    });
    assert_eq!(
        (failure, state),
        (None, Some(SegmentState::Synthesized)),
        "a final message spelled `is_final` failed the synthesis ({events} events)"
    );
}

/// A synthesis outlives a turn: the cascade's speech node keeps one synthesis until the end of
/// input. The single-stream binding opened a connection per segment, so a gap between segments
/// cost nothing; on one shared connection a server that closed it in the gap, as an inactivity
/// timeout does, must not fail the next segment, which can be spoken on a new connection.
#[test]
fn adversary_rf1_a_connection_closed_between_segments_does_not_fail_the_next_segment() {
    let server = Server::start(vec![
        vec![audio(0x55), final_message(), Act::Close(1000)],
        vec![audio(0x66), final_message()],
    ]);
    let mut synthesiser = server.synthesiser(Duration::from_secs(5));
    let (failure, next_frames, state) = within(Duration::from_secs(20), move || {
        let mut handle = synthesiser.start().unwrap();
        let mut segments = committed(&["First turn.", "After a pause."]).into_iter();
        handle.offer_segment(segments.next().unwrap()).unwrap();
        let first: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        assert!(
            first
                .iter()
                .any(|event| matches!(event, SynthesisEvent::Completed(_))),
            "the first segment completes: {:?}",
            handle.failure()
        );
        std::thread::sleep(Duration::from_millis(300));
        let next = segments.next().unwrap();
        let next_id = next.segment_id();
        handle.offer_segment(next).unwrap();
        handle.end_utterance().unwrap();
        let after: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        let state = handle.output().segment(next_id).map(|s| s.state());
        (handle.failure().cloned(), frames(&after).len(), state)
    });
    assert_eq!(
        (failure, next_frames, state),
        (None, 1, Some(SegmentState::Synthesized)),
        "the segment after the server's idle close was not spoken"
    );
}

/// A handle dropped between segments, nothing being spoken and the utterance not ended, still
/// sends `close_socket` before the close frame.
#[test]
fn adversary_rf1_a_handle_dropped_between_segments_sends_close_socket() {
    let server = Server::start(vec![vec![audio(0x77), final_message()]]);
    let mut synthesiser = server.synthesiser(Duration::from_secs(5));
    within(Duration::from_secs(20), move || {
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Then dropped."]).remove(0))
            .unwrap();
        let events: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, SynthesisEvent::Completed(_)))
        );
        drop(handle);
    });
    assert_eq!(server.next_end(Duration::from_secs(5)), Some(0));
    assert!(server.seen()[0].close_socket, "{:?}", server.seen()[0]);
}

/// A handle dropped right after a barge-in, the cancelled context still streaming, sends
/// `close_socket` and ends the connection within the close bound.
#[test]
fn adversary_rf1_a_handle_dropped_after_a_cancel_while_the_context_streams_closes_the_socket() {
    let server = Server::start(vec![vec![
        audio(0x01),
        Act::Stream(format!(r#"{{"audio":"{}"}}"#, STANDARD.encode([0x02; 80]))),
    ]]);
    let mut synthesiser = server.synthesiser(Duration::from_secs(5));
    let took = within(Duration::from_secs(20), move || {
        let mut handle = synthesiser.start().unwrap();
        let segment = committed(&["Barge."]).remove(0);
        let id = segment.segment_id();
        handle.offer_segment(segment).unwrap();
        assert!(matches!(handle.next(), Some(SynthesisEvent::Audio(_))));
        std::thread::sleep(Duration::from_millis(100));
        handle.discard(id).unwrap();
        let started = Instant::now();
        drop(handle);
        started.elapsed()
    });
    assert!(took <= Duration::from_secs(2), "the drop took {took:?}");
    assert_eq!(server.next_end(Duration::from_secs(5)), Some(0));
    assert!(server.seen()[0].close_socket);
}

/// Messages of a cancelled context move no read deadline: a cancelled context streaming without
/// end while the next context says nothing fails the synthesis as stalled within the read
/// timeout. A binding that moved the deadline on them would wait as long as the stream lasts.
#[test]
fn adversary_rf1_a_streaming_cancelled_context_does_not_hold_a_silent_next_context() {
    let server = Server::start(vec![
        vec![
            audio(0x01),
            Act::Stream(format!(r#"{{"audio":"{}"}}"#, STANDARD.encode([0x02; 80]))),
        ],
        vec![],
    ]);
    let mut synthesiser = server.synthesiser(Duration::from_millis(500));
    let (failure, took) = within(Duration::from_secs(20), move || {
        let mut handle = synthesiser.start().unwrap();
        let mut segments = committed(&["Cut.", "Silent."]).into_iter();
        let cut = segments.next().unwrap();
        let cut_id = cut.segment_id();
        handle.offer_segment(cut).unwrap();
        assert!(matches!(handle.next(), Some(SynthesisEvent::Audio(_))));
        handle.discard(cut_id).unwrap();
        handle.offer_segment(segments.next().unwrap()).unwrap();
        handle.end_utterance().unwrap();
        let started = Instant::now();
        let _: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        (handle.failure().cloned(), started.elapsed())
    });
    assert!(
        matches!(failure, Some(ElevenLabsError::Stalled(_))),
        "{failure:?}"
    );
    assert!(took < Duration::from_secs(3), "the stall took {took:?}");
}

/// Six barge-ins in a row, no cancelled context ever finishing: the seventh segment is spoken
/// whole on the same connection, and nothing of the six follows their cancels.
#[test]
fn adversary_rf1_six_barge_ins_in_a_row_leave_the_seventh_segment_clean() {
    let mut scripts: Vec<Vec<Act>> = (0..6).map(|_| vec![audio(0x10)]).collect();
    scripts.push(vec![
        audio_of("s0", 0x20),
        audio_of("s5", 0x20),
        audio(0x30),
        final_message(),
    ]);
    let server = Server::start(scripts);
    let mut synthesiser = server.synthesiser(Duration::from_secs(5));
    let (failure, after, last_id) = within(Duration::from_secs(25), move || {
        let mut handle = synthesiser.start().unwrap();
        let segments = committed(&["a.", "b.", "c.", "d.", "e.", "f.", "g."]);
        let last_id = segments[6].segment_id();
        let mut segments = segments.into_iter();
        for _ in 0..6 {
            let segment = segments.next().unwrap();
            let id = segment.segment_id();
            handle.offer_segment(segment).unwrap();
            assert!(
                matches!(handle.next(), Some(SynthesisEvent::Audio(_))),
                "{:?}",
                handle.failure()
            );
            handle.discard(id).unwrap();
        }
        handle.offer_segment(segments.next().unwrap()).unwrap();
        handle.end_utterance().unwrap();
        let after: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        (handle.failure().cloned(), after, last_id)
    });
    assert_eq!(failure, None);
    assert_eq!(frames(&after), vec![(last_id, 480, 0x30, 80)]);
    assert_eq!(server.seen().len(), 1);
}
