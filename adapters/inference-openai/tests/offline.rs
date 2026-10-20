//! Offline tests of the Realtime client against a local WebSocket server in the test. No network
//! and no key: the credential is a fixed test value from an in-test resolver.
//!
//! Every server event the scripts replay is written from the published Realtime API reference,
//! server events page (`https://developers.openai.com/api/reference/resources/realtime/server-events`),
//! and every client event the scripts expect from its client events page
//! (`https://developers.openai.com/api/reference/resources/realtime/client-events`). The comment
//! above each replayed event names the event of that page it was copied from.

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Eagerness, Endpoint,
    EnvironmentSource, FunctionTool, OPENAI_KEY_NAME, OutputModality, RealtimeClient,
    RealtimeError, ResponseStatus, ServerEvent, SessionConfig, TranscriptionUsage, TurnDetection,
    WireFormat, encode_pcm16_24k,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

const CREDENTIAL: &str = "offline-test-credential";
const MODEL: &str = "gpt-realtime-2.1";

/// Serves one fixed test value under [`OPENAI_KEY_NAME`], or refuses with `Missing` when built
/// with `missing`. Any other name is `Missing` too.
struct FixedSource {
    missing: bool,
}

impl CredentialSource for FixedSource {
    fn resolve<'a>(&'a self, name: &'a str) -> CredentialFuture<'a> {
        let found = !self.missing && name == OPENAI_KEY_NAME;
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

/// What the server saw of the opening handshake.
#[derive(Debug, Default, Clone)]
struct Handshake {
    uri: String,
    authorization: Option<String>,
    beta_header: bool,
}

/// Binds a local server, accepts one connection and runs `script` on it. Returns the endpoint the
/// client connects to, the handshake the server saw and the server task, whose panics carry the
/// script's assertion failures.
#[expect(
    clippy::result_large_err,
    reason = "the handshake callback's error type is tungstenite's, fixed by its `Callback` trait"
)]
async fn serve<F, Fut>(script: F) -> (Endpoint, Arc<Mutex<Handshake>>, JoinHandle<()>)
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Handshake::default()));
    let record = seen.clone();
    let task = tokio::spawn(async move {
        let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("the client did not connect within five seconds")
            .unwrap();
        let socket = tokio_tungstenite::accept_hdr_async(
            stream,
            move |request: &Request, response: Response| {
                let mut handshake = record.lock().unwrap();
                handshake.uri = request.uri().to_string();
                handshake.authorization = request
                    .headers()
                    .get("authorization")
                    .map(|value| value.to_str().unwrap().to_owned());
                handshake.beta_header = request.headers().contains_key("openai-beta");
                Ok(response)
            },
        )
        .await
        .unwrap();
        script(socket).await;
    });
    let endpoint = Endpoint::new(MODEL)
        .with_url(format!("ws://{address}/v1/realtime"))
        .with_read_timeout(Duration::from_secs(5));
    (endpoint, seen, task)
}

async fn send(socket: &mut Socket, event: Value) {
    socket.send(Message::text(event.to_string())).await.unwrap();
}

/// The next client event as JSON; fails the script after five seconds of silence.
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

/// Strips the optional client `event_id` so a script compares the event's content alone.
fn content(mut event: Value) -> Value {
    if let Some(object) = event.as_object_mut() {
        object.remove("event_id");
    }
    event
}

/// `session.created`, from the server events page, example trimmed to the fields a client reads.
fn session_created() -> Value {
    json!({
        "type": "session.created",
        "event_id": "event_C9G5RJeJ2gF77mV7f2B1j",
        "session": {
            "type": "realtime",
            "object": "realtime.session",
            "id": "sess_C9G5QPteg4UIbotdKLoYQ",
            "model": "gpt-realtime-2025-08-28",
            "output_modalities": ["audio"],
            "tools": [],
            "tool_choice": "auto",
            "max_output_tokens": "inf",
            "audio": {
                "input": {
                    "format": {"type": "audio/pcm", "rate": 24000},
                    "transcription": null,
                    "noise_reduction": null,
                    "turn_detection": {
                        "type": "server_vad",
                        "threshold": 0.5,
                        "prefix_padding_ms": 300,
                        "silence_duration_ms": 200,
                        "idle_timeout_ms": null,
                        "create_response": true,
                        "interrupt_response": true
                    }
                },
                "output": {"format": {"type": "audio/pcm", "rate": 24000}, "voice": "marin", "speed": 1}
            }
        }
    })
}

