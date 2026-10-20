//! A spoken conversation with a native speech model: the default microphone feeds an
//! `inference-duplex` session, the session's audio plays on the default output, both transcripts
//! are reported, a barge-in stops playout at once, and one demo tool tells the local time.
//!
//! [`Conversation`] holds the wiring and is generic over the session and the audio host, so the
//! tests drive it with the testkit's mock model and the device adapter's fake host, and
//! `examples/talk.rs` with a provider binding and the cpal host:
//!
//! ```text
//! cargo run -p realtime-voice --example talk -- --provider openai
//! cargo run -p realtime-voice --example talk -- --provider nova
//! ```
//!
//! The microphone captures in the session's input format, resampled by the device adapter; the
//! speaker plays the session's output format, resampled to the device.
//!
//! Barge-in: when the session reports that the user started speaking, or that the provider
//! cancelled the response being played, the speaker is stopped at once and the played position
//! read. With `auto_interrupt` off the caller's rule is to cancel the response in progress itself;
//! a model that takes truncation is then told to cut the played response at that position, and
//! any response queued behind it at its first sample. A model that takes neither, such as one that
//! cancels on its own side, only has its playout stopped.
//!
//! The demo tool, [`time_tool`], is answered by call id with what the conversation's clock says.
//! Once the response that proposed it ends, the conversation asks for the next response when the
//! model takes responses from the caller; a model that creates every response continues itself.

use std::collections::{HashSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_audio_device::{
    AudioHost, CaptureConfig, DeviceError, DeviceSelector, Microphone, PlayoutConfig, PushError,
    Speaker,
};
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_tools::{DeclareTool, Json, SideEffectClass, Tool};
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionEvent, SessionFailure,
    SpeechCallId, SpeechCapabilities, SpeechSessionHandle, SpeechToolCallProposed, TurnDetection,
    TurnTuning,
};

/// The name the demo tool is offered under.
pub const TIME_TOOL: &str = "local_time";

/// What the model is told at the start of the session.
pub const INSTRUCTIONS: &str = "You are a friendly voice assistant. Answer briefly, in one or \
     two spoken sentences. When the user asks for the time or the date, call the local_time tool \
     and say what it returns.";

/// `pcm_s16le` mono at `rate` hertz.
#[must_use]
pub fn pcm16(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).unwrap_or(NonZeroU32::MIN),
        channels: NonZeroU16::MIN,
    }
}

/// The demo tool: the local time, taking no arguments.
#[must_use]
pub fn time_tool() -> Tool {
    Tool::declare(DeclareTool {
        name: TIME_TOOL.to_owned(),
        schema: serde_json::json!({"type": "object", "properties": {}}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

/// The local date and time with the UTC offset, as the demo tool answers it.
#[must_use]
pub fn local_time() -> Json {
    let now = chrono::Local::now();
    serde_json::json!({
        "local_time": now.format("%A %-d %B %Y, %H:%M").to_string(),
        "utc_offset": now.format("%:z").to_string(),
    })
}

/// The session the demo opens on the OpenAI Realtime API: PCM16 at 24 kHz both ways, server
/// voice activity detection creating each response, `auto_interrupt` off so barge-in is the
/// caller's rule (cancel and truncate at the played position), and the demo tool.
#[must_use]
pub fn openai_config() -> SessionConfig {
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: INSTRUCTIONS.to_owned(),
        input_format: pcm16(24_000),
        output_format: pcm16(24_000),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: false,
        tools: vec![time_tool()],
        idle_limit: Duration::from_secs(30),
    }
}

/// The session the demo opens on Amazon Nova Sonic: LPCM at 16 kHz in and 24 kHz out, the
/// provider's own turn detection, responses and barge-in, and the demo tool.
#[must_use]
pub fn nova_config() -> SessionConfig {
    SessionConfig {
        voice: "matthew".to_owned(),
        instructions: INSTRUCTIONS.to_owned(),
        input_format: pcm16(16_000),
        output_format: pcm16(24_000),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: vec![time_tool()],
        idle_limit: Duration::from_secs(30),
    }
}

/// How a [`Conversation`] paces itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// How long one read of the session waits for the provider.
    pub wait: Duration,
    /// The most session events one [`Conversation::step`] handles before it turns back to the
    /// microphone and the speaker.
    pub max_events: usize,
    /// How much audio the speaker queues.
    pub playout: PlayoutConfig,
}

impl Default for Options {
    /// Reads of 10 ms, at most 32 events a step, four seconds of playout queue.
    fn default() -> Self {
        Self {
            wait: Duration::from_millis(10),
            max_events: 32,
            playout: PlayoutConfig {
                buffer: Duration::from_secs(4),
            },
        }
    }
}

/// Something the conversation did or heard, for the caller to report.
#[derive(Debug, Clone, PartialEq)]
pub enum Note {
    /// The provider's transcript of what the user said.
    Heard(String),
    /// A piece of the transcript of a response's audio.
    Said {
        response_id: ResponseId,
        text: String,
    },
    /// A response ended: completed, cancelled or failed.
    Ended {
        response_id: ResponseId,
        how: Ending,
    },
    /// Playout was stopped for a barge-in: the response being played and the position its
    /// listener heard up to, when audio was playing.
    BargeIn {
        response_id: Option<ResponseId>,
        heard: Option<SampleClockPosition>,
    },
    /// The session cut a response's audio at `heard`.
    Truncated {
        response_id: ResponseId,
        heard: SampleClockPosition,
    },
    /// A call of the demo tool, answered with `result`.
    Answered { call_id: SpeechCallId, result: Json },
    /// An operation was refused; the conversation goes on.
    Refused(String),
    /// The session closed.
    Closed,
    /// The session failed.
    Failed(SessionFailure),
}

/// How a response ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    Completed,
    Cancelled,
    Failed(SessionFailure),
}

/// Why a conversation did not start.
#[derive(Debug)]
pub enum StartError {
    /// The microphone or the speaker did not open.
    Device(DeviceError),
    /// The session did not open.
    Session(OpenError),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device(error) => write!(f, "the audio device did not open: {error}"),
            Self::Session(error) => write!(f, "the session did not open: {error}"),
        }
    }
}

