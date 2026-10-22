//! A client for OpenAI GPT-Live (`v1/live/sessions`) over its primary WebSocket.
//!
//! GPT-Live is a full-duplex voice model: it listens while it speaks, decides by itself when to
//! speak, and delegates reasoning and tool use to a backend, either a Responses model OpenAI runs
//! (Responses delegation) or whatever the application runs (client delegation). It is billed per
//! second of session; backend usage is billed apart. It is a different API from Realtime
//! ([`RealtimeClient`](crate::RealtimeClient)): other URL, other events, no response or turn
//! objects on the voice side.
//!
//! # What this module was written from
//!
//! Read on 2026-10-20, as the Markdown each page serves when `.md` is appended to its URL:
//!
//! * the model page, `https://developers.openai.com/api/docs/models/gpt-live-1`;
//! * the guide, `https://developers.openai.com/api/docs/guides/live`;
//! * the WebSocket connection guide,
//!   `https://developers.openai.com/api/docs/guides/voice-websockets?api=live`;
//! * the primary WebSocket reference, sessions and events,
//!   `https://developers.openai.com/api/reference/resources/live/primary-websocket` (the sideband
//!   and fork references beside it were fetched and not used);
//! * delegation and tools, `https://developers.openai.com/api/docs/guides/live-delegation`;
//! * managing sessions, `https://developers.openai.com/api/docs/guides/live-conversations`;
//! * prompting, `https://developers.openai.com/api/docs/guides/live-prompting`;
//! * migration from Realtime, `https://developers.openai.com/api/docs/guides/live-migration`;
//! * usage and cost, `https://developers.openai.com/api/docs/guides/voice-latency-cost?api=live`.
//!
//! What they say, as this client uses it:
//!
//! * Connect to `wss://api.openai.com/v1/live/sessions` with no query, `Authorization: Bearer`
//!   the key; send `session.start` first, its `session` naming `model` (required), `instructions`,
//!   `audio.format`, `audio.output.voice` and `delegation`; wait for `session.started`, whose
//!   `session` holds the resolved configuration and the session `id`. An `error` instead fails
//!   the start ([`RealtimeError::Server`]).
//! * One audio format serves input and output and cannot change: PCM16 mono at 24 kHz (the
//!   default) or 16 kHz, G.711 mu-law or A-law at 8 kHz ([`LiveFormat`]). Input goes as
//!   `session.input_audio.append` (base64, no acknowledgement); output arrives as
//!   `session.output_audio.delta` with no timing fields on the primary WebSocket and no event
//!   marking the end of output.
//! * Transcripts arrive as `session.input_transcript.delta` and `session.output_transcript.delta`,
//!   each with `start_ms` and `end_ms` on the session timeline; there is no turn, no item id and
//!   no transcript-done event.
//! * There is no commit, no turn detection setting, no voice `response.create`, no cancellation
//!   and no truncation: "GPT-Live decides when to speak", and stops when interrupted as its prompt
//!   tells it to. `response.create` exists only to start or continue Responses backend work.
//! * Delegation is chosen at start: `{"type": "client"}` (also what an omitted or `null`
//!   delegation means) or `{"type": "responses", "responses": {...}}` with a backend `model`
//!   (required), `instructions`, `tools`, `tool_choice`, `parallel_tool_calls`, `reasoning` and
//!   more. `session.delegation.created` announces a delegation (metadata only, never the task
//!   text); Responses backend events arrive nested in `response.event`. A function call is read
//!   from the nested `response.output_item.done` whose item is a `function_call`; its result goes
//!   back as `response.item.create` with a `function_call_output` item, then `response.create`
//!   continues the backend.
//! * `session.instructions.append`, `session.thinking.append` and `session.commentary.append`
//!   add context during the session, each with a required, nullable `delegation_id`.
//! * Usage: `session.usage.updated` carries the cumulative voice seconds (a running total, never
//!   to be summed); `session.closed`, after `session.close`, carries the close `reason` and the
//!   final seconds. Backend token usage is the `usage` of each nested `response.completed`.
//!   Without `session.closed` the final usage is unconfirmed. These events are read field by
//!   field: `session.closed` without its reason or seconds is still the end of the session, and a
//!   backend `response.completed`, `response.failed` or `response.incomplete` without its
//!   response id keeps its usage.
//! * `error` events carry `error.client_event_id` naming the command they answer; an `error`
//!   coded `responses_validation_failed` reports a backend setting refused after start, and the
//!   session stays open.
//!
//! What the documents leave open, named here and not guessed: whether audio arrives while the
//! model is silent (as silence) or only while it speaks; how output audio lines up with output
//! transcript intervals; how often `session.usage.updated` arrives; which nested events a
//! Responses backend forwards beyond those the guide names; the session's duration limit; and
//! the default `strict` of a backend function tool. This client sends `"strict": false` with
//! each function tool so that a schema written for the duplex contract's tools is not refused by
//! strict-mode validation.
//!
//! The client never logs, prints or keeps the credential: it goes once into the opening
//! handshake, as for the Realtime client, whose note on the WebSocket library's trace logging
//! applies here too.