/// `session.updated`, from the server events page: the session as the server now holds it.
fn session_updated(session: &Value) -> Value {
    let mut confirmed = session.clone();
    confirmed["object"] = json!("realtime.session");
    confirmed["id"] = json!("sess_C9G5QPteg4UIbotdKLoYQ");
    json!({"type": "session.updated", "event_id": "event_C9G8mqI3IucaojlVKE8Cs", "session": confirmed})
}

/// The server half of session setup: `session.created`, the client's `session.update`, then
/// `session.updated`. Returns the client's `session.update`.
async fn accept_session(socket: &mut Socket) -> Value {
    send(socket, session_created()).await;
    let update = receive(socket).await;
    assert_eq!(
        update["type"], "session.update",
        "first client event: {update}"
    );
    send(socket, session_updated(&update["session"])).await;
    update
}

async fn connect(endpoint: &Endpoint, session: &SessionConfig) -> RealtimeClient {
    RealtimeClient::connect(
        endpoint,
        &FixedSource { missing: false },
        OPENAI_KEY_NAME,
        session,
    )
    .await
    .expect("session setup")
}

/// Reads events until `response.done`, returning every event read.
async fn until_done(client: &mut RealtimeClient) -> Vec<ServerEvent> {
    let mut events = Vec::new();
    loop {
        let event = client.next_event().await.expect("server event");
        let done = matches!(event, ServerEvent::ResponseDone { .. });
        events.push(event);
        if done {
            return events;
        }
    }
}

fn telephony_session() -> SessionConfig {
    SessionConfig {
        instructions: Some("Answer in one short sentence.".to_owned()),
        voice: Some("marin".to_owned()),
        output_modality: OutputModality::Audio,
        input_format: WireFormat::Pcmu,
        output_format: WireFormat::Pcmu,
        turn_detection: TurnDetection::Server {
            threshold: Some(0.5),
            prefix_padding_ms: Some(300),
            silence_duration_ms: Some(500),
            create_response: false,
            interrupt_response: false,
        },
        input_transcription: Some("gpt-4o-mini-transcribe".to_owned()),
        tools: vec![FunctionTool {
            name: "get_weather".to_owned(),
            description: "Current weather for a city.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {"location": {"type": "string"}},
                "required": ["location"]
            }),
        }],
        speed: None,
    }
}

#[tokio::test]
async fn session_setup_sends_the_configuration_and_the_credential() {
    let (endpoint, handshake, server) = serve(|mut socket| async move {
        let update = accept_session(&mut socket).await;
        // `session.update`, client events page; the session object is the GA session
        // configuration: `type`, `output_modalities`, `instructions`, `audio.input`,
        // `audio.output`, `tools`.
        assert_eq!(
            content(update),
            json!({
                "type": "session.update",
                "session": {
                    "type": "realtime",
                    "output_modalities": ["audio"],
                    "instructions": "Answer in one short sentence.",
                    "audio": {
                        "input": {
                            "format": {"type": "audio/pcmu"},
                            "transcription": {"model": "gpt-4o-mini-transcribe"},
                            "turn_detection": {
                                "type": "server_vad",
                                "threshold": 0.5,
                                "prefix_padding_ms": 300,
                                "silence_duration_ms": 500,
                                "create_response": false,
                                "interrupt_response": false
                            }
                        },
                        "output": {"format": {"type": "audio/pcmu"}, "voice": "marin"}
                    },
                    "tools": [{
                        "type": "function",
                        "name": "get_weather",
                        "description": "Current weather for a city.",
                        "parameters": {
                            "type": "object",
                            "properties": {"location": {"type": "string"}},
                            "required": ["location"]
                        }
                    }]
                }
            })
        );
    })
    .await;
    let client = connect(&endpoint, &telephony_session()).await;
    server.await.unwrap();
    assert_eq!(client.session()["id"], "sess_C9G5QPteg4UIbotdKLoYQ");
    let handshake = handshake.lock().unwrap().clone();
    assert_eq!(handshake.uri, format!("/v1/realtime?model={MODEL}"));
    assert_eq!(
        handshake.authorization.as_deref(),
        Some(format!("Bearer {CREDENTIAL}").as_str())
    );
    assert!(
        !handshake.beta_header,
        "the GA interface takes no beta header"
    );
}

