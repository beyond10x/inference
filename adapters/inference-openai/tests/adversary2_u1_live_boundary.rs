//! Adversary pass 1 on the GPT-Live boundary between a cut answer and the model's next words
//! (`resumed` in `src/live_duplex.rs`), offline through the binding over a local WebSocket server.
//!
//! The rule under attack: the first output transcript fragment whose `start_ms` is at or after
//! the end of the caller's speech so far (`Spoken::interrupted`, the latest input `end_ms` seen)
//! ends the cut response and opens a new one. "So far" is the weak point: the caller's speech is
//! reported in input fragments, and the model's cut sentence can go on past the end of the first
//! fragment before the next one arrives. The unit's own replay (`gpt_live_duplex.rs`,
//! `spoken_over`) already delivers an output fragment (" a ship", 400 to 600) that runs past the
//! first input fragment's end (500) before the second input fragment arrives; these cases add one
//! more fragment of the cut sentence in that window.

#![cfg(all(feature = "gpt-live", feature = "duplex"))]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, ResponseId, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection,
    TurnTuning,
};
use inference_openai::live::LIVE_MODEL;
use inference_openai::live_duplex::OpenAiLive;
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const WAIT: Duration = Duration::from_millis(100);
const FRAME: usize = 2_400;

struct FixedSource;

impl CredentialSource for FixedSource {
    fn resolve<'a>(&'a self, name: &'a str) -> CredentialFuture<'a> {
        let found = name == OPENAI_KEY_NAME;
        Box::pin(async move {
            if found {
                Ok(Credential::new(b"offline-test-credential".to_vec()))
            } else {
                Err(CredentialError::Missing)
            }
        })
    }
}

type Socket = WebSocketStream<TcpStream>;

struct Server {
    endpoint: Endpoint,
    runtime: Runtime,
    task: JoinHandle<()>,
}

impl Server {
    fn start<F, Fut>(script: F) -> Self
    where
        F: FnOnce(Socket) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let runtime = Runtime::new().unwrap();
        let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let address = listener.local_addr().unwrap();
        let task = runtime.spawn(async move {
            let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .expect("the binding did not connect within five seconds")
                .unwrap();
            let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            script(socket).await;
        });
        let endpoint = Endpoint::live(LIVE_MODEL)
            .with_url(format!("ws://{address}/v1/live/sessions"))
            .with_read_timeout(Duration::from_secs(5));
        Self {
            endpoint,
            runtime,
            task,
        }
    }

    fn model(&self) -> OpenAiLive {
        OpenAiLive::new(
            self.endpoint.clone(),
            Box::new(FixedSource),
            OPENAI_KEY_NAME,
        )
    }

    fn finish(self) {
        self.runtime.block_on(self.task).unwrap();
    }
}

async fn send(socket: &mut Socket, event: Value) {
    socket.send(Message::text(event.to_string())).await.unwrap();
}

async fn receive(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("the binding sent nothing for five seconds")
            .expect("the binding closed the connection")
            .unwrap();
        match message {
            Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}

async fn accept_start(socket: &mut Socket) {
    let start = receive(socket).await;
    assert_eq!(start["type"], "session.start", "{start}");
    let mut resolved = start["session"].clone();
    resolved["id"] = json!("live_123");
    resolved["status"] = json!("active");
    send(
        socket,
        json!({"type": "session.started", "event_id": "event_started_1", "session": resolved}),
    )
    .await;
}

fn pcm_frame(level: i16) -> Vec<u8> {
    level
        .to_le_bytes()
        .iter()
        .copied()
        .cycle()
        .take(FRAME * 2)
        .collect()
}

fn audio(bytes: &[u8]) -> Value {
    json!({"type": "session.output_audio.delta", "delta": STANDARD.encode(bytes)})
}

fn silent() -> Value {
    audio(&pcm_frame(12))
}

fn voiced() -> Value {
    audio(&pcm_frame(3_000))
}

fn said(delta: &str, start_ms: u64) -> Value {
    json!({"type": "session.output_transcript.delta", "event_id": "evt_out", "delta": delta, "start_ms": start_ms, "end_ms": start_ms + 200})
}

fn heard(delta: &str, start_ms: u64) -> Value {
    json!({"type": "session.input_transcript.delta", "event_id": "evt_in", "delta": delta, "start_ms": start_ms, "end_ms": start_ms + 200})
}

fn config() -> SessionConfig {
    let pcm24 = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap(),
        channels: NonZeroU16::MIN,
    };
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm24,
        output_format: pcm24,
        turn_detection: TurnDetection::SemanticVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

/// The server: an answer, the caller's speech over it in two input fragments, the cut sentence
/// going on (" a ship", " today") while the second input fragment is still to come, then the
/// next words after the speech ended. `second` is where the second input fragment starts.
fn cut_sentence_outruns_the_speech(second: u64) -> Server {
    Server::start(move |mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Once upon a time", 0)).await;
        send(&mut socket, voiced()).await;
        send(&mut socket, said(" there was", 200)).await;
        send(&mut socket, voiced()).await;
        send(&mut socket, heard("wait", 300)).await;
        send(&mut socket, voiced()).await;
        send(&mut socket, said(" a ship", 400)).await;
        send(&mut socket, voiced()).await;
        send(&mut socket, said(" today", 600)).await;
        send(&mut socket, voiced()).await;
        send(&mut socket, heard(" stop", second)).await;
        send(&mut socket, voiced()).await;
        send(&mut socket, said(" Sure.", 1_000)).await;
        send(&mut socket, voiced()).await;
        send(&mut socket, voiced()).await;
        for _ in 0..10 {
            send(&mut socket, silent()).await;
        }
        let close = receive(&mut socket).await;
        assert_eq!(close["type"], "session.close");
        send(
            &mut socket,
            json!({"type": "session.closed", "event_id": "evt_c", "reason": "close_requested", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 1.5}}),
        )
        .await;
        let _ = socket.close(None).await;
    })
}

