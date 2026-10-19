//! [`MockSpeechModel`], the scripted mock of the `inference-duplex` contract.
//!
//! The mock hears speech in any frame holding a byte other than its encoding's silence
//! ([`AudioEncoding::silence`]) and silence in any other. A frame of speech after silence is the
//! start of speech; a frame of silence after speech is its end, and with turn detection on it
//! ends the user turn, as a commit does with turn detection off. Where the session reports that
//! speech is [`SpeechReports`]: where it hears it, or, for [`MockSpeechModel::server_turns`],
//! where the Nova Sonic binding records it.
//!
//! Its recogniser reads the speech bytes of a turn, its silence bytes left out, as text: when
//! they read as words, that is what it heard; otherwise it heard the `heard` text of the next
//! [`ScriptedTurn`]. Each turn takes the next scripted turn, whose [`Reply`] says what the model
//! answers, `{heard}` in it replaced by what the recogniser heard, so a reply derives from the
//! input it received rather than from the script alone.
//!
//! A turn that ends while a response is in progress and was not interrupted is input during
//! output: [`MockSpeechModel::native`] speaks its reply on that same response before it
//! completes, which gives the evidence `inference_duplex::conformance::native_duplex_evidence`
//! asks for; [`MockSpeechModel::restarting`] cancels the response and answers in a new one, which
//! fails it.
//!
//! A response streams one step per [`SpeechSessionHandle::next_event`]: its transcript, then
//! [`AUDIO_FRAMES`] frames of 20 ms silence in the output format, then its usage and its
//! completion. A [`Reply::Call`] proposes its tool call instead and completes; once the call is
//! answered, the next response says the reply's `then` with `{result}` replaced by the result;
//! a model that creates every response starts it by itself.
//!
//! Time is the mock's own: it starts when the session opens, and a `next_event` with nothing to
//! deliver advances it by `wait`, so the liveness bound is reached by waiting, deterministically.
//! [`Faults`] stop the provider after a number of events (a stall) or close its transport.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, ResponseUsage, SessionConfig, SessionError,
    SessionEvent, SessionFailure, SessionFailureKind, SessionLedger, SpeechCallId,
    SpeechCapabilities, SpeechSessionHandle, SpeechSessionId, TokenCounts, TranscriptionUsage,
    TuningSupport, TurnDetection,
};
use inference_tools::Json;

/// The frames of output audio each spoken response streams.
pub const AUDIO_FRAMES: u32 = 2;

/// One user turn of a script: what the user is heard to say when the audio does not read as
/// words, and what the model replies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptedTurn {
    pub heard: String,
    pub reply: Reply,
}

/// What a response does; `{heard}` in its text is replaced by what the recogniser heard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Speaks the text: transcript and audio.
    Say(String),
    /// Proposes a call of the session's tool named `tool` with `arguments`; the response after
    /// the call is answered says `then`, `{result}` replaced by the result.
    Call {
        tool: String,
        arguments: Json,
        then: String,
    },
}

impl Reply {
    fn heard(self, heard: &str) -> Self {
        match self {
            Self::Say(text) => Self::Say(text.replace("{heard}", heard)),
            Self::Call {
                tool,
                arguments,
                then,
            } => Self::Call {
                tool,
                arguments,
                then: then.replace("{heard}", heard),
            },
        }
    }
}

/// Faults a script injects, counted in provider events.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Faults {
    /// The provider sends nothing more after this many events.
    pub stall_after: Option<usize>,
    /// The provider's transport closes after this many events.
    pub close_after: Option<usize>,
}

/// Where a session reports the user's speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechReports {
    /// Where the mock hears it: speech started at the first frame of speech after silence,
    /// stopped at the first frame of silence after speech, as a provider that reports its voice
    /// activity detection does.
    Heard,
    /// Where a provider without voice activity events gives the first sign of it, as the Nova
    /// Sonic binding records it: speech started once per user turn, at the provider's
    /// interruption notice when a response was created since the turn before (also one already
    /// completed, whose audio the caller may still be playing), or else just before the
    /// provider commits the turn; never its end.
    ProviderNotice,
}

/// The scripted mock native speech model, in three configurations.
#[derive(Debug, Clone)]
pub struct MockSpeechModel {
    adapter: Adapter,
    capabilities: SpeechCapabilities,
    script: Vec<ScriptedTurn>,
    faults: Faults,
    restarts: bool,
    speech: SpeechReports,
}

/// `pcm_s16le` mono at `sample_rate`.
#[must_use]
pub fn pcm(sample_rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: std::num::NonZeroU32::new(sample_rate).unwrap_or(std::num::NonZeroU32::MIN),
        channels: std::num::NonZeroU16::MIN,
    }
}

/// A 20 ms frame of `format` at sample `position`, every byte `byte`.
#[must_use]
pub fn frame(format: AudioFormat, position: u64, byte: u8) -> AudioFrame {
    let samples = format.sample_rate.get() / 50;
    AudioFrame {
        format,
        position: SampleClockPosition::new(position).unwrap_or(SampleClockPosition::MAX),
        sample_count: samples,
        data: vec![byte; usize::try_from(format.data_len(samples)).unwrap_or(0)],
    }
}