#[tokio::test]
async fn text_in_audio_out_response() {
    let audio_one = vec![1u8, 2, 3, 4];
    let audio_two = vec![5u8, 6, 7, 8, 9, 10];
    let (one, two) = (STANDARD.encode(&audio_one), STANDARD.encode(&audio_two));
    let (endpoint, _, server) = serve(move |mut socket| async move {
        accept_session(&mut socket).await;
        // `conversation.item.create`, client events page, with a user message item
        // (`RealtimeConversationItemMessageUser`, content `input_text`).
        assert_eq!(
            content(receive(&mut socket).await),
            json!({
                "type": "conversation.item.create",
                "item": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Hello?"}]
                }
            })
        );
        // `response.create`, client events page.
        assert_eq!(content(receive(&mut socket).await), json!({"type": "response.create"}));
        // `response.created`, server events page.
        send(&mut socket, json!({
            "type": "response.created",
            "event_id": "event_C9G8pqbTEddBSIxbBN6Os",
            "response": {
                "object": "realtime.response",
                "id": "resp_C9G8p7IH2WxLbkgPNouYL",
                "status": "in_progress",
                "status_details": null,
                "output": [],
                "conversation_id": "conv_C9G8mmBkLhQJwCon3hoJN",
                "output_modalities": ["audio"],
                "max_output_tokens": "inf",
                "usage": null,
                "metadata": null
            }
        }))
        .await;
        // `response.output_audio.delta`, server events page.
        for delta in [one, two] {
            send(&mut socket, json!({
                "event_id": "event_4950",
                "type": "response.output_audio.delta",
                "response_id": "resp_C9G8p7IH2WxLbkgPNouYL",
                "item_id": "msg_008",
                "output_index": 0,
                "content_index": 0,
                "delta": delta
            }))
            .await;
        }
        // `response.output_audio_transcript.delta`, server events page.
        for delta in ["Hello, how can I ", "help?"] {
            send(&mut socket, json!({
                "event_id": "event_4546",
                "type": "response.output_audio_transcript.delta",
                "response_id": "resp_C9G8p7IH2WxLbkgPNouYL",
                "item_id": "msg_008",
                "output_index": 0,
                "content_index": 0,
                "delta": delta
            }))
            .await;
        }
        // `response.output_audio_transcript.done`, server events page.
        send(&mut socket, json!({
            "event_id": "event_4748",
            "type": "response.output_audio_transcript.done",
            "response_id": "resp_C9G8p7IH2WxLbkgPNouYL",
            "item_id": "msg_008",
            "output_index": 0,
            "content_index": 0,
            "transcript": "Hello, how can I help?"
        }))
        .await;
        // `response.output_audio.done`, server events page.
        send(&mut socket, json!({
            "event_id": "event_5152",
            "type": "response.output_audio.done",
            "response_id": "resp_C9G8p7IH2WxLbkgPNouYL",
            "item_id": "msg_008",
            "output_index": 0,
            "content_index": 0
        }))
        .await;
        // `response.done`, server events page, example as published.
        send(&mut socket, json!({
            "type": "response.done",
            "event_id": "event_CCXHxcMy86rrKhBLDdqCh",
            "response": {
                "object": "realtime.response",
                "id": "resp_C9G8p7IH2WxLbkgPNouYL",
                "status": "completed",
                "status_details": null,
                "output": [{
                    "id": "msg_008",
                    "type": "message",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{"type": "output_audio", "transcript": "Hello, how can I help?"}]
                }],
                "conversation_id": "conv_CCXHsurMKcaVxIZvaCI5m",
                "output_modalities": ["audio"],
                "max_output_tokens": "inf",
                "audio": {"output": {"format": {"type": "audio/pcm", "rate": 24000}, "voice": "alloy"}},
                "usage": {
                    "total_tokens": 253,
                    "input_tokens": 132,
                    "output_tokens": 121,
                    "input_token_details": {
                        "text_tokens": 119,
                        "audio_tokens": 13,
                        "image_tokens": 0,
                        "cached_tokens": 64,
                        "cached_tokens_details": {"text_tokens": 64, "audio_tokens": 0, "image_tokens": 0}
                    },
                    "output_token_details": {"text_tokens": 30, "audio_tokens": 91}
                },
                "metadata": null
            }
        }))
        .await;
    })
    .await;
    let mut client = connect(&endpoint, &SessionConfig::default()).await;
    client.send_text("Hello?").await.unwrap();
    client.create_response().await.unwrap();
    let events = until_done(&mut client).await;
    server.await.unwrap();

    let mut audio = Vec::new();
    let mut transcript = String::new();
    let mut created = None;
    let mut final_transcript = None;
    let mut audio_done = false;
    for event in &events {
        match event {
            ServerEvent::ResponseCreated { response_id } => created = Some(response_id.clone()),
            ServerEvent::AudioDelta {
                item_id,
                audio: bytes,
                ..
            } => {
                assert_eq!(item_id, "msg_008");
                audio.extend_from_slice(bytes);
            }
            ServerEvent::OutputTranscriptDelta { delta, .. } => transcript.push_str(delta),
            ServerEvent::OutputTranscriptDone { transcript, .. } => {
                final_transcript = Some(transcript.clone())
            }
            ServerEvent::AudioDone { .. } => audio_done = true,
            _ => {}
        }
    }
    assert_eq!(created.as_deref(), Some("resp_C9G8p7IH2WxLbkgPNouYL"));
    assert_eq!(audio, [audio_one, audio_two].concat());
    assert_eq!(transcript, "Hello, how can I help?");
    assert_eq!(final_transcript.as_deref(), Some("Hello, how can I help?"));
    assert!(audio_done);
    let Some(ServerEvent::ResponseDone {
        response_id,
        status,
        usage,
    }) = events.last()
    else {
        panic!("last event is not response.done: {events:?}");
    };
    assert_eq!(response_id, "resp_C9G8p7IH2WxLbkgPNouYL");
    assert_eq!(*status, ResponseStatus::Completed);
    let usage = usage.as_ref().expect("usage reported");
    assert_eq!(
        (usage.total_tokens, usage.input_tokens, usage.output_tokens),
        (253, 132, 121)
    );
    assert_eq!(usage.input_audio_tokens, Some(13));
    assert_eq!(usage.input_text_tokens, Some(119));
    assert_eq!(usage.cached_input_tokens, Some(64));
    assert_eq!(usage.output_audio_tokens, Some(91));
    assert_eq!(usage.output_text_tokens, Some(30));
}