use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::client::{Socket, check_model, close_socket, open_socket, read_text, send_json};
use crate::config::{DEFAULT_READ_TIMEOUT, Endpoint, FunctionTool};
use crate::credential::CredentialSource;
use crate::error::RealtimeError;
use crate::event::{ServerError, text_start};

/// The Live model the reference names, `gpt-live-1`.
pub const LIVE_MODEL: &str = "gpt-live-1";

/// The primary WebSocket URL of a Live session. It takes no query: the model goes in
/// `session.start`.
pub const LIVE_URL: &str = "wss://api.openai.com/v1/live/sessions";

/// How long [`LiveClient::close`] waits for `session.closed`, in all, when the endpoint's read
/// timeout is shorter: the guide's examples wait 15 seconds.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(15);

impl Endpoint {
    /// The hosted Live endpoint for `model`, with the default read timeout.
    pub fn live(model: impl Into<String>) -> Self {
        Self {
            url: LIVE_URL.to_owned(),
            model: model.into(),
            read_timeout: DEFAULT_READ_TIMEOUT,
        }
    }
}

/// The audio format of a Live session over WebSocket, the same for input and output, fixed at
/// start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LiveFormat {
    /// `audio/pcm` at 24 kHz: 16-bit little-endian signed PCM, mono. The default.
    #[default]
    Pcm24k,
    /// `audio/pcm` at 16 kHz.
    Pcm16k,
    /// `audio/pcmu`: G.711 mu-law, mono, 8 kHz, one byte per sample.
    Pcmu,
    /// `audio/pcma`: G.711 A-law, mono, 8 kHz, one byte per sample.
    Pcma,
}

impl LiveFormat {
    /// The sample rate of the format in hertz.
    pub const fn sample_rate(self) -> u32 {
        match self {
            Self::Pcm24k => 24_000,
            Self::Pcm16k => 16_000,
            Self::Pcmu | Self::Pcma => 8_000,
        }
    }

    /// The bytes of one sample.
    pub const fn sample_width(self) -> usize {
        match self {
            Self::Pcm24k | Self::Pcm16k => 2,
            Self::Pcmu | Self::Pcma => 1,
        }
    }

    fn to_json(self) -> Value {
        let kind = match self {
            Self::Pcm24k | Self::Pcm16k => "audio/pcm",
            Self::Pcmu => "audio/pcmu",
            Self::Pcma => "audio/pcma",
        };
        json!({"type": kind, "rate": self.sample_rate()})
    }
}

/// Where the Live model delegates reasoning and tool use, chosen at start for the whole session.
#[derive(Debug, Clone, PartialEq)]
pub enum Delegation {
    /// The application runs the backend: `session.delegation.created` with target `client`
    /// announces a task, and results go back as appended context.
    Client,
    /// OpenAI runs a Responses backend; the application runs its function tools.
    Responses(ResponsesBackend),
}

/// The Responses backend of a session with Responses delegation. `None` fields keep the
/// server's value and are not sent.
#[derive(Debug, Clone, PartialEq)]
pub struct ResponsesBackend {
    /// The backend model, required.
    pub model: String,
    /// The backend's prompt, apart from the Live instructions.
    pub instructions: Option<String>,
    /// Function tools the backend may call; each is sent with `"strict": false`.
    pub tools: Vec<FunctionTool>,
    /// `auto`, `none` or `required`.
    pub tool_choice: Option<String>,
    pub parallel_tool_calls: Option<bool>,
    /// `reasoning.effort`, as the backend model takes it.
    pub reasoning_effort: Option<String>,
}