/// A 20 ms frame of `format` at sample `position` that says `text` to the mock's recogniser: the
/// text's bytes, then silence.
#[must_use]
pub fn saying(format: AudioFormat, position: u64, text: &str) -> AudioFrame {
    let mut spoken = frame(format, position, format.encoding.silence());
    for (slot, byte) in spoken.data.iter_mut().zip(text.bytes()) {
        *slot = byte;
    }
    spoken
}

impl MockSpeechModel {
    /// A model that takes every option: PCM at 16 and 24 kHz both ways, every turn detection
    /// mode and its tuning, manual commit, responses and interruption, client cancel, truncation
    /// and tools. It answers input during output on the same response, and its adapter claims
    /// `native_duplex`.
    #[must_use]
    pub fn native(script: Vec<ScriptedTurn>) -> Self {
        Self::with(
            SpeechCapabilities {
                input_formats: vec![pcm(16_000), pcm(24_000)],
                output_formats: vec![pcm(16_000), pcm(24_000)],
                turn_detection: TurnDetection::ALL.to_vec(),
                turn_tuning: TuningSupport::ALL,
                manual_commit: true,
                manual_response: true,
                manual_interrupt: true,
                client_cancel: true,
                truncation: true,
                tool_calling: true,
            },
            vec![
                Capability::OutputStreaming,
                Capability::IncrementalInput,
                Capability::ConcurrentInputOutput,
                Capability::NativeDuplex,
                Capability::ToolCalling,
                Capability::Interruptible,
            ],
            script,
        )
    }

    /// [`MockSpeechModel::native`]'s options, but input during output cancels the response and
    /// is answered in a new one: an orchestrated restart. Its adapter claims
    /// `concurrent_input_output`, never `native_duplex`.
    #[must_use]
    pub fn restarting(script: Vec<ScriptedTurn>) -> Self {
        let mut model = Self::native(script);
        let adapter = Self::adapter_claiming(vec![
            Capability::OutputStreaming,
            Capability::IncrementalInput,
            Capability::ConcurrentInputOutput,
            Capability::ToolCalling,
            Capability::Interruptible,
        ]);
        model.adapter = adapter;
        model.restarts = true;
        model
    }

    /// A model whose turns are its own, as Nova Sonic's: server voice activity detection only
    /// with a sensitivity to tune, responses created and interrupted by the provider, a tool
    /// result answered in a new response by itself, no cancellation or truncation by the caller,
    /// PCM at 16 kHz in, tools. It reports speech as [`SpeechReports::ProviderNotice`].
    #[must_use]
    pub fn server_turns(script: Vec<ScriptedTurn>) -> Self {
        let model = Self::with(
            SpeechCapabilities {
                input_formats: vec![pcm(16_000)],
                output_formats: vec![pcm(16_000), pcm(24_000)],
                turn_detection: vec![TurnDetection::ServerVad],
                turn_tuning: TuningSupport {
                    sensitivity: true,
                    ..TuningSupport::NONE
                },
                manual_commit: false,
                manual_response: false,
                manual_interrupt: false,
                client_cancel: false,
                truncation: false,
                tool_calling: true,
            },
            vec![
                Capability::OutputStreaming,
                Capability::IncrementalInput,
                Capability::ConcurrentInputOutput,
                Capability::ToolCalling,
                Capability::Interruptible,
            ],
            script,
        );
        model.with_speech_reports(SpeechReports::ProviderNotice)
    }

    /// The same model reporting the user's speech as `speech` says.
    #[must_use]
    pub const fn with_speech_reports(mut self, speech: SpeechReports) -> Self {
        self.speech = speech;
        self
    }

    /// The same model with `faults` injected into every session it opens.
    #[must_use]
    pub const fn with_faults(mut self, faults: Faults) -> Self {
        self.faults = faults;
        self
    }

    fn adapter_claiming(claims: Vec<Capability>) -> Adapter {
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "inference-testkit scripted speech".to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::InProcess,
            locality: Locality::SameProcess,
            capabilities: claims,
            max_concurrency: 1,
        })
        .unwrap_or_else(|_| unreachable!("a concurrency of one is positive"));
        adapter
    }

    fn with(
        capabilities: SpeechCapabilities,
        claims: Vec<Capability>,
        script: Vec<ScriptedTurn>,
    ) -> Self {
        let adapter = Self::adapter_claiming(claims);
        Self {
            adapter,
            capabilities,
            script,
            faults: Faults::default(),
            restarts: false,
            speech: SpeechReports::Heard,
        }
    }
}

