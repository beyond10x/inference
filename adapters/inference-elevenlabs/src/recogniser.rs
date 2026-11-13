//! The binding of the STT contract: [`ElevenLabsRecogniser`] and its recognition handle.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use inference_stt::inference_audio::AudioFormat;
use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::model::{
    Adapter, AdapterDescriptor, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_stt::inference_text::{ReviseSpan, SpanId};
use inference_stt::{
    CONTRACT_REVISION, ConcurrencyLimitReached, InputEnded, Recogniser, RecognitionHandle,
    RecognitionRequest, RevisionRefused, TranscriptEvent, TranscriptOutput,
};
use tokio::sync::mpsc as async_mpsc;

use crate::config::Endpoint;
use crate::connection::Connection;
use crate::credential::{CredentialSource, ELEVENLABS_KEY_NAME, EnvironmentSource};
use crate::error::{ConfigError, ConnectError, ElevenLabsError};
use crate::stt_config::{CommitStrategy, TranscriptionConfig};
use crate::stt_protocol::{self, ServerMessage, SpanTiming};
use crate::wake::{Alarm, WakerSlot};

/// The backend name of the recogniser's adapter record.
pub const STT_BACKEND: &str = "elevenlabs-speech-to-text-realtime";

/// The most server messages that wait for the caller; while that many wait, the connection is
/// not read until the caller iterates.
pub const STT_INBOUND_DEPTH: usize = 32;

/// The most `warning` messages a recognition keeps; later ones are dropped.
pub const WARNINGS_KEPT_STT: usize = 16;

/// With the `vad` commit strategy, how long the endpoint must stay silent after the end-of-input
/// commit was answered, with no span open, before the recognition ends: a commit the endpoint
/// made on its own may answer for the binding's, and the transcript of the audio after it then
/// follows within this.
pub const VAD_END_QUIET: Duration = Duration::from_millis(500);

/// How much longer than the connect timeout the handle waits, once input has ended, for the
/// connection's thread to say how the opening ended, before it fails the recognition as
/// `Connect` itself: the thread's own bound cannot fire while a credential source blocks it.
pub const OPEN_SLACK: Duration = Duration::from_millis(500);

/// How long the connection's thread waits before it offers a message again to a full queue.
const DELIVERY_RETRY: Duration = Duration::from_millis(10);

/// The capabilities an adapter record of the recogniser claims. `incremental_input`: each frame
/// is sent as soon as the handle is iterated after it was offered, before input ends.
/// `output_streaming`: each partial transcript is a hypothesis or revision delivered before input
/// ends. Nothing else.
pub fn stt_claims() -> Vec<Capability> {
    vec![Capability::IncrementalInput, Capability::OutputStreaming]
}

/// What every recognition of one recogniser shares.
struct Shared {
    url: String,
    endpoint: Endpoint,
    config: TranscriptionConfig,
    format: AudioFormat,
    source: Arc<dyn CredentialSource>,
    credential: String,
}

/// ElevenLabs' realtime speech-to-text endpoint as a recogniser of the STT contract, at locality
/// `external_provider`. See the crate documentation.
pub struct ElevenLabsRecogniser {
    adapter: Adapter,
    shared: Arc<Shared>,
    running: Arc<AtomicUsize>,
}

impl fmt::Debug for ElevenLabsRecogniser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ElevenLabsRecogniser")
            .field("adapter", &self.adapter.adapter_id())
            .field("config", &self.shared.config)
            .field("endpoint", &self.shared.endpoint)
            .field("credential", &self.shared.credential)
            .finish_non_exhaustive()
    }
}

