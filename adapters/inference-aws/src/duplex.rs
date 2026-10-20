//! Amazon Nova Sonic as an `inference-duplex` native speech model.
//!
//! [`NovaSonicSpeech`] implements [`NativeSpeechModel`] over the [`nova_sonic`] client, and
//! [`NovaSonicSpeechSession`] records every provider event through the contract's
//! [`SessionLedger`], so it refuses what every native speech model refuses and drops what the
//! ledger refuses: late audio of an interrupted response, anything after the session closed.
//!
//! What Nova Sonic takes is [`capabilities`]: 16-bit mono linear PCM at 8, 16 or 24 kHz both ways,
//! its own turn detection only, tuned by the endpointing sensitivity of Nova 2 Sonic
//! ([`endpointing_applies`]), every response created and every barge-in decided by the provider,
//! no cancellation or truncation by the caller, and tools. The adapter claims [`CLAIMS`], never
//! `native_duplex`: speech during a response interrupts it, and the provider answers in a new one.
//!
//! The contract is synchronous and the client is not: the model runs a Tokio runtime of its own,
//! and a session's reads and writes are tasks of it, so the session's operations never block on
//! the provider. Drive a session from a thread, not from inside an async task.
//!
//! How provider events map to the record:
//!
//! - a user text block of final stage is the provider's commit of the input, its text the input's
//!   transcript;
//! - the first assistant block after a commit (or after a tool result) starts a response, under a
//!   [`ResponseId`] the binding issues; every assistant block of that turn, by content id, belongs
//!   to it;
//! - assistant audio is the response's output audio, the final assistant text its transcript; the
//!   speculative preview is not delivered;
//! - a final text block ending `END_TURN`, or a tool block ending `TOOL_USE`, completes the
//!   response; the interruption notice or an `INTERRUPTED` end cancels it, as the provider's own
//!   interruption;
//! - Nova Sonic reports no voice activity, so the first sign of the user's speech in a turn is
//!   recorded as speech started, once per turn: the interruption notice (also over a response that
//!   already completed and whose audio the caller may still be playing), an `INTERRUPTED` end, or
//!   else the turn's first user transcript, just before its commit; no end of speech is recorded;
//! - a tool use is a call under the provider's `toolUseId`;
//! - usage deltas are summed per response: what is reported while a response is in progress counts
//!   toward it; between responses, input counts toward the next and output toward the last;
//! - a service error fails the session, a closed stream fails it as `transport_closed`.
//!
//! The response boundaries are inferred from the order of content blocks the Nova 2 user guide
//! and the AWS samples show (speculative text, audio, then final text ending `END_TURN`), not
//! read from recorded service traffic: the offline replay follows the same order, so only the
//! first live run (`tests/nova_sonic_duplex_live.rs`) checks it. Should the service end a turn on
//! another block, a response would complete late (at the next user turn or the completion end) or
//! drop the content after its end.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::mpsc::{self as std_mpsc, RecvTimeoutError};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, ResponseUsage, SessionConfig, SessionError,
    SessionEvent, SessionFailure, SessionFailureKind, SessionLedger, SpeechCallId,
    SpeechCapabilities, SpeechSessionHandle, SpeechSessionId, TokenCounts, TuningSupport,
    TurnDetection, TurnSensitivity,
};
use tokio::runtime::Runtime;
use tokio::sync::mpsc as tokio_mpsc;

use crate::nova_sonic::{
    self, AudioOutput, AudioRate, ContentKind, Endpointing, GenerationStage, NovaSonic,
    NovaSonicError, OutputContentEnd, OutputContentStart, OutputEvent, Role, ServiceError,
    ServiceErrorKind, Session, SessionReader, SessionWriter, StopReason, TextOutput, ToolSpec,
    ToolUse, Usage, redact,
};

