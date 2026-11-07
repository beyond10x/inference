//! The Realtime WebSocket session.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use inference_audio::AudioFrame;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::audio::encode_pcm16_24k;
use crate::config::{Deadlines, Endpoint, FunctionTool, SessionConfig, ToolChoice, WireFormat};
use crate::credential::CredentialSource;
use crate::error::{ConnectError, RealtimeError};
use crate::event::ServerEvent;

pub(crate) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The writing half of a [`Socket`].
pub(crate) type SocketSink = SplitSink<Socket, Message>;

/// The reading half of a [`Socket`]. A write pending on its [`SocketSink`] does not hold it: each
/// poll of either half takes the socket only for that poll.
pub(crate) type SocketStream = SplitStream<Socket>;

/// One open Realtime session over WebSocket.
///
/// Every send method writes one client event and returns once it is written; answers arrive as
/// server events through [`RealtimeClient::next_event`]. The client holds no credential after the
/// handshake.
///
/// Every method takes the whole client, so a write the peer does not drain holds up every read
/// until it fails at the write deadline ([`Deadlines::write_timeout`]).
/// [`RealtimeClient::into_split`] gives a [`RealtimeSender`] and a [`RealtimeReceiver`] that two
/// tasks own instead.
pub struct RealtimeClient {
    sender: RealtimeSender,
    receiver: RealtimeReceiver,
}

/// The sending half of a Realtime session, from [`RealtimeClient::into_split`]: every send
/// method of [`RealtimeClient`], and the input format [`RealtimeSender::update_session`] sets and
/// [`RealtimeSender::append_audio`] follows.
///
/// A write it has pending does not hold up its [`RealtimeReceiver`].
pub struct RealtimeSender {
    sink: SocketSink,
    input_format: WireFormat,
    /// The deadline of each write ([`Deadlines::write_timeout`]).
    write_timeout: std::time::Duration,
}

/// The receiving half of a Realtime session, from [`RealtimeClient::into_split`]:
/// [`RealtimeReceiver::next_event`] with the events kept during setup, the function call names
/// read, [`RealtimeReceiver::unparsed_type`], [`RealtimeReceiver::session`] and the read timeout.
pub struct RealtimeReceiver {
    stream: SocketStream,
    read_timeout: std::time::Duration,
    /// When the last message was read, the end of the handshake before the first: the stall
    /// bound runs from it, across reads.
    last_message: tokio::time::Instant,
    session: Value,
    pending: VecDeque<ServerEvent>,
    /// The top-level `type` of the message the last read could not take as a Realtime event,
    /// when that message is JSON whose `type` is a string.
    unparsed_type: Option<String>,
    /// The names of `function_call` output items read, for calls the server sends without one.
    call_names: CallNames,
}

/// A sender and a receiver that [`RealtimeClient::reunite`] (or `LiveClient::reunite`, with
/// feature `gpt-live`) refused because they are not the halves of one client; both are handed
/// back as they were.
pub struct ReuniteError<S, R> {
    /// The sender as it was passed.
    pub sender: S,
    /// The receiver as it was passed.
    pub receiver: R,
}

impl<S, R> fmt::Debug for ReuniteError<S, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReuniteError").finish_non_exhaustive()
    }
}

impl<S, R> fmt::Display for ReuniteError<S, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the sender and the receiver are not halves of one client")
    }
}

impl<S, R> std::error::Error for ReuniteError<S, R> {}

/// The `item.name` of each `function_call` item of a `response.output_item.added` or
/// `response.output_item.done` read, by `item.id`, oldest first and at most [`CallNames::KEPT`]:
/// the oldest is forgotten first, and an entry once the call of its item is read. The ids of the
/// last [`CallNames::KEPT`] items whose call was read are kept too, so an output item that
/// arrives after its call names nothing again.
#[derive(Debug, Default)]
struct CallNames {
    names: VecDeque<(String, String)>,
    read: VecDeque<String>,
}

impl CallNames {
    const KEPT: usize = 64;

    /// Remembers the name of `event`'s item when it is a `function_call` output item with a
    /// non-empty name whose call has not been read.
    fn remember(&mut self, event: &ServerEvent) {
        let ServerEvent::Unknown { event_type, raw } = event else {
            return;
        };
        if !matches!(
            event_type.as_str(),
            "response.output_item.added" | "response.output_item.done"
        ) {
            return;
        }
        let item = &raw["item"];
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        let (Some(id), Some(name)) = (
            item.get("id").and_then(Value::as_str),
            item.get("name").and_then(Value::as_str),
        ) else {
            return;
        };
        if name.is_empty() || self.read.iter().any(|item_id| item_id == id) {
            return;
        }
        if let Some(entry) = self.names.iter_mut().find(|(item_id, _)| item_id == id) {
            name.clone_into(&mut entry.1);
            return;
        }
        bounded_push(&mut self.names, (id.to_owned(), name.to_owned()));
    }