impl ElevenLabsRecogniser {
    /// A recogniser of `config` at `endpoint`, resolving the secret `credential` names through
    /// `source` for every connection, under an adapter record of its own claiming
    /// [`stt_claims`] with a maximum concurrency of one.
    ///
    /// # Errors
    /// The [`ConfigError`] of [`TranscriptionConfig::validate`] or of the URL.
    pub fn new(
        endpoint: Endpoint,
        config: TranscriptionConfig,
        source: Arc<dyn CredentialSource>,
        credential: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        config.validate()?;
        let url = stt_protocol::request_url(&endpoint, &config)?;
        Ok(Self {
            adapter: declare(1)?,
            shared: Arc::new(Shared {
                url,
                endpoint,
                format: config.input_format.audio_format(),
                config,
                source,
                credential: credential.into(),
            }),
            running: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// [`ElevenLabsRecogniser::new`] with the key from the environment variable
    /// `ELEVENLABS_API_KEY`, read at every connection.
    ///
    /// # Errors
    /// As [`ElevenLabsRecogniser::new`].
    pub fn from_env(endpoint: Endpoint, config: TranscriptionConfig) -> Result<Self, ConfigError> {
        Self::new(
            endpoint,
            config,
            Arc::new(EnvironmentSource::elevenlabs()),
            ELEVENLABS_KEY_NAME,
        )
    }

    /// This recogniser under a new adapter record that allows `max_concurrency` recognitions at
    /// once, as the account's plan allows.
    ///
    /// # Errors
    /// [`ConfigError::MaxConcurrency`] when it is not positive.
    pub fn with_max_concurrency(mut self, max_concurrency: i64) -> Result<Self, ConfigError> {
        self.adapter = declare(max_concurrency)?;
        Ok(self)
    }

    /// The configuration it recognises with.
    pub fn config(&self) -> &TranscriptionConfig {
        &self.shared.config
    }

    /// The URL every recognition connects to. It carries no key.
    pub fn request_url(&self) -> &str {
        &self.shared.url
    }
}

fn declare(max_concurrency: i64) -> Result<Adapter, ConfigError> {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: STT_BACKEND.to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::HostedService,
        locality: Locality::ExternalProvider,
        capabilities: stt_claims(),
        max_concurrency,
        descriptor: AdapterDescriptor::default(),
    })
    .map(|(adapter, _)| adapter)
    .map_err(|_| ConfigError::MaxConcurrency(max_concurrency))
}

impl Recogniser for ElevenLabsRecogniser {
    type Handle = ElevenLabsRecognition;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(
        &mut self,
        request: RecognitionRequest,
    ) -> Result<ElevenLabsRecognition, ConcurrencyLimitReached> {
        let max_concurrency = self.adapter.max_concurrency();
        let limit = usize::try_from(max_concurrency).unwrap_or(usize::MAX);
        let slot = Slot(Arc::clone(&self.running));
        if self.running.fetch_add(1, Ordering::AcqRel) >= limit {
            drop(slot);
            return Err(ConcurrencyLimitReached { max_concurrency });
        }
        Ok(ElevenLabsRecognition {
            output: TranscriptOutput::start(&self.adapter, request),
            shared: Arc::clone(&self.shared),
            worker: None,
            started: false,
            open: None,
            timings: Vec::new(),
            outstanding: 0,
            end_sent: false,
            opened: false,
            started_at: std::time::Instant::now(),
            heard_at: std::time::Instant::now(),
            delivered: VecDeque::new(),
            failure: None,
            finished: false,
            cancelled: false,
            warnings: Vec::new(),
            alarm: Alarm::default(),
            _slot: slot,
        })
    }
}

/// The place of one running recognition, given back when it is dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// What the handle tells the thread that drives the connection.
enum Command {
    /// One `input_audio_chunk` of these bytes.
    Audio(Vec<u8>),
    /// One `input_audio_chunk` without audio and with `commit` true.
    Commit,
    Cancel,
}

/// What that thread tells the handle, and what the handle makes of its silence.
enum Inbound {
    /// The handshake succeeded; every message follows this.
    Opened,
    Message(ServerMessage),
    Failed(ElevenLabsError),
    /// The read timeout passed after the end of input without a message.
    Quiet,
    /// With the `vad` strategy, [`VAD_END_QUIET`] passed without a message once every commit was
    /// answered and no span was open.
    Settled,
}

/// The thread that drives the connection, as the handle sees it.
struct Worker {
    commands: async_mpsc::UnboundedSender<Command>,
    inbound: mpsc::Receiver<Inbound>,
    /// Woken by the thread after each message it queued and when it lets go of the queue.
    waker: Arc<WakerSlot>,
}

/// Why [`wait`] took nothing from the queue.
enum Waited {
    Timeout,
    Disconnected,
}

/// The next thing `worker`'s thread queued, waiting until `until`, or without a deadline when
/// there is none. Without `cx` it blocks the calling thread; with `cx`, the async form's, it is
/// `Pending` instead, and the thread queuing a message or letting go of the queue, or `alarm` at
/// `until`, wakes `cx`.
fn wait(
    worker: &Worker,
    alarm: &mut Alarm,
    until: Option<std::time::Instant>,
    cx: Option<&mut Context<'_>>,
) -> Poll<Result<Inbound, Waited>> {
    let Some(cx) = cx else {
        let received = match until {
            Some(until) => worker
                .inbound
                .recv_timeout(until.saturating_duration_since(std::time::Instant::now()))
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => Waited::Timeout,
                    mpsc::RecvTimeoutError::Disconnected => Waited::Disconnected,
                }),
            None => worker.inbound.recv().map_err(|_| Waited::Disconnected),
        };
        return Poll::Ready(received);
    };
    worker.waker.register(cx.waker());
    match worker.inbound.try_recv() {
        Ok(inbound) => Poll::Ready(Ok(inbound)),
        Err(mpsc::TryRecvError::Disconnected) => Poll::Ready(Err(Waited::Disconnected)),
        Err(mpsc::TryRecvError::Empty) => match until {
            Some(until) if alarm.poll_at(until, cx).is_ready() => Poll::Ready(Err(Waited::Timeout)),
            _ => Poll::Pending,
        },
    }
}