impl NativeSpeechModel for MockSpeechModel {
    type Session = MockSpeechSession;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SessionConfig) -> Result<MockSpeechSession, OpenError> {
        let now = Instant::now();
        let (ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &self.adapter,
            &self.capabilities,
            config,
            now,
        )?;
        Ok(MockSpeechSession {
            ledger,
            script: self.script.iter().cloned().collect(),
            faults: self.faults,
            restarts: self.restarts,
            now,
            speaking: false,
            spoken: Vec::new(),
            pending: None,
            after_result: None,
            plan: VecDeque::new(),
            sent: 0,
            emitted: 0,
            calls: 0,
            speech: self.speech,
            answered: false,
            reported: false,
        })
    }
}

/// One step of a response in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Transcript(String),
    Audio,
    Call { tool: String, arguments: Json },
    Usage,
    Complete,
}

/// A session of [`MockSpeechModel`].
#[derive(Debug, Clone)]
pub struct MockSpeechSession {
    ledger: SessionLedger,
    script: VecDeque<ScriptedTurn>,
    faults: Faults,
    restarts: bool,
    now: Instant,
    speaking: bool,
    spoken: Vec<u8>,
    pending: Option<Reply>,
    after_result: Option<String>,
    plan: VecDeque<Step>,
    sent: usize,
    emitted: u64,
    calls: u32,
    speech: SpeechReports,
    /// A response was created since the user's last turn began.
    answered: bool,
    /// The user's speech of the current turn was reported.
    reported: bool,
}

impl MockSpeechSession {
    /// Reports that the user started speaking at `position`, once per user turn when the
    /// provider's notice is what reports it.
    fn report_speech(&mut self, position: SampleClockPosition) -> Result<(), SessionError> {
        if self.speech == SpeechReports::ProviderNotice && self.reported {
            return Ok(());
        }
        self.ledger.speech_started(position, self.now)?;
        self.reported = true;
        self.sent += 1;
        Ok(())
    }

    /// Whether the model creates every response, and so answers a tool result by itself.
    fn creates_responses(&self) -> bool {
        !self.ledger.capabilities().manual_response
    }

    /// Starts a response saying what is pending.
    fn start_response(&mut self) -> Result<(), SessionError> {
        let response_id = ResponseId::generate();
        self.ledger.response_created(response_id, self.now)?;
        self.sent += 1;
        self.emitted = 0;
        self.answered = true;
        self.plan = match self.pending.take() {
            Some(Reply::Say(text)) => Self::spoken(text),
            Some(Reply::Call {
                tool,
                arguments,
                then,
            }) => {
                self.after_result = Some(then);
                VecDeque::from([Step::Call { tool, arguments }, Step::Usage, Step::Complete])
            }
            None => VecDeque::from([Step::Usage, Step::Complete]),
        };
        Ok(())
    }

    fn spoken(text: String) -> VecDeque<Step> {
        let mut steps = VecDeque::from([Step::Transcript(text)]);
        steps.extend((0..AUDIO_FRAMES).map(|_| Step::Audio));
        steps.extend([Step::Usage, Step::Complete]);
        steps
    }

    /// What the recogniser heard in the turn's speech bytes, when they read as words.
    fn recognised(&mut self) -> Option<String> {
        let bytes = std::mem::take(&mut self.spoken);
        let text = String::from_utf8(bytes).ok()?;
        let words = text
            .chars()
            .all(|character| character.is_ascii_graphic() || character == ' ')
            && text
                .chars()
                .any(|character| character.is_ascii_alphabetic());
        words.then(|| text.trim().to_owned())
    }

    /// The user turn ended: the next turn of the script is taken, what was heard is
    /// transcribed, and the reply is spoken on the response in progress, in a new one after a
    /// restart, or awaits the next response, which `auto_response` creates at once.
    fn end_turn(&mut self) -> Result<(), SessionError> {
        let recognised = self.recognised();
        let Some(turn) = self.script.pop_front() else {
            return Ok(());
        };
        let heard = recognised.unwrap_or(turn.heard);
        let reply = turn.reply.heard(&heard);
        self.ledger.input_transcribed(heard, self.now)?;
        self.ledger.transcription_usage(
            TranscriptionUsage {
                input_tokens: 10,
                output_tokens: 3,
            },
            self.now,
        )?;
        self.sent += 2;
        if let Some(response_id) = self.ledger.in_progress() {
            if self.restarts {
                self.ledger.response_cancelled(response_id, self.now)?;
                self.plan.clear();
                self.pending = Some(reply);
                return self.start_response();
            }
            if let Reply::Say(text) = reply {
                let at = self
                    .plan
                    .iter()
                    .position(|step| matches!(step, Step::Usage | Step::Complete))
                    .unwrap_or(self.plan.len());
                for (offset, step) in Self::spoken(text)
                    .into_iter()
                    .filter(|step| !matches!(step, Step::Usage | Step::Complete))
                    .enumerate()
                {
                    self.plan.insert(at + offset, step);
                }
            }
            return Ok(());
        }
        self.pending = Some(reply);
        if self.ledger.session().config().auto_response {
            self.start_response()?;
        }
        Ok(())
    }

