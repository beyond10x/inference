//! The GPT-Live client offline: a local WebSocket server plays the provider. No network and no
//! key: the credential is a fixed test value.
//!
//! Every event the server sends is a documented example of the Live primary WebSocket reference
//! (`https://developers.openai.com/api/reference/resources/live/primary-websocket`), read on
//! 2026-10-20, or one built from its schema where the reference gives no example; every client
//! event the server expects follows the same reference.

#![cfg(feature = "gpt-live")]

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_openai::live::{
    BackendEvent, BackendUsage, CLOSE_TIMEOUT, Delegation, LIVE_MODEL, LIVE_URL, LiveClient,
    LiveClosed, LiveEvent, LiveFormat, LiveSessionConfig, ResponsesBackend,
};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, FunctionTool,
    OPENAI_KEY_NAME, RealtimeError,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

const CREDENTIAL: &str = "offline-test-credential";

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

/// The request line and authorization header the server saw.
#[derive(Debug)]
struct Handshake {
    uri: String,
    authorization: Option<String>,
}

/// A local server running `script`; the endpoint points at it.
#[expect(
    clippy::result_large_err,
    reason = "the handshake callback's error type is tungstenite's, fixed by its `Callback` trait"
)]
async fn serve<F, Fut>(script: F) -> (Endpoint, oneshot::Receiver<Handshake>)
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (seen, handshake) = oneshot::channel();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut seen = Some(seen);
        let socket = tokio_tungstenite::accept_hdr_async(stream, |request: &Request, response| {
            let authorization = request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            if let Some(seen) = seen.take() {
                let _ = seen.send(Handshake {
                    uri: request.uri().to_string(),
                    authorization,
                });
            }
            Ok::<Response, _>(response)
        })
        .await
        .unwrap();
        script(socket).await;
    });
    let endpoint = Endpoint::live(LIVE_MODEL)
        .with_url(format!("ws://{address}/v1/live/sessions"))
        .with_read_timeout(Duration::from_secs(5));
    (endpoint, handshake)
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

/// `session.started` with the documented excerpt.
fn started() -> Value {
    json!({
        "type": "session.started",
        "event_id": "event_started_1",
        "session": {"id": "live_123", "expires_at": 1_788_307_200, "status": "active", "model": "gpt-live-1"}
    })
}

/// Startup: reads `session.start`, answers `session.started`, returns what the client sent.
async fn accept_start(socket: &mut Socket) -> Value {
    let start = receive(socket).await;
    send(socket, started()).await;
    start
}

fn weather() -> FunctionTool {
    FunctionTool {
        name: "weather".to_owned(),
        description: "The weather in a city.".to_owned(),
        parameters: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
    }
}

fn responses_config() -> LiveSessionConfig {
    let mut backend = ResponsesBackend::new("gpt-6-luna");
    backend.instructions = Some("Look things up.".to_owned());
    backend.tools = vec![weather()];
    backend.tool_choice = Some("auto".to_owned());
    LiveSessionConfig {
        instructions: Some("Be concise.".to_owned()),
        voice: Some("marin".to_owned()),
        format: LiveFormat::Pcm24k,
        delegation: Delegation::Responses(backend),
    }
}

