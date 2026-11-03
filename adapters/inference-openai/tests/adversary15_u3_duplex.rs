//! Adversary cases, pass 15, unit U3, through the duplex binding: a function call without its
//! `name`, named from its `function_call` output item or by nothing. A local WebSocket server
//! replays the provider; no network and no key.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionError, SessionEvent, SpeechSessionHandle,
    SpeechSessionState, TurnDetection, TurnTuning,
};
use inference_openai::duplex::OpenAiRealtime;
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
};
use inference_tools::{DeclareTool, SideEffectClass, Tool};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const WAIT: Duration = Duration::from_millis(100);

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
        let endpoint = Endpoint::new("gpt-realtime-2.1")
            .with_url(format!("ws://{address}/v1/realtime"))
            .with_read_timeout(Duration::from_secs(5));
        Self {
            endpoint,
            runtime,
            task,
        }
    }

    fn model(&self) -> OpenAiRealtime {
        OpenAiRealtime::new(
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
            Message::Text(text) => {
                let mut event: Value = serde_json::from_str(text.as_str()).unwrap();
                if let Some(object) = event.as_object_mut() {
                    object.remove("event_id");
                }
                return event;
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}

async fn accept_session(socket: &mut Socket) -> Value {
    send(
        socket,
        json!({"type": "session.created", "event_id": "event_1", "session": {"type": "realtime", "id": "sess_1"}}),
    )
    .await;
    let update = receive(socket).await;
    assert_eq!(update["type"], "session.update", "{update}");
    let mut confirmed = update["session"].clone();
    confirmed["id"] = json!("sess_1");
    send(
        socket,
        json!({"type": "session.updated", "event_id": "event_2", "session": confirmed}),
    )
    .await;
    update["session"].clone()
}

fn response_created(id: &str) -> Value {
    json!({"type": "response.created", "event_id": "event_rc", "response": {"id": id, "object": "realtime.response", "status": "in_progress", "output": []}})
}

fn response_done(id: &str, status: &str) -> Value {
    json!({"type": "response.done", "event_id": "event_rd", "response": {
        "id": id, "object": "realtime.response", "status": status, "output": [],
        "usage": {
            "total_tokens": 40, "input_tokens": 25, "output_tokens": 15,
            "input_token_details": {"text_tokens": 5, "audio_tokens": 20, "cached_tokens": 4},
            "output_token_details": {"text_tokens": 3, "audio_tokens": 12}
        }
    }})
}

fn output_item(event_type: &str, item: &str, call: &str, name: &str) -> Value {
    json!({"type": event_type, "event_id": "event_oi", "response_id": "resp_1", "output_index": 0, "item": {
        "id": item, "object": "realtime.item", "type": "function_call", "status": "in_progress",
        "call_id": call, "name": name, "arguments": ""
    }})
}

fn nameless_call(item: &str, call: &str) -> Value {
    json!({"type": "response.function_call_arguments.done", "event_id": "event_fc", "response_id": "resp_1", "item_id": item, "output_index": 0, "call_id": call, "arguments": "{\"city\":\"Berlin\"}"})
}

fn pcm24() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn tool(name: &str) -> Tool {
    Tool::declare(DeclareTool {
        name: name.to_owned(),
        schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

fn config(tools: Vec<Tool>) -> SessionConfig {
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm24(),
        output_format: pcm24(),
        turn_detection: TurnDetection::Disabled,
        turn_tuning: TurnTuning::default(),
        auto_response: false,
        auto_interrupt: false,
        tools,
        idle_limit: Duration::from_secs(5),
    }
}

fn until(
    session: &mut impl SpeechSessionHandle,
    stop: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let mut idle = 0;
    while idle < 20 {
        match session.next_event(WAIT) {
            Some(event) => {
                idle = 0;
                let done = stop(&event);
                events.push(event);
                if done {
                    break;
                }
            }
            None => idle += 1,
        }
    }
    events
}

fn any_completed(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::ResponseCompleted(_))
}

/// One response carrying `frames` between its creation and its end, then the binding's close.
fn one_response(
    frames: Vec<Value>,
) -> impl FnOnce(Socket) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    move |mut socket| {
        Box::pin(async move {
            accept_session(&mut socket).await;
            assert_eq!(
                receive(&mut socket).await,
                json!({"type": "response.create"})
            );
            send(&mut socket, response_created("resp_1")).await;
            for frame in frames {
                send(&mut socket, frame).await;
            }
            send(&mut socket, response_done("resp_1", "completed")).await;
            let _ = socket.next().await;
        })
    }
}

/// End to end: the client's fill reaches the binding, which proposes the call of the tool the
/// output item named.
#[test]
fn duplex_a_carried_name_proposes_the_offered_tool() {
    let server = Server::start(one_response(vec![
        output_item(
            "response.output_item.added",
            "item_fc",
            "call_abc",
            "weather",
        ),
        nameless_call("item_fc", "call_abc"),
        output_item(
            "response.output_item.done",
            "item_fc",
            "call_abc",
            "weather",
        ),
    ]));
    let mut model = server.model();
    let tools = vec![tool("weather")];
    let tool_id = tools[0].tool_id();
    let mut session = model.open(config(tools)).expect("the session opens");
    session.create_response().expect("none in progress");
    let events = until(&mut session, any_completed);
    let proposed = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::SpeechToolCallProposed(proposed) => Some(proposed.tool_id),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a proposed call: {events:#?}"));
    assert_eq!(proposed, tool_id);
    assert!(
        !session
            .ledger()
            .dropped_events()
            .iter()
            .any(|dropped| matches!(dropped, SessionError::ToolNotOffered { .. })),
        "{:?}",
        session.ledger().dropped_events()
    );
    session.close().expect("open");
    server.finish();
}

/// A call no one named is a call of no tool, even when the caller declared a tool whose name is
/// the empty string: the brief routes it to the not-offered path, and `Tool::declare` accepts an
/// empty name.
#[test]
fn duplex_a_nameless_call_is_not_offered_when_a_tool_is_named_empty() {
    let server = Server::start(one_response(vec![nameless_call("item_fc", "call_abc")]));
    let mut model = server.model();
    let mut session = model
        .open(config(vec![tool("")]))
        .expect("the session opens");
    session.create_response().expect("none in progress");
    let events = until(&mut session, any_completed);
    let proposed = events
        .iter()
        .any(|event| matches!(event, SessionEvent::SpeechToolCallProposed(_)));
    let dropped = session.ledger().dropped_events().to_vec();
    let state = session.ledger().session().state();
    session.close().expect("open");
    server.finish();
    assert_eq!(state, SpeechSessionState::Open);
    assert!(
        !proposed,
        "a call without any name was proposed as the empty-named tool: {events:#?}"
    );
    assert_eq!(dropped.len(), 1, "{dropped:?}");
}