    /// `event`, a function call without its name named from the item of its `item_id`. The
    /// entry of that item is forgotten once its call is read, named or not, and the item is not
    /// remembered again.
    fn fill(&mut self, mut event: ServerEvent) -> ServerEvent {
        if let ServerEvent::FunctionCall { item_id, name, .. } = &mut event {
            if let Some(at) = self.names.iter().position(|(item, _)| item == item_id)
                && let Some((_, remembered)) = self.names.remove(at)
            {
                name.get_or_insert(remembered);
            }
            if !self.read.iter().any(|read| read == item_id) {
                bounded_push(&mut self.read, item_id.clone());
            }
        }
        event
    }
}

/// Appends `entry` to `queue`, first forgetting its oldest entry when it holds
/// [`CallNames::KEPT`].
fn bounded_push<T>(queue: &mut VecDeque<T>, entry: T) {
    if queue.len() == CallNames::KEPT {
        queue.pop_front();
    }
    queue.push_back(entry);
}

impl fmt::Debug for RealtimeClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RealtimeClient")
            .field("read_timeout", &self.receiver.read_timeout)
            .field("input_format", &self.sender.input_format)
            .field("pending", &self.receiver.pending.len())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for RealtimeSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RealtimeSender")
            .field("input_format", &self.input_format)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for RealtimeReceiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RealtimeReceiver")
            .field("read_timeout", &self.read_timeout)
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

impl RealtimeClient {
    /// Opens a session: resolves the secret `credential` names through `source`, connects to
    /// `<endpoint.url>?model=<endpoint.model>` with it as the bearer token, waits for
    /// `session.created`, sends `session` as `session.update` and waits for `session.updated`.
    /// Any other server event that arrives during setup is kept and returned first by
    /// [`RealtimeClient::next_event`].
    ///
    /// The credential is resolved before any connection is made, is sent only in the opening
    /// handshake and is never part of an error. The URL is checked before the credential is
    /// resolved: `wss://`, or `ws://` only to a loopback host, since `ws://` sends the key in the
    /// clear. The opening handshake takes at most [`DEFAULT_CONNECT_TIMEOUT`](crate::DEFAULT_CONNECT_TIMEOUT),
    /// and each write at most [`DEFAULT_WRITE_TIMEOUT`](crate::DEFAULT_WRITE_TIMEOUT);
    /// [`RealtimeClient::connect_with`] takes other [`Deadlines`].
    ///
    /// # Errors
    /// [`RealtimeError::Credential`] when the source refuses or the credential is not a valid
    /// header value; [`RealtimeError::InvalidEndpoint`] for a URL that is neither `wss://` nor
    /// `ws://` to a loopback host, before anything is resolved or connected;
    /// [`RealtimeError::Connect`], without a status when the handshake passed its deadline;
    /// [`RealtimeError::Server`] when the server answers setup with `error`; and the errors of
    /// [`RealtimeClient::next_event`].
    pub async fn connect(
        endpoint: &Endpoint,
        source: &dyn CredentialSource,
        credential: &str,
        session: &SessionConfig,
    ) -> Result<Self, RealtimeError> {
        Self::connect_with(endpoint, Deadlines::default(), source, credential, session).await
    }

    /// [`RealtimeClient::connect`] under `deadlines`: the opening handshake takes at most
    /// [`Deadlines::connect_timeout`], and each write of the session at most
    /// [`Deadlines::write_timeout`].
    ///
    /// # Errors
    /// The errors of [`RealtimeClient::connect`].
    pub async fn connect_with(
        endpoint: &Endpoint,
        deadlines: Deadlines,
        source: &dyn CredentialSource,
        credential: &str,
        session: &SessionConfig,
    ) -> Result<Self, RealtimeError> {
        let url = request_url(endpoint)?;
        let (sink, stream) = open_socket(&url, source, credential, deadlines.connect_timeout())
            .await?
            .split();

        let mut sender = RealtimeSender {
            sink,
            input_format: session.input_format,
            write_timeout: deadlines.write_timeout(),
        };
        let mut receiver = RealtimeReceiver {
            stream,
            read_timeout: endpoint.read_timeout,
            last_message: tokio::time::Instant::now(),
            session: Value::Null,
            pending: VecDeque::new(),
            unparsed_type: None,
            call_names: CallNames::default(),
        };
        receiver
            .await_setup(|event| matches!(event, ServerEvent::SessionCreated { .. }))
            .await?;
        sender.update_session(session).await?;
        if let ServerEvent::SessionUpdated { session } = receiver
            .await_setup(|event| matches!(event, ServerEvent::SessionUpdated { .. }))
            .await?
        {
            receiver.session = session;
        }
        Ok(Self { sender, receiver })
    }

    /// Splits the session into a sender, which every send method of the client is on, and a
    /// receiver, which reads the server events, so that two tasks own them: a write the peer
    /// does not drain then holds up no read. The receiver returns the events kept during setup
    /// first and holds the session, the read timeout and what the client remembered of the reads
    /// so far; the sender holds the input format.
    ///
    /// [`RealtimeClient::reunite`] puts the halves back together, for
    /// [`RealtimeClient::close`].
    pub fn into_split(self) -> (RealtimeSender, RealtimeReceiver) {
        (self.sender, self.receiver)
    }