/// The capabilities the adapter claims: output streaming, incremental input, input while output
/// streams, tool calling and interruption. Not `native_duplex`, which needs the evidence of
/// `inference_duplex::conformance::native_duplex_evidence` that Nova Sonic does not give.
pub const CLAIMS: [Capability; 5] = [
    Capability::OutputStreaming,
    Capability::IncrementalInput,
    Capability::ConcurrentInputOutput,
    Capability::ToolCalling,
    Capability::Interruptible,
];

/// Whether `model_id` names Nova 2 Sonic, whose session start takes an endpointing sensitivity;
/// a model id or an inference-profile ARN naming `nova-2-sonic`.
#[must_use]
pub fn endpointing_applies(model_id: &str) -> bool {
    model_id.contains("nova-2-sonic")
}

/// What Nova Sonic under `model_id` takes: `pcm_s16le` mono at 8, 16 and 24 kHz in and out, server
/// voice activity detection only, its sensitivity when [`endpointing_applies`], no commit,
/// response, barge-in, cancellation or truncation by the caller, and tools.
#[must_use]
pub fn capabilities(model_id: &str) -> SpeechCapabilities {
    let lpcm: Vec<AudioFormat> = [AudioRate::Hz8000, AudioRate::Hz16000, AudioRate::Hz24000]
        .into_iter()
        .map(lpcm)
        .collect();
    SpeechCapabilities {
        input_formats: lpcm.clone(),
        output_formats: lpcm,
        turn_detection: vec![TurnDetection::ServerVad],
        turn_tuning: TuningSupport {
            sensitivity: endpointing_applies(model_id),
            ..TuningSupport::NONE
        },
        manual_commit: false,
        manual_response: false,
        manual_interrupt: false,
        client_cancel: false,
        truncation: false,
        tool_calling: true,
    }
}

/// `pcm_s16le` mono at `rate`.
fn lpcm(rate: AudioRate) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate.hertz()).unwrap_or(NonZeroU32::MIN),
        channels: NonZeroU16::MIN,
    }
}

/// How a session reaches the provider: [`NovaSonic`] for the service, or an offline replay of it.
pub trait Connector {
    /// The model id it invokes, which decides whether [`endpointing_applies`].
    fn model_id(&self) -> &str;

    /// Opens a client session from `config`; the future runs on the binding's runtime.
    fn connect(
        &self,
        config: nova_sonic::SessionConfig,
    ) -> impl Future<Output = Result<Session, NovaSonicError>> + Send + 'static;
}

impl Connector for NovaSonic {
    fn model_id(&self) -> &str {
        NovaSonic::model_id(self)
    }

    fn connect(
        &self,
        config: nova_sonic::SessionConfig,
    ) -> impl Future<Output = Result<Session, NovaSonicError>> + Send + 'static {
        let nova = self.clone();
        async move { nova.start(&config).await }
    }
}

/// The binding's Tokio runtime, shut down without waiting so that dropping it never blocks.
#[derive(Debug)]
struct Pool(Option<Runtime>);

