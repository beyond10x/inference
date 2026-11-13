//! The binding of the STT contract: [`DeepgramRecogniser`] and its recognition handle.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_stt::inference_audio::{AudioFormat, AudioFrame};
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
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::config::{Endpoint, ListenConfig};
use crate::connection::{self, CLOSE_BOUND, Socket};
use crate::credential::{CredentialSource, DEEPGRAM_KEY_NAME, EnvironmentSource};
use crate::error::{CloseCause, ConfigError, DeepgramError};
use crate::listen::{self, CLOSE_STREAM, KEEP_ALIVE, ServerMessage};
use crate::wake::{Alarm, WakerSlot};

/// The backend name of the adapter record.
pub const BACKEND: &str = "deepgram-listen";

/// The capabilities an adapter record of the binding claims. `incremental_input`: each frame is
/// sent as soon as the handle is iterated after it was offered, before input ends.
/// `output_streaming`: each interim result is a hypothesis or revision delivered before input
/// ends. Nothing else.
pub fn claims() -> Vec<Capability> {
    vec![Capability::IncrementalInput, Capability::OutputStreaming]
}

/// What every recognition of one recogniser shares.
struct Shared {
    base: String,
    config: ListenConfig,
    endpoint: Endpoint,
    source: Arc<dyn CredentialSource>,
    credential: String,
}

/// Deepgram's live transcription endpoint as a recogniser of the STT contract, at locality
/// `external_provider`. See the crate documentation.
pub struct DeepgramRecogniser {
    adapter: Adapter,
    shared: Arc<Shared>,
    running: Arc<AtomicUsize>,
}

impl fmt::Debug for DeepgramRecogniser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeepgramRecogniser")
            .field("adapter", &self.adapter.adapter_id())
            .field("config", &self.shared.config)
            .field("endpoint", &self.shared.endpoint)
            .field("credential", &self.shared.credential)
            .finish_non_exhaustive()
    }
}

