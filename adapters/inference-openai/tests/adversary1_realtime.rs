//! Adversary cases, pass 1: claims of the client read as written, against a local WebSocket
//! server. No network and no key; the credential is a fixed test value.
//!
//! Replayed server events are the examples of the published Realtime API reference, server events
//! page (`https://developers.openai.com/api/reference/resources/realtime/server-events`); expected
//! client events follow its client events page.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
    RealtimeClient, RealtimeError, ResponseStatus, ServerEvent, SessionConfig, WireFormat,
    encode_pcm16_24k,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

const CREDENTIAL: &str = "adversary-test-credential";

struct FixedSource;

impl CredentialSource for FixedSource {
    fn resolve<'a>(&'a self, name: &'a str) -> CredentialFuture<'a> {
        let found = name == OPENAI_KEY_NAME;
        Box::pin(async move {
            if found {
                Ok(Credential::new(CREDENTIAL.as_bytes().to_vec()))
            } else {
                Err(CredentialError::Missing)
            }
        })
    }
}

type Socket = WebSocketStream<TcpStream>;

async fn serve<F, Fut>(script: F) -> (Endpoint, JoinHandle<()>)
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("the client did not connect within five seconds")
            .unwrap();
        let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        script(socket).await;
    });
    let endpoint = Endpoint::new("gpt-realtime-2.1")
        .with_url(format!("ws://{address}/v1/realtime"))
        .with_read_timeout(Duration::from_secs(5));
    (endpoint, task)
}

async fn send(socket: &mut Socket, event: Value) {
    socket.send(Message::text(event.to_string())).await.unwrap();
}

async fn receive(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("the client sent nothing for five seconds")
            .expect("the client closed the connection")
            .unwrap();
        match message {
            Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}

fn content(mut event: Value) -> Value {
    if let Some(object) = event.as_object_mut() {
        object.remove("event_id");
    }
    event
}

async fn accept_session(socket: &mut Socket) {
    send(
        socket,
        json!({"type": "session.created", "event_id": "event_1", "session": {"type": "realtime", "id": "sess_1"}}),
    )
    .await;
    let update = receive(socket).await;
    assert_eq!(update["type"], "session.update");
    let mut session = update["session"].clone();
    session["id"] = json!("sess_1");
    send(
        socket,
        json!({"type": "session.updated", "event_id": "event_2", "session": session}),
    )
    .await;
}

async fn connect(endpoint: &Endpoint, session: &SessionConfig) -> RealtimeClient {
    RealtimeClient::connect(endpoint, &FixedSource, OPENAI_KEY_NAME, session)
        .await
        .expect("session setup")
}

fn pcm16_frame(rate: u32, samples: &[i16]) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(rate).unwrap(),
            channels: NonZeroU16::new(1).unwrap(),
        },
        position: SampleClockPosition::new(0).unwrap(),
        sample_count: samples.len() as u32,
        data: samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect(),
    }
}

fn decode(pcm: &[u8]) -> Vec<i16> {
    pcm.chunks(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}

/// `audio.rs:113`: "the rate is converted by linear interpolation". A ramp at 8 kHz becomes the
/// ramp at 24 kHz; the last two samples hold, as documented (no look-ahead).
#[test]
fn an_8_khz_ramp_is_linearly_interpolated_to_24_khz() {
    let pcm = encode_pcm16_24k(&pcm16_frame(8_000, &[0, 3000, 6000, 9000])).unwrap();
    assert_eq!(
        decode(&pcm),
        [
            0, 1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000, 9000, 9000, 9000
        ]
    );
}

/// 48 kHz to 24 kHz takes every second sample (linear interpolation at whole positions).
#[test]
fn a_48_khz_ramp_is_halved() {
    let samples: Vec<i16> = (0..8).map(|index| index * 100).collect();
    let pcm = encode_pcm16_24k(&pcm16_frame(48_000, &samples)).unwrap();
    assert_eq!(decode(&pcm), [0, 200, 400, 600]);
}

/// `client.rs:456`: `clear_audio` sends `input_audio_buffer.clear`.
#[tokio::test]
async fn clear_audio_sends_input_audio_buffer_clear() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            content(receive(&mut socket).await),
            json!({"type": "input_audio_buffer.clear"})
        );
    })
    .await;
    let mut client = connect(&endpoint, &SessionConfig::default()).await;
    client.clear_audio().await.unwrap();
    server.await.unwrap();
}

/// `client.rs:504`: `cancel_response(None)` cancels the response in progress: no `response_id`
/// member at all.
#[tokio::test]
async fn cancel_without_an_id_sends_no_response_id() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            content(receive(&mut socket).await),
            json!({"type": "response.cancel"})
        );
    })
    .await;
    let mut client = connect(&endpoint, &SessionConfig::default()).await;
    client.cancel_response(None).await.unwrap();
    server.await.unwrap();
}