impl Pool {
    fn start() -> io::Result<Arc<Self>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("inference-aws-duplex")
            .enable_all()
            .build()?;
        Ok(Arc::new(Pool(Some(runtime))))
    }

    /// Runs `future` on the runtime and waits for its output on this thread.
    fn run<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Option<T> {
        let runtime = self.0.as_ref()?;
        let (done, result) = std_mpsc::channel();
        runtime.spawn(async move {
            let _ = done.send(future.await);
        });
        result.recv().ok()
    }

    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        if let Some(runtime) = &self.0 {
            runtime.spawn(future);
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

/// Nova Sonic as a native speech model: an adapter record, the [`capabilities`] of its model id,
/// and a [`Connector`].
#[derive(Debug)]
pub struct NovaSonicSpeech<C = NovaSonic> {
    connector: C,
    adapter: Adapter,
    capabilities: SpeechCapabilities,
    pool: Arc<Pool>,
}

impl NovaSonicSpeech<NovaSonic> {
    /// The model over `client`.
    ///
    /// # Errors
    ///
    /// The I/O error of starting the binding's runtime.
    pub fn new(client: NovaSonic) -> io::Result<Self> {
        Self::with_connector(client)
    }

    /// The model over [`NovaSonic::from_env`]: credentials and region from the AWS SDK's default
    /// chain, the model id of [`nova_sonic::model_id_from_env`].
    ///
    /// # Errors
    ///
    /// The I/O error of starting the binding's runtime.
    pub fn from_env() -> io::Result<Self> {
        let pool = Pool::start()?;
        let client = pool.run(NovaSonic::from_env()).ok_or_else(|| {
            io::Error::other("the runtime stopped while loading the configuration")
        })?;
        Ok(Self::declared(client, pool))
    }
}

impl<C: Connector> NovaSonicSpeech<C> {
    /// The model over `connector`, declared as an adapter of a hosted external provider whose
    /// backend names the model id, account identifiers removed.
    ///
    /// # Errors
    ///
    /// The I/O error of starting the binding's runtime.
    pub fn with_connector(connector: C) -> io::Result<Self> {
        Ok(Self::declared(connector, Pool::start()?))
    }

    fn declared(connector: C, pool: Arc<Pool>) -> Self {
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: format!("amazon-bedrock {}", redact(connector.model_id())),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: CLAIMS.to_vec(),
            max_concurrency: 1,
        })
        .unwrap_or_else(|_| unreachable!("a concurrency of one is positive"));
        let capabilities = capabilities(connector.model_id());
        Self {
            connector,
            adapter,
            capabilities,
            pool,
        }
    }

    /// The connector sessions are opened through.
    pub fn connector(&self) -> &C {
        &self.connector
    }

    /// The client configuration of an admitted `config`.
    fn client_config(config: &SessionConfig) -> nova_sonic::SessionConfig {
        let rate = |format: &AudioFormat| {
            AudioRate::try_from(format.sample_rate.get()).unwrap_or(AudioRate::Hz16000)
        };
        let mut client = nova_sonic::SessionConfig::new(config.instructions.clone());
        client.voice_id.clone_from(&config.voice);
        client.input_rate = rate(&config.input_format);
        client.output_rate = rate(&config.output_format);
        client.endpointing = config
            .turn_tuning
            .sensitivity
            .map(|sensitivity| match sensitivity {
                TurnSensitivity::Low => Endpointing::Low,
                TurnSensitivity::Medium => Endpointing::Medium,
                TurnSensitivity::High => Endpointing::High,
            });
        client.tools = config
            .tools
            .iter()
            .map(|tool| ToolSpec {
                name: tool.name().to_owned(),
                description: tool.name().to_owned(),
                input_schema: tool.schema().clone(),
            })
            .collect();
        client.read_timeout = config.idle_limit;
        client
    }
}

impl<C: Connector> NativeSpeechModel for NovaSonicSpeech<C> {
    type Session = NovaSonicSpeechSession;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    /// Opens a session once the [`SessionLedger`] admits `config`, then connects. A connection
    /// the service refused fails as `rejected`, one that never reached it as `transport_closed`.
    fn open(&mut self, config: SessionConfig) -> Result<NovaSonicSpeechSession, OpenError> {
        let client = Self::client_config(&config);
        let (ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &self.adapter,
            &self.capabilities,
            config,
            Instant::now(),
        )?;
        let session = self
            .pool
            .run(self.connector.connect(client))
            .ok_or_else(|| {
                OpenError::Failed(failure(
                    SessionFailureKind::TransportClosed,
                    "the runtime stopped while connecting",
                ))
            })?
            .map_err(|error| OpenError::Failed(open_failure(&error)))?;
        let Session { writer, reader } = session;
        let (arrived, arrivals) = std_mpsc::channel();
        let (commands, queued) = tokio_mpsc::unbounded_channel();
        self.pool.spawn(forward(reader, arrived.clone()));
        self.pool.spawn(write(writer, queued, arrived));
        Ok(NovaSonicSpeechSession {
            ledger,
            commands,
            arrivals,
            turns: Turns {
                open: true,
                ..Turns::default()
            },
            _pool: Arc::clone(&self.pool),
        })
    }
}