#[tokio::test]
async fn barge_in_cancels_the_response_and_truncates_the_played_audio() {
    let (endpoint, _, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        // `input_audio_buffer.speech_started`, server events page.
        send(
            &mut socket,
            json!({
                "event_id": "event_1516",
                "type": "input_audio_buffer.speech_started",
                "audio_start_ms": 1000,
                "item_id": "msg_003"
            }),
        )
        .await;
        // `response.cancel`, client events page.
        assert_eq!(
            content(receive(&mut socket).await),
            json!({"type": "response.cancel", "response_id": "resp_001"})
        );
        // `conversation.item.truncate`, client events page.
        assert_eq!(
            content(receive(&mut socket).await),
            json!({
                "type": "conversation.item.truncate",
                "item_id": "msg_002",
                "content_index": 0,
                "audio_end_ms": 1500
            })
        );
        // `input_audio_buffer.speech_stopped`, server events page.
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
        // `conversation.item.input_audio_transcription.delta`, server events page.
        send(
            &mut socket,
            json!({
                "type": "conversation.item.input_audio_transcription.delta",
                "event_id": "event_CCXGRxsAimPAs8kS2Wc7Z",
                "item_id": "msg_003",
                "content_index": 0,
                "delta": "Hey",
                "obfuscation": "aLxx0jTEciOGe"
            }),
        )
        .await;
        // `conversation.item.input_audio_transcription.completed`, server events page.
        send(
            &mut socket,
            json!({
                "type": "conversation.item.input_audio_transcription.completed",
                "event_id": "event_CCXGRvtUVrax5SJAnNOWZ",
                "item_id": "msg_003",
                "content_index": 0,
                "transcript": "Hey, can you hear me?",
                "usage": {
                    "type": "tokens",
                    "total_tokens": 22,
                    "input_tokens": 13,
                    "input_token_details": {"text_tokens": 0, "audio_tokens": 13},
                    "output_tokens": 9
                }
            }),
        )
        .await;
        // `response.done`, server events page, with `status` `cancelled` as `response.cancel`
        // describes.
        send(
            &mut socket,
            json!({
                "type": "response.done",
                "event_id": "event_3132",
                "response": {
                    "object": "realtime.response",
                    "id": "resp_001",
                    "status": "cancelled",
                    "status_details": {"type": "cancelled", "reason": "client_cancelled"},
                    "output": [],
                    "usage": null
                }
            }),
        )
        .await;
    })
    .await;
    let mut client = connect(&endpoint, &telephony_session()).await;
    match client.next_event().await.unwrap() {
        ServerEvent::SpeechStarted {
            item_id,
            audio_start_ms,
        } => {
            assert_eq!((item_id.as_str(), audio_start_ms), ("msg_003", 1000));
        }
        other => panic!("expected speech started, got {other:?}"),
    }
    client.cancel_response(Some("resp_001")).await.unwrap();
    client.truncate("msg_002", 0, 1500).await.unwrap();
    let events = until_done(&mut client).await;
    server.await.unwrap();
    assert_eq!(
        events[..3],
        [
            ServerEvent::SpeechStopped {
                item_id: "msg_003".to_owned(),
                audio_end_ms: 2000
            },
            ServerEvent::InputTranscriptDelta {
                item_id: "msg_003".to_owned(),
                delta: "Hey".to_owned()
            },
            ServerEvent::InputTranscript {
                item_id: "msg_003".to_owned(),
                transcript: "Hey, can you hear me?".to_owned(),
                usage: Some(TranscriptionUsage::Tokens {
                    total_tokens: Some(22),
                    input_tokens: Some(13),
                    output_tokens: Some(9)
                })
            },
        ]
    );
    assert_eq!(
        events[3],
        ServerEvent::ResponseDone {
            response_id: "resp_001".to_owned(),
            status: ResponseStatus::Cancelled,
            usage: None
        }
    );
}