    /// The client `sender` and `receiver` were split from by [`RealtimeClient::into_split`].
    ///
    /// # Errors
    /// A boxed [`ReuniteError`], handing both halves back, when they are not the halves of one
    /// client.
    pub fn reunite(
        sender: RealtimeSender,
        receiver: RealtimeReceiver,
    ) -> Result<Self, Box<ReuniteError<RealtimeSender, RealtimeReceiver>>> {
        if sender.sink.is_pair_of(&receiver.stream) {
            Ok(Self { sender, receiver })
        } else {
            Err(Box::new(ReuniteError { sender, receiver }))
        }
    }

    /// The session as the server confirmed it in `session.updated` during
    /// [`RealtimeClient::connect`].
    pub fn session(&self) -> &Value {
        self.receiver.session()
    }

    /// The top-level `type` of the message the last read from the socket could not take as a
    /// Realtime event ([`RealtimeError::Protocol`]), when that message is JSON whose `type` is a
    /// string; `None` after a read that gave an event, and for a message without a readable
    /// `type`: not JSON, a binary message, or no string `type`.
    pub fn unparsed_type(&self) -> Option<&str> {
        self.receiver.unparsed_type()
    }

    /// Sends `session.update` with `session`. Later appends follow its input format; the
    /// server's `session.updated` arrives through [`RealtimeClient::next_event`].
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn update_session(&mut self, session: &SessionConfig) -> Result<(), RealtimeError> {
        self.sender.update_session(session).await
    }

    /// Sends `input_audio_buffer.append` with `frame` as PCM16 at 24 kHz, resampled and mixed
    /// down by [`encode_pcm16_24k`].
    ///
    /// # Errors
    /// [`RealtimeError::Audio`] when the session's input format is not PCM16 (G.711 goes through
    /// [`RealtimeClient::append_encoded`]) or the frame is malformed;
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn append_audio(&mut self, frame: &AudioFrame) -> Result<(), RealtimeError> {
        self.sender.append_audio(frame).await
    }