/// One recognition of an [`ElevenLabsRecogniser`]; it runs until it is dropped or cancelled.
///
/// The connection is driven on a thread of its own, so audio and transcripts move while the
/// caller is elsewhere. Before the end of input, iterating takes the frames offered and yields
/// what has arrived, without waiting: `None` then means nothing has arrived yet. Once input has
/// ended, iterating blocks until the answer to the end-of-input commit (with `vad`, followed by
/// [`VAD_END_QUIET`] of silence with no span open), a failure, or the read timeout passing
/// without a server message that delivers an event or answers a commit, and `None` means the
/// recognition has ended. The read timeout runs from the latest of the end of input, the opening
/// of the connection and the last such message; while the handshake is still pending it does not
/// run, and the handshake's own bound applies, the connect timeout and [`OPEN_SLACK`] from the
/// start of the opening. A failure ends the iteration;
/// [`ElevenLabsRecognition::failure`] says why, and the usage record, delivered once input has
/// ended, is then not final.
pub struct ElevenLabsRecognition {
    output: TranscriptOutput,
    shared: Arc<Shared>,
    worker: Option<Worker>,
    /// Whether a connection was started; it is started at most once.
    started: bool,
    open: Option<(SpanId, String)>,
    timings: Vec<(SpanId, SpanTiming)>,
    /// The commits sent and not yet answered by a committed transcript.
    outstanding: usize,
    end_sent: bool,
    /// Whether the connection's thread said the handshake succeeded.
    opened: bool,
    /// When the opening of the connection started.
    started_at: std::time::Instant,
    /// When the read timeout after the end of input starts: the latest of the end-of-input
    /// commit, the opening of the connection and the last server message that delivered an event
    /// or answered a commit. A message that does neither, such as a `warning` or a partial
    /// transcript repeating the open span's text, moves nothing.
    heard_at: std::time::Instant,
    delivered: VecDeque<EventEnvelope<TranscriptEvent>>,
    failure: Option<ElevenLabsError>,
    finished: bool,
    cancelled: bool,
    warnings: Vec<String>,
    /// Wakes the async form when a deadline of the recognition passes.
    alarm: Alarm,
    _slot: Slot,
}

impl fmt::Debug for ElevenLabsRecognition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ElevenLabsRecognition")
            .field("format", &self.shared.format)
            .field("spans", &self.output.spans().len())
            .field("connected", &self.worker.is_some())
            .field("failure", &self.failure)
            .field("finished", &self.finished)
            .field("cancelled", &self.cancelled)
            .finish_non_exhaustive()
    }
}