impl Error for StartError {}

/// The microphone, a native speech session and the speaker, wired together.
pub struct Conversation<S> {
    session: S,
    capabilities: SpeechCapabilities,
    microphone: Microphone,
    speaker: Speaker,
    options: Options,
    clock: Box<dyn Fn() -> Json>,
    playing: Option<ResponseId>,
    backlog: VecDeque<(ResponseId, AudioFrame)>,
    finished: bool,
    follow_up: HashSet<ResponseId>,
    current: Option<ResponseId>,
    frames_sent: u64,
}

impl<S> fmt::Debug for Conversation<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conversation")
            .field("microphone", self.microphone.device())
            .field("speaker", self.speaker.device())
            .field("playing", &self.playing)
            .field("backlog", &self.backlog.len())
            .field("frames_sent", &self.frames_sent)
            .finish_non_exhaustive()
    }
}

impl<S: SpeechSessionHandle> Conversation<S> {
    /// Opens the default microphone in `config`'s input format and the default output on `host`,
    /// then a session of `model` from `config`. `clock` answers the demo tool.
    ///
    /// # Errors
    ///
    /// [`StartError::Device`] when a device does not open, [`StartError::Session`] when the
    /// model refuses the session; nothing stays open.
    pub fn open<M>(
        model: &mut M,
        config: SessionConfig,
        host: &dyn AudioHost,
        options: Options,
        clock: Box<dyn Fn() -> Json>,
    ) -> Result<Self, StartError>
    where
        M: NativeSpeechModel<Session = S>,
    {
        let input = config.input_format;
        let microphone = Microphone::open(
            host,
            &DeviceSelector::Default,
            CaptureConfig {
                sample_rate: input.sample_rate,
                encoding: input.encoding,
                ..CaptureConfig::default()
            },
        )
        .map_err(StartError::Device)?;
        let speaker = Speaker::open(host, &DeviceSelector::Default, options.playout)
            .map_err(StartError::Device)?;
        let capabilities = model.capabilities().clone();
        let session = model.open(config).map_err(StartError::Session)?;
        Ok(Self {
            session,
            capabilities,
            microphone,
            speaker,
            options,
            clock,
            playing: None,
            backlog: VecDeque::new(),
            finished: true,
            follow_up: HashSet::new(),
            current: None,
            frames_sent: 0,
        })
    }

    /// The session.
    pub const fn session(&self) -> &S {
        &self.session
    }

    /// The microphone.
    pub const fn microphone(&self) -> &Microphone {
        &self.microphone
    }

    /// The speaker.
    pub const fn speaker(&self) -> &Speaker {
        &self.speaker
    }