impl ResponsesBackend {
    /// A backend of `model` with no instructions, no tools and the server's defaults.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            instructions: None,
            tools: Vec::new(),
            tool_choice: None,
            parallel_tool_calls: None,
            reasoning_effort: None,
        }
    }

    fn to_json(&self) -> Value {
        let mut backend = json!({"model": self.model});
        if let Some(instructions) = &self.instructions {
            backend["instructions"] = json!(instructions);
        }
        if !self.tools.is_empty() {
            backend["tools"] = self
                .tools
                .iter()
                .map(|tool| {
                    let mut tool = tool.to_json();
                    tool["strict"] = json!(false);
                    tool
                })
                .collect();
        }
        if let Some(choice) = &self.tool_choice {
            backend["tool_choice"] = json!(choice);
        }
        if let Some(parallel) = self.parallel_tool_calls {
            backend["parallel_tool_calls"] = json!(parallel);
        }
        if let Some(effort) = &self.reasoning_effort {
            backend["reasoning"] = json!({"effort": effort});
        }
        backend
    }
}

/// The session configuration of `session.start`. `None` fields keep the server's default and
/// are not sent.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveSessionConfig {
    /// The conversation prompt of the Live model.
    pub instructions: Option<String>,
    /// The voice; the server's default is `marin`.
    pub voice: Option<String>,
    pub format: LiveFormat,
    pub delegation: Delegation,
}

impl Default for LiveSessionConfig {
    /// PCM16 at 24 kHz, client delegation, the server's voice and no instructions.
    fn default() -> Self {
        Self {
            instructions: None,
            voice: None,
            format: LiveFormat::Pcm24k,
            delegation: Delegation::Client,
        }
    }
}

impl LiveSessionConfig {
    /// The `session` object of `session.start` for `model`: `model`, `instructions`,
    /// `audio.format`, `audio.output.voice` and `delegation`, unset fields left out.
    pub fn to_session_json(&self, model: &str) -> Value {
        let mut session = json!({"model": model});
        if let Some(instructions) = &self.instructions {
            session["instructions"] = json!(instructions);
        }
        let mut audio = json!({"format": self.format.to_json()});
        if let Some(voice) = &self.voice {
            audio["output"] = json!({"voice": voice});
        }
        session["audio"] = audio;
        session["delegation"] = match &self.delegation {
            Delegation::Client => json!({"type": "client"}),
            Delegation::Responses(backend) => {
                json!({"type": "responses", "responses": backend.to_json()})
            }
        };
        session
    }
}

/// The token usage of one backend response, from its nested `response.completed` (or failed or
/// incomplete). Each count is `None` when the server did not report it or sent it in another
/// shape; the total is the server's, else the sum of input and output when both are known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BackendUsage {
    pub input_tokens: Option<u64>,
    /// Input tokens served from the cache, counted inside `input_tokens`.
    pub cached_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Reasoning tokens, counted inside `output_tokens`.
    pub reasoning_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

impl fmt::Display for BackendUsage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn detail(value: Option<u64>) -> String {
            value.map_or_else(|| "-".to_owned(), |value| value.to_string())
        }
        write!(
            f,
            "total {} tokens; input {} (cached {}); output {} (reasoning {})",
            detail(self.total_tokens),
            detail(self.input_tokens),
            detail(self.cached_tokens),
            detail(self.output_tokens),
            detail(self.reasoning_tokens),
        )
    }
}

/// A Responses streaming event nested in a `response.event` envelope. The types this client
/// reads are typed; any other is [`BackendEvent::Other`] with its JSON kept whole.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    /// `response.created`.
    Created { response_id: String },
    /// `response.output_item.done` whose item is a `function_call`: a complete call.
    FunctionCall {
        item_id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    /// `response.completed`, with the usage the server reported; `response_id` is `None` when
    /// the server left it out. `unreadable_usage` says what of a `usage` the client could not
    /// read: a `response` or `usage` that is no object gives no usage, and a count missing or in
    /// another shape is `None` in the usage kept.
    Completed {
        response_id: Option<String>,
        usage: Option<BackendUsage>,
        unreadable_usage: Option<String>,
    },
    /// `response.failed` or `response.incomplete`, with the response's status; `response_id`
    /// is `None` when the server left it out. `usage` and `unreadable_usage` are read as for
    /// [`BackendEvent::Completed`].
    Ended {
        response_id: Option<String>,
        status: String,
        usage: Option<BackendUsage>,
        unreadable_usage: Option<String>,
    },
    /// Any other nested event, the whole event as received.
    Other { event_type: String, raw: Value },
}

