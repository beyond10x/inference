//! Adversary pass on story:elevenlabs-streaming-tts-adapter. Each case attacks one decision the
//! binding took where the protocol notes leave it open, against the fixture server of
//! `tests/support/` or a scripted loopback server of this file. Every case that could hang fails
//! by its own deadline.

mod support;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_elevenlabs::protocol::{SERVER_MESSAGE_LIMIT, parse_server_message};
use inference_elevenlabs::{
    CLOSE_BOUND, ConfigError, ElevenLabsError, ElevenLabsSynthesiser, Endpoint, OutputFormat,
    VoiceConfig,
};
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixedKey, FixtureServer, KEY, Script, VOICE, committed};
use tokio_tungstenite::tungstenite::Message;

/// One step a scripted server takes once the client has closed a context
/// (story:elevenlabs-multi-context-tts-connection: the scripts follow contexts, not
/// connections). Every text sent names the context in `contextId` when it is a JSON object that
/// names none.
#[derive(Clone)]
enum Act {
    /// Sends this text message.
    Send(String),
    /// Sends this text message every 5 ms until the send fails.
    Stream(String),
    /// Sends a ping every 50 ms, until the send fails.
    PingForever,
}

/// A loopback server answering the n-th context it is asked for, across its connections, with
/// `scripts[n]`, the last for every one past them. It reports each connection's end, by the
/// client's close or a failed send.
struct Scripted {
    url: String,
    ends: Mutex<mpsc::Receiver<usize>>,
}

impl Scripted {
    fn start(scripts: Vec<Vec<Act>>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener.set_nonblocking(true).expect("non-blocking");
        let url = format!("ws://{}", listener.local_addr().expect("an address"));
        let (end_tx, end_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("a listener");
                let scripts = Arc::new((scripts, Mutex::new(0_usize)));
                let mut index = 0;
                while let Ok((stream, _)) = listener.accept().await {
                    tokio::spawn(serve(stream, index, Arc::clone(&scripts), end_tx.clone()));
                    index += 1;
                }
            });
        });
        Self {
            url,
            ends: Mutex::new(end_rx),
        }
    }

    fn next_end(&self, bound: Duration) -> Option<usize> {
        self.ends.lock().unwrap().recv_timeout(bound).ok()
    }

    fn synthesiser(&self, format: OutputFormat, read_timeout: Duration) -> ElevenLabsSynthesiser {
        ElevenLabsSynthesiser::new(
            Endpoint::new()
                .with_url(&self.url)
                .with_read_timeout(read_timeout)
                .with_connect_timeout(Duration::from_secs(5)),
            VoiceConfig::new(VOICE, format),
            Arc::new(FixedKey(KEY)),
            "elevenlabs-api-key",
        )
        .expect("a valid configuration")
    }
}

type Scripts = Arc<(Vec<Vec<Act>>, Mutex<usize>)>;

async fn serve(
    stream: tokio::net::TcpStream,
    index: usize,
    scripts: Scripts,
    ends: mpsc::Sender<usize>,
) {
    let Ok(socket) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (sink, mut stream) = socket.split();
    let sink = Arc::new(tokio::sync::Mutex::new(sink));
    let mut acting = Vec::new();
    while let Some(Ok(message)) = stream.next().await {
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };
        let Ok(serde_json::Value::Object(object)) =
            serde_json::from_str::<serde_json::Value>(text.as_str())
        else {
            continue;
        };
        if object.contains_key("close_socket") {
            break;
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
        )));
    }
    for task in acting {
        task.abort();
    }
    let _ = sink.lock().await.close().await;
    let _ = ends.send(index);
}

type Sink = Arc<
    tokio::sync::Mutex<
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
            Message,
        >,
    >,
>;