/// What the session asks of the writer task.
#[derive(Debug)]
enum Command {
    Audio(Vec<u8>),
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    Close,
}

/// What reached the session from the provider's side, and when.
#[derive(Debug)]
struct Arrival {
    at: Instant,
    what: Received,
}

#[derive(Debug)]
enum Received {
    Event(OutputEvent),
    /// The output stream ended.
    Ended,
    /// The client failed: an undecodable frame, or the input side closed.
    Failed(NovaSonicError),
    /// The stream was quiet for the client's read timeout; not provider traffic.
    Quiet,
}

/// Forwards the reader's events until the stream ends or the session is gone.
async fn forward(mut reader: SessionReader, arrived: std_mpsc::Sender<Arrival>) {
    loop {
        let what = match reader.recv().await {
            Ok(Some(event)) => Received::Event(event),
            Ok(None) => Received::Ended,
            Err(NovaSonicError::Stall { .. }) => Received::Quiet,
            Err(error) => Received::Failed(error),
        };
        let ended = matches!(what, Received::Ended);
        let arrival = Arrival {
            at: Instant::now(),
            what,
        };
        if arrived.send(arrival).is_err() || ended {
            return;
        }
    }
}

/// Sends what the session asks until it closes or is gone; a failed send is reported.
async fn write(
    mut writer: SessionWriter,
    mut queued: tokio_mpsc::UnboundedReceiver<Command>,
    arrived: std_mpsc::Sender<Arrival>,
) {
    while let Some(command) = queued.recv().await {
        let sent = match command {
            Command::Audio(pcm) => writer.send_audio(&pcm).await,
            Command::ToolResult {
                tool_use_id,
                content,
            } => writer.send_tool_result(&tool_use_id, &content).await,
            Command::Close => {
                let _ = writer.close().await;
                return;
            }
        };
        if let Err(error) = sent {
            let _ = arrived.send(Arrival {
                at: Instant::now(),
                what: Received::Failed(error),
            });
            return;
        }
    }
}

fn failure(kind: SessionFailureKind, detail: &str) -> SessionFailure {
    SessionFailure {
        kind,
        detail: detail.to_owned(),
    }
}

/// The failure of a connection that did not open: refused by the service, or never reaching it.
fn open_failure(error: &NovaSonicError) -> SessionFailure {
    let kind = match error {
        NovaSonicError::Connect(message) if message.starts_with(nova_sonic::SERVICE_ERROR) => {
            SessionFailureKind::Rejected
        }
        _ => SessionFailureKind::TransportClosed,
    };
    failure(kind, &error.to_string())
}

/// The failure a service error stands for: a refused request, a lost transport, or the
/// provider's own error.
fn service_failure(error: &ServiceError) -> SessionFailure {
    let kind = match &error.kind {
        ServiceErrorKind::Validation | ServiceErrorKind::Throttling => SessionFailureKind::Rejected,
        ServiceErrorKind::Other(name) if name == "Transport" => SessionFailureKind::TransportClosed,
        _ => SessionFailureKind::ProviderError,
    };
    failure(kind, &error.to_string())
}

/// The failure a client error stands for.
fn client_failure(error: &NovaSonicError) -> SessionFailure {
    let kind = match error {
        NovaSonicError::Decode(_) => SessionFailureKind::ProviderError,
        _ => SessionFailureKind::TransportClosed,
    };
    failure(kind, &error.to_string())
}

/// One content block the provider started.
#[derive(Debug, Clone)]
struct Block {
    role: Role,
    kind: ContentKind,
    stage: Option<GenerationStage>,
    rate: Option<u32>,
    response: Option<ResponseId>,
}

impl Block {
    fn speculative(&self) -> bool {
        self.stage == Some(GenerationStage::Speculative)
    }
}