/// A Live server event. Each typed variant names the reference event it is read from; any other
/// event is [`LiveEvent::Unknown`] with its JSON kept whole.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveEvent {
    /// `session.started`, with the resolved session.
    SessionStarted { session: Value },
    /// `session.updated`, with the resulting session.
    SessionUpdated { session: Value },
    /// `session.output_audio.delta`, its base64 `delta` decoded into bytes of the session's
    /// format.
    OutputAudio { audio: Vec<u8> },
    /// `session.input_transcript.delta`, its interval in milliseconds of the session timeline.
    InputTranscript {
        delta: String,
        start_ms: u64,
        end_ms: u64,
    },
    /// `session.output_transcript.delta`.
    OutputTranscript {
        delta: String,
        start_ms: u64,
        end_ms: u64,
    },
    /// `session.delegation.created`: `target` is `client`, `responses` or `decisions`;
    /// `response_id` names the backend response of a Responses delegation.
    DelegationCreated {
        delegation_id: String,
        target: String,
        response_id: Option<String>,
        offset_ms: u64,
    },
    /// `response.event`: a nested Responses backend event and the delegation it belongs to.
    Backend {
        delegation_id: Option<String>,
        event: BackendEvent,
    },
    /// `session.usage.updated`: the cumulative voice seconds, a running total.
    UsageUpdated {
        seconds: f64,
        context_usage_ratio: Option<f64>,
    },
    /// An acknowledgement of a command: `session.input_audio.muted` and `unmuted`,
    /// `session.instructions.appended`, `session.thinking.appended`,
    /// `session.commentary.appended`.
    Acknowledged {
        event_type: String,
        client_event_id: Option<String>,
    },
    /// `session.closed`: why the session ended and its final voice seconds, each `None` when the
    /// server left it out or sent it in another shape. It ends the session either way.
    SessionClosed {
        reason: Option<String>,
        seconds: Option<f64>,
    },
    /// `error`; [`ServerError::event_id`] holds its `client_event_id`.
    Error(ServerError),
    /// `info`.
    Info { code: String, message: String },
    /// Any other event: its `type` and the whole event as received.
    Unknown { event_type: String, raw: Value },
}