    /// Sends `input_audio_buffer.append` with `audio` as it is: bytes already in the session's
    /// input format, such as G.711 mu-law at 8 kHz from a telephone line.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn append_encoded(&mut self, audio: &[u8]) -> Result<(), RealtimeError> {
        self.sender.append_encoded(audio).await
    }

    /// Sends `input_audio_buffer.commit`: the buffered input becomes a user item. With turn
    /// detection off a response still needs [`RealtimeClient::create_response`].
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn commit_audio(&mut self) -> Result<(), RealtimeError> {
        self.sender.commit_audio().await
    }

    /// Sends `input_audio_buffer.clear`.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn clear_audio(&mut self) -> Result<(), RealtimeError> {
        self.sender.clear_audio().await
    }

    /// Writes one WebSocket ping frame carrying `payload`, a keepalive for an idle session.
    ///
    /// The caller schedules pings; the client runs no timer. The server's pong is not a server
    /// event: [`RealtimeClient::next_event`] skips it, and it does not reset the read timeout.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when `payload` is longer than the 125 bytes a control frame
    /// carries (RFC 6455, section 5.5), refused before anything is written, or when the write
    /// fails.
    pub async fn ping(&mut self, payload: &[u8]) -> Result<(), RealtimeError> {
        self.sender.ping(payload).await
    }

    /// Sends `conversation.item.create` with `item` as its `item`, and with `options.event_id` as
    /// the event's `event_id` and `options.previous_item_id` as its `previous_item_id`, each at the
    /// top level of the event and left out when `None`. The server's answer, and an `error`
    /// naming the `event_id` when the event is refused, arrive through
    /// [`RealtimeClient::next_event`].
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn create_item(
        &mut self,
        item: &ConversationItem,
        options: &ItemOptions,
    ) -> Result<(), RealtimeError> {
        self.sender.create_item(item, options).await
    }

    /// Sends `conversation.item.create` with a user message of one `input_text` part: a
    /// [`ConversationItem::UserText`] through [`RealtimeClient::create_item`] with no
    /// `event_id` and no `previous_item_id`.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn send_text(&mut self, text: &str) -> Result<(), RealtimeError> {
        self.sender.send_text(text).await
    }

    /// Sends `conversation.item.create` with a `function_call_output` item answering `call_id`:
    /// a [`ConversationItem::FunctionCallOutput`] through [`RealtimeClient::create_item`] with no
    /// `event_id` and no `previous_item_id`.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn send_function_output(
        &mut self,
        call_id: &str,
        output: &str,
    ) -> Result<(), RealtimeError> {
        self.sender.send_function_output(call_id, output).await
    }

    /// Sends `response.create` with no `event_id` and no `response` object, exactly
    /// `{"type": "response.create"}`: the response takes its instructions, tool choice and tools
    /// from the session, reads the conversation's items and joins the default conversation. It is
    /// [`RealtimeClient::create_response_with`] with [`ResponseOptions::default`].
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn create_response(&mut self) -> Result<(), RealtimeError> {
        self.sender.create_response().await
    }

    /// Sends `response.create` with `options.event_id` as the event's top-level `event_id`, and a
    /// `response` object holding only the options that are set: `instructions`, `tool_choice`,
    /// `conversation`, `input` (an array of item objects) and `tools` (an array of function
    /// tools). Each applies to this response only and leaves the session's configuration as it
    /// is. When no option inside `response` is set, no `response` key is sent.
    ///
    /// A response with [`ResponseConversation::None`] is out of band: its output items are not
    /// added to the default conversation. Its server events still arrive through
    /// [`RealtimeClient::next_event`], told apart from other responses' by their `response_id`.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn create_response_with(
        &mut self,
        options: &ResponseOptions,
    ) -> Result<(), RealtimeError> {
        self.sender.create_response_with(options).await
    }

    /// Sends `response.cancel`, for `response_id` or, when `None`, the response in progress. The
    /// server answers with `response.done` of status `cancelled`.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn cancel_response(
        &mut self,
        response_id: Option<&str>,
    ) -> Result<(), RealtimeError> {
        self.sender.cancel_response(response_id).await
    }

    /// Sends `conversation.item.truncate`: the assistant audio of `item_id` is cut at
    /// `audio_end_ms`, the position the listener heard, and its transcript with it.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn truncate(
        &mut self,
        item_id: &str,
        content_index: u32,
        audio_end_ms: u32,
    ) -> Result<(), RealtimeError> {
        self.sender
            .truncate(item_id, content_index, audio_end_ms)
            .await
    }

    /// The next server event, events kept during setup first.
    ///
    /// The read timeout is a stall bound that holds across calls: it runs from the last message
    /// read (a text or binary message; pings and other control frames do not count), from the end
    /// of the handshake before the first, not from this call. A caller that drops the read (a
    /// `select!`, or a shorter timeout around it) and calls again still sees the stall once the
    /// read timeout has passed since the last message. A message that already arrived is returned
    /// even when the bound has passed. After a stall the bound runs again from the stall.
    ///
    /// # Errors
    /// [`RealtimeError::Stalled`] when no message arrived for the endpoint's read timeout, pings
    /// and other frames that are not messages counted in it;
    /// [`RealtimeError::Closed`] when the server closed the connection;
    /// [`RealtimeError::Transport`] when the read fails; [`RealtimeError::Protocol`] for a
    /// message that is not a Realtime event.
    pub async fn next_event(&mut self) -> Result<ServerEvent, RealtimeError> {
        self.receiver.next_event().await
    }

    /// Sends a close frame and waits for the server's. Writing the frame and waiting for the
    /// server's together take at most the read timeout, counted from this call.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the close handshake fails;
    /// [`RealtimeError::Stalled`] when the server does not close within the read timeout, or
    /// does not drain the close frame within it.
    pub async fn close(mut self) -> Result<(), RealtimeError> {
        close_socket(
            &mut self.sender.sink,
            &mut self.receiver.stream,
            self.receiver.read_timeout,
        )
        .await
    }
}

impl RealtimeSender {
    /// The deadline of each write on this half ([`Deadlines::write_timeout`]).
    pub fn write_timeout(&self) -> std::time::Duration {
        self.write_timeout
    }