    /// The frames of microphone audio the session accepted.
    pub const fn frames_sent(&self) -> u64 {
        self.frames_sent
    }

    /// Whether the session is still open.
    pub fn is_open(&self) -> bool {
        self.session.ledger().session().require_open().is_ok()
    }

    /// One turn of the loop: the captured audio to the session, the session's events handled,
    /// the model's audio to the speaker.
    pub fn step(&mut self) -> Vec<Note> {
        let mut notes = self.pump_microphone();
        notes.extend(self.pump_events(self.options.max_events));
        self.feed_speaker(&mut notes);
        notes
    }

    /// Sends every frame the microphone has captured to the session.
    pub fn pump_microphone(&mut self) -> Vec<Note> {
        let mut notes = Vec::new();
        while let Some(frame) = self.microphone.try_frame() {
            if !self.is_open() {
                break;
            }
            match self.session.append_audio(frame) {
                Ok(()) => self.frames_sent += 1,
                Err(error) => notes.push(Note::Refused(format!("input audio: {error}"))),
            }
        }
        notes
    }

    /// Handles at most `max` events of the session, each read waiting at most the configured
    /// wait; the first read that finds nothing ends the turn.
    pub fn pump_events(&mut self, max: usize) -> Vec<Note> {
        let mut notes = Vec::new();
        for _ in 0..max {
            let Some(event) = self.session.next_event(self.options.wait) else {
                break;
            };
            self.handle(event, &mut notes);
        }
        notes
    }

    /// Queues the model's audio at the speaker as far as it has room. Audio of a later response
    /// waits until the speaker has played the one before, so the speaker holds one response's
    /// audio at a time and its played position is a position of that response.
    pub fn feed_speaker(&mut self, notes: &mut Vec<Note>) {
        while let Some((response_id, frame)) = self.backlog.front() {
            let response_id = *response_id;
            if self.playing != Some(response_id) {
                if self.speaker.queued() > 0 {
                    break;
                }
                self.playing = Some(response_id);
            }
            match self.speaker.try_push(frame) {
                Ok(()) => self.finished = false,
                Err(PushError::Full) => break,
                Err(error) => notes.push(Note::Refused(format!("output audio: {error}"))),
            }
            self.backlog.pop_front();
        }
        let done = self
            .playing
            .is_some_and(|playing| self.current != Some(playing));
        if self.backlog.is_empty() && done && !self.finished {
            self.speaker.finish();
            self.finished = true;
        }
    }

    /// Handles one event of the session.
    fn handle(&mut self, event: SessionEvent, notes: &mut Vec<Note>) {
        match event {
            SessionEvent::SpeechStarted(_) => self.barge_in(None, notes),
            SessionEvent::InputTranscribed(input) => notes.push(Note::Heard(input.text)),
            SessionEvent::OutputTranscriptDelta(delta) => notes.push(Note::Said {
                response_id: delta.response_id,
                text: delta.text,
            }),
            SessionEvent::OutputAudio(audio) => {
                self.backlog.push_back((audio.response_id, audio.frame));
                self.feed_speaker(notes);
            }
            SessionEvent::ResponseCreated(created) => self.current = Some(created.response_id),
            SessionEvent::ResponseCompleted(done) => {
                self.ended(done.response_id);
                notes.push(Note::Ended {
                    response_id: done.response_id,
                    how: Ending::Completed,
                });
                self.follow(done.response_id, notes);
            }
            SessionEvent::ResponseCancelled(cancelled) => {
                self.ended(cancelled.response_id);
                self.barge_in(Some(cancelled.response_id), notes);
                self.follow_up.remove(&cancelled.response_id);
                notes.push(Note::Ended {
                    response_id: cancelled.response_id,
                    how: Ending::Cancelled,
                });
            }
            SessionEvent::ResponseFailed(failed) => {
                self.ended(failed.response_id);
                self.follow_up.remove(&failed.response_id);
                notes.push(Note::Ended {
                    response_id: failed.response_id,
                    how: Ending::Failed(failed.failure),
                });
            }
            SessionEvent::OutputTruncated(truncated) => notes.push(Note::Truncated {
                response_id: truncated.response_id,
                heard: truncated.heard,
            }),
            SessionEvent::SpeechToolCallProposed(call) => self.answer(&call, notes),
            SessionEvent::SpeechToolCallAnswered(answered) => notes.push(Note::Answered {
                call_id: answered.call_id,
                result: answered.result,
            }),
            SessionEvent::SpeechSessionClosed(_) => {
                self.silence();
                notes.push(Note::Closed);
            }
            SessionEvent::SpeechSessionFailed(failed) => {
                self.silence();
                notes.push(Note::Failed(failed.failure));
            }
            SessionEvent::InputAccepted(_)
            | SessionEvent::InputCommitted(_)
            | SessionEvent::SpeechStopped(_)
            | SessionEvent::TranscriptionUsageRecorded(_)
            | SessionEvent::ResponseUsageRecorded(_) => {}
        }
    }