/// `client.rs:406`: "Later appends follow its input format". After switching the session to
/// mu-law, `append_audio` refuses a PCM frame.
#[tokio::test]
async fn update_session_to_mulaw_makes_append_audio_refuse_pcm() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        let update = content(receive(&mut socket).await);
        assert_eq!(update["type"], "session.update");
        assert_eq!(
            update["session"]["audio"]["input"]["format"],
            json!({"type": "audio/pcmu"})
        );
    })
    .await;
    let mut client = connect(&endpoint, &SessionConfig::default()).await;
    let telephony = SessionConfig {
        input_format: WireFormat::Pcmu,
        output_format: WireFormat::Pcmu,
        ..SessionConfig::default()
    };
    client.update_session(&telephony).await.unwrap();
    let refused = client.append_audio(&pcm16_frame(8_000, &[0; 80])).await;
    server.await.unwrap();
    assert!(
        matches!(refused, Err(RealtimeError::Audio(_))),
        "a PCM frame after switching to mu-law: {refused:?}"
    );
}

/// A server that closes mid-response with a close code: the code and reason reach the caller.
#[tokio::test]
async fn a_server_close_mid_response_carries_its_code_and_reason() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        send(
            &mut socket,
            json!({
                "event_id": "event_4950",
                "type": "response.output_audio.delta",
                "response_id": "resp_001",
                "item_id": "msg_008",
                "output_index": 0,
                "content_index": 0,
                "delta": STANDARD.encode([1u8, 2])
            }),
        )
        .await;
        socket
            .close(Some(CloseFrame {
                code: CloseCode::Error,
                reason: "internal".into(),
            }))
            .await
            .unwrap();
        while let Some(Ok(_)) = socket.next().await {}
    })
    .await;
    let mut client = connect(&endpoint, &SessionConfig::default()).await;
    assert!(matches!(
        client.next_event().await.unwrap(),
        ServerEvent::AudioDelta { .. }
    ));
    let closed = client.next_event().await;
    drop(client);
    server.await.unwrap();
    match closed {
        Err(RealtimeError::Closed { code, reason }) => {
            assert_eq!((code, reason.as_str()), (Some(1011), "internal"));
        }
        other => panic!("expected a close with its code, got {other:?}"),
    }
}

/// A stall is not fatal: after `Stalled`, the next event of a long user turn still arrives.
#[tokio::test]
async fn a_stall_leaves_the_session_usable() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        send(
            &mut socket,
            json!({
                "event_id": "event_1718",
                "type": "input_audio_buffer.speech_stopped",
                "audio_end_ms": 2000,
                "item_id": "msg_003"
            }),
        )
        .await;
        while let Some(Ok(_)) = socket.next().await {}
    })
    .await;
    let endpoint = endpoint.with_read_timeout(Duration::from_millis(150));
    let mut client = connect(&endpoint, &SessionConfig::default()).await;
    assert!(matches!(
        client.next_event().await,
        Err(RealtimeError::Stalled(_))
    ));
    let mut next = client.next_event().await;
    while matches!(next, Err(RealtimeError::Stalled(_))) {
        next = client.next_event().await;
    }
    assert_eq!(
        next.unwrap(),
        ServerEvent::SpeechStopped {
            item_id: "msg_003".to_owned(),
            audio_end_ms: 2000
        }
    );
    // The close waits one read timeout, the 150 ms of the stall check, for the server's close
    // frame; under load that can pass first, and the session is dropped all the same.
    match client.close().await {
        Ok(()) | Err(RealtimeError::Stalled(_)) => {}
        Err(other) => panic!("closing failed: {other:?}"),
    }
    server.await.unwrap();
}

/// `config.rs:17` and `error.rs:68`: a stall is no *server event* within the read timeout.
/// WebSocket pings are not server events, yet each one restarts the timeout.
#[tokio::test]
async fn pings_without_events_still_stall() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        for _ in 0..24 {
            socket.send(Message::Ping(Vec::new().into())).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        send(
            &mut socket,
            json!({"type": "a.late.event", "event_id": "event_9"}),
        )
        .await;
        while let Some(Ok(_)) = socket.next().await {}
    })
    .await;
    let endpoint = endpoint.with_read_timeout(Duration::from_millis(200));
    let mut client = connect(&endpoint, &SessionConfig::default()).await;
    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(5), client.next_event())
        .await
        .expect("next_event returned within five seconds");
    let waited = started.elapsed();
    drop(client);
    server.abort();
    assert!(
        matches!(result, Err(RealtimeError::Stalled(_))),
        "no server event for {waited:?} with a 200 ms read timeout, got {result:?}"
    );
}