    /// [`RealtimeClient::update_session`] on this half: sends `session.update`, and later
    /// appends follow its input format.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn update_session(&mut self, session: &SessionConfig) -> Result<(), RealtimeError> {
        self.send(json!({"type": "session.update", "session": session.to_session_json()}))
            .await?;
        self.input_format = session.input_format;
        Ok(())
    }

    /// [`RealtimeClient::append_audio`] on this half.
    ///
    /// # Errors
    /// [`RealtimeError::Audio`] when the session's input format is not PCM16 or the frame is
    /// malformed; [`RealtimeError::Transport`] when the write fails.
    pub async fn append_audio(&mut self, frame: &AudioFrame) -> Result<(), RealtimeError> {
        if self.input_format != WireFormat::Pcm16 {
            return Err(RealtimeError::Audio(format!(
                "the session takes {:?} input; send it encoded with append_encoded",
                self.input_format
            )));
        }
        let pcm = encode_pcm16_24k(frame)?;
        self.append_encoded(&pcm).await
    }

    /// [`RealtimeClient::append_encoded`] on this half.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn append_encoded(&mut self, audio: &[u8]) -> Result<(), RealtimeError> {
        self.send(json!({"type": "input_audio_buffer.append", "audio": STANDARD.encode(audio)}))
            .await
    }

    /// [`RealtimeClient::commit_audio`] on this half.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn commit_audio(&mut self) -> Result<(), RealtimeError> {
        self.send(json!({"type": "input_audio_buffer.commit"}))
            .await
    }

    /// [`RealtimeClient::clear_audio`] on this half.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn clear_audio(&mut self) -> Result<(), RealtimeError> {
        self.send(json!({"type": "input_audio_buffer.clear"})).await
    }

    /// [`RealtimeClient::ping`] on this half; the server's pong is skipped by the receiver.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when `payload` is longer than 125 bytes, refused before
    /// anything is written, or when the write fails.
    pub async fn ping(&mut self, payload: &[u8]) -> Result<(), RealtimeError> {
        send_ping(&mut self.sink, payload, self.write_timeout).await
    }

    /// [`RealtimeClient::create_item`] on this half.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn create_item(
        &mut self,
        item: &ConversationItem,
        options: &ItemOptions,
    ) -> Result<(), RealtimeError> {
        let mut event = json!({"type": "conversation.item.create", "item": item.to_json()});
        if let Some(event_id) = &options.event_id {
            event["event_id"] = json!(event_id);
        }
        if let Some(previous_item_id) = &options.previous_item_id {
            event["previous_item_id"] = json!(previous_item_id);
        }
        self.send(event).await
    }

    /// [`RealtimeClient::send_text`] on this half.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn send_text(&mut self, text: &str) -> Result<(), RealtimeError> {
        self.create_item(
            &ConversationItem::UserText(text.to_owned()),
            &ItemOptions::default(),
        )
        .await
    }

    /// [`RealtimeClient::send_function_output`] on this half.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn send_function_output(
        &mut self,
        call_id: &str,
        output: &str,
    ) -> Result<(), RealtimeError> {
        self.create_item(
            &ConversationItem::FunctionCallOutput {
                call_id: call_id.to_owned(),
                output: output.to_owned(),
            },
            &ItemOptions::default(),
        )
        .await
    }

    /// [`RealtimeClient::create_response`] on this half.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn create_response(&mut self) -> Result<(), RealtimeError> {
        self.create_response_with(&ResponseOptions::default()).await
    }

    /// [`RealtimeClient::create_response_with`] on this half.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn create_response_with(
        &mut self,
        options: &ResponseOptions,
    ) -> Result<(), RealtimeError> {
        let mut event = json!({"type": "response.create"});
        if let Some(event_id) = &options.event_id {
            event["event_id"] = json!(event_id);
        }
        let response = options.response_json();
        if !response.is_empty() {
            event["response"] = Value::Object(response);
        }
        self.send(event).await
    }

    /// [`RealtimeClient::cancel_response`] on this half.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn cancel_response(
        &mut self,
        response_id: Option<&str>,
    ) -> Result<(), RealtimeError> {
        let mut event = json!({"type": "response.cancel"});
        if let Some(response_id) = response_id {
            event["response_id"] = json!(response_id);
        }
        self.send(event).await
    }

    /// [`RealtimeClient::truncate`] on this half.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn truncate(
        &mut self,
        item_id: &str,
        content_index: u32,
        audio_end_ms: u32,
    ) -> Result<(), RealtimeError> {
        self.send(json!({
            "type": "conversation.item.truncate",
            "item_id": item_id,
            "content_index": content_index,
            "audio_end_ms": audio_end_ms,
        }))
        .await
    }

    async fn send(&mut self, event: Value) -> Result<(), RealtimeError> {
        send_json(&mut self.sink, &event, self.write_timeout).await
    }
}

impl RealtimeReceiver {
    /// Reads until an event `wanted` accepts, keeping every other event for
    /// [`RealtimeReceiver::next_event`]; an `error` event fails setup.
    async fn await_setup(
        &mut self,
        wanted: impl Fn(&ServerEvent) -> bool,
    ) -> Result<ServerEvent, RealtimeError> {
        loop {
            let event = self.read_event().await?;
            if wanted(&event) {
                return Ok(event);
            }
            if let ServerEvent::Error(error) = event {
                return Err(RealtimeError::Server(error));
            }
            self.pending.push_back(event);
        }
    }

    /// [`RealtimeClient::session`]: the session as `session.updated` confirmed it during setup.
    pub fn session(&self) -> &Value {
        &self.session
    }

    /// [`RealtimeClient::unparsed_type`]: the top-level `type` of the message the last read
    /// could not take as a Realtime event, when it is JSON whose `type` is a string.
    pub fn unparsed_type(&self) -> Option<&str> {
        self.unparsed_type.as_deref()
    }

    /// [`RealtimeClient::next_event`] on this half: the next server event, events kept during
    /// setup first. A write pending on the sender does not hold it up. The stall bound runs from
    /// the last message this half read, across calls, as [`RealtimeClient::next_event`] states.
    ///
    /// # Errors
    /// The errors of [`RealtimeClient::next_event`].
    pub async fn next_event(&mut self) -> Result<ServerEvent, RealtimeError> {
        match self.pending.pop_front() {
            Some(event) => Ok(event),
            None => self.read_event().await,
        }
    }

    async fn read_event(&mut self) -> Result<ServerEvent, RealtimeError> {
        self.unparsed_type = None;
        let text = read_text(&mut self.stream, self.read_timeout, &mut self.last_message).await?;
        match ServerEvent::parse(&text) {
            Ok(event) => {
                self.call_names.remember(&event);
                Ok(self.call_names.fill(event))
            }
            Err(error) => {
                self.unparsed_type = top_level_type(&text);
                Err(error)
            }
        }
    }
}