impl DeepgramRecogniser {
    /// A recogniser of `config` at `endpoint`, resolving the secret `credential` names through
    /// `source` for every connection, under an adapter record of its own claiming [`claims`] with
    /// a maximum concurrency of one.
    ///
    /// # Errors
    /// The [`ConfigError`] of [`ListenConfig::validate`] or of [`Endpoint::base`].
    pub fn new(
        endpoint: Endpoint,
        config: ListenConfig,
        source: Arc<dyn CredentialSource>,
        credential: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        config.validate()?;
        let base = endpoint.base()?.to_owned();
        Ok(Self {
            adapter: declare(1)?,
            shared: Arc::new(Shared {
                base,
                config,
                endpoint,
                source,
                credential: credential.into(),
            }),
            running: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// [`DeepgramRecogniser::new`] with the key from the environment variable
    /// `DEEPGRAM_API_KEY`, read at every connection.
    ///
    /// # Errors
    /// As [`DeepgramRecogniser::new`].
    pub fn from_env(endpoint: Endpoint, config: ListenConfig) -> Result<Self, ConfigError> {
        Self::new(
            endpoint,
            config,
            Arc::new(EnvironmentSource::deepgram()),
            DEEPGRAM_KEY_NAME,
        )
    }

    /// This recogniser under a new adapter record that allows `max_concurrency` recognitions at
    /// once, as the account allows.
    ///
    /// # Errors
    /// [`ConfigError::MaxConcurrency`] when it is not positive.
    pub fn with_max_concurrency(mut self, max_concurrency: i64) -> Result<Self, ConfigError> {
        self.adapter = declare(max_concurrency)?;
        Ok(self)
    }

    /// The configuration it recognises with.
    pub fn config(&self) -> &ListenConfig {
        &self.shared.config
    }

    /// The URL a recognition whose first frame is in `format` connects to. It carries no key.
    ///
    /// # Errors
    /// The [`DeepgramError::UnsupportedAudio`] of [`listen::encoding`].
    pub fn request_url(&self, format: AudioFormat) -> Result<String, DeepgramError> {
        request_url(&self.shared, format)
    }
}

fn request_url(shared: &Shared, format: AudioFormat) -> Result<String, DeepgramError> {
    Ok(format!(
        "{}{}?{}",
        shared.base,
        listen::PATH,
        listen::query(&shared.config, format)?
    ))
}

fn declare(max_concurrency: i64) -> Result<Adapter, ConfigError> {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: BACKEND.to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::HostedService,
        locality: Locality::ExternalProvider,
        capabilities: claims(),
        max_concurrency,
        descriptor: AdapterDescriptor::default(),
    })
    .map(|(adapter, _)| adapter)
    .map_err(|_| ConfigError::MaxConcurrency(max_concurrency))
}

impl Recogniser for DeepgramRecogniser {
    type Handle = DeepgramRecognition;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(
        &mut self,
        request: RecognitionRequest,
    ) -> Result<DeepgramRecognition, ConcurrencyLimitReached> {
        let max_concurrency = self.adapter.max_concurrency();
        let limit = usize::try_from(max_concurrency).unwrap_or(usize::MAX);
        let slot = Slot(Arc::clone(&self.running));
        if self.running.fetch_add(1, Ordering::AcqRel) >= limit {
            drop(slot);
            return Err(ConcurrencyLimitReached { max_concurrency });
        }
        Ok(DeepgramRecognition {
            output: TranscriptOutput::start(&self.adapter, request),
            shared: Arc::clone(&self.shared),
            worker: None,
            format: None,
            open: None,
            open_through: None,
            committed_through: None,
            delivered: VecDeque::new(),
            failure: None,
            finished: false,
            cancelled: false,
            duration: None,
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
    Audio(Vec<u8>),
    End,
    Cancel,
}

/// What that thread tells the handle.
enum Inbound {
    Message(ServerMessage),
    /// The summary `Metadata`, read after the thread sent `CloseStream`, with its duration.
    Summary(Option<f64>),
    Failed(DeepgramError),
}

/// The thread that drives the connection, as the handle sees it.
struct Worker {
    commands: async_mpsc::UnboundedSender<Command>,
    inbound: mpsc::Receiver<Inbound>,
    /// Woken by the thread after each message it queued and when it lets go of the queue.
    waker: Arc<WakerSlot>,
    /// When the summary is due at the latest, once the end of input was sent.
    due: Option<std::time::Instant>,
}

/// How long the thread waits before it offers a message again to a full queue.
const DELIVERY_RETRY: Duration = Duration::from_millis(10);

/// How far two span ends may differ and still be the same audio, in seconds.
const SAME_END: f64 = 1e-6;

/// One recognition of a [`DeepgramRecogniser`]; it runs until it is dropped or cancelled.
///
/// The connection is driven on a thread of its own, so audio, `KeepAlive` and transcripts move
/// while the caller is elsewhere. Before the end of input, iterating takes the frames offered and
/// yields what has arrived, without waiting: `None` then means nothing has arrived yet. Once input
/// has ended, iterating blocks until the summary `Metadata` or a failure, all waits together
/// bounded by the endpoint's read timeout plus [`CLOSE_BOUND`], and `None` means the recognition
/// has ended. A failure ends the
/// iteration; [`DeepgramRecognition::failure`] says why, and the usage record, delivered once
/// input has ended, is then not final.
pub struct DeepgramRecognition {
    output: TranscriptOutput,
    shared: Arc<Shared>,
    worker: Option<Worker>,
    format: Option<AudioFormat>,
    open: Option<(SpanId, String)>,
    /// Where the audio of the open span ends, as far as its results said.
    open_through: Option<f64>,
    /// Where the audio of the committed spans ends, as far as their results said.
    committed_through: Option<f64>,
    delivered: VecDeque<EventEnvelope<TranscriptEvent>>,
    failure: Option<DeepgramError>,
    finished: bool,
    cancelled: bool,
    duration: Option<f64>,
    /// Wakes the async form when the summary's deadline passes.
    alarm: Alarm,
    _slot: Slot,
}

impl fmt::Debug for DeepgramRecognition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeepgramRecognition")
            .field("format", &self.format)
            .field("spans", &self.output.spans().len())
            .field("connected", &self.worker.is_some())
            .field("failure", &self.failure)
            .field("finished", &self.finished)
            .field("cancelled", &self.cancelled)
            .finish_non_exhaustive()
    }
}

impl DeepgramRecognition {
    /// Why the recognition failed, once it has.
    pub fn failure(&self) -> Option<&DeepgramError> {
        self.failure.as_ref()
    }