impl LiveEvent {
    /// Reads one Live server event from the text of a WebSocket message.
    ///
    /// # Errors
    /// [`RealtimeError::Protocol`] when the text is not a JSON object with a string `type`,
    /// naming the start of the text, or when a typed event lacks a field the reference requires
    /// and without which it means nothing. `session.closed` and the backend's response ends,
    /// which end the session or carry billable usage, are never refused for a field.
    pub fn parse(text: &str) -> Result<Self, RealtimeError> {
        let raw: Value = serde_json::from_str(text).map_err(|error| {
            RealtimeError::Protocol(format!(
                "server event is not JSON ({error}); it begins {:?}",
                text_start(text)
            ))
        })?;
        let Some(event_type) = raw.get("type").and_then(Value::as_str) else {
            return Err(RealtimeError::Protocol(format!(
                "server event has no string `type`; it begins {:?}",
                text_start(text)
            )));
        };
        let event = Fields {
            event_type,
            raw: &raw,
        };
        Ok(match event_type {
            "session.started" => Self::SessionStarted {
                session: event.object("session")?,
            },
            "session.updated" => Self::SessionUpdated {
                session: event.object("session")?,
            },
            "session.output_audio.delta" => Self::OutputAudio {
                audio: event.base64("delta")?,
            },
            "session.input_transcript.delta" => Self::InputTranscript {
                delta: event.string("delta")?,
                start_ms: event.millis("start_ms")?,
                end_ms: event.millis("end_ms")?,
            },
            "session.output_transcript.delta" => Self::OutputTranscript {
                delta: event.string("delta")?,
                start_ms: event.millis("start_ms")?,
                end_ms: event.millis("end_ms")?,
            },
            "session.delegation.created" => {
                let delegation = event.nested("delegation")?;
                Self::DelegationCreated {
                    delegation_id: delegation.string("id")?,
                    target: delegation.string("target")?,
                    response_id: delegation.optional_string("response_id")?,
                    offset_ms: event.millis("offset_ms")?,
                }
            }
            "response.event" => Self::Backend {
                delegation_id: event.optional_string("delegation_id")?,
                event: backend(event.nested("event")?)?,
            },
            "session.usage.updated" => Self::UsageUpdated {
                seconds: event.nested("usage")?.number("seconds")?,
                context_usage_ratio: event
                    .get("context_window")
                    .and_then(|window| window.get("usage_ratio"))
                    .and_then(Value::as_f64),
            },
            "session.input_audio.muted"
            | "session.input_audio.unmuted"
            | "session.instructions.appended"
            | "session.thinking.appended"
            | "session.commentary.appended" => Self::Acknowledged {
                event_type: event_type.to_owned(),
                client_event_id: event.optional_string("client_event_id")?,
            },
            "session.closed" => Self::SessionClosed {
                reason: event.optional_string("reason").ok().flatten(),
                seconds: event
                    .nested("usage")
                    .and_then(|usage| usage.number("seconds"))
                    .ok(),
            },
            "error" => {
                let error = event.nested("error")?;
                Self::Error(ServerError {
                    kind: error.string("type")?,
                    code: error.optional_string("code")?,
                    message: error.string("message")?,
                    param: error.optional_string("param")?,
                    event_id: error.optional_string("client_event_id")?,
                })
            }
            "info" => Self::Info {
                code: event.string("code")?,
                message: event.string("message")?,
            },
            _ => Self::Unknown {
                event_type: event_type.to_owned(),
                raw: raw.clone(),
            },
        })
    }
}

/// The nested Responses event of a `response.event`.
fn backend(event: Fields<'_>) -> Result<BackendEvent, RealtimeError> {
    let event_type = event.string("type")?;
    let nested = Fields {
        event_type: "response.event",
        raw: event.raw,
    };
    Ok(match event_type.as_str() {
        "response.created" => BackendEvent::Created {
            response_id: nested.nested("response")?.string("id")?,
        },
        "response.output_item.done"
            if nested
                .get("item")
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
                == Some("function_call") =>
        {
            let item = nested.nested("item")?;
            BackendEvent::FunctionCall {
                item_id: item.optional_string("id")?.unwrap_or_default(),
                call_id: item.string("call_id")?,
                name: item.string("name")?,
                arguments: item.string("arguments")?,
            }
        }
        "response.completed" => {
            let response = nested.nested("response");
            let (usage, unreadable_usage) = response_usage(&response);
            let response = response.ok();
            BackendEvent::Completed {
                response_id: response.and_then(|response| response.lenient_string("id")),
                usage,
                unreadable_usage,
            }
        }
        "response.failed" | "response.incomplete" => {
            let response = nested.nested("response");
            let (usage, unreadable_usage) = response_usage(&response);
            let response = response.ok();
            BackendEvent::Ended {
                response_id: response.and_then(|response| response.lenient_string("id")),
                status: response
                    .and_then(|response| response.lenient_string("status"))
                    .unwrap_or_else(|| event_type.trim_start_matches("response.").to_owned()),
                usage,
                unreadable_usage,
            }
        }
        _ => BackendEvent::Other {
            event_type,
            raw: event.raw.clone(),
        },
    })
}

/// The usage of a backend response end's `response` and what of it could not be read; a
/// `response` missing or not an object has no usage, and that is what could not be read.
fn response_usage(
    response: &Result<Fields<'_>, RealtimeError>,
) -> (Option<BackendUsage>, Option<String>) {
    match response {
        Ok(response) => response.backend_usage(),
        Err(RealtimeError::Protocol(what)) => (None, Some(what.clone())),
        Err(error) => (None, Some(error.to_string())),
    }
}

/// Field access on one JSON object of a typed event; a missing or mistyped field the reference
/// requires is a protocol violation naming the event and the field.
#[derive(Clone, Copy)]
struct Fields<'a> {
    event_type: &'a str,
    raw: &'a Value,
}