/// Takes `acts` for the context `context_id`, each text naming the context, until a send fails.
async fn act(acts: Vec<Act>, context_id: String, sink: Sink) {
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
        match act {
            Act::Send(text) => {
                if sink
                    .lock()
                    .await
                    .send(Message::text(named(&text)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
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
            Act::PingForever => loop {
                if sink
                    .lock()
                    .await
                    .send(Message::Ping(Vec::new().into()))
                    .await
                    .is_err()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            },
        }
    }
}

fn audio(bytes: &[u8]) -> Act {
    Act::Send(format!(r#"{{"audio":"{}"}}"#, STANDARD.encode(bytes)))
}

fn final_message() -> Act {
    Act::Send(r#"{"audio":null,"isFinal":true}"#.to_owned())
}

/// Runs `work` on a thread of its own and fails the case when it does not return within `bound`.
fn within<T: Send + 'static>(bound: Duration, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(bound)
        .unwrap_or_else(|_| panic!("the case did not finish within {bound:?}"))
}

fn frames(events: &[SynthesisEvent]) -> Vec<(u64, u32, Vec<u8>)> {
    events
        .iter()
        .filter_map(|event| match event {
            SynthesisEvent::Audio(audio) => Some((
                audio.frame().position.get(),
                audio.frame().sample_count,
                audio.frame().data.clone(),
            )),
            _ => None,
        })
        .collect()
}

/// The remedy `ElevenLabsError::InsideRuntime` itself names, a thread from
/// `tokio::task::spawn_blocking`, must let a synthesis run: blocking there does not panic.
#[test]
fn adversary1_a_synthesis_runs_on_a_spawn_blocking_thread_as_its_error_advises() {
    let server = FixtureServer::start(OutputFormat::Ulaw8000, Script::Serve);
    let mut synthesiser = server.synthesiser(OutputFormat::Ulaw8000);
    let outcome = within(Duration::from_secs(20), move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a runtime");
        runtime.block_on(async move {
            tokio::task::spawn_blocking(move || {
                let mut handle = synthesiser.start().unwrap();
                handle
                    .offer_segment(committed(&["Spoken from a blocking thread."]).remove(0))
                    .unwrap();
                handle.end_utterance().unwrap();
                let events: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
                (handle.failure().cloned(), frames(&events).len())
            })
            .await
            .expect("the blocking task does not panic")
        })
    });
    assert_eq!(
        outcome,
        (None, 4),
        "a synthesis on a spawn_blocking thread failed or spoke nothing"
    );
}

/// The control of the case above: on a spawn_blocking thread a runtime handle is current, which
/// is what the binding's guard tests, and yet a fresh runtime blocks there without a panic, which
/// is what the guard claims to prevent.
#[test]
fn adversary1_control_a_fresh_runtime_blocks_on_a_spawn_blocking_thread_without_panic() {
    let (handle_current, blocked) = within(Duration::from_secs(10), || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a runtime");
        runtime.block_on(async {
            tokio::task::spawn_blocking(|| {
                let current = tokio::runtime::Handle::try_current().is_ok();
                let own = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("a runtime");
                let value = own.block_on(async {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    7
                });
                (current, value)
            })
            .await
            .expect("blocking on a fresh runtime does not panic here")
        })
    });
    assert!(handle_current);
    assert_eq!(blocked, 7);
}

/// The crate doc reads a JSON object carrying `error` "and no audio" as the server's error. An
/// error object whose `audio` is null carries no audio; it must not pass as nothing and leave the
/// synthesis to stall.
#[test]
fn adversary1_an_error_with_null_audio_is_the_servers_error_and_not_a_stall() {
    let server = Scripted::start(vec![vec![
        audio(&[0xff; 4]),
        Act::Send(r#"{"audio":null,"error":"quota_exceeded"}"#.to_owned()),
    ]]);
    let mut synthesiser = server.synthesiser(OutputFormat::Ulaw8000, Duration::from_secs(1));
    let failure = within(Duration::from_secs(10), move || {
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Over quota."]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let _: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        handle.failure().cloned()
    });
    assert_eq!(
        failure,
        Some(ElevenLabsError::Server("quota_exceeded".to_owned()))
    );
}

/// The cap falls on characters: multibyte UTF-8 across the 256th character neither panics nor
/// keeps a broken character.
#[test]
fn adversary1_a_multibyte_server_error_is_cut_at_the_cap_on_a_character() {
    let said = format!(
        "{}{}",
        "a".repeat(SERVER_MESSAGE_LIMIT - 1),
        "\u{20ac}\u{1f600}".repeat(100)
    );
    let message = serde_json::json!({ "detail": { "message": said } }).to_string();
    let Err(ElevenLabsError::Server(kept)) = parse_server_message(&message) else {
        panic!("not a server error");
    };
    assert_eq!(kept.chars().count(), SERVER_MESSAGE_LIMIT);
    assert!(kept.ends_with('\u{20ac}'));
}

/// PCM16: one-byte and odd chunks carry their half sample over, an empty `audio` and a null
/// `audio` without `isFinal` emit nothing, and the frames are the bytes sent, on one clock.
#[test]
fn adversary1_pcm_one_byte_odd_empty_and_null_chunks_keep_every_sample() {
    let server = Scripted::start(vec![vec![
        audio(&[1]),
        audio(&[2]),
        audio(&[3, 4, 5]),
        Act::Send(r#"{"audio":""}"#.to_owned()),
        Act::Send(r#"{"audio":null}"#.to_owned()),
        audio(&[6]),
        final_message(),
    ]]);
    let mut synthesiser = server.synthesiser(OutputFormat::Pcm24000, Duration::from_secs(3));
    let (failure, events) = within(Duration::from_secs(15), move || {
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Odd."]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let events: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        (handle.failure().cloned(), events)
    });
    assert_eq!(failure, None);
    assert_eq!(
        frames(&events),
        vec![(0, 1, vec![1, 2]), (1, 1, vec![3, 4]), (2, 1, vec![5, 6])]
    );
    assert!(matches!(events.last(), Some(SynthesisEvent::Usage(record)) if record.r#final));
}

/// G.711: one byte is one sample, and positions run on across segments.
#[test]
fn adversary1_alaw_positions_count_one_sample_per_byte_across_segments() {
    let segment = vec![
        audio(&[0xd5]),
        audio(&[0xd5; 7]),
        audio(&[0x55; 2]),
        final_message(),
    ];
    let server = Scripted::start(vec![segment]);
    let mut synthesiser = server.synthesiser(OutputFormat::Alaw8000, Duration::from_secs(3));
    let (failure, events) = within(Duration::from_secs(15), move || {
        let mut handle = synthesiser.start().unwrap();
        for segment in committed(&["One.", "Two."]) {
            handle.offer_segment(segment).unwrap();
        }
        handle.end_utterance().unwrap();
        let events: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        (handle.failure().cloned(), events)
    });
    assert_eq!(failure, None);
    let clock: Vec<(u64, u32)> = frames(&events)
        .into_iter()
        .map(|(position, count, data)| {
            assert_eq!(data.len(), count as usize);
            (position, count)
        })
        .collect();
    assert_eq!(
        clock,
        vec![(0, 1), (1, 7), (8, 2), (10, 1), (11, 7), (18, 2)]
    );
}

/// A cancel against a server that keeps streaming the cancelled context's audio: the discard
/// returns within the close bound, nothing of the cancelled segment follows, the next segment is
/// spoken on the same connection (story:elevenlabs-multi-context-tts-connection), and a second
/// discard is refused.
#[test]
fn adversary1_a_cancel_against_a_server_that_keeps_streaming_is_bounded_and_clean() {
    let server = Scripted::start(vec![
        vec![
            audio(&[0xff; 160]),
            Act::Stream(format!(r#"{{"audio":"{}"}}"#, STANDARD.encode([0x7f; 160]))),
        ],
        vec![audio(&[0x01; 80]), final_message()],
    ]);
    let mut synthesiser = server.synthesiser(OutputFormat::Ulaw8000, Duration::from_secs(3));
    let (took, after, states, twice) = within(Duration::from_secs(15), move || {
        let mut handle = synthesiser.start().unwrap();
        let mut segments = committed(&["Cut.", "Next."]).into_iter();
        let cut = segments.next().unwrap();
        let cut_id = cut.segment_id();
        handle.offer_segment(cut).unwrap();
        assert!(matches!(handle.next(), Some(SynthesisEvent::Audio(_))));
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        handle.discard(cut_id).unwrap();
        let took = started.elapsed();
        let next = segments.next().unwrap();
        let next_id = next.segment_id();
        handle.offer_segment(next).unwrap();
        handle.end_utterance().unwrap();
        let after: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        let states = (
            handle.output().segment(cut_id).map(|s| s.state()),
            handle.output().segment(next_id).map(|s| s.state()),
        );
        let twice = handle.discard(cut_id).is_err();
        let after = (
            after.iter().all(|e| e.segment_id() != Some(cut_id)),
            frames(&after),
        );
        (took, after, states, twice)
    });
    assert!(took <= CLOSE_BOUND, "the cancel took {took:?}");
    assert!(
        after.0,
        "a frame of the cancelled segment followed the cancel"
    );
    assert_eq!(after.1, vec![(160, 80, vec![0x01; 80])]);
    assert_eq!(
        states,
        (
            Some(SegmentState::Discarded),
            Some(SegmentState::Synthesized)
        )
    );
    assert!(twice, "a second discard of the same segment is refused");
    assert_eq!(server.next_end(Duration::from_secs(5)), Some(0));
}

/// A cancel after `isFinal` closes nothing more and leaves the synthesis whole.
#[test]
fn adversary1_a_cancel_after_the_final_message_keeps_the_synthesis_whole() {
    let server = FixtureServer::start(OutputFormat::Ulaw8000, Script::Serve);
    let mut synthesiser = server.synthesiser(OutputFormat::Ulaw8000);
    let (discarded, twice, tail) = within(Duration::from_secs(15), move || {
        let mut handle = synthesiser.start().unwrap();
        let segment = committed(&["Done."]).remove(0);
        let id = segment.segment_id();
        handle.offer_segment(segment).unwrap();
        while !matches!(handle.next(), Some(SynthesisEvent::Completed(_))) {}
        let discarded = handle.discard(id).is_ok();
        let twice = handle.discard(id).is_err();
        handle.end_utterance().unwrap();
        let tail: Vec<SynthesisEvent> = handle.by_ref().take(10).collect();
        (discarded, twice, tail)
    });
    assert!(discarded && twice);
    assert!(
        matches!(tail.as_slice(), [SynthesisEvent::Usage(record)] if record.r#final),
        "{tail:?}"
    );
}

/// Pings move no read deadline: a server that only pings stalls the synthesis.
#[test]
fn adversary1_a_server_that_only_pings_stalls_within_the_read_timeout() {
    let server = Scripted::start(vec![vec![Act::PingForever]]);
    let mut synthesiser = server.synthesiser(OutputFormat::Ulaw8000, Duration::from_millis(500));
    let failure = within(Duration::from_secs(5), move || {
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Ping."]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let _: Vec<SynthesisEvent> = handle.by_ref().take(10).collect();
        handle.failure().cloned()
    });
    assert_eq!(
        failure,
        Some(ElevenLabsError::Stalled(Duration::from_millis(500)))
    );
}

/// One message past the WebSocket frame limit ends the synthesis as a typed failure.
#[test]
fn adversary1_a_huge_single_message_is_refused_as_a_typed_failure() {
    let huge = format!(r#"{{"audio":"{}"}}"#, "A".repeat(20 * 1024 * 1024));
    let server = Scripted::start(vec![vec![Act::Send(huge)]]);
    let mut synthesiser = server.synthesiser(OutputFormat::Ulaw8000, Duration::from_secs(5));
    let failure = within(Duration::from_secs(30), move || {
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Huge."]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let _: Vec<SynthesisEvent> = handle.by_ref().take(10).collect();
        handle.failure().cloned()
    });
    assert!(
        matches!(failure, Some(ElevenLabsError::Transport(_))),
        "{failure:?}"
    );
}

/// `ws://` sends the key in the clear. A loopback host is the fixture server's; any other host
/// must be refused at configuration.
#[test]
fn adversary1_a_plaintext_base_url_to_a_remote_host_is_refused() {
    let make = |url: &str| {
        ElevenLabsSynthesiser::new(
            Endpoint::new().with_url(url),
            VoiceConfig::new(VOICE, OutputFormat::Ulaw8000),
            Arc::new(FixedKey(KEY)),
            "elevenlabs-api-key",
        )
    };
    assert!(make("ws://127.0.0.1:9").is_ok());
    assert!(matches!(
        make("ws://api.elevenlabs.io"),
        Err(ConfigError::InvalidUrl(_))
    ));
}

/// Every error a fixture can reach, and the Debug of the synthesiser and its handle, rendered,
/// never carry the key; the handshake carries it in the header only.
#[test]
fn adversary1_no_reachable_error_or_debug_output_carries_the_key() {
    let mut rendered = Vec::new();
    for script in [
        Script::Refuse,
        Script::CloseAfterFirst,
        Script::SendAfterFirst("not json"),
        Script::SendAfterFirst(r#"{"error":"bad"}"#),
        Script::SendAfterFirst(r#"{"audio":"@@"}"#),
        Script::Silent,
    ] {
        let server = FixtureServer::start(OutputFormat::Ulaw8000, script);
        let mut synthesiser = ElevenLabsSynthesiser::new(
            server
                .endpoint()
                .with_read_timeout(Duration::from_millis(500)),
            VoiceConfig::new(VOICE, OutputFormat::Ulaw8000),
            Arc::new(FixedKey(KEY)),
            "elevenlabs-api-key",
        )
        .unwrap();
        rendered.push(format!("{synthesiser:?}"));
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Key?"]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let _: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        let failure = handle.failure().cloned().expect("each script fails");
        rendered.push(format!("{failure} {failure:?} {handle:?}"));
        for seen in server.seen() {
            assert!(!seen.query.contains(KEY) && !seen.path.contains(KEY));
            assert!(seen.messages.iter().all(|message| !message.contains(KEY)));
        }
    }
    let server = FixtureServer::start(OutputFormat::Ulaw8000, Script::Serve);
    let mut synthesiser = ElevenLabsSynthesiser::new(
        server.endpoint(),
        VoiceConfig::new(VOICE, OutputFormat::Ulaw8000),
        Arc::new(FixedKey("fixture-key-d41d8cd98f00b204\r\n")),
        "elevenlabs-api-key",
    )
    .unwrap();
    let mut handle = synthesiser.start().unwrap();
    handle
        .offer_segment(committed(&["Bad header."]).remove(0))
        .unwrap();
    let _ = handle.next();
    let failure = handle
        .failure()
        .cloned()
        .expect("an invalid header value fails");
    rendered.push(format!("{failure} {failure:?}"));
    for text in &rendered {
        assert!(!text.contains(KEY), "the key in {text}");
    }
}

/// The concurrency limit, and a handle dropped mid-segment inside an async runtime: no panic,
/// its connection ends and its slot comes back.
#[test]
fn adversary1_a_handle_dropped_inside_a_runtime_mid_segment_closes_and_frees_its_slot() {
    let server = FixtureServer::start(OutputFormat::Ulaw8000, Script::NoFinal);
    let mut synthesiser = server.synthesiser(OutputFormat::Ulaw8000);
    let mut handle = synthesiser.start().unwrap();
    assert!(matches!(
        synthesiser.start(),
        Err(inference_tts::ConcurrencyLimitReached { max_concurrency: 1 })
    ));
    handle
        .offer_segment(committed(&["Drop me."]).remove(0))
        .unwrap();
    assert!(matches!(handle.next(), Some(SynthesisEvent::Audio(_))));
    within(Duration::from_secs(10), move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        runtime.block_on(async move { drop(handle) });
    });
    assert_eq!(server.next_client_close(Duration::from_secs(5)), Some(0));
    assert!(synthesiser.start().is_ok());
}