    /// The seconds of audio the endpoint reported processing in its summary `Metadata`, once it
    /// has arrived. The usage record does not carry it: its `input_audio_ms` is the audio
    /// offered (on a failed call, the audio forwarded), as the contract measures it.
    pub fn provider_duration(&self) -> Option<f64> {
        self.duration
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
    fn fail(&mut self, error: DeepgramError) {
        self.stop_worker();
        self.output.mark_failed();
        self.failure = Some(error);
    }

    /// Sends every frame offered and not yet taken, connecting at the first. A frame is taken
    /// only once it can be sent: the first the binding refuses fails the recognition and stays
    /// offered with every frame after it, so a failed call's usage counts only the audio
    /// forwarded. A frame the connection's thread can no longer take is dropped: that thread has
    /// stopped, and why arrives through its own message, or as its silence once input has ended.
    fn forward_input(&mut self) {
        while self.failure.is_none() {
            let Some(format) = self.output.next_offered().map(|frame| frame.format) else {
                return;
            };
            if let Err(error) = self.prepare(format) {
                self.fail(error);
                return;
            }
            let Ok(Some(frame)) = self.output.take_next() else {
                return;
            };
            self.forward(frame);
        }
    }

    /// Readies the connection for a frame in `format`: connects at the first frame, and refuses
    /// a frame in another format than the first's.
    fn prepare(&mut self, format: AudioFormat) -> Result<(), DeepgramError> {
        match self.format {
            None => {
                let url = request_url(&self.shared, format)?;
                self.worker = Some(spawn(Arc::clone(&self.shared), url)?);
                self.format = Some(format);
            }
            Some(first) if first != format => {
                return Err(DeepgramError::UnsupportedAudio(format!(
                    "a frame in {format:?} after frames in {first:?}; one recognition takes one \
                     format"
                )));
            }
            Some(_) => {}
        }
        Ok(())
    }

    /// Sends `frame`, taken and in the connection's format; an empty frame is not sent.
    fn forward(&mut self, frame: AudioFrame) {
        if frame.data.is_empty() {
            return;
        }
        if let Some(worker) = self.worker.as_ref() {
            let _ = worker.commands.send(Command::Audio(frame.data));
        }
    }

    /// The next thing the connection's thread said: without waiting before the end of input,
    /// and after it by the summary's one deadline, the read timeout from the end of input plus
    /// [`CLOSE_BOUND`]. `None` when nothing has arrived before the end of input. With `cx`, the
    /// async form's, it does not block: `Pending` until the thread queues a message or lets go
    /// of the queue, or the deadline passes, each of which wakes `cx`.
    fn receive(&mut self, cx: Option<&mut Context<'_>>) -> Poll<Option<Inbound>> {
        let ended = self.output.input_ended();
        let bound = self.shared.endpoint.read_timeout + CLOSE_BOUND;
        let Some(worker) = self.worker.as_mut() else {
            return Poll::Ready(None);
        };
        if ended && worker.due.is_none() {
            worker.due = Some(std::time::Instant::now() + bound);
            let _ = worker.commands.send(Command::End);
        }
        let received = match (worker.due, cx) {
            (Some(due), None) => worker
                .inbound
                .recv_timeout(due.saturating_duration_since(std::time::Instant::now()))
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => DeepgramError::Stalled(bound),
                    mpsc::RecvTimeoutError::Disconnected => stopped(),
                }),
            (Some(due), Some(cx)) => {
                worker.waker.register(cx.waker());
                match worker.inbound.try_recv() {
                    Ok(inbound) => Ok(inbound),
                    Err(mpsc::TryRecvError::Disconnected) => Err(stopped()),
                    Err(mpsc::TryRecvError::Empty) => {
                        if self.alarm.poll_at(due, cx).is_pending() {
                            return Poll::Pending;
                        }
                        Err(DeepgramError::Stalled(bound))
                    }
                }
            }
            (None, _) => match worker.inbound.try_recv() {
                Ok(inbound) => Ok(inbound),
                Err(mpsc::TryRecvError::Empty) => return Poll::Ready(None),
                Err(mpsc::TryRecvError::Disconnected) => Err(stopped()),
            },
        };
        Poll::Ready(Some(received.unwrap_or_else(Inbound::Failed)))
    }