impl ElevenLabsRecognition {
    /// Why the recognition failed, once it has.
    pub fn failure(&self) -> Option<&ElevenLabsError> {
        self.failure.as_ref()
    }

    /// Where the committed span `span_id` lies in the audio sent, as the endpoint timed its
    /// words; none for a span not committed by a timed `committed_transcript_with_timestamps`.
    /// The STT contract's events carry no timing, so it is read here.
    pub fn span_timing(&self, span_id: SpanId) -> Option<SpanTiming> {
        self.timings
            .iter()
            .find(|(id, _)| *id == span_id)
            .map(|(_, timing)| *timing)
    }

    /// The `warning` texts the endpoint sent, the first [`WARNINGS_KEPT_STT`] of them.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Asks the endpoint to commit what it has heard so far: sends every frame offered and not
    /// yet sent, then a commit. Its committed transcript commits the open span when it arrives.
    /// It does nothing before the first frame was sent, once input has ended (the end of input
    /// commits) and once the recognition has ended.
    pub fn commit(&mut self) {
        if self.failure.is_some() || self.finished || self.cancelled || self.output.input_ended() {
            return;
        }
        self.forward_input();
        if let Some(worker) = self.worker.as_ref()
            && worker.commands.send(Command::Commit).is_ok()
        {
            self.outstanding += 1;
        }
    }