/// Every event of the session, through its close.
fn run(server: Server) -> Vec<SessionEvent> {
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let mut events = Vec::new();
    let mut idle = 0;
    let mut completed = 0;
    while idle < 20 && completed < 3 {
        match session.next_event(WAIT) {
            Some(event) => {
                idle = 0;
                if matches!(event, SessionEvent::ResponseCompleted(_)) {
                    completed += 1;
                }
                events.push(event);
            }
            None => idle += 1,
        }
    }
    session.close().expect("open");
    server.finish();
    events.extend(std::iter::from_fn(|| session.next_event(Duration::ZERO)));
    events
}

fn transcripts(events: &[SessionEvent]) -> Vec<(ResponseId, String)> {
    let mut out: Vec<(ResponseId, String)> = Vec::new();
    for event in events {
        match event {
            SessionEvent::ResponseCreated(created) => {
                out.push((created.response_id, String::new()))
            }
            SessionEvent::OutputTranscriptDelta(delta) => {
                if let Some(entry) = out.iter_mut().find(|(id, _)| *id == delta.response_id) {
                    entry.1.push_str(&delta.text);
                }
            }
            _ => {}
        }
    }
    out
}

/// The response that is spoken over last before its audio arrives, for each output frame: the
/// frames a `Conversation` would play are those on a response with no `SpeechStarted` between its
/// creation and the frame.
fn playable_after_speech(events: &[SessionEvent]) -> Vec<(ResponseId, usize)> {
    let mut spoken_over: Vec<ResponseId> = Vec::new();
    let mut current: Option<ResponseId> = None;
    let mut speech = false;
    let mut played = Vec::new();
    for (index, event) in events.iter().enumerate() {
        match event {
            SessionEvent::ResponseCreated(created) => current = Some(created.response_id),
            SessionEvent::SpeechStarted(_) => {
                speech = true;
                spoken_over.extend(current);
            }
            SessionEvent::OutputAudio(output)
                if speech && !spoken_over.contains(&output.response_id) =>
            {
                played.push((output.response_id, index));
            }
            _ => {}
        }
    }
    played
}

/// The caller says "wait" (300 to 500) then " stop" (500 to 700) over the answer; the model's cut
/// sentence goes on with " today" at 600, inside the caller's speech, before " stop" arrives. The
/// speech ends at 700, so " today" and its audio are the rest of the cut sentence and belong on
/// the cut response; the next words are " Sure." at 1000. The binding takes " today" as the
/// boundary (600 is at or after 500, the speech end so far), opens the playable response on it,
/// and " stop", starting before that response's first fragment, reports no speech over it: the
/// rest of the cut sentence plays, and " Sure." is glued onto it.
#[test]
fn the_cut_sentence_that_outruns_the_first_input_fragment_stays_on_the_cut_response() {
    let events = run(cut_sentence_outruns_the_speech(500));
    let said = transcripts(&events);
    let texts: Vec<&str> = said.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(
        texts,
        vec![" Once upon a time there was a ship today", " Sure."],
        "speech 300 to 700: \" today\" at 600 is the cut sentence, \" Sure.\" at 1000 the next words; {events:#?}"
    );
}

/// The same with " stop" starting at 600, a pause of 100 ms in the caller's speech: the binding
/// opens a response on " today", then " stop" reports speech over it. The frame of " today"
/// arrives on that response before the second speech start, so a caller playing every response
/// not yet spoken over plays a frame of the cut sentence during the talk-over.
#[test]
fn no_frame_of_the_cut_sentence_is_on_a_response_not_spoken_over_during_the_talk_over() {
    let events = run(cut_sentence_outruns_the_speech(600));
    let said = transcripts(&events);
    let next = said
        .iter()
        .find(|(_, text)| text.contains("Sure."))
        .map(|(id, _)| *id)
        .unwrap_or_else(|| panic!("no next words: {events:#?}"));
    let played: Vec<ResponseId> = playable_after_speech(&events)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(
        played.iter().all(|id| *id == next),
        "after the first speech start only the next words' frames are playable; played on {:?}, next words on {next:?}, transcripts {said:?}",
        played
    );
}