    /// Acts on one server message; see the crate documentation for the mapping.
    fn apply(&mut self, message: ServerMessage) -> Result<(), DeepgramError> {
        match message {
            ServerMessage::Results {
                transcript,
                is_final,
                speech_final,
                end,
            } => {
                if self.open.is_none() && covered(end, self.committed_through) {
                    return Ok(());
                }
                if !transcript.trim().is_empty() {
                    self.hear(transcript)?;
                }
                if self.open.is_some() {
                    self.open_through = later(self.open_through, end);
                }
                if is_final || speech_final {
                    self.commit_open()?;
                }
            }
            ServerMessage::UtteranceEnd { last_word_end } => {
                if last_word_end >= 0.0 {
                    self.commit_open()?;
                }
            }
            ServerMessage::Metadata { .. } | ServerMessage::Ignored => {}
        }
        Ok(())
    }

    /// Ends the recognition at its summary `Metadata`, committing a span still open.
    fn summarise(&mut self, duration: Option<f64>) -> Result<(), DeepgramError> {
        self.duration = duration;
        self.commit_open()?;
        self.finished = true;
        self.worker = None;
        Ok(())
    }

    /// Opens a span reading `text`, or revises the open one to read it.
    fn hear(&mut self, text: String) -> Result<(), DeepgramError> {
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

    /// Commits the open span, if there is one.
    fn commit_open(&mut self) -> Result<(), DeepgramError> {
        if let Some((span_id, _)) = self.open.take() {
            let envelope = self.output.commit(span_id).map_err(refused)?;
            self.delivered.push_back(envelope);
            self.committed_through = later(self.committed_through, self.open_through.take());
        }
        Ok(())
    }
}

/// The later of two span ends, either unknown.
fn later(known: Option<f64>, other: Option<f64>) -> Option<f64> {
    match (known, other) {
        (Some(known), Some(other)) => Some(known.max(other)),
        (known, other) => known.or(other),
    }
}

/// Whether a result whose audio ends at `end` is audio already committed up to `through`.
fn covered(end: Option<f64>, through: Option<f64>) -> bool {
    matches!((end, through), (Some(end), Some(through)) if end <= through + SAME_END)
}

fn stopped() -> DeepgramError {
    DeepgramError::Runtime("the thread driving the connection stopped without a word".to_owned())
}

/// A refusal of the STT contract, which a well-formed stream of this binding never meets.
fn refused(error: impl fmt::Display) -> DeepgramError {
    DeepgramError::Protocol(format!("the STT contract refused the transcript: {error}"))
}

impl Iterator for DeepgramRecognition {
    type Item = EventEnvelope<TranscriptEvent>;

    /// The next event; see [`DeepgramRecognition`] for when it waits.
    fn next(&mut self) -> Option<Self::Item> {
        match self.advance(None) {
            Poll::Ready(envelope) => envelope,
            Poll::Pending => None,
        }
    }
}

impl DeepgramRecognition {
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
            let Some(inbound) = ready!(self.receive(cx.as_deref_mut())) else {
                return Poll::Ready(None);
            };
            let outcome = match inbound {
                Inbound::Message(message) => self.apply(message),
                Inbound::Summary(duration) => self.summarise(duration),
                Inbound::Failed(error) => Err(error),
            };
            if let Err(error) = outcome {
                self.fail(error);
            }
        }
    }
}

impl Drop for DeepgramRecognition {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

impl RecognitionHandle for DeepgramRecognition {
    fn output(&self) -> &TranscriptOutput {
        &self.output
    }

