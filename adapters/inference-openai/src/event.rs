//! Server events as typed values.

use std::fmt;

use serde_json::Value;

use crate::error::RealtimeError;

/// A server event. Each typed variant names the reference event it is read from; any other event
/// is [`ServerEvent::Unknown`] with its JSON kept whole.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerEvent {
    /// `session.created`, with the session object.
    SessionCreated { session: Value },
    /// `session.updated`, with the session object.
    SessionUpdated { session: Value },
    /// `response.created`.
    ResponseCreated { response_id: String },
    /// `response.output_audio.delta`, its base64 `delta` decoded into bytes of the session's
    /// output format.
    AudioDelta {
        response_id: String,
        item_id: String,
        content_index: u32,
        audio: Vec<u8>,
    },
    /// `response.output_audio.done`.
    AudioDone {
        response_id: String,
        item_id: String,
    },
    /// `response.output_audio_transcript.delta`.
    OutputTranscriptDelta {
        response_id: String,
        item_id: String,
        delta: String,
    },
    /// `response.output_audio_transcript.done`.
    OutputTranscriptDone {
        response_id: String,
        item_id: String,
        transcript: String,
    },
    /// `response.output_text.delta`, for text output.
    TextDelta {
        response_id: String,
        item_id: String,
        delta: String,
    },
    /// `conversation.item.input_audio_transcription.delta`.
    InputTranscriptDelta { item_id: String, delta: String },
    /// `conversation.item.input_audio_transcription.completed`, with the usage the server
    /// reported for the transcription. It is read field by field: a field the server left out or
    /// sent in another shape is `None`, so the usage of an event without its `transcript` still
    /// counts. A `usage` the server sent in a shape that cannot be read is `None` too, and
    /// `unreadable_usage` says why.
    InputTranscript {
        item_id: Option<String>,
        transcript: Option<String>,
        usage: Option<TranscriptionUsage>,
        unreadable_usage: Option<String>,
    },
    /// `input_audio_buffer.speech_started`.
    SpeechStarted {
        item_id: String,
        audio_start_ms: u64,
    },
    /// `input_audio_buffer.speech_stopped`.
    SpeechStopped { item_id: String, audio_end_ms: u64 },
    /// `input_audio_buffer.committed`.
    AudioCommitted { item_id: String },
    /// `conversation.item.truncated`.
    ItemTruncated {
        item_id: String,
        content_index: u32,
        audio_end_ms: u64,
    },
    /// `response.function_call_arguments.done`: a complete function call.
    FunctionCall {
        response_id: String,
        item_id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    /// `response.done`, with the response's final status and the usage the server reported. It
    /// is read field by field: a field of `response` the server left out or sent in another shape
    /// is `None`, and so is each when `response` is. It still ends a response: Realtime ends its
    /// responses in the order it created them, so one without its id ends the oldest response
    /// still awaiting its end, and its usage counts whatever else it lacks. A `usage` the server
    /// sent in a shape that cannot be read is `None` too, and `unreadable_usage` says why.
    ResponseDone {
        response_id: Option<String>,
        status: Option<ResponseStatus>,
        usage: Option<Usage>,
        unreadable_usage: Option<String>,
    },
    /// `error`.
    Error(ServerError),
    /// Any other event: its `type` and the whole event as received.
    Unknown { event_type: String, raw: Value },
}