#[tokio::test]
async fn a_function_call_is_answered_with_its_output() {
    let (endpoint, _, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        // `response.function_call_arguments.delta`, server events page.
        send(&mut socket, json!({
            "event_id": "event_5354",
            "type": "response.function_call_arguments.delta",
            "response_id": "resp_002",
            "item_id": "fc_001",
            "output_index": 0,
            "call_id": "call_001",
            "delta": "{\"location\": \"San\""
        }))
        .await;
        // `response.function_call_arguments.done`, server events page.
        send(&mut socket, json!({
            "event_id": "event_5556",
            "type": "response.function_call_arguments.done",
            "response_id": "resp_002",
            "item_id": "fc_001",
            "output_index": 0,
            "call_id": "call_001",
            "name": "get_weather",
            "arguments": "{\"location\": \"San Francisco\"}"
        }))
        .await;
        // `conversation.item.create`, client events page, with a function call output item
        // (`RealtimeConversationItemFunctionCallOutput`).
        assert_eq!(
            content(receive(&mut socket).await),
            json!({
                "type": "conversation.item.create",
                "item": {
                    "type": "function_call_output",
                    "call_id": "call_001",
                    "output": "{\"temperature_c\": 18}"
                }
            })
        );
        assert_eq!(content(receive(&mut socket).await), json!({"type": "response.create"}));
        send(&mut socket, json!({
            "type": "response.done",
            "event_id": "event_5758",
            "response": {"object": "realtime.response", "id": "resp_003", "status": "completed", "output": []}
        }))
        .await;
    })
    .await;
    let mut client = connect(&endpoint, &telephony_session()).await;
    let mut call = None;
    while call.is_none() {
        if let ServerEvent::FunctionCall {
            call_id,
            name,
            arguments,
            ..
        } = client.next_event().await.unwrap()
        {
            call = Some((call_id, name, arguments));
        }
    }
    let (call_id, name, arguments) = call.unwrap();
    assert_eq!(name, "get_weather");
    assert_eq!(arguments, "{\"location\": \"San Francisco\"}");
    client
        .send_function_output(&call_id, "{\"temperature_c\": 18}")
        .await
        .unwrap();
    client.create_response().await.unwrap();
    let events = until_done(&mut client).await;
    server.await.unwrap();
    assert!(matches!(
        events.last(),
        Some(ServerEvent::ResponseDone {
            status: ResponseStatus::Completed,
            usage: None,
            ..
        })
    ));
}