    fn offer_frame(&mut self, frame: AudioFrame) -> Result<(), InputEnded> {
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

/// Starts the thread that drives one connection to `url`.
fn spawn(shared: Arc<Shared>, url: String) -> Result<Worker, DeepgramError> {
    let (commands, receiver) = async_mpsc::unbounded_channel();
    let (sender, inbound) = mpsc::sync_channel(listen::INBOUND_DEPTH);
    let waker = Arc::new(WakerSlot::default());
    let sender = Inbox::new(sender, Arc::clone(&waker));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| DeepgramError::Runtime(error.to_string()))?;
    std::thread::Builder::new()
        .name("inference-deepgram".to_owned())
        .spawn(move || runtime.block_on(drive(shared, url, receiver, sender)))
        .map_err(|error| DeepgramError::Runtime(error.to_string()))?;
    Ok(Worker {
        commands,
        inbound,
        waker,
        due: None,
    })
}

/// What the thread does after one event of its loop.
enum Step {
    Go,
    /// The handle is gone or cancelled: close the connection, say nothing.
    Stop,
    /// Close the connection and tell the handle this, its last word.
    Finish(Inbound),
    /// The connection is lost: tell the handle this, its last word.
    Lost(DeepgramError),
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

/// The open connection and what the thread keeps about it.
struct Link {
    socket: Socket,
    read_timeout: Duration,
    last_sent: Instant,
    /// The one deadline of the summary, set when `CloseStream` is sent and never moved.
    closing: Option<Instant>,
}

impl Link {
    async fn command(&mut self, command: Option<Command>) -> Step {
        let written = match command {
            Some(Command::Audio(data)) => {
                self.last_sent = Instant::now();
                write(&mut self.socket, Message::binary(data), self.read_timeout).await
            }
            Some(Command::End) if self.closing.is_none() => {
                self.closing = Some(Instant::now() + self.read_timeout);
                write(
                    &mut self.socket,
                    Message::text(CLOSE_STREAM),
                    self.read_timeout,
                )
                .await
            }
            Some(Command::End) => Ok(()),
            Some(Command::Cancel) | None => return Step::Stop,
        };
        match written {
            Ok(()) => Step::Go,
            Err(error) => Step::Finish(Inbound::Failed(error)),
        }
    }