    /// Performs the next step of the response in progress.
    fn step(&mut self) -> Result<(), SessionError> {
        let Some(response_id) = self.ledger.in_progress() else {
            self.plan.clear();
            return Ok(());
        };
        let Some(step) = self.plan.pop_front() else {
            return Ok(());
        };
        let now = self.now;
        match step {
            Step::Transcript(text) => self.ledger.output_transcript(response_id, text, now)?,
            Step::Audio => {
                let format = *self.ledger.session().output_format();
                let audio = frame(format, self.emitted, format.encoding.silence());
                let samples = u64::from(audio.sample_count);
                self.ledger.output_audio(response_id, audio, now)?;
                self.emitted += samples;
            }
            Step::Call { tool, arguments } => {
                self.calls += 1;
                let call_id = SpeechCallId::new(format!("call_{}", self.calls));
                self.ledger
                    .tool_call(call_id, response_id, &tool, arguments, now)?;
            }
            Step::Usage => self.ledger.response_usage(
                response_id,
                ResponseUsage {
                    input: TokenCounts {
                        text_tokens: 20,
                        audio_tokens: 50,
                        cached_tokens: 0,
                    },
                    output: TokenCounts {
                        text_tokens: 8,
                        audio_tokens: 30,
                        cached_tokens: 0,
                    },
                },
                now,
            )?,
            Step::Complete => {
                self.ledger.response_completed(response_id, now)?;
                if self.creates_responses() && self.pending.is_some() {
                    self.start_response()?;
                }
            }
        }
        self.sent += 1;
        Ok(())
    }
}