/// Checks `url` ([`check_url`]), resolves the secret `credential` names through `source` and
/// opens a WebSocket to `url` with it as the bearer token, the whole opening handshake (TCP, TLS
/// for `wss://`, the HTTP upgrade) taking at most `connect_timeout`. The credential is resolved
/// after the URL is checked and before any connection is made, is sent only in the opening
/// handshake, marked sensitive, and is never part of an error.
pub(crate) async fn open_socket(
    url: &str,
    source: &dyn CredentialSource,
    credential: &str,
    connect_timeout: std::time::Duration,
) -> Result<Socket, RealtimeError> {
    let tls = check_url(url)?;
    let resolved = source
        .resolve(credential)
        .await
        .map_err(RealtimeError::Credential)?;
    let header = resolved
        .into_header(b"Bearer ")
        .map_err(RealtimeError::Credential)?;

    let mut request = url
        .into_client_request()
        .map_err(|error| RealtimeError::InvalidEndpoint(error.to_string()))?;
    request.headers_mut().insert("authorization", header);
    let connector = if tls {
        Some(Connector::Rustls(tls_config()?))
    } else {
        None
    };
    let handshake =
        tokio_tungstenite::connect_async_tls_with_config(request, None, true, connector);
    let (socket, _) = within(tokio::time::Instant::now(), connect_timeout, handshake)
        .await
        .ok_or_else(|| {
            RealtimeError::Connect(ConnectError {
                status: None,
                detail: format!(
                    "the opening handshake did not complete within {connect_timeout:?}"
                ),
            })
        })?
        .map_err(|error| RealtimeError::Connect(handshake_error(&error)))?;
    Ok(socket)
}

/// Whether `url` takes TLS: `true` for `wss://`, `false` for `ws://` to a loopback host
/// ([`is_loopback`]). `ws://` sends the bearer key in the clear, so any other host is refused, and
/// so is any other scheme; the scheme is read without regard to case.
///
/// # Errors
/// [`RealtimeError::InvalidEndpoint`], naming the rule and never the credential.
pub(crate) fn check_url(url: &str) -> Result<bool, RealtimeError> {
    let refused = |detail: &str| Err(RealtimeError::InvalidEndpoint(detail.to_owned()));
    let Some((scheme, rest)) = url.split_once("://") else {
        return refused("the URL has no scheme; it must be wss://, or ws:// to a loopback host");
    };
    if scheme.eq_ignore_ascii_case("wss") {
        Ok(true)
    } else if scheme.eq_ignore_ascii_case("ws") {
        if is_loopback(rest) {
            Ok(false)
        } else {
            refused(
                "ws:// would send the key in the clear; it is taken only to a loopback host, use wss://",
            )
        }
    } else {
        refused("the scheme must be wss://, or ws:// to a loopback host")
    }
}

/// Whether the authority at the start of `rest`, what follows `ws://`, names a loopback host:
/// `localhost`, an IPv4 address in `127.0.0.0/8` or `[::1]`, with or without a port. A user
/// part (`user@host`) is refused, since it could make a host read as another.
pub(crate) fn is_loopback(rest: &str) -> bool {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return false;
    }
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => match bracketed.split_once(']') {
            Some((host, after)) if after.is_empty() || after.starts_with(':') => host,
            _ => return false,
        },
        None => authority.split(':').next().unwrap_or_default(),
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// `future`'s output, or `None` when `start + bound` passes first. A sum too large for an
/// `Instant` is a bound that never passes, so `future` then runs without a deadline.
pub(crate) async fn within<F: Future>(
    start: tokio::time::Instant,
    bound: std::time::Duration,
    future: F,
) -> Option<F::Output> {
    match start.checked_add(bound) {
        Some(deadline) => tokio::time::timeout_at(deadline, future).await.ok(),
        None => Some(future.await),
    }
}

/// The text of the next text message, waiting at most until `last_message + read_timeout`, the
/// stall bound, however many calls it spans: `last_message` is when the reader last read a text
/// or binary message (the end of the handshake before the first), and each such message read
/// moves it. Pings and other frames that are not messages count in the wait and move nothing. A
/// message that already arrived is returned even when the bound has passed, since the socket is
/// polled before the bound. A stall moves `last_message` to the stall, so the next read waits one
/// read timeout more instead of failing at once. Cancel safe: `last_message` moves only together
/// with the message or the stall that is returned.
pub(crate) async fn read_text(
    socket: &mut SocketStream,
    read_timeout: std::time::Duration,
    last_message: &mut tokio::time::Instant,
) -> Result<String, RealtimeError> {
    loop {
        let Some(message) = within(*last_message, read_timeout, socket.next()).await else {
            *last_message = tokio::time::Instant::now();
            return Err(RealtimeError::Stalled(read_timeout));
        };
        let message = match message {
            None => {
                return Err(RealtimeError::Closed {
                    code: None,
                    reason: String::new(),
                });
            }
            Some(message) => {
                message.map_err(|error| RealtimeError::Transport(error.to_string()))?
            }
        };
        match message {
            Message::Text(text) => {
                *last_message = tokio::time::Instant::now();
                return Ok(text.as_str().to_owned());
            }
            Message::Close(frame) => {
                return Err(RealtimeError::Closed {
                    code: frame.as_ref().map(|frame| u16::from(frame.code)),
                    reason: frame
                        .map(|frame| frame.reason.as_str().to_owned())
                        .unwrap_or_default(),
                });
            }
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            Message::Binary(data) => {
                *last_message = tokio::time::Instant::now();
                return Err(RealtimeError::Protocol(format!(
                    "a binary message of {} bytes; the API sends text",
                    data.len()
                )));
            }
        }
    }
}