impl<'a> Fields<'a> {
    fn missing(&self, field: &str, what: &str) -> RealtimeError {
        RealtimeError::Protocol(self.lacks(field, what))
    }

    fn lacks(&self, field: &str, what: &str) -> String {
        format!("`{}` lacks {what} field `{field}`", self.event_type)
    }

    fn get(&self, field: &str) -> Option<&'a Value> {
        self.raw.get(field).filter(|value| !value.is_null())
    }

    fn string(&self, field: &str) -> Result<String, RealtimeError> {
        self.optional_string(field)?
            .ok_or_else(|| self.missing(field, "the string"))
    }

    fn optional_string(&self, field: &str) -> Result<Option<String>, RealtimeError> {
        match self.get(field) {
            None => Ok(None),
            Some(Value::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err(self.missing(field, "the string")),
        }
    }

    /// A string field, `None` when it is missing or not a string.
    fn lenient_string(&self, field: &str) -> Option<String> {
        self.optional_string(field).ok().flatten()
    }

    fn number(&self, field: &str) -> Result<f64, RealtimeError> {
        self.get(field)
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| self.missing(field, "the non-negative number"))
    }

    /// A timeline position in milliseconds; the reference types it `number`, so a fraction is
    /// rounded down.
    fn millis(&self, field: &str) -> Result<u64, RealtimeError> {
        let value = self
            .get(field)
            .ok_or_else(|| self.missing(field, "the number"))?;
        if let Some(whole) = value.as_u64() {
            return Ok(whole);
        }
        let number = value
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| self.missing(field, "the non-negative number"))?;
        Ok(number.floor() as u64)
    }

    fn object(&self, field: &str) -> Result<Value, RealtimeError> {
        Ok(self.nested(field)?.raw.clone())
    }

    fn nested(&self, field: &str) -> Result<Fields<'a>, RealtimeError> {
        match self.get(field) {
            Some(value) if value.is_object() => Ok(Fields {
                event_type: self.event_type,
                raw: value,
            }),
            _ => Err(self.missing(field, "the object")),
        }
    }

    fn base64(&self, field: &str) -> Result<Vec<u8>, RealtimeError> {
        let text = self.string(field)?;
        STANDARD
            .decode(text)
            .map_err(|_| self.missing(field, "the base64"))
    }

    /// The Responses `usage` of a response object, read field by field, and what of it could not
    /// be read: `(None, None)` when it has none; no usage when it is not an object; otherwise
    /// every count it carries, `input_tokens` or `output_tokens` `None` when missing or not a
    /// non-negative integer, and then named as what could not be read.
    fn backend_usage(&self) -> (Option<BackendUsage>, Option<String>) {
        let Some(usage) = self.get("usage") else {
            return (None, None);
        };
        if !usage.is_object() {
            return (None, Some(self.lacks("usage", "the object")));
        }
        let count = |value: Option<&Value>| value.and_then(Value::as_u64);
        let input_tokens = count(usage.get("input_tokens"));
        let output_tokens = count(usage.get("output_tokens"));
        let unreadable = match (input_tokens, output_tokens) {
            (Some(_), Some(_)) => None,
            (None, Some(_)) => Some(self.lacks("usage.input_tokens", "the non-negative integer")),
            (Some(_), None) => Some(self.lacks("usage.output_tokens", "the non-negative integer")),
            (None, None) => Some(format!(
                "`{}` lacks the non-negative integer fields `usage.input_tokens` and \
                 `usage.output_tokens`",
                self.event_type
            )),
        };
        let usage = BackendUsage {
            input_tokens,
            cached_tokens: count(
                usage
                    .get("input_tokens_details")
                    .and_then(|details| details.get("cached_tokens")),
            ),
            output_tokens,
            reasoning_tokens: count(
                usage
                    .get("output_tokens_details")
                    .and_then(|details| details.get("reasoning_tokens")),
            ),
            total_tokens: count(usage.get("total_tokens")).or_else(|| {
                input_tokens
                    .zip(output_tokens)
                    .map(|(input, output)| input + output)
            }),
        };
        (Some(usage), unreadable)
    }
}

/// How a Live session ended, from `session.closed`: the close reason (`close_requested`,
/// `expired`, `content`, `remote_hangup`, `connection_lost`) and the final voice seconds, each
/// `None` when the server left it out.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveClosed {
    pub reason: Option<String>,
    pub seconds: Option<f64>,
}