impl ServerEvent {
    /// Reads one server event from the text of a WebSocket message.
    ///
    /// # Errors
    /// [`RealtimeError::Protocol`] when the text is not a JSON object with a string `type`,
    /// naming the start of the text, or when a typed event lacks a field the reference requires
    /// and without which it means nothing. `response.done` and
    /// `conversation.item.input_audio_transcription.completed`, which end a response or carry
    /// billable usage, are read field by field and never refused for a field.
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
            "session.created" => Self::SessionCreated {
                session: event.object("session")?,
            },
            "session.updated" => Self::SessionUpdated {
                session: event.object("session")?,
            },
            "response.created" => Self::ResponseCreated {
                response_id: event.response()?.string("id")?,
            },
            "response.output_audio.delta" => Self::AudioDelta {
                response_id: event.string("response_id")?,
                item_id: event.string("item_id")?,
                content_index: event.index("content_index")?,
                audio: event.base64("delta")?,
            },
            "response.output_audio.done" => Self::AudioDone {
                response_id: event.string("response_id")?,
                item_id: event.string("item_id")?,
            },
            "response.output_audio_transcript.delta" => Self::OutputTranscriptDelta {
                response_id: event.string("response_id")?,
                item_id: event.string("item_id")?,
                delta: event.string("delta")?,
            },
            "response.output_audio_transcript.done" => Self::OutputTranscriptDone {
                response_id: event.string("response_id")?,
                item_id: event.string("item_id")?,
                transcript: event.string("transcript")?,
            },
            "response.output_text.delta" => Self::TextDelta {
                response_id: event.string("response_id")?,
                item_id: event.string("item_id")?,
                delta: event.string("delta")?,
            },
            "conversation.item.input_audio_transcription.delta" => Self::InputTranscriptDelta {
                item_id: event.string("item_id")?,
                delta: event.optional_string("delta")?.unwrap_or_default(),
            },
            "conversation.item.input_audio_transcription.completed" => {
                let (usage, unreadable_usage) = split(event.transcription_usage());
                Self::InputTranscript {
                    item_id: event.lenient_string("item_id"),
                    transcript: event.lenient_string("transcript"),
                    usage,
                    unreadable_usage,
                }
            }
            "input_audio_buffer.speech_started" => Self::SpeechStarted {
                item_id: event.string("item_id")?,
                audio_start_ms: event.unsigned("audio_start_ms")?,
            },
            "input_audio_buffer.speech_stopped" => Self::SpeechStopped {
                item_id: event.string("item_id")?,
                audio_end_ms: event.unsigned("audio_end_ms")?,
            },
            "input_audio_buffer.committed" => Self::AudioCommitted {
                item_id: event.string("item_id")?,
            },
            "conversation.item.truncated" => Self::ItemTruncated {
                item_id: event.string("item_id")?,
                content_index: event.index("content_index")?,
                audio_end_ms: event.unsigned("audio_end_ms")?,
            },
            "response.function_call_arguments.done" => Self::FunctionCall {
                response_id: event.string("response_id")?,
                item_id: event.string("item_id")?,
                call_id: event.string("call_id")?,
                name: event.string("name")?,
                arguments: event.string("arguments")?,
            },
            "response.done" => {
                let response = event.response().ok();
                let (usage, unreadable_usage) =
                    response.map_or((None, None), |response| split(response.usage()));
                Self::ResponseDone {
                    response_id: response.and_then(|response| response.lenient_string("id")),
                    status: response
                        .and_then(|response| response.lenient_string("status"))
                        .map(|status| ResponseStatus::from_wire(&status)),
                    usage,
                    unreadable_usage,
                }
            }
            "error" => {
                let error = event.nested("error")?;
                Self::Error(ServerError {
                    kind: error.string("type")?,
                    code: error.optional_string("code")?,
                    message: error.string("message")?,
                    param: error.optional_string("param")?,
                    event_id: error.optional_string("event_id")?,
                })
            }
            _ => Self::Unknown {
                event_type: event_type.to_owned(),
                raw: raw.clone(),
            },
        })
    }
}

/// A usage as read, `None` when the server sent none, and why it could not be read when it
/// sent one in another shape.
fn split<T>(usage: Result<Option<T>, RealtimeError>) -> (Option<T>, Option<String>) {
    match usage {
        Ok(usage) => (usage, None),
        Err(RealtimeError::Protocol(what)) => (None, Some(what)),
        Err(error) => (None, Some(error.to_string())),
    }
}