    /// Cancels the recognition: closes its connection, sending a close frame within
    /// [`CLOSE_BOUND`](crate::CLOSE_BOUND), and drops every event not yet delivered. Nothing
    /// more is delivered, a usage record neither, as for a recognition dropped before its input
    /// ended.
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.delivered.clear();
        self.stop_worker();
    }

    fn stop_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.commands.send(Command::Cancel);
        }
    }

    /// Ends the recognition as failed by `error`: its connection closed, its call marked failed,
    /// and nothing more recognised. A span left open stays open.
    fn fail(&mut self, error: ElevenLabsError) {
        self.stop_worker();
        self.output.mark_failed();
        self.failure = Some(error);
    }

    /// Ends the recognition: the connection is closed by its thread once the handle lets go.
    fn finish(&mut self) {
        self.finished = true;
        self.worker = None;
    }

    /// Sends every frame offered and not yet taken, connecting at the first. A frame is taken
    /// only once it can be sent: the first in another format than the configured one, or whose
    /// bytes do not hold its samples, fails the recognition and stays offered with every frame
    /// after it, so a failed call's usage counts only the audio sent. A frame the connection's
    /// thread can no longer take is dropped: that thread has stopped, and why arrives through its
    /// own message, or as its silence once input has ended.
    fn forward_input(&mut self) {
        while self.failure.is_none() {
            let Some((format, bytes, samples)) = self
                .output
                .next_offered()
                .map(|frame| (frame.format, frame.data.len(), frame.sample_count))
            else {
                return;
            };
            let expected = self.shared.format;
            if format != expected {
                let detail = format!(
                    "a frame in {format:?} where the configured input format {} is {expected:?}",
                    self.shared.config.input_format
                );
                self.fail(ElevenLabsError::UnsupportedAudio(detail));
                return;
            }
            if u64::try_from(bytes).ok()
                != u64::from(samples).checked_mul(expected.encoding.width())
            {
                let detail = format!("a frame of {samples} samples in {bytes} bytes");
                self.fail(ElevenLabsError::UnsupportedAudio(detail));
                return;
            }
            if !self.started {
                self.started = true;
                self.started_at = std::time::Instant::now();
                match spawn(Arc::clone(&self.shared)) {
                    Ok(worker) => self.worker = Some(worker),
                    Err(error) => {
                        self.fail(error);
                        return;
                    }
                }
            }
            let Ok(Some(frame)) = self.output.take_next() else {
                return;
            };
            if !frame.data.is_empty()
                && let Some(worker) = self.worker.as_ref()
            {
                let _ = worker.commands.send(Command::Audio(frame.data));
            }
        }
    }

    /// The next thing the connection's thread said. Before the end of input, without waiting:
    /// `None` when nothing has arrived. After it, while the handshake is pending, until the
    /// thread says how it ended, at most the connect timeout and [`OPEN_SLACK`] from the start of
    /// the opening, after which it is a `Connect` failure, so a credential source that blocks the
    /// thread's runtime cannot hold the caller; once the connection is open, until the read
    /// timeout from `heard_at` passes, which is [`Inbound::Quiet`], or with `vad` and nothing left
    /// to answer, until [`VAD_END_QUIET`] passes, which is [`Inbound::Settled`]. An instant too far
    /// to represent is no deadline. With `cx`, the async form's, it never blocks: `Pending` where
    /// it would wait, woken as [`wait`] says.
    fn receive(&mut self, cx: Option<&mut Context<'_>>) -> Poll<Option<Inbound>> {
        let now = std::time::Instant::now();
        if self.output.input_ended() && !self.end_sent {
            let Some(worker) = self.worker.as_ref() else {
                return Poll::Ready(None);
            };
            if worker.commands.send(Command::Commit).is_ok() {
                self.outstanding += 1;
            }
            self.end_sent = true;
            self.heard_at = now;
        }
        let settling = matches!(self.shared.config.commit_strategy, CommitStrategy::Vad(_))
            && self.outstanding == 0
            && self.open.is_none();
        let due = self.heard_at.checked_add(self.shared.endpoint.read_timeout);
        let settle = self.heard_at.checked_add(VAD_END_QUIET);
        let settle_first = settling
            && match (settle, due) {
                (Some(settle), Some(due)) => settle < due,
                (Some(_), None) => true,
                (None, _) => false,
            };
        let Some(worker) = self.worker.as_ref() else {
            return Poll::Ready(None);
        };
        let received = if !self.end_sent {
            match worker.inbound.try_recv() {
                Ok(inbound) => inbound,
                Err(mpsc::TryRecvError::Empty) => return Poll::Ready(None),
                Err(mpsc::TryRecvError::Disconnected) => Inbound::Failed(stopped()),
            }
        } else if !self.opened {
            let bound = self
                .shared
                .endpoint
                .connect_timeout
                .saturating_add(OPEN_SLACK);
            let until = self.started_at.checked_add(bound);
            match ready!(wait(worker, &mut self.alarm, until, cx)) {
                Ok(inbound) => inbound,
                Err(Waited::Timeout) => Inbound::Failed(ElevenLabsError::Connect(ConnectError {
                    status: None,
                    detail: format!(
                        "no connection within {:?}",
                        self.shared.endpoint.connect_timeout
                    ),
                })),
                Err(Waited::Disconnected) => Inbound::Failed(stopped()),
            }
        } else {
            let until = if settle_first { settle } else { due };
            match ready!(wait(worker, &mut self.alarm, until, cx)) {
                Ok(inbound) => inbound,
                Err(Waited::Timeout) if settle_first => Inbound::Settled,
                Err(Waited::Timeout) => Inbound::Quiet,
                Err(Waited::Disconnected) => Inbound::Failed(stopped()),
            }
        };
        if self.end_sent && matches!(received, Inbound::Opened) {
            self.heard_at = std::time::Instant::now();
        }
        Poll::Ready(Some(received))
    }

    /// Acts on one server message; see the crate documentation for the mapping.
    fn apply(&mut self, message: ServerMessage) -> Result<(), ElevenLabsError> {
        match message {
            ServerMessage::SessionStarted | ServerMessage::Ignored => {}
            ServerMessage::Warning(warning) => {
                if self.warnings.len() < WARNINGS_KEPT_STT {
                    self.warnings.push(warning);
                }
            }
            ServerMessage::Partial(text) | ServerMessage::Committed(text) => {
                if !text.trim().is_empty() {
                    self.hear(text)?;
                }
            }
            ServerMessage::CommittedWithTimestamps { text, timing } => {
                if !text.trim().is_empty() {
                    self.hear(text)?;
                }
                self.commit_open(timing)?;
                self.outstanding = self.outstanding.saturating_sub(1);
                let manual = self.shared.config.commit_strategy == CommitStrategy::Manual;
                if manual && self.end_sent && self.outstanding == 0 {
                    self.finish();
                }
            }
        }
        Ok(())
    }

    /// The silence of the server for the read timeout after the end of input: a stall while a
    /// commit is unanswered or a span is open, and otherwise the end, since every commit was
    /// answered and nothing heard is left uncommitted.
    fn quiet(&mut self) -> Result<(), ElevenLabsError> {
        if self.open.is_some() || self.outstanding > 0 {
            return Err(ElevenLabsError::Stalled(self.shared.endpoint.read_timeout));
        }
        self.finish();
        Ok(())
    }

    /// Opens a span reading `text`, or revises the open one to read it.
    fn hear(&mut self, text: String) -> Result<(), ElevenLabsError> {
        match self.open.as_mut() {
            None => {
                let envelope = self.output.hypothesis(text.clone()).map_err(refused)?;
                let span_id = envelope
                    .payload
                    .span_id()
                    .ok_or_else(|| refused("a hypothesis about no span"))?;
                self.open = Some((span_id, text));
                self.delivered.push_back(envelope);
            }
            Some((span_id, current)) if *current != text => {
                let envelope = self
                    .output
                    .revise(ReviseSpan {
                        span_id: *span_id,
                        text: text.clone(),
                    })
                    .map_err(refused)?;
                *current = text;
                self.delivered.push_back(envelope);
            }
            Some(_) => {}
        }
        Ok(())
    }

    /// Commits the open span, if there is one, with `timing`.
    fn commit_open(&mut self, timing: Option<SpanTiming>) -> Result<(), ElevenLabsError> {
        if let Some((span_id, _)) = self.open.take() {
            let envelope = self.output.commit(span_id).map_err(refused)?;
            self.delivered.push_back(envelope);
            if let Some(timing) = timing {
                self.timings.push((span_id, timing));
            }
        }
        Ok(())
    }
}