/// One open Live session over its primary WebSocket.
///
/// Every send method writes one client event and returns once it is written; answers arrive as
/// server events through [`LiveClient::next_event`]. The client holds no credential after the
/// handshake.
pub struct LiveClient {
    socket: Socket,
    read_timeout: Duration,
    session: Value,
    pending: VecDeque<LiveEvent>,
}

impl fmt::Debug for LiveClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveClient")
            .field("read_timeout", &self.read_timeout)
            .field("session_id", &self.session_id())
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

impl LiveClient {
    /// Opens a session: resolves the secret `credential` names through `source`, connects to
    /// `endpoint.url` with it as the bearer token, sends `session.start` with `session` for
    /// `endpoint.model` and waits for `session.started`. Any other server event that arrives
    /// first is kept and returned first by [`LiveClient::next_event`].
    ///
    /// # Errors
    /// [`RealtimeError::InvalidEndpoint`] for a model id outside the characters model ids use or
    /// a URL with a query, before anything is resolved or connected;
    /// [`RealtimeError::Credential`], [`RealtimeError::Connect`]; [`RealtimeError::Server`] when
    /// the server answers the start with `error`; and the errors of [`LiveClient::next_event`].
    pub async fn connect(
        endpoint: &Endpoint,
        source: &dyn CredentialSource,
        credential: &str,
        session: &LiveSessionConfig,
    ) -> Result<Self, RealtimeError> {
        check_model(&endpoint.model)?;
        if endpoint.url.contains('?') {
            return Err(RealtimeError::InvalidEndpoint(
                "the Live URL takes no query; the model goes in session.start".to_owned(),
            ));
        }
        let socket = open_socket(&endpoint.url, source, credential).await?;
        let mut client = Self {
            socket,
            read_timeout: endpoint.read_timeout,
            session: Value::Null,
            pending: VecDeque::new(),
        };
        client
            .send(json!({
                "type": "session.start",
                "event_id": "session_start",
                "session": session.to_session_json(&endpoint.model),
            }))
            .await?;
        loop {
            match client.read_event().await? {
                LiveEvent::SessionStarted { session } => {
                    client.session = session;
                    return Ok(client);
                }
                LiveEvent::Error(error) => return Err(RealtimeError::Server(error)),
                other => client.pending.push_back(other),
            }
        }
    }

    /// The session as `session.started` resolved it.
    pub fn session(&self) -> &Value {
        &self.session
    }

    /// The session id `session.started` gave.
    pub fn session_id(&self) -> Option<&str> {
        self.session.get("id").and_then(Value::as_str)
    }