/// The first 32 characters of a server message, to name one that could not be read.
pub(crate) fn text_start(text: &str) -> &str {
    text.char_indices()
        .nth(32)
        .map_or(text, |(end, _)| &text[..end])
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
        RealtimeError::Protocol(format!(
            "`{}` lacks {what} field `{field}`",
            self.event_type
        ))
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

    fn unsigned(&self, field: &str) -> Result<u64, RealtimeError> {
        self.get(field)
            .and_then(Value::as_u64)
            .ok_or_else(|| self.missing(field, "the non-negative integer"))
    }

    fn optional_unsigned(&self, field: &str) -> Option<u64> {
        self.get(field).and_then(Value::as_u64)
    }

    fn index(&self, field: &str) -> Result<u32, RealtimeError> {
        u32::try_from(self.unsigned(field)?).map_err(|_| self.missing(field, "the index"))
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

    fn response(&self) -> Result<Fields<'a>, RealtimeError> {
        self.nested("response")
    }

    fn base64(&self, field: &str) -> Result<Vec<u8>, RealtimeError> {
        use base64::Engine;
        let text = self.string(field)?;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(|_| self.missing(field, "the base64"))
    }

    /// The `usage` object of a response; `None` when the server sent none or `null`.
    fn usage(&self) -> Result<Option<Usage>, RealtimeError> {
        if self.get("usage").is_none() {
            return Ok(None);
        }
        let usage = self.nested("usage")?;
        let input = usage.get("input_token_details").map(|raw| Fields {
            event_type: self.event_type,
            raw,
        });
        let output = usage.get("output_token_details").map(|raw| Fields {
            event_type: self.event_type,
            raw,
        });
        let detail = |details: Option<Fields<'_>>, field: &str| {
            details.and_then(|details| details.optional_unsigned(field))
        };
        let input_text_tokens = detail(input, "text_tokens");
        let input_audio_tokens = detail(input, "audio_tokens");
        let output_text_tokens = detail(output, "text_tokens");
        let output_audio_tokens = detail(output, "audio_tokens");
        let input_tokens = usage
            .optional_unsigned("input_tokens")
            .unwrap_or_else(|| input_text_tokens.unwrap_or(0) + input_audio_tokens.unwrap_or(0));
        let output_tokens = usage
            .optional_unsigned("output_tokens")
            .unwrap_or_else(|| output_text_tokens.unwrap_or(0) + output_audio_tokens.unwrap_or(0));
        Ok(Some(Usage {
            total_tokens: usage
                .optional_unsigned("total_tokens")
                .unwrap_or(input_tokens + output_tokens),
            input_tokens,
            output_tokens,
            input_text_tokens,
            input_audio_tokens,
            cached_input_tokens: detail(input, "cached_tokens"),
            output_text_tokens,
            output_audio_tokens,
        }))
    }

    /// The `usage` of a completed input transcription; `None` when the server sent none.
    fn transcription_usage(&self) -> Result<Option<TranscriptionUsage>, RealtimeError> {
        if self.get("usage").is_none() {
            return Ok(None);
        }
        let usage = self.nested("usage")?;
        if usage.optional_string("type")?.as_deref() == Some("duration") {
            let seconds = usage
                .get("seconds")
                .and_then(Value::as_f64)
                .ok_or_else(|| self.missing("seconds", "the number"))?;
            return Ok(Some(TranscriptionUsage::Duration { seconds }));
        }
        let input_tokens = usage.optional_unsigned("input_tokens");
        let output_tokens = usage.optional_unsigned("output_tokens");
        Ok(Some(TranscriptionUsage::Tokens {
            total_tokens: usage.optional_unsigned("total_tokens").or_else(|| {
                input_tokens
                    .zip(output_tokens)
                    .map(|(input, output)| input + output)
            }),
            input_tokens,
            output_tokens,
        }))
    }
}

/// The usage of a completed input transcription: tokens of the transcription model, or, for a
/// model billed by time, the seconds of audio it transcribed. A count the server left out is
/// `None`, never 0; the total is the sum of the two when the server sent none and both are
/// known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TranscriptionUsage {
    Tokens {
        total_tokens: Option<u64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    Duration {
        seconds: f64,
    },
}

impl ResponseStatus {
    fn from_wire(status: &str) -> Self {
        match status {
            "completed" => Self::Completed,
            "cancelled" => Self::Cancelled,
            "failed" => Self::Failed,
            "incomplete" => Self::Incomplete,
            "in_progress" => Self::InProgress,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// The final status of a response, from `response.done`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResponseStatus {
    Completed,
    Cancelled,
    Failed,
    Incomplete,
    InProgress,
    /// A status this client does not know, as the server sent it.
    Other(String),
}

/// The token usage of a response, from `response.done`. The details are `None` when the server
/// did not report them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Usage {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub input_text_tokens: Option<u64>,
    pub input_audio_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_text_tokens: Option<u64>,
    pub output_audio_tokens: Option<u64>,
}

impl fmt::Display for Usage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn detail(value: Option<u64>) -> String {
            value.map_or_else(|| "-".to_owned(), |value| value.to_string())
        }
        write!(
            f,
            "total {} tokens; input {} (text {}, audio {}, cached {}); output {} (text {}, audio {})",
            self.total_tokens,
            self.input_tokens,
            detail(self.input_text_tokens),
            detail(self.input_audio_tokens),
            detail(self.cached_input_tokens),
            self.output_tokens,
            detail(self.output_text_tokens),
            detail(self.output_audio_tokens),
        )
    }
}

/// The `error` object of an `error` server event. `kind` is its `type`; `event_id` names the
/// client event that caused it, when the server knows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerError {
    pub kind: String,
    pub code: Option<String>,
    pub message: String,
    pub param: Option<String>,
    pub event_id: Option<String>,
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)?;
        if let Some(code) = &self.code {
            write!(f, " ({code})")?;
        }
        write!(f, ": {}", self.message)?;
        if let Some(param) = &self.param {
            write!(f, " [param {param}]")?;
        }
        Ok(())
    }
}