/// Writes `event` as one text message, taking at most `write_timeout` ([`write_within`]).
pub(crate) async fn send_json(
    socket: &mut SocketSink,
    event: &Value,
    write_timeout: std::time::Duration,
) -> Result<(), RealtimeError> {
    write_within(socket, Message::text(event.to_string()), write_timeout).await
}

/// Writes `message` on `socket`, failing as [`RealtimeError::Transport`] when the write has not
/// completed within `write_timeout` of the call: a peer that stops draining the connection fails
/// the write instead of holding it. A write that passes its deadline may leave part of the
/// message on the connection. A bound too large to add to the current instant is no bound.
async fn write_within(
    socket: &mut SocketSink,
    message: Message,
    write_timeout: std::time::Duration,
) -> Result<(), RealtimeError> {
    within(
        tokio::time::Instant::now(),
        write_timeout,
        socket.send(message),
    )
    .await
    .ok_or_else(|| {
        RealtimeError::Transport(format!(
            "the write did not complete within {write_timeout:?}; the peer is not reading"
        ))
    })?
    .map_err(|error| RealtimeError::Transport(error.to_string()))
}

/// The most payload a ping frame carries: RFC 6455, section 5.5, caps every control frame's
/// payload at 125 bytes.
pub(crate) const MAX_PING_PAYLOAD: usize = 125;

/// Writes one ping frame carrying `payload` on `socket`; a payload over [`MAX_PING_PAYLOAD`] is
/// refused before anything is written.
pub(crate) async fn send_ping(
    socket: &mut SocketSink,
    payload: &[u8],
    write_timeout: std::time::Duration,
) -> Result<(), RealtimeError> {
    if payload.len() > MAX_PING_PAYLOAD {
        return Err(RealtimeError::Transport(format!(
            "a ping payload of {} bytes; a control frame carries at most {MAX_PING_PAYLOAD}",
            payload.len()
        )));
    }
    write_within(
        socket,
        Message::Ping(payload.to_vec().into()),
        write_timeout,
    )
    .await
}

/// One item of `conversation.item.create`'s `item`, as the client events reference gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationItem {
    /// A `message` item with `role: "user"` and one content part of type `input_text`.
    UserText(String),
    /// A `message` item with `role: "system"` and one content part of type `input_text`:
    /// instructions in context, for example in the `input` of an out-of-band response
    /// ([`ResponseConversation::None`]), which then joins no conversation.
    SystemText(String),
    /// A `message` item with `role: "assistant"` and one content part of type `output_text`: a
    /// line the application already played or scripts, added to the conversation as said.
    AssistantText(String),
    /// A `function_call_output` item: `output` answers the function call `call_id`.
    FunctionCallOutput {
        /// The `call_id` of the function call this answers.
        call_id: String,
        /// The `output`, the function's result as the model reads it, usually JSON text.
        output: String,
    },
}

impl ConversationItem {
    /// The `item` object of `conversation.item.create`.
    pub(crate) fn to_json(&self) -> Value {
        match self {
            Self::UserText(text) => json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": text}],
            }),
            Self::SystemText(text) => json!({
                "type": "message",
                "role": "system",
                "content": [{"type": "input_text", "text": text}],
            }),
            Self::AssistantText(text) => json!({
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": text}],
            }),
            Self::FunctionCallOutput { call_id, output } => {
                json!({"type": "function_call_output", "call_id": call_id, "output": output})
            }
        }
    }
}

/// The top-level fields of `conversation.item.create` beside its `item`; each is left out of the
/// event when `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemOptions {
    /// The client event's `event_id`, which a server `error` caused by this event names.
    pub event_id: Option<String>,
    /// The `previous_item_id`: the id of the item the new item is inserted after.
    pub previous_item_id: Option<String>,
}

/// Which conversation a response's output joins, `response.conversation` of `response.create`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResponseConversation {
    /// `"auto"`: the output is added to the default conversation, as without the option.
    Auto,
    /// `"none"`: an out-of-band response; its output items are not added to the default
    /// conversation.
    None,
}

impl ResponseConversation {
    /// The `conversation` value in the reference's shape: `"auto"` or `"none"`.
    fn to_json(self) -> Value {
        match self {
            Self::Auto => json!("auto"),
            Self::None => json!("none"),
        }
    }
}