#[tokio::test]
async fn an_error_event_is_typed() {
    let (endpoint, _, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        // `error`, server events page, example as published.
        send(
            &mut socket,
            json!({
                "event_id": "event_890",
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "code": "invalid_event",
                    "message": "The 'type' field is missing.",
                    "param": null,
                    "event_id": "event_567"
                }
            }),
        )
        .await;
    })
    .await;
    let mut client = connect(&endpoint, &SessionConfig::default()).await;
    let event = client.next_event().await.unwrap();
    server.await.unwrap();
    let ServerEvent::Error(error) = event else {
        panic!("expected an error event, got {event:?}");
    };
    assert_eq!(error.kind, "invalid_request_error");
    assert_eq!(error.code.as_deref(), Some("invalid_event"));
    assert_eq!(error.message, "The 'type' field is missing.");
    assert_eq!(error.param, None);
    assert_eq!(error.event_id.as_deref(), Some("event_567"));
}

#[tokio::test]
async fn an_error_during_setup_fails_the_connection() {
    let (endpoint, _, server) = serve(|mut socket| async move {
        send(&mut socket, session_created()).await;
        let _update = receive(&mut socket).await;
        send(
            &mut socket,
            json!({
                "event_id": "event_891",
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "code": "invalid_value",
                    "message": "Invalid value: 'nobody'.",
                    "param": "session.audio.output.voice",
                    "event_id": null
                }
            }),
        )
        .await;
    })
    .await;
    let result = RealtimeClient::connect(
        &endpoint,
        &FixedSource { missing: false },
        OPENAI_KEY_NAME,
        &SessionConfig::default(),
    )
    .await;
    server.await.unwrap();
    match result {
        Err(RealtimeError::Server(error)) => {
            assert_eq!(error.param.as_deref(), Some("session.audio.output.voice"));
        }
        Err(other) => panic!("expected a server error, got {other:?}"),
        Ok(_) => panic!("expected a server error, got a session"),
    }
}

#[tokio::test]
async fn unknown_server_events_are_kept() {
    // `rate_limits.updated`, server events page: a published event the client has no type for.
    let rate_limits = json!({
        "event_id": "event_5758",
        "type": "rate_limits.updated",
        "rate_limits": [
            {"name": "requests", "limit": 1000, "remaining": 999, "reset_seconds": 60},
            {"name": "tokens", "limit": 50000, "remaining": 49950, "reset_seconds": 60}
        ]
    });
    let invented = json!({"type": "a.future.event", "event_id": "event_9999", "payload": [1, 2]});
    let replay = (rate_limits.clone(), invented.clone());
    let (endpoint, _, server) = serve(move |mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, replay.0).await;
        send(&mut socket, replay.1).await;
    })
    .await;
    let mut client = connect(&endpoint, &SessionConfig::default()).await;
    let first = client.next_event().await.unwrap();
    let second = client.next_event().await.unwrap();
    server.await.unwrap();
    assert_eq!(
        first,
        ServerEvent::Unknown {
            event_type: "rate_limits.updated".to_owned(),
            raw: rate_limits
        }
    );
    assert_eq!(
        second,
        ServerEvent::Unknown {
            event_type: "a.future.event".to_owned(),
            raw: invented
        }
    );
}