    /// The response `response_id` is no longer in progress, as far as the events handled say.
    fn ended(&mut self, response_id: ResponseId) {
        if self.current == Some(response_id) {
            self.current = None;
        }
    }

    /// A barge-in: the user started speaking, or the provider cancelled `cancelled`. With
    /// `auto_interrupt` off the caller cancels the response in progress itself. Audio still to
    /// play is stopped at once; a model that takes truncation is told where its listener stopped
    /// hearing the response played, and that the responses queued behind it were not heard.
    ///
    /// The response in progress is the one the events handled so far say, not the session's
    /// record: the record may already hold a response created after the speech that started
    /// this barge-in, and that response is the answer to it.
    fn barge_in(&mut self, cancelled: Option<ResponseId>, notes: &mut Vec<Note>) {
        if cancelled.is_none()
            && !self.session.ledger().session().config().auto_interrupt
            && self.capabilities.client_cancel
            && let Some(response_id) = self.current
            && self.session.ledger().in_progress() == Some(response_id)
            && let Err(error) = self.session.cancel_response(response_id)
        {
            notes.push(Note::Refused(format!("cancel: {error}")));
        }
        if self.speaker.queued() == 0 && self.backlog.is_empty() {
            return;
        }
        let playing = self.playing.take();
        let heard = self.speaker.stop();
        self.finished = true;
        let mut unheard: Vec<ResponseId> = Vec::new();
        for (response_id, _) in self.backlog.drain(..) {
            if Some(response_id) != playing && !unheard.contains(&response_id) {
                unheard.push(response_id);
            }
        }
        notes.push(Note::BargeIn {
            response_id: playing,
            heard,
        });
        if !self.capabilities.truncation || !self.is_open() {
            return;
        }
        let start = SampleClockPosition::new(0);
        let cuts = playing
            .zip(heard)
            .into_iter()
            .chain(unheard.into_iter().zip(start));
        for (response_id, heard) in cuts {
            if let Err(error) = self.session.truncate_output(response_id, heard) {
                notes.push(Note::Refused(format!("truncate: {error}")));
            }
        }
    }

    /// Answers a call of the demo tool by call id. A model that takes responses from the caller
    /// is asked for the next one once the proposing response has ended.
    fn answer(&mut self, call: &SpeechToolCallProposed, notes: &mut Vec<Note>) {
        let offered = self
            .session
            .ledger()
            .tools()
            .iter()
            .find(|tool| tool.tool_id() == call.tool_id)
            .map(|tool| tool.name().to_owned());
        let result = if offered.as_deref() == Some(TIME_TOOL) {
            (self.clock)()
        } else {
            serde_json::json!({"error": "no such tool"})
        };
        match self.session.submit_tool_result(&call.call_id, result) {
            Ok(()) => {
                if self.capabilities.manual_response {
                    self.follow_up.insert(call.response_id);
                    if self.current != Some(call.response_id) {
                        self.follow(call.response_id, notes);
                    }
                }
            }
            Err(error) => notes.push(Note::Refused(format!("tool result: {error}"))),
        }
    }

    /// Asks for the response that speaks the answers to the calls `ended` proposed.
    fn follow(&mut self, ended: ResponseId, notes: &mut Vec<Note>) {
        if !self.follow_up.remove(&ended) {
            return;
        }
        if let Err(error) = self.session.create_response() {
            notes.push(Note::Refused(format!(
                "response after a tool call: {error}"
            )));
        }
    }

    /// Stops playout for good: the session has ended.
    fn silence(&mut self) {
        let _ = self.speaker.stop();
        self.backlog.clear();
        self.playing = None;
        self.finished = true;
    }

    /// Closes the session and handles what it reports last.
    pub fn close(&mut self) -> Vec<Note> {
        let mut notes = Vec::new();
        if let Err(error) = self.session.close() {
            notes.push(Note::Refused(error.to_string()));
        }
        notes.extend(self.pump_events(usize::MAX));
        notes
    }
}