/// The per-response options of `response.create`, for [`RealtimeClient::create_response_with`].
/// `event_id` goes at the top level of the event, every other field into its `response` object;
/// each is left out when `None`, with the meaning its field states.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResponseOptions {
    /// The client event's `event_id`, which a server `error` caused by this event names; `None`
    /// sends no `event_id`.
    pub event_id: Option<String>,
    /// `response.instructions`: the instructions for this response only; `None` keeps the
    /// session's `instructions`.
    pub instructions: Option<String>,
    /// `response.tool_choice`: which tools the model may call in this response; `None` keeps the
    /// session's `tool_choice`.
    pub tool_choice: Option<ToolChoice>,
    /// `response.conversation`: `"auto"`, or `"none"` for an out-of-band response; `None` adds
    /// the output to the default conversation, as `"auto"` does.
    pub conversation: Option<ResponseConversation>,
    /// `response.input`: the items the response reads, as an array of item objects in the shape
    /// of `conversation.item.create`'s `item`; `None` has the response read the conversation's
    /// items.
    pub input: Option<Vec<ConversationItem>>,
    /// `response.tools`: the function tools of this response; `None` keeps the session's
    /// `tools`. `Some(vec![])` sends `[]`, which gives this response no tools whatever the
    /// session holds.
    pub tools: Option<Vec<FunctionTool>>,
}

impl ResponseOptions {
    /// The `response` object of `response.create`: only the options that are set.
    fn response_json(&self) -> serde_json::Map<String, Value> {
        let mut response = serde_json::Map::new();
        if let Some(instructions) = &self.instructions {
            response.insert("instructions".to_owned(), json!(instructions));
        }
        if let Some(tool_choice) = &self.tool_choice {
            response.insert("tool_choice".to_owned(), tool_choice.to_json());
        }
        if let Some(conversation) = self.conversation {
            response.insert("conversation".to_owned(), conversation.to_json());
        }
        if let Some(input) = &self.input {
            let items = input.iter().map(ConversationItem::to_json).collect();
            response.insert("input".to_owned(), Value::Array(items));
        }
        if let Some(tools) = &self.tools {
            let tools = tools.iter().map(FunctionTool::to_json).collect();
            response.insert("tools".to_owned(), Value::Array(tools));
        }
        response
    }
}

/// Sends a close frame on `sink` and waits for the server's on `socket`, the reading half of the
/// same socket; the write and the wait together take at most `read_timeout`.
pub(crate) async fn close_socket(
    sink: &mut SocketSink,
    socket: &mut SocketStream,
    read_timeout: std::time::Duration,
) -> Result<(), RealtimeError> {
    use tokio_tungstenite::tungstenite::Error;
    let started = tokio::time::Instant::now();
    within(started, read_timeout, sink.close())
        .await
        .ok_or(RealtimeError::Stalled(read_timeout))?
        .map_err(|error| RealtimeError::Transport(error.to_string()))?;
    loop {
        match within(started, read_timeout, socket.next()).await {
            None => return Err(RealtimeError::Stalled(read_timeout)),
            Some(None | Some(Err(Error::ConnectionClosed | Error::AlreadyClosed))) => {
                return Ok(());
            }
            Some(Some(Ok(_))) => {}
            Some(Some(Err(error))) => return Err(RealtimeError::Transport(error.to_string())),
        }
    }
}

/// The top-level `type` of `text` when it is JSON whose `type` is a string.
fn top_level_type(text: &str) -> Option<String> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("type")?
        .as_str()
        .map(str::to_owned)
}

/// Refuses a model id outside the characters model ids use.
pub(crate) fn check_model(model: &str) -> Result<(), RealtimeError> {
    if model.is_empty()
        || !model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_'))
    {
        return Err(RealtimeError::InvalidEndpoint(format!(
            "model id `{model}` is empty or holds a character outside A-Z, a-z, 0-9, '-', '.', '_'"
        )));
    }
    Ok(())
}

/// `<url>?model=<model>`, refusing a model id outside the characters model ids use.
fn request_url(endpoint: &Endpoint) -> Result<String, RealtimeError> {
    let model = &endpoint.model;
    check_model(model)?;
    if endpoint.url.contains('?') {
        return Err(RealtimeError::InvalidEndpoint(
            "the endpoint URL already has a query; the client adds `model`".to_owned(),
        ));
    }
    Ok(format!("{}?model={model}", endpoint.url))
}

/// TLS with the ring provider and the webpki roots, independent of any process-wide default
/// provider.
fn tls_config() -> Result<Arc<rustls::ClientConfig>, RealtimeError> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| {
        RealtimeError::Connect(ConnectError {
            status: None,
            detail: error.to_string(),
        })
    })?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The handshake failure of `error`, with the HTTP status when the server answered the upgrade
/// with one. The description is tungstenite's, which names the status and never the request.
fn handshake_error(error: &tokio_tungstenite::tungstenite::Error) -> ConnectError {
    let status = match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => Some(response.status().as_u16()),
        _ => None,
    };
    ConnectError {
        status,
        detail: error.to_string(),
    }
}