#[tokio::test]
async fn silence_past_the_read_timeout_is_a_stall() {
    let (endpoint, _, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
    })
    .await;
    let endpoint = endpoint.with_read_timeout(Duration::from_millis(150));
    let mut client = connect(&endpoint, &SessionConfig::default()).await;
    let result = client.next_event().await;
    server.await.unwrap();
    match result {
        Err(RealtimeError::Stalled(waited)) => assert_eq!(waited, Duration::from_millis(150)),
        other => panic!("expected a stall, got {other:?}"),
    }
}

#[tokio::test]
async fn a_missing_credential_is_refused_before_connecting() {
    let endpoint = Endpoint::new(MODEL).with_url("ws://127.0.0.1:9/v1/realtime");
    let result = RealtimeClient::connect(
        &endpoint,
        &FixedSource { missing: true },
        OPENAI_KEY_NAME,
        &SessionConfig::default(),
    )
    .await;
    assert!(
        matches!(
            result,
            Err(RealtimeError::Credential(CredentialError::Missing))
        ),
        "{:?}",
        result.err()
    );
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

#[tokio::test]
async fn pcm_frames_are_resampled_to_24_khz_and_committed() {
    let frame = pcm16_frame(16_000, &[1000; 160]);
    let (endpoint, _, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        // `input_audio_buffer.append`, client events page: base64 audio in the session format.
        let append = content(receive(&mut socket).await);
        assert_eq!(append["type"], "input_audio_buffer.append");
        let audio = STANDARD.decode(append["audio"].as_str().unwrap()).unwrap();
        assert_eq!(audio.len(), 240 * 2, "10 ms at 24 kHz is 240 PCM16 samples");
        assert!(
            audio
                .chunks(2)
                .all(|pair| i16::from_le_bytes([pair[0], pair[1]]) == 1000)
        );
        // `input_audio_buffer.commit`, client events page.
        assert_eq!(
            content(receive(&mut socket).await),
            json!({"type": "input_audio_buffer.commit"})
        );
    })
    .await;
    let mut client = connect(&endpoint, &SessionConfig::default()).await;
    client.append_audio(&frame).await.unwrap();
    client.commit_audio().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn mulaw_passes_through_unchanged() {
    let mulaw: Vec<u8> = (0..160u16).map(|index| (index % 256) as u8).collect();
    let expected = STANDARD.encode(&mulaw);
    let (endpoint, _, server) = serve(move |mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            content(receive(&mut socket).await),
            json!({"type": "input_audio_buffer.append", "audio": expected})
        );
    })
    .await;
    let mut client = connect(&endpoint, &telephony_session()).await;
    client.append_encoded(&mulaw).await.unwrap();
    let refused = client.append_audio(&pcm16_frame(8_000, &[0; 80])).await;
    server.await.unwrap();
    assert!(
        matches!(refused, Err(RealtimeError::Audio(_))),
        "a PCM frame into a mu-law session: {refused:?}"
    );
}

#[test]
fn semantic_turn_detection_without_auto_interrupt_or_auto_response() {
    let session = SessionConfig {
        turn_detection: TurnDetection::Semantic {
            eagerness: Eagerness::Low,
            create_response: false,
            interrupt_response: false,
        },
        ..SessionConfig::default()
    };
    // `audio.input.turn_detection`, Semantic VAD, of the session configuration on the client
    // events page (`session.update`).
    assert_eq!(
        session.to_session_json()["audio"]["input"]["turn_detection"],
        json!({
            "type": "semantic_vad",
            "eagerness": "low",
            "create_response": false,
            "interrupt_response": false
        })
    );
}

#[test]
fn the_default_session_is_pcm16_at_24_khz_with_turn_detection_off() {
    assert_eq!(
        SessionConfig::default().to_session_json(),
        json!({
            "type": "realtime",
            "output_modalities": ["audio"],
            "audio": {
                "input": {"format": {"type": "audio/pcm", "rate": 24000}, "turn_detection": null},
                "output": {"format": {"type": "audio/pcm", "rate": 24000}}
            }
        })
    );
}