fn stopped() -> ElevenLabsError {
    ElevenLabsError::Runtime("the thread driving the connection stopped without a word".to_owned())
}

/// A refusal of the STT contract, which a well-formed stream of this binding never meets.
fn refused(error: impl fmt::Display) -> ElevenLabsError {
    ElevenLabsError::Protocol(format!("the STT contract refused the transcript: {error}"))
}

impl Iterator for ElevenLabsRecognition {
    type Item = EventEnvelope<TranscriptEvent>;

    /// The next event; see [`ElevenLabsRecognition`] for when it waits.
    fn next(&mut self) -> Option<Self::Item> {
        match self.advance(None) {
            Poll::Ready(envelope) => envelope,
            Poll::Pending => None,
        }
    }
}

impl ElevenLabsRecognition {
    /// The next event: blocking where the iterator waits, or with `cx`, the async form's,
    /// `Pending` there instead.
    fn advance(
        &mut self,
        mut cx: Option<&mut Context<'_>>,
    ) -> Poll<Option<EventEnvelope<TranscriptEvent>>> {
        loop {
            if let Some(envelope) = self.delivered.pop_front() {
                return Poll::Ready(Some(envelope));
            }
            if self.cancelled {
                return Poll::Ready(None);
            }
            if self.failure.is_none() && !self.finished {
                self.forward_input();
            }
            if self.failure.is_some() || self.finished {
                return Poll::Ready(self.output.usage().ok());
            }
            if self.worker.is_none() {
                if !self.output.input_ended() {
                    return Poll::Ready(None);
                }
                self.finished = true;
                continue;
            }
            let Some(received) = ready!(self.receive(cx.as_deref_mut())) else {
                return Poll::Ready(None);
            };
            let outcome = match received {
                Inbound::Opened => {
                    self.opened = true;
                    Ok(())
                }
                Inbound::Message(message) => {
                    let (delivered, outstanding) = (self.delivered.len(), self.outstanding);
                    let applied = self.apply(message);
                    if self.end_sent
                        && (self.delivered.len() != delivered || self.outstanding != outstanding)
                    {
                        self.heard_at = std::time::Instant::now();
                    }
                    applied
                }
                Inbound::Failed(error) => Err(error),
                Inbound::Quiet => self.quiet(),
                Inbound::Settled => {
                    self.finish();
                    Ok(())
                }
            };
            if let Err(error) = outcome {
                self.fail(error);
            }
        }
    }
}