/// The provider's turns as the binding follows them: its content blocks, the responses issued
/// for them, and the usage summed per response.
#[derive(Debug, Default)]
struct Turns {
    blocks: HashMap<String, Block>,
    /// Whether the next assistant block starts a response: after a commit or a tool result.
    open: bool,
    /// The response issued last.
    last: Option<ResponseId>,
    /// Samples of output audio the last response emitted.
    emitted: u64,
    /// Usage reported before the next response began.
    pending: ResponseUsage,
    totals: HashMap<ResponseId, ResponseUsage>,
    /// Whether the user's speech of the current turn was reported.
    speech_reported: bool,
    /// The sample after the last input frame accepted.
    input_end: u64,
}

/// A session of [`NovaSonicSpeech`].
#[derive(Debug)]
pub struct NovaSonicSpeechSession {
    ledger: SessionLedger,
    commands: tokio_mpsc::UnboundedSender<Command>,
    arrivals: std_mpsc::Receiver<Arrival>,
    turns: Turns,
    _pool: Arc<Pool>,
}

impl NovaSonicSpeechSession {
    fn send(&mut self, command: Command) {
        if self.commands.send(command).is_err() {
            self.transport_closed();
        }
    }

    fn transport_closed(&mut self) {
        let _ = self.ledger.fail(failure(
            SessionFailureKind::TransportClosed,
            "the provider's stream closed",
        ));
    }

    fn record(&mut self, arrival: Arrival) {
        let at = arrival.at;
        match arrival.what {
            Received::Quiet => {}
            Received::Ended => self.transport_closed(),
            Received::Failed(error) => {
                let _ = self.ledger.fail(client_failure(&error));
            }
            Received::Event(event) => {
                self.ledger.heard(at);
                self.translate(event, at);
            }
        }
    }

    /// Records one provider event; what the ledger refuses is dropped.
    fn translate(&mut self, event: OutputEvent, at: Instant) {
        match event {
            OutputEvent::ContentStart(start) => self.content_start(start, at),
            OutputEvent::TextOutput(text) => self.text(text, at),
            OutputEvent::AudioOutput(audio) => self.audio(audio, at),
            OutputEvent::ToolUse(tool) => self.tool_use(tool, at),
            OutputEvent::ContentEnd(end) => self.content_end(&end, at),
            OutputEvent::Interrupted { content_id, .. } => {
                self.report_speech(at);
                let response = self
                    .turns
                    .blocks
                    .get(&content_id)
                    .and_then(|block| block.response)
                    .or_else(|| self.ledger.in_progress());
                if let Some(response) = response {
                    let _ = self.ledger.response_cancelled(response, at);
                }
            }
            OutputEvent::Usage(usage) => self.usage(&usage, at),
            OutputEvent::CompletionEnd { .. } => {
                if let Some(response) = self.ledger.in_progress() {
                    let _ = self.ledger.response_completed(response, at);
                }
            }
            OutputEvent::Error(error) => {
                let _ = self.ledger.fail(service_failure(&error));
            }
            OutputEvent::CompletionStart(_) | OutputEvent::Unknown { .. } => {}
        }
    }

    /// Records that the user started speaking, once per user turn: Nova Sonic sends no voice
    /// activity events, so its interruption notice, or failing that the first transcript of the
    /// turn, is the first sign of speech. The position is the end of the input sent so far, not
    /// the onset of the speech, which the provider does not report.
    fn report_speech(&mut self, at: Instant) {
        if self.turns.speech_reported {
            return;
        }
        let position =
            SampleClockPosition::new(self.turns.input_end).unwrap_or(SampleClockPosition::MAX);
        if self.ledger.speech_started(position, at).is_ok() {
            self.turns.speech_reported = true;
        }
    }