#[test]
fn an_output_speed_is_sent_under_audio_output_only_when_set() {
    // `audio.output.speed` of the session configuration on the realtime reference
    // (`https://developers.openai.com/api/reference/resources/realtime`, model
    // `realtime_audio_config_output`): a multiple of the original speed, 0.25 to 1.5, 1.0 the
    // default, changed only between model turns.
    let session = SessionConfig {
        voice: Some("marin".to_owned()),
        speed: Some(0.9),
        ..SessionConfig::default()
    };
    assert_eq!(
        session.to_session_json()["audio"]["output"],
        json!({"format": {"type": "audio/pcm", "rate": 24000}, "voice": "marin", "speed": 0.9})
    );
    let unset = SessionConfig::default().to_session_json();
    assert!(unset["audio"]["output"].get("speed").is_none(), "{unset}");
}

#[test]
fn the_model_defaults_to_the_current_ga_realtime_model() {
    assert_eq!(inference_openai::DEFAULT_MODEL, "gpt-realtime-2.1");
    assert_eq!(
        inference_openai::MODEL_VARIABLE,
        "INFERENCE_OPENAI_REALTIME_MODEL"
    );
    assert_eq!(Endpoint::new("m").url, "wss://api.openai.com/v1/realtime");
}

#[test]
fn stereo_float_frames_are_mixed_down_and_converted() {
    let samples: [f32; 4] = [0.5, -0.5, 0.25, 0.25];
    let frame = AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmF32le,
            sample_rate: NonZeroU32::new(24_000).unwrap(),
            channels: NonZeroU16::new(2).unwrap(),
        },
        position: SampleClockPosition::new(0).unwrap(),
        sample_count: 2,
        data: samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect(),
    };
    let pcm = encode_pcm16_24k(&frame).unwrap();
    let decoded: Vec<i16> = pcm
        .chunks(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    assert_eq!(decoded, [0, 8192]);
}

#[test]
fn a_24_khz_mono_pcm16_frame_is_sent_as_is() {
    let samples: Vec<i16> = (0..240).map(|index| index * 97 - 11_000).collect();
    let frame = pcm16_frame(24_000, &samples);
    assert_eq!(encode_pcm16_24k(&frame).unwrap(), frame.data);
}

#[test]
fn a_frame_whose_data_does_not_match_its_sample_count_is_refused() {
    let mut frame = pcm16_frame(16_000, &[0; 16]);
    frame.sample_count = 17;
    assert!(matches!(
        encode_pcm16_24k(&frame),
        Err(RealtimeError::Audio(_))
    ));
}

#[tokio::test]
async fn the_environment_source_reads_only_the_bound_variable() {
    let source = EnvironmentSource::new([
        ("bound".to_owned(), "PATH".to_owned()),
        (
            "unset".to_owned(),
            "INFERENCE_OPENAI_TEST_VARIABLE_NEVER_SET".to_owned(),
        ),
    ]);
    let path = std::env::var("PATH").expect("PATH is set where tests run");
    let resolved = source.resolve("bound").await.unwrap();
    assert!(resolved.expose() == path.as_bytes());
    assert_eq!(format!("{resolved:?}"), "Credential([REDACTED])");
    assert_eq!(
        source.resolve("unset").await.err(),
        Some(CredentialError::Missing)
    );
    assert_eq!(
        source.resolve("PATH").await.err(),
        Some(CredentialError::Missing),
        "a name is never read as a variable of its own"
    );
}

#[test]
fn the_openai_environment_source_binds_the_key_name_to_openai_api_key() {
    assert_eq!(OPENAI_KEY_NAME, "openai-api-key");
    assert_eq!(inference_openai::OPENAI_KEY_VARIABLE, "OPENAI_API_KEY");
    assert_eq!(
        EnvironmentSource::openai(),
        EnvironmentSource::new([("openai-api-key".to_owned(), "OPENAI_API_KEY".to_owned())])
    );
}