#[tokio::test]
async fn session_start_is_the_first_message_and_names_model_format_voice_and_delegation() {
    let (endpoint, handshake) = serve(|mut socket| async move {
        let start = accept_start(&mut socket).await;
        assert_eq!(start["type"], "session.start", "{start}");
        assert_eq!(
            start["session"],
            json!({
                "model": "gpt-live-1",
                "instructions": "Be concise.",
                "audio": {"format": {"type": "audio/pcm", "rate": 24000}, "output": {"voice": "marin"}},
                "delegation": {
                    "type": "responses",
                    "responses": {
                        "model": "gpt-6-luna",
                        "instructions": "Look things up.",
                        "tools": [{
                            "type": "function",
                            "name": "weather",
                            "description": "The weather in a city.",
                            "parameters": {"type": "object", "properties": {"city": {"type": "string"}}},
                            "strict": false
                        }],
                        "tool_choice": "auto"
                    }
                }
            })
        );
        let _ = receive(&mut socket).await;
    })
    .await;
    let client = LiveClient::connect(
        &endpoint,
        &FixedSource,
        OPENAI_KEY_NAME,
        &responses_config(),
    )
    .await
    .expect("the session starts");
    assert_eq!(client.session_id(), Some("live_123"));
    assert_eq!(client.session()["model"], "gpt-live-1");
    let seen = handshake.await.unwrap();
    assert_eq!(
        seen.uri, "/v1/live/sessions",
        "no query: the model is in session.start"
    );
    assert_eq!(
        seen.authorization.as_deref(),
        Some("Bearer offline-test-credential")
    );
    assert_eq!(LIVE_URL, "wss://api.openai.com/v1/live/sessions");
    assert_eq!(Endpoint::live(LIVE_MODEL).url, LIVE_URL);
}

#[test]
fn each_format_and_client_delegation_map_onto_the_session_object() {
    for (format, wire, rate) in [
        (LiveFormat::Pcm24k, "audio/pcm", 24_000),
        (LiveFormat::Pcm16k, "audio/pcm", 16_000),
        (LiveFormat::Pcmu, "audio/pcmu", 8_000),
        (LiveFormat::Pcma, "audio/pcma", 8_000),
    ] {
        let config = LiveSessionConfig {
            format,
            ..LiveSessionConfig::default()
        };
        let session = config.to_session_json("gpt-live-1");
        assert_eq!(
            session,
            json!({
                "model": "gpt-live-1",
                "audio": {"format": {"type": wire, "rate": rate}},
                "delegation": {"type": "client"}
            }),
            "{format:?}"
        );
        assert_eq!(format.sample_rate(), rate);
    }
}

#[tokio::test]
async fn commands_follow_the_reference() {
    let (endpoint, _) = serve(|mut socket| async move {
        let _ = accept_start(&mut socket).await;
        let expected = [
            json!({"type": "session.input_audio.append", "audio": STANDARD.encode([1_u8, 2, 3, 4])}),
            json!({"type": "session.input_audio.mute"}),
            json!({"type": "session.input_audio.unmute"}),
            json!({"type": "session.instructions.append", "delegation_id": null, "content": "Say the disclosure."}),
            json!({"type": "session.thinking.append", "delegation_id": "del_abc123", "content": "Checking."}),
            json!({"type": "session.commentary.append", "delegation_id": "del_abc123", "content": "It is sunny."}),
            json!({"type": "response.item.create", "item": {"type": "function_call_output", "call_id": "call_123", "output": "{\"ok\":true}"}}),
            json!({"type": "response.create"}),
            json!({"type": "session.close"}),
        ];
        for want in expected {
            let mut got = receive(&mut socket).await;
            if let Some(object) = got.as_object_mut() {
                object.remove("event_id");
            }
            assert_eq!(got, want);
        }
    })
    .await;
    let mut client = LiveClient::connect(
        &endpoint,
        &FixedSource,
        OPENAI_KEY_NAME,
        &responses_config(),
    )
    .await
    .expect("the session starts");
    client.append_audio(&[1, 2, 3, 4]).await.unwrap();
    client.mute().await.unwrap();
    client.unmute().await.unwrap();
    client
        .append_instructions(None, "Say the disclosure.")
        .await
        .unwrap();
    client
        .append_thinking(Some("del_abc123"), "Checking.")
        .await
        .unwrap();
    client
        .append_commentary(Some("del_abc123"), "It is sunny.")
        .await
        .unwrap();
    client
        .send_function_output("call_123", "{\"ok\":true}")
        .await
        .unwrap();
    client.create_response().await.unwrap();
    client.request_close().await.unwrap();
}