    /// Reads one message: the summary when `CloseStream` was sent and it is a `Metadata`.
    fn read(
        &self,
        message: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    ) -> Result<Option<Inbound>, Step> {
        let text = match read(message) {
            Ok(Some(text)) => text,
            Ok(None) => return Ok(None),
            Err(error) => return Err(Step::Lost(error)),
        };
        match listen::parse_server_message(&text) {
            Ok(ServerMessage::Metadata { duration }) if self.closing.is_some() => {
                Err(Step::Finish(Inbound::Summary(duration)))
            }
            Ok(message) => Ok(Some(Inbound::Message(message))),
            Err(error) => Err(Step::Finish(Inbound::Failed(error))),
        }
    }
}

/// Drives one connection: opens it, sends audio, `KeepAlive` and `CloseStream` as the handle
/// asks, and hands every server message to the handle, until the summary `Metadata`, a failure,
/// a cancel or the handle's drop. A cancel or drop during the opening handshake drops the attempt
/// at once. While the handle's queue is full the connection is not read.
async fn drive(
    shared: Arc<Shared>,
    url: String,
    mut commands: async_mpsc::UnboundedReceiver<Command>,
    inbound: Inbox,
) {
    let endpoint = &shared.endpoint;
    let opening = connection::open(
        &url,
        shared.source.as_ref(),
        &shared.credential,
        endpoint.connect_timeout,
    );
    tokio::pin!(opening);
    let mut early = VecDeque::new();
    let socket = loop {
        tokio::select! {
            opened = &mut opening => match opened {
                Ok(socket) => break socket,
                Err(error) => {
                    let _ = inbound.send(Inbound::Failed(error));
                    return;
                }
            },
            command = commands.recv() => match command {
                Some(Command::Cancel) | None => return,
                Some(command) => early.push_back(command),
            },
        }
    };
    let mut link = Link {
        socket,
        read_timeout: endpoint.read_timeout,
        last_sent: Instant::now(),
        closing: None,
    };
    let keepalive = shared.config.keepalive_interval;
    let mut held: Option<Inbound> = None;
    loop {
        let keepalive_at = link.last_sent + keepalive;
        let step = if let Some(command) = early.pop_front() {
            link.command(Some(command)).await
        } else {
            tokio::select! {
                command = commands.recv() => link.command(command).await,
                message = link.socket.next(), if held.is_none() => match link.read(message) {
                    Ok(None) => Step::Go,
                    Ok(Some(item)) => match offer(&inbound, item) {
                        Offered::Taken => Step::Go,
                        Offered::Full(item) => {
                            held = Some(item);
                            Step::Go
                        }
                        Offered::Gone => Step::Stop,
                    },
                    Err(step) => step,
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
                () = tokio::time::sleep_until(keepalive_at), if link.closing.is_none() => {
                    link.last_sent = Instant::now();
                    match write(&mut link.socket, Message::text(KEEP_ALIVE), link.read_timeout).await {
                        Ok(()) => Step::Go,
                        Err(error) => Step::Finish(Inbound::Failed(error)),
                    }
                }
                () = tokio::time::sleep_until(link.closing.unwrap_or(keepalive_at)), if link.closing.is_some() => {
                    Step::Finish(Inbound::Failed(DeepgramError::Stalled(link.read_timeout)))
                }
            }
        };
        let last = match step {
            Step::Go => continue,
            Step::Stop => {
                connection::close(link.socket).await;
                return;
            }
            Step::Finish(last) => {
                connection::close(link.socket).await;
                last
            }
            Step::Lost(error) => Inbound::Failed(error),
        };
        if held.is_none_or(|item| inbound.send(item).is_ok()) {
            let _ = inbound.send(last);
        }
        return;
    }
}

/// Writes `message`, waiting at most `bound`.
async fn write(
    socket: &mut Socket,
    message: Message,
    bound: Duration,
) -> Result<(), DeepgramError> {
    tokio::time::timeout(bound, socket.send(message))
        .await
        .map_err(|_| DeepgramError::Stalled(bound))?
        .map_err(|error| DeepgramError::Transport(error.to_string()))
}

/// The text of a message read, `None` for a ping or pong; a close, the end of the stream, a
/// transport error or a binary message is the failure it is.
fn read(
    message: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
) -> Result<Option<String>, DeepgramError> {
    match message {
        None => Err(DeepgramError::Closed {
            code: None,
            reason: String::new(),
            cause: CloseCause::Other,
        }),
        Some(Err(error)) => Err(DeepgramError::Transport(error.to_string())),
        Some(Ok(Message::Text(text))) => Ok(Some(text.as_str().to_owned())),
        Some(Ok(Message::Close(frame))) => {
            let code = frame.as_ref().map(|frame| u16::from(frame.code));
            let reason = frame
                .map(|frame| frame.reason.as_str().to_owned())
                .unwrap_or_default();
            Err(DeepgramError::Closed {
                code,
                cause: CloseCause::of(code, &reason),
                reason,
            })
        }
        Some(Ok(Message::Binary(data))) => Err(DeepgramError::Protocol(format!(
            "a binary message of {} bytes; the endpoint sends text",
            data.len()
        ))),
        Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};

    use inference_stt::inference_audio::AudioEncoding;
    use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
    use inference_stt::inference_text::TurnId;

    use super::*;
    use crate::credential::{Credential, CredentialFuture};

    struct NoSource;

    impl CredentialSource for NoSource {
        fn resolve<'a>(&'a self, _: &'a str) -> CredentialFuture<'a> {
            Box::pin(std::future::ready(Ok(Credential::new(Vec::new()))))
        }
    }

    fn recognition() -> DeepgramRecognition {
        let mut recogniser = DeepgramRecogniser::new(
            Endpoint::new(),
            ListenConfig::new(),
            Arc::new(NoSource),
            "deepgram-api-key",
        )
        .unwrap();
        recogniser
            .start(RecognitionRequest::new(
                RunId::generate(),
                TurnId::generate(),
                StreamId::generate(),
                NodeId::generate(),
                "text_out",
            ))
            .unwrap()
    }

    fn results(transcript: &str, is_final: bool, speech_final: bool) -> ServerMessage {
        ServerMessage::Results {
            transcript: transcript.to_owned(),
            is_final,
            speech_final,
            end: None,
        }
    }

    fn ending(transcript: &str, is_final: bool, end: f64) -> ServerMessage {
        ServerMessage::Results {
            transcript: transcript.to_owned(),
            is_final,
            speech_final: false,
            end: Some(end),
        }
    }

    #[test]
    fn a_final_result_of_audio_an_utterance_end_already_committed_is_dropped() {
        let mut recognition = recognition();
        recognition
            .apply(ending("good morning", false, 0.5))
            .unwrap();
        recognition
            .apply(ServerMessage::UtteranceEnd { last_word_end: 1.2 })
            .unwrap();
        recognition
            .apply(ending("good morning", true, 0.5))
            .unwrap();
        recognition.apply(ending("bye", false, 1.7)).unwrap();
        recognition.apply(ending("bye", true, 1.7)).unwrap();
        assert_eq!(
            kinds(&mut recognition),
            [
                "hypothesis good morning",
                "commit",
                "hypothesis bye",
                "commit"
            ]
        );
    }

    #[test]
    fn results_without_a_span_end_are_never_dropped() {
        let mut recognition = recognition();
        recognition.apply(ending("one", true, 0.5)).unwrap();
        recognition.apply(results("two", true, false)).unwrap();
        assert_eq!(
            kinds(&mut recognition),
            ["hypothesis one", "commit", "hypothesis two", "commit"]
        );
    }

    #[test]
    fn a_metadata_message_never_ends_the_recognition_only_the_summary_does() {
        let mut recognition = recognition();
        recognition.apply(results("open", false, false)).unwrap();
        recognition
            .apply(ServerMessage::Metadata {
                duration: Some(0.1),
            })
            .unwrap();
        assert!(!recognition.finished);
        assert_eq!(recognition.provider_duration(), None);
        recognition.summarise(Some(0.2)).unwrap();
        assert!(recognition.finished);
        assert_eq!(recognition.provider_duration(), Some(0.2));
        assert_eq!(kinds(&mut recognition), ["hypothesis open", "commit"]);
    }

    #[test]
    fn a_full_queue_holds_the_message_and_a_dropped_one_says_gone() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let sender = Inbox::new(sender, Arc::default());
        assert!(matches!(
            offer(&sender, Inbound::Summary(None)),
            Offered::Taken
        ));
        assert!(matches!(
            offer(&sender, Inbound::Summary(Some(1.0))),
            Offered::Full(Inbound::Summary(Some(_)))
        ));
        drop(receiver);
        assert!(matches!(
            offer(&sender, Inbound::Summary(None)),
            Offered::Gone
        ));
    }

    fn kinds(recognition: &mut DeepgramRecognition) -> Vec<String> {
        recognition
            .delivered
            .drain(..)
            .map(|envelope| match envelope.payload {
                TranscriptEvent::Hypothesis(event) => format!("hypothesis {}", event.text),
                TranscriptEvent::Revision(event) => format!("revision {}", event.text),
                TranscriptEvent::Committed(_) => "commit".to_owned(),
                TranscriptEvent::Usage(_) => "usage".to_owned(),
            })
            .collect()
    }

    #[test]
    fn interim_then_final_is_a_hypothesis_a_revision_and_a_commit() {
        let mut recognition = recognition();
        for message in [
            results("hello", false, false),
            results("hello there", false, false),
            results("hello there", true, true),
        ] {
            recognition.apply(message).unwrap();
        }
        assert_eq!(
            kinds(&mut recognition),
            ["hypothesis hello", "revision hello there", "commit"]
        );
    }

    #[test]
    fn utterance_end_commits_the_open_span_unless_it_was_already_finalized() {
        let mut recognition = recognition();
        recognition.apply(results("wait", false, false)).unwrap();
        recognition
            .apply(ServerMessage::UtteranceEnd {
                last_word_end: -1.0,
            })
            .unwrap();
        recognition
            .apply(results("wait for it", false, false))
            .unwrap();
        recognition
            .apply(ServerMessage::UtteranceEnd { last_word_end: 1.2 })
            .unwrap();
        recognition
            .apply(ServerMessage::UtteranceEnd { last_word_end: 1.4 })
            .unwrap();
        assert_eq!(
            kinds(&mut recognition),
            ["hypothesis wait", "revision wait for it", "commit"]
        );
    }

    #[test]
    fn an_empty_transcript_emits_nothing_and_a_final_one_commits_what_is_open() {
        let mut recognition = recognition();
        recognition.apply(results("", false, false)).unwrap();
        recognition.apply(results("  ", true, false)).unwrap();
        assert_eq!(kinds(&mut recognition), Vec::<String>::new());
        recognition.apply(results("hi", false, false)).unwrap();
        recognition.apply(results("", false, false)).unwrap();
        recognition.apply(results("", true, false)).unwrap();
        assert_eq!(kinds(&mut recognition), ["hypothesis hi", "commit"]);
    }

    #[test]
    fn a_frame_in_another_format_than_the_first_fails_the_recognition() {
        let mut recognition = recognition();
        recognition.format = Some(AudioFormat {
            encoding: AudioEncoding::PcmMulaw,
            sample_rate: NonZeroU32::new(8_000).unwrap(),
            channels: NonZeroU16::MIN,
        });
        let frame = inference_testkit::stt::frames(1).remove(0);
        assert!(matches!(
            recognition.prepare(frame.format),
            Err(DeepgramError::UnsupportedAudio(_))
        ));
    }
}