    fn content_start(&mut self, start: OutputContentStart, at: Instant) {
        let mut block = Block {
            role: start.role,
            kind: start.kind,
            stage: start.stage,
            rate: start.audio_rate_hertz,
            response: None,
        };
        match (&block.role, &block.kind) {
            (Role::User, ContentKind::Text) if !block.speculative() => {
                self.report_speech(at);
                if let Some(response) = self.ledger.in_progress() {
                    let _ = self.ledger.response_completed(response, at);
                }
                if self.ledger.input_committed(at).is_ok() {
                    self.turns.open = true;
                    self.turns.speech_reported = false;
                }
            }
            (Role::User, _) => {}
            (Role::Assistant | Role::Tool, _) | (_, ContentKind::Tool) => {
                block.response = match self.ledger.in_progress() {
                    Some(response) => Some(response),
                    None if self.turns.open => self.create(at),
                    None => self.turns.last,
                };
            }
            _ => {}
        }
        self.turns.blocks.insert(start.content_id, block);
    }

    /// Issues a response for the provider's new turn, its input usage reported so far with it.
    fn create(&mut self, at: Instant) -> Option<ResponseId> {
        let response = ResponseId::generate();
        self.ledger.response_created(response, at).ok()?;
        self.turns.open = false;
        self.turns.last = Some(response);
        self.turns.emitted = 0;
        let pending = std::mem::take(&mut self.turns.pending);
        if pending != ResponseUsage::default() {
            self.add_usage(response, pending, at);
        }
        Some(response)
    }

    fn text(&mut self, text: TextOutput, at: Instant) {
        let Some(block) = self.turns.blocks.get(&text.content_id) else {
            return;
        };
        if block.speculative() {
            return;
        }
        match (&block.role, block.response) {
            (Role::User, _) => {
                let _ = self.ledger.input_transcribed(text.content, at);
            }
            (Role::Assistant, Some(response)) => {
                let _ = self.ledger.output_transcript(response, text.content, at);
            }
            _ => {}
        }
    }

    fn audio(&mut self, audio: AudioOutput, at: Instant) {
        let Some(block) = self.turns.blocks.get(&audio.content_id) else {
            return;
        };
        let Some(response) = block.response else {
            return;
        };
        let mut format = *self.ledger.session().output_format();
        if let Some(rate) = block.rate.and_then(NonZeroU32::new) {
            format.sample_rate = rate;
        }
        if !audio.pcm.len().is_multiple_of(2) {
            return;
        }
        let Ok(samples) = u32::try_from(audio.pcm.len() / 2) else {
            return;
        };
        let frame = AudioFrame {
            format,
            position: SampleClockPosition::new(self.turns.emitted)
                .unwrap_or(SampleClockPosition::MAX),
            sample_count: samples,
            data: audio.pcm,
        };
        if self.ledger.output_audio(response, frame, at).is_ok() {
            self.turns.emitted += u64::from(samples);
        }
    }

    fn tool_use(&mut self, tool: ToolUse, at: Instant) {
        let response = self
            .turns
            .blocks
            .get(&tool.content_id)
            .and_then(|block| block.response)
            .or_else(|| self.ledger.in_progress());
        let Some(response) = response else {
            return;
        };
        let arguments = serde_json::from_str(&tool.content).unwrap_or(Json::String(tool.content));
        let _ = self.ledger.tool_call(
            SpeechCallId::new(tool.tool_use_id),
            response,
            &tool.tool_name,
            arguments,
            at,
        );
    }

    fn content_end(&mut self, end: &OutputContentEnd, at: Instant) {
        let Some(block) = self.turns.blocks.get(&end.content_id).cloned() else {
            return;
        };
        let Some(response) = block.response else {
            return;
        };
        let _ = match end.stop_reason {
            Some(StopReason::Interrupted) => {
                self.report_speech(at);
                self.ledger.response_cancelled(response, at)
            }
            Some(StopReason::ToolUse) => self.ledger.response_completed(response, at),
            Some(StopReason::EndTurn)
                if block.kind == ContentKind::Text && !block.speculative() =>
            {
                self.ledger.response_completed(response, at)
            }
            _ => Ok(()),
        };
    }