#[test]
fn documented_server_events_parse_into_typed_events() {
    let parse = |value: Value| LiveEvent::parse(&value.to_string()).expect("a Live event");
    assert_eq!(
        parse(json!({"type": "session.output_audio.delta", "delta": "AACAAIAAAIAAAP9/AIAAgA=="})),
        LiveEvent::OutputAudio {
            audio: STANDARD.decode("AACAAIAAAIAAAP9/AIAAgA==").unwrap()
        }
    );
    assert_eq!(
        parse(
            json!({"type": "session.input_transcript.delta", "event_id": "evt_input_transcript_001", "delta": "A table for two at seven, please.", "start_ms": 1600, "end_ms": 3400})
        ),
        LiveEvent::InputTranscript {
            delta: "A table for two at seven, please.".to_owned(),
            start_ms: 1600,
            end_ms: 3400
        }
    );
    assert_eq!(
        parse(
            json!({"type": "session.output_transcript.delta", "event_id": "evt_output_transcript_001", "delta": "Would you like me to reserve that table?", "start_ms": 5400, "end_ms": 7200})
        ),
        LiveEvent::OutputTranscript {
            delta: "Would you like me to reserve that table?".to_owned(),
            start_ms: 5400,
            end_ms: 7200
        }
    );
    assert_eq!(
        parse(
            json!({"type": "session.delegation.created", "event_id": "evt_delegation_001", "offset_ms": 3600, "delegation": {"id": "del_abc123", "type": "delegation", "target": "client"}})
        ),
        LiveEvent::DelegationCreated {
            delegation_id: "del_abc123".to_owned(),
            target: "client".to_owned(),
            response_id: None,
            offset_ms: 3600
        }
    );
    assert_eq!(
        parse(
            json!({"type": "session.usage.updated", "event_id": "evt_usage_001", "usage": {"seconds": 32.5}, "context_window": {"usage_ratio": 0.12}})
        ),
        LiveEvent::UsageUpdated {
            seconds: 32.5,
            context_usage_ratio: Some(0.12)
        }
    );
    assert_eq!(
        parse(
            json!({"type": "session.closed", "event_id": "evt_closed_001", "client_event_id": "evt_close_001", "reason": "close_requested", "session": {"id": "live_abc123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 45.8}})
        ),
        LiveEvent::SessionClosed {
            reason: Some("close_requested".to_owned()),
            seconds: Some(45.8)
        }
    );
    let LiveEvent::Error(error) = parse(
        json!({"type": "error", "event_id": "evt_error_001", "error": {"type": "invalid_request_error", "code": "unknown_parameter", "message": "Unknown parameter: 'session.voice'.", "param": "session.voice", "client_event_id": "evt_invalid_001"}}),
    ) else {
        panic!("an error event");
    };
    assert_eq!(error.kind, "invalid_request_error");
    assert_eq!(error.code.as_deref(), Some("unknown_parameter"));
    assert_eq!(error.param.as_deref(), Some("session.voice"));
    assert_eq!(error.event_id.as_deref(), Some("evt_invalid_001"));
    assert_eq!(
        parse(
            json!({"type": "session.thinking.appended", "event_id": "evt_thinking_002", "client_event_id": "evt_thinking_001", "start_ms": 4600, "end_ms": 4800})
        ),
        LiveEvent::Acknowledged {
            event_type: "session.thinking.appended".to_owned(),
            client_event_id: Some("evt_thinking_001".to_owned())
        }
    );
    assert_eq!(
        parse(
            json!({"type": "info", "event_id": "evt_info_001", "code": "data_channel_permissions", "message": "restricted"})
        ),
        LiveEvent::Info {
            code: "data_channel_permissions".to_owned(),
            message: "restricted".to_owned()
        }
    );
    assert!(matches!(
        parse(json!({"type": "transport.ringing", "event_id": "e", "session_id": "live_u0_123"})),
        LiveEvent::Unknown { event_type, .. } if event_type == "transport.ringing"
    ));
    assert!(matches!(
        LiveEvent::parse(r#"{"type": "session.output_audio.delta"}"#),
        Err(RealtimeError::Protocol(_))
    ));
}

#[test]
fn nested_responses_events_are_unwrapped() {
    let parse = |event: Value| {
        LiveEvent::parse(
            &json!({"type": "response.event", "event_id": "evt_response_002", "delegation_id": "del_responses123", "event": event})
                .to_string(),
        )
        .expect("a Live event")
    };
    let backend = |event: LiveEvent| match event {
        LiveEvent::Backend {
            delegation_id,
            event,
        } => {
            assert_eq!(delegation_id.as_deref(), Some("del_responses123"));
            event
        }
        other => panic!("a backend event, got {other:?}"),
    };
    assert_eq!(
        backend(parse(
            json!({"type": "response.created", "sequence_number": 0, "response": {"id": "resp_1", "status": "in_progress", "output": []}})
        )),
        BackendEvent::Created {
            response_id: "resp_1".to_owned()
        }
    );
    assert_eq!(
        backend(parse(
            json!({"type": "response.output_item.done", "sequence_number": 5, "output_index": 0, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_123", "name": "weather", "arguments": "{\"city\":\"Lisbon\"}", "status": "completed"}})
        )),
        BackendEvent::FunctionCall {
            item_id: "fc_1".to_owned(),
            call_id: "call_123".to_owned(),
            name: "weather".to_owned(),
            arguments: "{\"city\":\"Lisbon\"}".to_owned()
        }
    );
    assert_eq!(
        backend(parse(
            json!({"type": "response.completed", "sequence_number": 9, "response": {"id": "resp_1", "status": "completed", "output": [], "usage": {"input_tokens": 120, "input_tokens_details": {"cached_tokens": 64}, "output_tokens": 30, "output_tokens_details": {"reasoning_tokens": 12}, "total_tokens": 150}}})
        )),
        BackendEvent::Completed {
            response_id: Some("resp_1".to_owned()),
            usage: Some(BackendUsage {
                input_tokens: 120,
                cached_tokens: Some(64),
                output_tokens: Some(30),
                reasoning_tokens: Some(12),
                total_tokens: Some(150)
            }),
            unreadable_usage: None
        }
    );
    assert_eq!(
        backend(parse(
            json!({"type": "response.failed", "sequence_number": 3, "response": {"id": "resp_2", "status": "failed", "output": []}})
        )),
        BackendEvent::Ended {
            response_id: Some("resp_2".to_owned()),
            status: "failed".to_owned(),
            usage: None,
            unreadable_usage: None
        }
    );
    assert!(matches!(
        backend(parse(
            json!({"type": "response.output_text.delta", "item_id": "msg_abc123", "output_index": 0, "content_index": 0, "delta": "An outdoor table", "sequence_number": 3, "logprobs": []})
        )),
        BackendEvent::Other { event_type, .. } if event_type == "response.output_text.delta"
    ));
    assert!(matches!(
        backend(parse(
            json!({"type": "response.output_item.done", "sequence_number": 4, "output_index": 0, "item": {"type": "message", "id": "msg_1", "role": "assistant", "content": []}})
        )),
        BackendEvent::Other { event_type, .. } if event_type == "response.output_item.done"
    ));
}

#[tokio::test]
async fn events_flow_after_start_and_close_waits_for_session_closed() {
    let (endpoint, _) = serve(|mut socket| async move {
        let _ = accept_start(&mut socket).await;
        send(
            &mut socket,
            json!({"type": "session.output_transcript.delta", "event_id": "e1", "delta": "Hello", "start_ms": 100, "end_ms": 400}),
        )
        .await;
        let close = receive(&mut socket).await;
        assert_eq!(close["type"], "session.close");
        send(
            &mut socket,
            json!({"type": "session.usage.updated", "event_id": "e2", "usage": {"seconds": 3.0}}),
        )
        .await;
        send(
            &mut socket,
            json!({"type": "session.closed", "event_id": "e3", "reason": "close_requested", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 3.5}}),
        )
        .await;
        let _ = socket.close(None).await;
    })
    .await;
    let mut client = LiveClient::connect(
        &endpoint,
        &FixedSource,
        OPENAI_KEY_NAME,
        &LiveSessionConfig::default(),
    )
    .await
    .expect("the session starts");
    assert_eq!(
        client.next_event().await.unwrap(),
        LiveEvent::OutputTranscript {
            delta: "Hello".to_owned(),
            start_ms: 100,
            end_ms: 400
        }
    );
    assert_eq!(
        client.close().await.expect("finalized"),
        LiveClosed {
            reason: Some("close_requested".to_owned()),
            seconds: Some(3.5)
        }
    );
}

#[tokio::test]
async fn an_error_answering_session_start_fails_the_connect() {
    let (endpoint, _) = serve(|mut socket| async move {
        let _ = receive(&mut socket).await;
        send(
            &mut socket,
            json!({"type": "error", "event_id": "evt_error_001", "error": {"type": "invalid_request_error", "code": "model_not_found", "message": "The model does not exist or you do not have access to it."}}),
        )
        .await;
        let _ = socket.next().await;
    })
    .await;
    let refused = LiveClient::connect(
        &endpoint,
        &FixedSource,
        OPENAI_KEY_NAME,
        &LiveSessionConfig::default(),
    )
    .await;
    assert!(
        matches!(&refused, Err(RealtimeError::Server(error)) if error.code.as_deref() == Some("model_not_found")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_model_id_or_url_that_cannot_form_the_request_is_refused_before_connecting() {
    for endpoint in [
        Endpoint::live("gpt live"),
        Endpoint::live(LIVE_MODEL).with_url("ws://127.0.0.1:1/v1/live/sessions?model=gpt-live-1"),
    ] {
        let refused = LiveClient::connect(
            &endpoint,
            &FixedSource,
            OPENAI_KEY_NAME,
            &LiveSessionConfig::default(),
        )
        .await;
        assert!(
            matches!(refused, Err(RealtimeError::InvalidEndpoint(_))),
            "{endpoint:?}"
        );
    }
}

/// `LiveClient::close` waits for `session.closed` at most `CLOSE_TIMEOUT` in all, not per read:
/// a server that answers the close only with messages the client cannot read, one every 100 ms,
/// is given up at that bound, with the connection closed.
#[tokio::test]
async fn close_gives_up_at_its_bound_across_every_read() {
    let (endpoint, _) = serve(|mut socket| async move {
        let _ = accept_start(&mut socket).await;
        let close = receive(&mut socket).await;
        assert_eq!(close["type"], "session.close");
        let until = tokio::time::Instant::now() + CLOSE_TIMEOUT * 3;
        while tokio::time::Instant::now() < until {
            if socket.send(Message::text("not json")).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    let client = LiveClient::connect(
        &endpoint,
        &FixedSource,
        OPENAI_KEY_NAME,
        &LiveSessionConfig::default(),
    )
    .await
    .expect("the session starts");
    let started = tokio::time::Instant::now();
    let closed = tokio::time::timeout(CLOSE_TIMEOUT + Duration::from_secs(3), client.close())
        .await
        .expect("close() outlived its bound");
    assert!(
        matches!(closed, Err(RealtimeError::Stalled(_))),
        "{closed:?}"
    );
    assert!(started.elapsed() >= CLOSE_TIMEOUT - Duration::from_millis(100));
}

/// The two fields of `session.closed` are read each on its own: one the server left out is
/// `None`, and the event is still the session's end.
#[test]
fn a_session_closed_without_its_reason_or_usage_is_still_the_end() {
    assert_eq!(
        LiveEvent::parse(r#"{"type": "session.closed", "event_id": "e"}"#).unwrap(),
        LiveEvent::SessionClosed {
            reason: None,
            seconds: None
        }
    );
    assert_eq!(
        LiveEvent::parse(
            r#"{"type": "session.closed", "event_id": "e", "reason": "expired", "usage": {}}"#
        )
        .unwrap(),
        LiveEvent::SessionClosed {
            reason: Some("expired".to_owned()),
            seconds: None
        }
    );
}

/// A backend `response.completed` or `response.failed` whose `response` lacks its id keeps its
/// usage: the binding sums it onto its own response and never reads the backend's id.
#[test]
fn a_backend_end_without_its_id_keeps_its_usage() {
    let usage = json!({"input_tokens": 5, "output_tokens": 2, "total_tokens": 7});
    let completed = LiveEvent::parse(
        &json!({"type": "response.event", "event_id": "e", "event": {"type": "response.completed", "response": {"usage": usage}}})
            .to_string(),
    )
    .unwrap();
    assert!(
        matches!(
            &completed,
            LiveEvent::Backend { event: BackendEvent::Completed { response_id: None, usage: Some(usage), unreadable_usage: None }, .. }
                if usage.input_tokens == 5 && usage.output_tokens == Some(2)
        ),
        "{completed:?}"
    );
    let failed = LiveEvent::parse(
        &json!({"type": "response.event", "event_id": "e", "event": {"type": "response.failed", "response": {"usage": usage}}})
            .to_string(),
    )
    .unwrap();
    assert!(
        matches!(
            &failed,
            LiveEvent::Backend {
                event: BackendEvent::Ended {
                    response_id: None,
                    usage: Some(_),
                    ..
                },
                ..
            }
        ),
        "{failed:?}"
    );
}

/// A backend usage is read field by field: without `output_tokens` it keeps its input tokens and
/// its output is unknown; without `input_tokens`, or not an object, there is none. Each says
/// what could not be read.
#[test]
fn a_backend_usage_is_read_field_by_field() {
    let read = |usage: Value| {
        let parsed = LiveEvent::parse(
            &json!({"type": "response.event", "event_id": "e", "event": {"type": "response.completed", "response": {"id": "resp_1", "usage": usage}}})
                .to_string(),
        )
        .unwrap();
        match parsed {
            LiveEvent::Backend {
                event:
                    BackendEvent::Completed {
                        usage,
                        unreadable_usage,
                        ..
                    },
                ..
            } => (usage, unreadable_usage),
            other => panic!("{other:?}"),
        }
    };
    let (usage, unreadable) = read(json!({"input_tokens": 20, "total_tokens": 20}));
    assert_eq!(
        usage,
        Some(BackendUsage {
            input_tokens: 20,
            cached_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            total_tokens: Some(20)
        })
    );
    assert!(
        unreadable
            .as_deref()
            .is_some_and(|what| what.contains("`usage.output_tokens`")),
        "{unreadable:?}"
    );
    let (usage, unreadable) = read(json!({"input_tokens": 20}));
    assert_eq!(usage.and_then(|usage| usage.total_tokens), None);
    assert!(unreadable.is_some());
    let (usage, unreadable) = read(json!({"output_tokens": 2}));
    assert_eq!(usage, None);
    assert!(
        unreadable
            .as_deref()
            .is_some_and(|what| what.contains("`usage.input_tokens`")),
        "{unreadable:?}"
    );
    let (usage, unreadable) = read(json!("tokens"));
    assert_eq!(usage, None);
    assert!(
        unreadable
            .as_deref()
            .is_some_and(|what| what.contains("`usage`")),
        "{unreadable:?}"
    );
    assert_eq!(read(Value::Null), (None, None));
}

/// A message that is not JSON is named by the start of its text.
#[test]
fn a_message_that_is_not_json_is_named_by_its_start() {
    let refused = LiveEvent::parse("<html>bad gateway</html>");
    assert!(
        matches!(&refused, Err(RealtimeError::Protocol(detail)) if detail.contains("<html>bad gateway")),
        "{refused:?}"
    );
}