impl Drop for ElevenLabsRecognition {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

impl RecognitionHandle for ElevenLabsRecognition {
    fn output(&self) -> &TranscriptOutput {
        &self.output
    }

    fn offer_frame(
        &mut self,
        frame: inference_stt::inference_audio::AudioFrame,
    ) -> Result<(), InputEnded> {
        self.output.offer(frame)
    }

    fn end_input(&mut self) -> Result<(), InputEnded> {
        self.output.end_input()
    }

    /// Takes what the connection's thread queued. While input is open, `Ready(None)` means
    /// drained for now, as iterating's `None` does: poll again after the next offer or after the
    /// end of input. From the end of input on, `Pending` where iterating blocks, woken by the
    /// thread when it queues the finals, and by a deadline of the recognition.
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<EventEnvelope<TranscriptEvent>>> {
        self.advance(Some(cx))
    }

    fn revise(&mut self, input: ReviseSpan) -> Result<(), RevisionRefused> {
        let span_id = input.span_id;
        let text = input.text.clone();
        let envelope = self.output.revise(input)?;
        if let Some((open, current)) = self.open.as_mut()
            && *open == span_id
        {
            *current = text;
        }
        self.delivered.push_back(envelope);
        Ok(())
    }
}

/// Starts the thread that drives one connection.
fn spawn(shared: Arc<Shared>) -> Result<Worker, ElevenLabsError> {
    let (commands, receiver) = async_mpsc::unbounded_channel();
    let (sender, inbound) = mpsc::sync_channel(STT_INBOUND_DEPTH);
    let waker = Arc::new(WakerSlot::default());
    let sender = Inbox::new(sender, Arc::clone(&waker));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| ElevenLabsError::Runtime(error.to_string()))?;
    std::thread::Builder::new()
        .name("inference-elevenlabs-stt".to_owned())
        .spawn(move || runtime.block_on(drive(shared, receiver, sender)))
        .map_err(|error| ElevenLabsError::Runtime(error.to_string()))?;
    Ok(Worker {
        commands,
        inbound,
        waker,
    })
}

/// What the thread does after one event of its loop.
enum Step {
    Go,
    /// The handle is gone or cancelled: close the connection, say nothing.
    Stop,
    /// Close the connection and tell the handle this, its last word.
    Finish(ElevenLabsError),
    /// The connection is lost: tell the handle this, its last word.
    Lost(ElevenLabsError),
}

/// Whether a message offered to the handle's queue was taken.
enum Offered {
    Taken,
    Full(Inbound),
    Gone,
}

/// The handle's queue as the connection's thread fills it: each message queued, and the
/// thread letting go of the queue, wakes the handle's async form.
struct Inbox {
    sender: Option<mpsc::SyncSender<Inbound>>,
    waker: Arc<WakerSlot>,
}

impl Inbox {
    fn new(sender: mpsc::SyncSender<Inbound>, waker: Arc<WakerSlot>) -> Self {
        Self {
            sender: Some(sender),
            waker,
        }
    }

    fn try_send(&self, item: Inbound) -> Result<(), mpsc::TrySendError<Inbound>> {
        let sent = match self.sender.as_ref() {
            Some(sender) => sender.try_send(item),
            None => Err(mpsc::TrySendError::Disconnected(item)),
        };
        if sent.is_ok() {
            self.waker.wake();
        }
        sent
    }

    fn send(&self, item: Inbound) -> Result<(), mpsc::SendError<Inbound>> {
        let sent = match self.sender.as_ref() {
            Some(sender) => sender.send(item),
            None => Err(mpsc::SendError(item)),
        };
        if sent.is_ok() {
            self.waker.wake();
        }
        sent
    }
}

impl Drop for Inbox {
    /// Lets go of the queue first, so the handle the wake reaches finds it disconnected.
    fn drop(&mut self) {
        drop(self.sender.take());
        self.waker.wake();
    }
}

fn offer(inbound: &Inbox, item: Inbound) -> Offered {
    match inbound.try_send(item) {
        Ok(()) => Offered::Taken,
        Err(mpsc::TrySendError::Full(item)) => Offered::Full(item),
        Err(mpsc::TrySendError::Disconnected(_)) => Offered::Gone,
    }
}