impl SpeechSessionHandle for MockSpeechSession {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        let position = frame.position;
        let silence = frame.format.encoding.silence();
        let speech: Vec<u8> = frame
            .data
            .iter()
            .copied()
            .filter(|byte| *byte != silence)
            .collect();
        self.ledger.accept_input(frame)?;
        if !speech.is_empty() {
            self.spoken.extend(&speech);
        }
        let heard = self.speech == SpeechReports::Heard;
        if !speech.is_empty() && !self.speaking {
            self.speaking = true;
            if heard || self.answered {
                self.report_speech(position)?;
            }
            self.answered = false;
            if let Some(response_id) = self.ledger.in_progress()
                && self.ledger.session().config().auto_interrupt
            {
                self.ledger.response_cancelled(response_id, self.now)?;
                self.plan.clear();
            }
        } else if speech.is_empty() && self.speaking {
            self.speaking = false;
            if heard {
                self.ledger.speech_stopped(position, self.now)?;
                self.sent += 1;
            }
            if self.ledger.session().turn_detection() != TurnDetection::Disabled {
                if !heard {
                    self.report_speech(position)?;
                }
                self.ledger.input_committed(self.now)?;
                self.reported = false;
                self.end_turn()?;
            }
        }
        Ok(())
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.ledger.commit_input(self.now)?;
        self.end_turn()
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.ledger.request_response(self.now)?;
        self.start_response()
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.ledger.cancel(response_id)?;
        self.plan.clear();
        Ok(())
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.ledger.truncate(response_id, heard)
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        let rendered = result.to_string();
        self.ledger.answer(call_id, result)?;
        if let Some(then) = self.after_result.take() {
            self.pending = Some(Reply::Say(then.replace("{result}", &rendered)));
            if self.creates_responses() && self.ledger.in_progress().is_none() {
                self.start_response()?;
            }
        }
        Ok(())
    }

    fn close(&mut self) -> Result<(), SessionError> {
        self.ledger.close()?;
        self.plan.clear();
        Ok(())
    }

    fn next_event(&mut self, wait: Duration) -> Option<SessionEvent> {
        if let Some(event) = self.ledger.next_event() {
            return Some(event);
        }
        if self.ledger.session().require_open().is_err() {
            return None;
        }
        if self
            .faults
            .close_after
            .is_some_and(|after| self.sent >= after)
        {
            let _ = self.ledger.fail(SessionFailure {
                kind: SessionFailureKind::TransportClosed,
                detail: "the scripted transport closed".to_owned(),
            });
            return self.ledger.next_event();
        }
        let stalled = self
            .faults
            .stall_after
            .is_some_and(|after| self.sent >= after);
        if !stalled && !self.plan.is_empty() {
            let _ = self.step();
            return self.ledger.next_event();
        }
        self.now += wait;
        self.ledger.check_liveness(self.now);
        self.ledger.next_event()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use inference_duplex::conformance::{
        NativeDuplexProbe, NoNativeDuplex, native_duplex_evidence,
    };
    use inference_duplex::inference_audio::SampleClockPosition;
    use inference_duplex::{
        NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionEvent, SessionFailureKind,
        SpeechSessionHandle, SpeechSessionState, TuningOption, TurnDetection, TurnSensitivity,
        TurnTuning, Unsupported,
    };
    use inference_tools::{DeclareTool, SideEffectClass, Tool};
    use serde_json::json;

    use super::{Faults, MockSpeechModel, Reply, ScriptedTurn, frame, pcm, saying};

    const WAIT: Duration = Duration::from_millis(100);

    fn turn(heard: &str, reply: &str) -> ScriptedTurn {
        ScriptedTurn {
            heard: heard.to_owned(),
            reply: Reply::Say(reply.to_owned()),
        }
    }

    fn weather() -> Tool {
        Tool::declare(DeclareTool {
            name: "weather".to_owned(),
            schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
            schema_revision: "1".to_owned(),
            side_effect: SideEffectClass::ReadOnly,
            requires_confirmation: false,
        })
        .0
    }

    fn config(turn_detection: TurnDetection, auto: bool) -> SessionConfig {
        SessionConfig {
            voice: "calm".to_owned(),
            instructions: "answer briefly".to_owned(),
            input_format: pcm(16_000),
            output_format: pcm(16_000),
            turn_detection,
            turn_tuning: TurnTuning::default(),
            auto_response: auto,
            auto_interrupt: auto,
            tools: vec![weather()],
            idle_limit: Duration::from_secs(5),
        }
    }

    /// Speaks `frames` frames of speech then one of silence, from sample `from`.
    fn speak(session: &mut impl SpeechSessionHandle, from: u64, frames: u64) {
        for index in 0..=frames {
            let byte = if index < frames { 9 } else { 0 };
            session
                .append_audio(frame(pcm(16_000), from + index * 320, byte))
                .expect("input in the session's format");
        }
    }

    fn drain(session: &mut impl SpeechSessionHandle) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        while let Some(event) = session.next_event(WAIT) {
            events.push(event);
            if events.len() > 200 {
                break;
            }
        }
        events
    }

    fn created(events: &[SessionEvent]) -> Vec<ResponseId> {
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ResponseCreated(created) => Some(created.response_id),
                _ => None,
            })
            .collect()
    }

    /// The response every output event of `events` belongs to.
    fn output_of(events: &[SessionEvent]) -> Vec<ResponseId> {
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::OutputAudio(audio) => Some(audio.response_id),
                SessionEvent::OutputTranscriptDelta(delta) => Some(delta.response_id),
                _ => None,
            })
            .collect()
    }

    fn transcript(events: &[SessionEvent], response_id: ResponseId) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::OutputTranscriptDelta(delta) if delta.response_id == response_id => {
                    Some(delta.text.as_str())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn audio_in_gives_audio_and_transcript_out_on_one_response_id() {
        let mut model = MockSpeechModel::native(vec![turn("what time is it", "It is noon")]);
        let mut session = model
            .open(config(TurnDetection::Disabled, false))
            .expect("the model takes the configuration");
        speak(&mut session, 0, 3);
        session.commit_input().expect("a manual commit");
        session.create_response().expect("no response in progress");
        let events = drain(&mut session);

        let responses = created(&events);
        assert_eq!(responses.len(), 1, "{events:#?}");
        let response = responses[0];
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::InputTranscribed(heard) if heard.text == "what time is it"
        )));
        let audio = events
            .iter()
            .filter(|event| matches!(event, SessionEvent::OutputAudio(_)))
            .count();
        assert_eq!(audio, 2, "two frames of output audio");
        assert_eq!(transcript(&events, response), "It is noon");
        assert!(output_of(&events).iter().all(|id| *id == response));
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::ResponseUsageRecorded(usage) if usage.response_id == response
        )));
        assert!(matches!(
            events.last(),
            Some(SessionEvent::ResponseCompleted(done)) if done.response_id == response
        ));
    }

    #[test]
    fn speech_during_a_response_cancels_it_and_the_next_response_has_a_new_id() {
        let mut model = MockSpeechModel::native(vec![
            turn("tell me a story", "Once upon a time"),
            turn("stop", "Sure"),
        ]);
        let mut session = model
            .open(config(TurnDetection::ServerVad, true))
            .expect("the model takes the configuration");
        speak(&mut session, 0, 3);
        let mut events = Vec::new();
        while !events
            .iter()
            .any(|event| matches!(event, SessionEvent::OutputTranscriptDelta(_)))
        {
            events.push(
                session
                    .next_event(WAIT)
                    .expect("the first response streams"),
            );
        }
        let first = created(&events)[0];

        speak(&mut session, 1_280, 2);
        let rest = drain(&mut session);
        let cancelled = rest
            .iter()
            .position(|event| matches!(event, SessionEvent::ResponseCancelled(c) if c.response_id == first))
            .expect("the first response is cancelled");
        let started = rest
            .iter()
            .position(|event| matches!(event, SessionEvent::SpeechStarted(_)))
            .expect("speech started");
        assert!(started < cancelled, "{rest:#?}");
        assert!(
            rest[cancelled..]
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::OutputAudio(audio) => Some(audio.response_id),
                    SessionEvent::OutputTranscriptDelta(delta) => Some(delta.response_id),
                    _ => None,
                })
                .all(|id| id != first),
            "nothing of the cancelled response is delivered after it: {rest:#?}"
        );
        let next = created(&rest);
        assert_eq!(next.len(), 1, "{rest:#?}");
        assert_ne!(next[0], first);
        assert_eq!(transcript(&rest, next[0]), "Sure");
    }

    #[test]
    fn barge_in_decided_by_the_caller_cancels_and_truncates_at_the_heard_sample() {
        let mut model = MockSpeechModel::native(vec![turn("hello", "Hello there")]);
        let mut caller = config(TurnDetection::ServerVad, true);
        caller.auto_interrupt = false;
        let mut session = model
            .open(caller)
            .expect("the model takes the configuration");
        speak(&mut session, 0, 2);
        let mut events = Vec::new();
        while !events
            .iter()
            .any(|event| matches!(event, SessionEvent::OutputAudio(_)))
        {
            events.push(session.next_event(WAIT).expect("the response streams"));
        }
        let response = created(&events)[0];
        session
            .append_audio(frame(pcm(16_000), 960, 9))
            .expect("speech during the response");
        let heard = SampleClockPosition::new(160).expect("a position");
        assert!(
            session.ledger().in_progress() == Some(response),
            "the provider leaves the response running"
        );
        session.cancel_response(response).expect("in progress");
        session
            .truncate_output(response, heard)
            .expect("within the emitted audio");
        let rest = drain(&mut session);
        assert!(rest.iter().any(|event| matches!(
            event,
            SessionEvent::OutputTruncated(truncated)
                if truncated.response_id == response && truncated.heard == heard
        )));
        assert!(
            rest.iter()
                .any(|event| matches!(event, SessionEvent::ResponseCancelled(_)))
        );
        assert!(output_of(&rest).is_empty(), "{rest:#?}");
    }

    #[test]
    fn a_tool_call_answered_on_the_session_yields_a_response_that_uses_its_result() {
        let mut model = MockSpeechModel::native(vec![ScriptedTurn {
            heard: "how warm is it in Berlin".to_owned(),
            reply: Reply::Call {
                tool: "weather".to_owned(),
                arguments: json!({"city": "Berlin"}),
                then: "It is {result} degrees".to_owned(),
            },
        }]);
        let mut session = model
            .open(config(TurnDetection::Disabled, false))
            .expect("the model takes the configuration");
        speak(&mut session, 0, 2);
        session.commit_input().expect("a manual commit");
        session.create_response().expect("no response in progress");
        let events = drain(&mut session);
        let (call_id, tool_id, arguments) = events
            .iter()
            .find_map(|event| match event {
                SessionEvent::SpeechToolCallProposed(proposed) => Some((
                    proposed.call_id.clone(),
                    proposed.tool_id,
                    proposed.arguments.clone(),
                )),
                _ => None,
            })
            .expect("a proposed call");
        assert_eq!(tool_id, session.ledger().tools()[0].tool_id());
        assert_eq!(arguments, json!({"city": "Berlin"}));

        session
            .submit_tool_result(&call_id, json!(21))
            .expect("the call awaits its result");
        session
            .create_response()
            .expect("the first response completed");
        let answer = drain(&mut session);
        let second = created(&answer);
        assert_eq!(second.len(), 1, "{answer:#?}");
        assert_ne!(second[0], created(&events)[0]);
        assert_eq!(transcript(&answer, second[0]), "It is 21 degrees");
        assert!(answer.iter().any(|event| matches!(
            event,
            SessionEvent::SpeechToolCallAnswered(answered) if answered.call_id == call_id
        )));
    }

    #[test]
    fn input_during_output_is_answered_on_the_same_response_before_it_completes() {
        let mut model = MockSpeechModel::native(vec![
            turn("what is the weather", "The weather is"),
            turn("in Berlin", " mild in Berlin"),
        ]);
        let mut caller = config(TurnDetection::ServerVad, true);
        caller.auto_interrupt = false;
        let mut session = model
            .open(caller)
            .expect("the model takes the configuration");
        speak(&mut session, 0, 2);
        let mut events = Vec::new();
        while !events
            .iter()
            .any(|event| matches!(event, SessionEvent::OutputTranscriptDelta(_)))
        {
            events.push(session.next_event(WAIT).expect("the response streams"));
        }
        speak(&mut session, 960, 2);
        events.extend(drain(&mut session));

        let responses = created(&events);
        assert_eq!(responses.len(), 1, "no second response id: {events:#?}");
        assert_eq!(
            transcript(&events, responses[0]),
            "The weather is mild in Berlin"
        );
        let heard = events
            .iter()
            .position(
                |event| matches!(event, SessionEvent::InputTranscribed(t) if t.text == "in Berlin"),
            )
            .expect("the second input is heard");
        let dependent = events
            .iter()
            .position(|event| matches!(event, SessionEvent::OutputTranscriptDelta(d) if d.text.contains("Berlin")))
            .expect("output that depends on it");
        let completed = events
            .iter()
            .position(|event| matches!(event, SessionEvent::ResponseCompleted(_)))
            .expect("the response completes");
        assert!(heard < dependent && dependent < completed, "{events:#?}");
    }

    #[test]
    fn a_silent_provider_yields_a_stall_failure_at_the_bound_and_a_closed_transport_at_once() {
        let mut stalling = MockSpeechModel::native(vec![turn("hi", "Hello")]).with_faults(Faults {
            stall_after: Some(0),
            close_after: None,
        });
        let mut session = stalling
            .open(config(TurnDetection::Disabled, false))
            .expect("the model takes the configuration");
        session.create_response().expect("no response in progress");
        let mut waits = 0;
        let failure = loop {
            match session.next_event(WAIT) {
                Some(SessionEvent::SpeechSessionFailed(failed)) => break failed.failure,
                Some(_) => {}
                None => waits += 1,
            }
            assert!(waits < 1_000, "the session never failed");
        };
        assert_eq!(failure.kind, SessionFailureKind::Stalled);
        assert_eq!(
            waits, 49,
            "five seconds of 100 ms waits, the fiftieth fails it"
        );
        assert_eq!(
            session.ledger().session().state(),
            SpeechSessionState::Failed
        );

        let mut closing = MockSpeechModel::native(vec![turn("hi", "Hello")]).with_faults(Faults {
            stall_after: None,
            close_after: Some(0),
        });
        let mut session = closing
            .open(config(TurnDetection::Disabled, false))
            .expect("the model takes the configuration");
        assert!(matches!(
            session.next_event(WAIT),
            Some(SessionEvent::SpeechSessionFailed(failed))
                if failed.failure.kind == SessionFailureKind::TransportClosed
        ));
        assert!(session.append_audio(frame(pcm(16_000), 0, 9)).is_err());
    }

    #[test]
    fn a_model_whose_turns_are_its_own_refuses_caller_turns_at_open() {
        let mut model = MockSpeechModel::server_turns(Vec::new());
        assert_eq!(
            model.open(config(TurnDetection::Disabled, false)).err(),
            Some(OpenError::Unsupported(Unsupported::TurnDetection(
                TurnDetection::Disabled
            )))
        );
        let mut barge_in_by_caller = config(TurnDetection::ServerVad, true);
        barge_in_by_caller.auto_interrupt = false;
        assert_eq!(
            model.open(barge_in_by_caller).err(),
            Some(OpenError::Unsupported(Unsupported::ManualInterrupt))
        );
        assert!(model.open(config(TurnDetection::ServerVad, true)).is_ok());
    }

    /// The probe of `native_duplex_evidence` for the mock: a question, then during the answer a
    /// place the mock's recogniser hears from the audio, which no script names.
    fn probe() -> NativeDuplexProbe {
        let mut probe_config = config(TurnDetection::ServerVad, true);
        probe_config.auto_interrupt = false;
        NativeDuplexProbe {
            config: probe_config,
            opening: vec![
                saying(pcm(16_000), 0, "what is the weather"),
                frame(pcm(16_000), 320, 0),
            ],
            during: vec![
                saying(pcm(16_000), 640, "in Lisbon"),
                frame(pcm(16_000), 960, 0),
            ],
            marker: "lisbon".to_owned(),
            wait: WAIT,
            patience: 20,
        }
    }

    fn weather_script() -> Vec<ScriptedTurn> {
        vec![
            turn("a question", "The weather is"),
            turn("a place", " mild {heard}"),
        ]
    }

    #[test]
    fn the_native_mock_gives_evidence_for_native_duplex_from_what_it_heard() {
        let evidence =
            native_duplex_evidence(&mut MockSpeechModel::native(weather_script()), &probe())
                .expect("the native mock answers input during output on the same response");
        assert_eq!(evidence.before, "The weather is");
        assert_eq!(evidence.after, " mild in Lisbon");
    }

    #[test]
    fn a_mock_that_restarts_on_input_during_output_gives_no_evidence_for_native_duplex() {
        let refused =
            native_duplex_evidence(&mut MockSpeechModel::restarting(weather_script()), &probe());
        assert!(
            matches!(refused, Err(NoNativeDuplex::Restarted { first, second }) if first != second),
            "{refused:?}"
        );
    }

    #[test]
    fn output_that_does_not_derive_from_the_input_gives_no_evidence() {
        let scripted = vec![
            turn("a question", "The weather is"),
            turn("a place", " mild everywhere"),
        ];
        let refused = native_duplex_evidence(&mut MockSpeechModel::native(scripted), &probe());
        assert!(
            matches!(refused, Err(NoNativeDuplex::NotDependent { .. })),
            "{refused:?}"
        );
    }

    #[test]
    fn a_model_without_client_cancel_or_tuning_refuses_them() {
        let mut model = MockSpeechModel::server_turns(vec![turn("hi", "Hello")]);
        let mut tuned = config(TurnDetection::ServerVad, true);
        tuned.turn_tuning.silence = Some(Duration::from_millis(500));
        assert_eq!(
            model.open(tuned).err(),
            Some(OpenError::Unsupported(Unsupported::TurnTuning(
                TuningOption::Silence
            )))
        );
        let mut sensitive = config(TurnDetection::ServerVad, true);
        sensitive.turn_tuning = TurnTuning {
            sensitivity: Some(TurnSensitivity::Low),
            ..TurnTuning::default()
        };
        let mut session = model.open(sensitive).expect("sensitivity is honoured");
        speak(&mut session, 0, 2);
        let response = session
            .ledger()
            .in_progress()
            .expect("the provider answers the turn by itself");
        assert_eq!(
            session.cancel_response(response),
            Err(inference_duplex::SessionError::Unsupported(
                Unsupported::ClientCancel
            ))
        );
    }

    fn speech_started(events: &[SessionEvent]) -> Vec<usize> {
        events
            .iter()
            .enumerate()
            .filter(|(_, event)| matches!(event, SessionEvent::SpeechStarted(_)))
            .map(|(index, _)| index)
            .collect()
    }

    /// The Nova Sonic binding reports speech once per user turn, at the provider's interruption
    /// notice (also over an answer already completed) or else at the turn's first transcript,
    /// and never its end; the mock that stands in for it reports it at the same points.
    #[test]
    fn the_server_turns_mock_reports_speech_where_the_nova_binding_does() {
        let mut model = MockSpeechModel::server_turns(vec![
            turn("tell me a story", "Once upon a time"),
            turn("stop", "Sure"),
        ]);
        let mut session = model
            .open(config(TurnDetection::ServerVad, true))
            .expect("the model takes the configuration");
        speak(&mut session, 0, 2);
        let first_turn = drain(&mut session);
        let committed = first_turn
            .iter()
            .position(|event| matches!(event, SessionEvent::InputCommitted(_)))
            .expect("the provider commits the turn");
        assert_eq!(
            speech_started(&first_turn),
            vec![committed - 1],
            "{first_turn:#?}"
        );
        assert!(
            !first_turn
                .iter()
                .any(|event| matches!(event, SessionEvent::SpeechStopped(_)))
        );
        let first = created(&first_turn)[0];

        for index in 0..2 {
            session
                .append_audio(frame(pcm(16_000), 960 + index * 320, 9))
                .expect("speech over the finished answer");
        }
        let spoken = drain(&mut session);
        assert_eq!(
            speech_started(&spoken),
            vec![1],
            "the notice, after the frame that carried it: {spoken:#?}"
        );
        assert!(
            !spoken
                .iter()
                .any(|event| matches!(event, SessionEvent::ResponseCancelled(_))),
            "a completed response is not cancelled: {spoken:#?}"
        );
        session
            .append_audio(frame(pcm(16_000), 1_600, 0))
            .expect("silence");
        let rest = drain(&mut session);
        assert!(speech_started(&rest).is_empty(), "{rest:#?}");
        assert!(
            rest.iter()
                .any(|event| matches!(event, SessionEvent::InputCommitted(_)))
        );
        assert_ne!(created(&rest)[0], first);

        let mut native = MockSpeechModel::native(vec![turn("hi", "Hello")]);
        let mut session = native
            .open(config(TurnDetection::ServerVad, true))
            .expect("the model takes the configuration");
        speak(&mut session, 0, 2);
        let events = drain(&mut session);
        assert_eq!(
            speech_started(&events),
            vec![1],
            "the native mock reports what it hears: {events:#?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, SessionEvent::SpeechStopped(_)))
        );
    }

    /// A model that creates every response answers a tool result in a new response by itself.
    #[test]
    fn the_server_turns_mock_answers_a_tool_result_by_itself() {
        let mut model = MockSpeechModel::server_turns(vec![ScriptedTurn {
            heard: "how warm is it".to_owned(),
            reply: Reply::Call {
                tool: "weather".to_owned(),
                arguments: json!({"city": "Berlin"}),
                then: "It is {result} degrees".to_owned(),
            },
        }]);
        let mut session = model
            .open(config(TurnDetection::ServerVad, true))
            .expect("the model takes the configuration");
        speak(&mut session, 0, 2);
        let events = drain(&mut session);
        let call_id = events
            .iter()
            .find_map(|event| match event {
                SessionEvent::SpeechToolCallProposed(proposed) => Some(proposed.call_id.clone()),
                _ => None,
            })
            .expect("a proposed call");
        session
            .submit_tool_result(&call_id, json!(21))
            .expect("the call awaits its result");
        let answer = drain(&mut session);
        let second = created(&answer);
        assert_eq!(second.len(), 1, "{answer:#?}");
        assert_eq!(transcript(&answer, second[0]), "It is 21 degrees");
        assert_eq!(
            session.ledger().session().state(),
            SpeechSessionState::Open,
            "not stalled"
        );
    }
}