    /// Sums a usage delta per response: into the response in progress, or between responses its
    /// input into the next and its output into the last.
    fn usage(&mut self, usage: &Usage, at: Instant) {
        let counts = |counts: nova_sonic::TokenCounts| TokenCounts {
            text_tokens: counts.text,
            audio_tokens: counts.speech,
            cached_tokens: 0,
        };
        let delta = ResponseUsage {
            input: counts(usage.delta.input),
            output: counts(usage.delta.output),
        };
        if let Some(response) = self.ledger.in_progress() {
            self.add_usage(response, delta, at);
            return;
        }
        add_counts(&mut self.turns.pending.input, delta.input);
        if delta.output == TokenCounts::default() {
            return;
        }
        match self.turns.last {
            Some(response) => self.add_usage(
                response,
                ResponseUsage {
                    input: TokenCounts::default(),
                    output: delta.output,
                },
                at,
            ),
            None => add_counts(&mut self.turns.pending.output, delta.output),
        }
    }

    fn add_usage(&mut self, response: ResponseId, delta: ResponseUsage, at: Instant) {
        if delta == ResponseUsage::default() {
            return;
        }
        let total = self.turns.totals.entry(response).or_default();
        add_counts(&mut total.input, delta.input);
        add_counts(&mut total.output, delta.output);
        let total = *total;
        let _ = self.ledger.response_usage(response, total, at);
    }
}

fn add_counts(total: &mut TokenCounts, delta: TokenCounts) {
    total.text_tokens = total.text_tokens.saturating_add(delta.text_tokens);
    total.audio_tokens = total.audio_tokens.saturating_add(delta.audio_tokens);
    total.cached_tokens = total.cached_tokens.saturating_add(delta.cached_tokens);
}

impl SpeechSessionHandle for NovaSonicSpeechSession {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    /// Sends the frame's bytes untouched once the ledger accepts it.
    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        if self.commands.is_closed() {
            self.transport_closed();
        }
        let pcm = frame.data.clone();
        let end = frame
            .position
            .get()
            .saturating_add(u64::from(frame.sample_count));
        self.ledger.accept_input(frame)?;
        self.turns.input_end = self.turns.input_end.max(end);
        self.send(Command::Audio(pcm));
        Ok(())
    }

    /// Refused: Nova Sonic's turn detection commits the input.
    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.ledger.commit_input(Instant::now())
    }

    /// Refused: Nova Sonic creates every response.
    fn create_response(&mut self) -> Result<(), SessionError> {
        self.ledger.request_response(Instant::now())
    }

    /// Refused: Nova Sonic takes no cancellation from the caller.
    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.ledger.cancel(response_id)
    }

    /// Refused: Nova Sonic truncates no output audio.
    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.ledger.truncate(response_id, heard)
    }

    /// Sends the result under the provider's `toolUseId`. Nova Sonic takes a JSON object; any
    /// other value is sent as `{"result": value}`.
    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        let content = match &result {
            Json::Object(_) => result.to_string(),
            other => serde_json::json!({ "result": other }).to_string(),
        };
        self.ledger.answer(call_id, result)?;
        self.turns.open = true;
        self.send(Command::ToolResult {
            tool_use_id: call_id.as_str().to_owned(),
            content,
        });
        Ok(())
    }

    /// Closes the session and ends the prompt and the session on the stream; nothing the provider
    /// sends after is delivered.
    fn close(&mut self) -> Result<(), SessionError> {
        self.ledger.close()?;
        let _ = self.commands.send(Command::Close);
        Ok(())
    }

    fn next_event(&mut self, wait: Duration) -> Option<SessionEvent> {
        if let Some(event) = self.ledger.next_event() {
            return Some(event);
        }
        if self.ledger.session().require_open().is_err() {
            return None;
        }
        let deadline = Instant::now() + wait;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.arrivals.recv_timeout(remaining) {
                Ok(arrival) => {
                    self.record(arrival);
                    if let Some(event) = self.ledger.next_event() {
                        return Some(event);
                    }
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    self.transport_closed();
                    return self.ledger.next_event();
                }
            }
        }
        self.ledger.check_liveness(Instant::now());
        self.ledger.next_event()
    }
}