/// Carries out one command of the handle on `connection`.
async fn command(connection: &mut Connection, command: Option<Command>, rate: u32) -> Step {
    let written = match command {
        Some(Command::Audio(data)) => {
            connection
                .send(stt_protocol::audio_chunk(&data, rate, false))
                .await
        }
        Some(Command::Commit) => {
            connection
                .send(stt_protocol::audio_chunk(&[], rate, true))
                .await
        }
        Some(Command::Cancel) | None => return Step::Stop,
    };
    match written {
        Ok(()) => Step::Go,
        Err(error) => Step::Finish(error),
    }
}

/// Drives one connection: opens it, sends audio and commits as the handle asks, and hands every
/// server message to the handle, until a failure, a cancel or the handle letting go. The opening,
/// the credential's resolution included, is bounded by the connect timeout, and its outcome is
/// the first thing the handle hears: [`Inbound::Opened`] or the failure. A cancel or drop during
/// the opening handshake drops the attempt at once. While the handle's queue is full the
/// connection is not read.
async fn drive(
    shared: Arc<Shared>,
    mut commands: async_mpsc::UnboundedReceiver<Command>,
    inbound: Inbox,
) {
    let endpoint = &shared.endpoint;
    let connect_timeout = endpoint.connect_timeout;
    let opening = tokio::time::timeout(
        connect_timeout,
        Connection::open(
            &shared.url,
            shared.source.as_ref(),
            &shared.credential,
            connect_timeout,
            endpoint.read_timeout,
        ),
    );
    tokio::pin!(opening);
    let mut early = VecDeque::new();
    let mut connection = loop {
        tokio::select! {
            opened = &mut opening => match opened {
                Ok(Ok(connection)) => {
                    if inbound.send(Inbound::Opened).is_err() {
                        connection.close(None).await;
                        return;
                    }
                    break connection;
                }
                Ok(Err(error)) => {
                    let _ = inbound.send(Inbound::Failed(error));
                    return;
                }
                Err(_) => {
                    let _ = inbound.send(Inbound::Failed(ElevenLabsError::Connect(ConnectError {
                        status: None,
                        detail: format!("no connection within {connect_timeout:?}"),
                    })));
                    return;
                }
            },
            next = commands.recv() => match next {
                Some(Command::Cancel) | None => return,
                Some(next) => early.push_back(next),
            },
        }
    };
    let rate = shared.config.input_format.sample_rate();
    let mut held: Option<Inbound> = None;
    loop {
        let step = if let Some(next) = early.pop_front() {
            command(&mut connection, Some(next), rate).await
        } else {
            tokio::select! {
                next = commands.recv() => command(&mut connection, next, rate).await,
                text = connection.read_until(None), if held.is_none() => match text {
                    Ok(text) => match stt_protocol::parse_server_message(&text) {
                        Ok(message) => match offer(&inbound, Inbound::Message(message)) {
                            Offered::Taken => Step::Go,
                            Offered::Full(item) => {
                                held = Some(item);
                                Step::Go
                            }
                            Offered::Gone => Step::Stop,
                        },
                        Err(error) => Step::Finish(error),
                    },
                    Err(error) => Step::Lost(error),
                },
                () = tokio::time::sleep(DELIVERY_RETRY), if held.is_some() => {
                    match held.take().map(|item| offer(&inbound, item)) {
                        Some(Offered::Full(item)) => {
                            held = Some(item);
                            Step::Go
                        }
                        Some(Offered::Gone) => Step::Stop,
                        Some(Offered::Taken) | None => Step::Go,
                    }
                }
            }
        };
        let last = match step {
            Step::Go => continue,
            Step::Stop => {
                connection.close(None).await;
                return;
            }
            Step::Finish(error) => {
                connection.close(None).await;
                error
            }
            Step::Lost(error) => error,
        };
        if held.is_none_or(|item| inbound.send(item).is_ok()) {
            let _ = inbound.send(Inbound::Failed(last));
        }
        return;
    }
}