/// `client.rs:554`: `close` "Sends a close frame and waits for the server's". A server that
/// answers the close 400 ms late makes `close` take at least that long.
#[tokio::test]
async fn close_waits_for_the_server_close_frame() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        while let Some(Ok(_)) = socket.next().await {}
    })
    .await;
    let client = connect(&endpoint, &SessionConfig::default()).await;
    let started = Instant::now();
    client.close().await.unwrap();
    let took = started.elapsed();
    server.await.unwrap();
    assert!(
        took >= Duration::from_millis(300),
        "close returned after {took:?}, before the server's close frame (sent after 400 ms)"
    );
}

/// A handshake the server refuses (HTTP 401) is `Connect`, and neither its `Display` nor its
/// `Debug` carries the credential.
#[tokio::test]
#[expect(
    clippy::result_large_err,
    reason = "the handshake callback's error type is tungstenite's, fixed by its `Callback` trait"
)]
async fn a_refused_handshake_does_not_carry_the_credential() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = tokio_tungstenite::accept_hdr_async(
            stream,
            |_: &tokio_tungstenite::tungstenite::handshake::server::Request,
             _: tokio_tungstenite::tungstenite::handshake::server::Response| {
                let mut refusal =
                    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(Some(
                        format!("Incorrect API key provided: {CREDENTIAL}"),
                    ));
                *refusal.status_mut() =
                    tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;
                Err(refusal)
            },
        )
        .await;
    });
    let endpoint =
        Endpoint::new("gpt-realtime-2.1").with_url(format!("ws://{address}/v1/realtime"));
    let result = RealtimeClient::connect(
        &endpoint,
        &FixedSource,
        OPENAI_KEY_NAME,
        &SessionConfig::default(),
    )
    .await;
    server.await.unwrap();
    let error = result.expect_err("the handshake is refused");
    assert!(matches!(error, RealtimeError::Connect(_)), "{error:?}");
    assert!(!error.to_string().contains(CREDENTIAL), "{error}");
    assert!(!format!("{error:?}").contains(CREDENTIAL), "{error:?}");
}

/// Typed events no offline case decodes, each the reference example as published.
#[test]
fn reference_examples_of_the_untested_typed_events_decode() {
    let text = ServerEvent::parse(
        &json!({
            "event_id": "event_4142",
            "type": "response.output_text.delta",
            "response_id": "resp_001",
            "item_id": "msg_007",
            "output_index": 0,
            "content_index": 0,
            "delta": "Sure, I can h"
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        text,
        ServerEvent::TextDelta {
            response_id: "resp_001".to_owned(),
            item_id: "msg_007".to_owned(),
            delta: "Sure, I can h".to_owned()
        }
    );
    let committed = ServerEvent::parse(
        &json!({
            "event_id": "event_1121",
            "type": "input_audio_buffer.committed",
            "previous_item_id": "msg_001",
            "item_id": "msg_002"
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        committed,
        ServerEvent::AudioCommitted {
            item_id: "msg_002".to_owned()
        }
    );
    let truncated = ServerEvent::parse(
        &json!({
            "event_id": "event_2526",
            "type": "conversation.item.truncated",
            "item_id": "msg_004",
            "content_index": 0,
            "audio_end_ms": 1500
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        truncated,
        ServerEvent::ItemTruncated {
            item_id: "msg_004".to_owned(),
            content_index: 0,
            audio_end_ms: 1500
        }
    );
    for (wire, status) in [
        ("failed", ResponseStatus::Failed),
        ("incomplete", ResponseStatus::Incomplete),
        ("in_progress", ResponseStatus::InProgress),
        ("expired", ResponseStatus::Other("expired".to_owned())),
    ] {
        let done = ServerEvent::parse(
            &json!({
                "type": "response.done",
                "event_id": "event_3132",
                "response": {"object": "realtime.response", "id": "resp_9", "status": wire}
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            done,
            ServerEvent::ResponseDone {
                response_id: Some("resp_9".to_owned()),
                status: Some(status),
                usage: None
            }
        );
    }
}

/// A binary message is a protocol violation, not an event and not a silent drop.
#[tokio::test]
async fn a_binary_message_is_a_protocol_error() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        socket
            .send(Message::Binary(vec![1u8, 2, 3].into()))
            .await
            .unwrap();
        while let Some(Ok(_)) = socket.next().await {}
    })
    .await;
    let mut client = connect(&endpoint, &SessionConfig::default()).await;
    let result = client.next_event().await;
    drop(client);
    server.await.unwrap();
    assert!(
        matches!(result, Err(RealtimeError::Protocol(_))),
        "{result:?}"
    );
}