    /// Sends `session.input_audio.append` with `audio` as it is: raw bytes in the session's
    /// format, PCM chunks of whole samples.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn append_audio(&mut self, audio: &[u8]) -> Result<(), RealtimeError> {
        self.send(json!({"type": "session.input_audio.append", "audio": STANDARD.encode(audio)}))
            .await
    }

    /// Sends `session.input_audio.mute`; `session.input_audio.muted` acknowledges it.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn mute(&mut self) -> Result<(), RealtimeError> {
        self.send(json!({"type": "session.input_audio.mute"})).await
    }

    /// Sends `session.input_audio.unmute`; `session.input_audio.unmuted` acknowledges it.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn unmute(&mut self) -> Result<(), RealtimeError> {
        self.send(json!({"type": "session.input_audio.unmute"}))
            .await
    }

    /// Sends `session.instructions.append`: instructions for the running conversation, about
    /// the client delegation `delegation_id` or, `None`, the whole session.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn append_instructions(
        &mut self,
        delegation_id: Option<&str>,
        content: &str,
    ) -> Result<(), RealtimeError> {
        self.append("session.instructions.append", delegation_id, content)
            .await
    }

    /// Sends `session.thinking.append`: context the model may use without saying it now.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn append_thinking(
        &mut self,
        delegation_id: Option<&str>,
        content: &str,
    ) -> Result<(), RealtimeError> {
        self.append("session.thinking.append", delegation_id, content)
            .await
    }

    /// Sends `session.commentary.append`: a result the model should say, in its own words.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn append_commentary(
        &mut self,
        delegation_id: Option<&str>,
        content: &str,
    ) -> Result<(), RealtimeError> {
        self.append("session.commentary.append", delegation_id, content)
            .await
    }

    /// Sends `response.item.create` with a `function_call_output` item answering `call_id`. The
    /// backend continues only after [`LiveClient::create_response`].
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn send_function_output(
        &mut self,
        call_id: &str,
        output: &str,
    ) -> Result<(), RealtimeError> {
        self.send(json!({
            "type": "response.item.create",
            "item": {"type": "function_call_output", "call_id": call_id, "output": output},
        }))
        .await
    }

    /// Sends `response.create`: starts or continues Responses backend work. It needs Responses
    /// delegation and creates no speech by itself.
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn create_response(&mut self) -> Result<(), RealtimeError> {
        self.send(json!({"type": "response.create"})).await
    }

    /// Sends `session.close` without waiting; `session.closed` arrives through
    /// [`LiveClient::next_event`].
    ///
    /// # Errors
    /// [`RealtimeError::Transport`] when the write fails.
    pub async fn request_close(&mut self) -> Result<(), RealtimeError> {
        self.send(json!({"type": "session.close", "event_id": "session_close"}))
            .await
    }

    /// The next server event, events kept during start first.
    ///
    /// # Errors
    /// [`RealtimeError::Stalled`] when no event arrives within the endpoint's read timeout;
    /// [`RealtimeError::Closed`] when the server closed the connection;
    /// [`RealtimeError::Transport`] when the read fails; [`RealtimeError::Protocol`] for a
    /// message that is not a Live event.
    pub async fn next_event(&mut self) -> Result<LiveEvent, RealtimeError> {
        match self.pending.pop_front() {
            Some(event) => Ok(event),
            None => self.read_event().await,
        }
    }

    /// Closes the session gracefully: sends `session.close`, reads until `session.closed`, then
    /// closes the connection. Events read on the way are dropped, and so are messages that are not
    /// GPT-Live events: the client reads on past them.
    ///
    /// The whole close, every read and the closing of the connection, takes at most the longer
    /// of the endpoint's read timeout and [`CLOSE_TIMEOUT`], however often the server sends.
    ///
    /// # Errors
    /// The errors of [`LiveClient::next_event`] when the connection ends before
    /// `session.closed`, and [`RealtimeError::Stalled`] when that bound passes without it, the
    /// connection then closed: the final usage is unconfirmed.
    pub async fn close(self) -> Result<LiveClosed, RealtimeError> {
        self.close_seeing(|_| {}).await
    }

    /// [`LiveClient::close`], handing `seen` each event read on the way and each message that
    /// is not a GPT-Live event, in the order they arrived.
    pub(crate) async fn close_seeing(
        mut self,
        mut seen: impl FnMut(Result<LiveEvent, RealtimeError>),
    ) -> Result<LiveClosed, RealtimeError> {
        let bound = self.read_timeout.max(CLOSE_TIMEOUT);
        let deadline = tokio::time::Instant::now() + bound;
        self.read_timeout = bound;
        self.request_close().await?;
        let closed = loop {
            match tokio::time::timeout_at(deadline, self.next_event()).await {
                Err(_) => break Err(RealtimeError::Stalled(bound)),
                Ok(Ok(LiveEvent::SessionClosed { reason, seconds })) => {
                    break Ok(LiveClosed { reason, seconds });
                }
                Ok(Ok(event)) => seen(Ok(event)),
                Ok(Err(error @ RealtimeError::Protocol(_))) => seen(Err(error)),
                Ok(Err(error)) => return Err(error),
            }
        };
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let _ = close_socket(&mut self.socket, left).await;
        closed
    }

    async fn append(
        &mut self,
        kind: &str,
        delegation_id: Option<&str>,
        content: &str,
    ) -> Result<(), RealtimeError> {
        self.send(json!({"type": kind, "delegation_id": delegation_id, "content": content}))
            .await
    }

    async fn read_event(&mut self) -> Result<LiveEvent, RealtimeError> {
        let text = read_text(&mut self.socket, self.read_timeout).await?;
        LiveEvent::parse(&text)
    }

    async fn send(&mut self, event: Value) -> Result<(), RealtimeError> {
        send_json(&mut self.socket, &event).await
    }
}
