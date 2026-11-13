//! The binding of the TTS contract: [`CartesiaSynthesiser`] and its synthesis handle.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, ready};

use inference_tts::inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::inference_core::model::{
    Adapter, AdapterDescriptor, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_tts::inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued, SegmentState,
};
use inference_tts::{
    CONTRACT_REVISION, ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt,
    PlaybackRefused, SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};

use crate::config::{Endpoint, VoiceConfig};
use crate::connection::{CLOSE_BOUND, Connection};
use crate::credential::{CARTESIA_KEY_NAME, CredentialSource, EnvironmentSource};
use crate::error::{CartesiaError, ConfigError};
use crate::protocol::{self, ServerMessage};
use crate::wake;

/// The backend name of the adapter record.
pub const BACKEND: &str = "cartesia-tts-websocket";

/// The capabilities an adapter record of the binding claims. `incremental_input`: a segment is
/// sent as soon as the handle is iterated after it was offered, before the utterance ends.
/// `output_streaming`: each audio chunk is emitted as a frame while its segment is still Queued.
/// Nothing else: a segment offered while another speaks waits for it, and the binding declares
/// no latency for a cancel.
pub fn claims() -> Vec<Capability> {
    vec![Capability::OutputStreaming, Capability::IncrementalInput]
}

/// What every synthesis of one synthesiser shares.
struct Shared {
    url: String,
    endpoint: Endpoint,
    config: VoiceConfig,
    format: AudioFormat,
    source: Arc<dyn CredentialSource>,
    credential: String,
}

/// Cartesia's text-to-speech WebSocket as a synthesiser of the TTS contract, at locality
/// `external_provider`. See the crate documentation.
pub struct CartesiaSynthesiser {
    adapter: Adapter,
    shared: Arc<Shared>,
    running: Arc<AtomicUsize>,
}

impl fmt::Debug for CartesiaSynthesiser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CartesiaSynthesiser")
            .field("adapter", &self.adapter.adapter_id())
            .field("config", &self.shared.config)
            .field("endpoint", &self.shared.endpoint)
            .field("credential", &self.shared.credential)
            .finish_non_exhaustive()
    }
}

impl CartesiaSynthesiser {
    /// A synthesiser of `config` at `endpoint`, resolving the secret `credential` names through
    /// `source` for every connection, under an adapter record of its own claiming [`claims`] with
    /// a maximum concurrency of one.
    ///
    /// # Errors
    /// The [`ConfigError`] of [`VoiceConfig::validate`] or of the URL.
    pub fn new(
        endpoint: Endpoint,
        config: VoiceConfig,
        source: Arc<dyn CredentialSource>,
        credential: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        config.validate()?;
        let url = protocol::request_url(&endpoint)?;
        let adapter = declare(1)?;
        Ok(Self {
            adapter,
            shared: Arc::new(Shared {
                url,
                endpoint,
                format: config.output_format.audio_format(),
                config,
                source,
                credential: credential.into(),
            }),
            running: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// [`CartesiaSynthesiser::new`] with the key from the environment variable
    /// `CARTESIA_API_KEY`, read at every connection.
    ///
    /// # Errors
    /// As [`CartesiaSynthesiser::new`].
    pub fn from_env(endpoint: Endpoint, config: VoiceConfig) -> Result<Self, ConfigError> {
        Self::new(
            endpoint,
            config,
            Arc::new(EnvironmentSource::cartesia()),
            CARTESIA_KEY_NAME,
        )
    }

    /// This synthesiser under a new adapter record that allows `max_concurrency` syntheses at
    /// once, as the account's plan allows.
    ///
    /// # Errors
    /// [`ConfigError::MaxConcurrency`] when it is not positive.
    pub fn with_max_concurrency(mut self, max_concurrency: i64) -> Result<Self, ConfigError> {
        self.adapter = declare(max_concurrency)?;
        Ok(self)
    }

    /// The configuration it synthesises with.
    pub fn config(&self) -> &VoiceConfig {
        &self.shared.config
    }

    /// The format of every frame it emits.
    pub fn format(&self) -> AudioFormat {
        self.shared.format
    }
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

impl Synthesiser for CartesiaSynthesiser {
    type Handle = CartesiaSynthesis;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(&mut self) -> Result<CartesiaSynthesis, ConcurrencyLimitReached> {
        let max_concurrency = self.adapter.max_concurrency();
        let limit = usize::try_from(max_concurrency).unwrap_or(usize::MAX);
        let slot = Slot(Arc::clone(&self.running));
        if self.running.fetch_add(1, Ordering::AcqRel) >= limit {
            drop(slot);
            return Err(ConcurrencyLimitReached { max_concurrency });
        }
        Ok(CartesiaSynthesis {
            output: SynthesisOutput::start(&self.adapter),
            shared: Arc::clone(&self.shared),
            runtime: None,
            connection: None,
            exchange: None,
            live: None,
            retired: HashSet::new(),
            contexts: 0,
            pending: VecDeque::new(),
            delivered: VecDeque::new(),
            next_sample: 0,
            failure: None,
            _slot: slot,
        })
    }
}

/// The place of one running synthesis, given back when it is dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The segment being spoken: its context, and the bytes of a sample split across chunks.
struct Live {
    segment_id: SegmentId,
    context_id: String,
    carry: Vec<u8>,
}

/// One synthesis of a [`CartesiaSynthesiser`]; it runs until it is dropped.
///
/// Each exchange with the server runs on a runtime of the synthesis's own, with one thread.
/// Iterating it blocks while it waits for the server, each wait bounded by the endpoint's read
/// timeout; its async form, [`SynthesisHandle::poll_event`], returns `Pending` there instead and
/// is woken when the exchange ends. A failure ends the iteration of the segments it took; [`CartesiaSynthesis::failure`]
/// says why, the usage record then is not final, and the segments offered after it are taken and
/// not spoken.
pub struct CartesiaSynthesis {
    output: SynthesisOutput,
    shared: Arc<Shared>,
    runtime: Option<tokio::runtime::Runtime>,
    connection: Option<Connection>,
    /// The exchange in flight, which holds the connection until it ends.
    exchange: Option<Exchange>,
    live: Option<Live>,
    /// The contexts cancelled or finished, whose later messages are dropped.
    retired: HashSet<String>,
    /// How many contexts the synthesis has opened.
    contexts: u64,
    pending: VecDeque<(SegmentId, String)>,
    delivered: VecDeque<SynthesisEvent>,
    next_sample: u64,
    failure: Option<CartesiaError>,
    _slot: Slot,
}

impl fmt::Debug for CartesiaSynthesis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CartesiaSynthesis")
            .field("format", &self.shared.format)
            .field("segments", &self.output.segments().len())
            .field("speaking", &self.live.as_ref().map(|live| live.segment_id))
            .field("connected", &self.connection.is_some())
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl CartesiaSynthesis {
    /// Why the synthesis failed, once it has.
    pub fn failure(&self) -> Option<&CartesiaError> {
        self.failure.as_ref()
    }

    /// The format of every frame it emits.
    pub fn format(&self) -> AudioFormat {
        self.shared.format
    }
}

impl CartesiaSynthesis {
    /// Moves every segment offered and not yet taken into the queue of segments to speak.
    fn take_input(&mut self) {
        if let Ok(taken) = self.output.take_input() {
            self.pending.extend(
                taken
                    .iter()
                    .map(|segment| (segment.segment_id(), segment.text().to_owned())),
            );
        }
    }

    fn state(&self, segment_id: SegmentId) -> Option<SegmentState> {
        self.output.segment(segment_id).map(OutputSegment::state)
    }

    /// Starts speaking the segment `segment_id` on the context `context_id`: the segment is the
    /// one being spoken from now on, and its generation request `message` is sent, on a
    /// connection opened first when the synthesis has none.
    fn send_request(&mut self, segment_id: SegmentId, context_id: String, message: String) {
        let shared = Arc::clone(&self.shared);
        let connection = self.connection.take();
        let task = self.start(async move {
            let mut connection = match connection {
                Some(connection) => connection,
                None => {
                    let endpoint = &shared.endpoint;
                    Connection::open(
                        &shared.url,
                        shared.source.as_ref(),
                        &shared.credential,
                        endpoint.connect_timeout,
                        endpoint.read_timeout,
                    )
                    .await?
                }
            };
            connection.send(message).await?;
            Ok::<_, CartesiaError>(connection)
        });
        match task {
            Ok(task) => {
                self.live = Some(Live {
                    segment_id,
                    context_id,
                    carry: Vec::new(),
                });
                self.exchange = Some(Exchange::Send(task));
            }
            Err(error) => self.fail(error),
        }
    }

    /// Starts reading the next message of the connection.
    fn read(&mut self) -> Result<(), CartesiaError> {
        let Some(mut connection) = self.connection.take() else {
            return Err(CartesiaError::Closed {
                code: None,
                reason: String::new(),
            });
        };
        let task = self.start(async move {
            let text = connection.read().await;
            (connection, text)
        })?;
        self.exchange = Some(Exchange::Read(task));
        Ok(())
    }

    /// The synthesis's runtime, built on first use, with one worker thread of its own.
    fn runtime(&mut self) -> Result<&tokio::runtime::Runtime, CartesiaError> {
        if self.runtime.is_none() {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("inference-cartesia")
                .enable_all()
                .build()
                .map_err(|error| CartesiaError::Runtime(error.to_string()))?;
            self.runtime = Some(runtime);
        }
        self.runtime
            .as_ref()
            .ok_or_else(|| CartesiaError::Runtime("no runtime".to_owned()))
    }

    /// Runs `future` on the synthesis's runtime.
    fn start<F>(&mut self, future: F) -> Result<tokio::task::JoinHandle<F::Output>, CartesiaError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        Ok(self.runtime()?.spawn(future))
    }

    /// Waits for the exchange in flight to end and acts on how it ended: blocking without `cx`,
    /// `Pending` with it until the exchange ends, which wakes `cx`.
    fn settle(&mut self, cx: Option<&mut Context<'_>>) -> Poll<()> {
        let outcome = match self.exchange.as_mut() {
            None => return Poll::Ready(()),
            Some(Exchange::Send(task)) => {
                ready!(join(task, cx))
                    .and_then(|sent| sent)
                    .map(|connection| {
                        self.connection = Some(connection);
                        String::new()
                    })
            }
            Some(Exchange::Read(task)) => ready!(join(task, cx)).and_then(|(connection, read)| {
                self.connection = Some(connection);
                read
            }),
            Some(Exchange::Close(task)) => {
                let _ = ready!(join(task, cx));
                Ok(String::new())
            }
        };
        let read = matches!(self.exchange.take(), Some(Exchange::Read(_)));
        let outcome = match outcome {
            Ok(text) if read => self.receive(&text),
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = outcome {
            self.fail(error);
        }
        Poll::Ready(())
    }

    /// Acts on `text`, one message read from the connection.
    fn receive(&mut self, text: &str) -> Result<(), CartesiaError> {
        let message = protocol::parse_server_message(text)?;
        let live_context = self.live.as_ref().map(|live| live.context_id.as_str());
        match message.context_id() {
            Some(context_id) if Some(context_id) == live_context => {}
            Some(context_id) if self.retired.contains(context_id) => return Ok(()),
            Some(_) => {
                return Err(CartesiaError::Protocol(
                    "a message of a context the binding did not open".to_owned(),
                ));
            }
            None => {}
        }
        match message {
            ServerMessage::Chunk { audio, done, .. } => {
                self.emit(audio)?;
                if done {
                    self.finish_segment()?;
                }
            }
            ServerMessage::Done { .. } => self.finish_segment()?,
            ServerMessage::Error { error, .. } => return Err(CartesiaError::Server(error)),
            ServerMessage::Ignored { .. } => {}
        }
        Ok(())
    }

    /// Emits the whole samples of `audio`, after the bytes kept from the chunk before, as one
    /// frame of the segment being spoken at the next position on the synthesis's sample clock.
    /// The bytes of a sample split across chunks are kept for the next.
    fn emit(&mut self, audio: Vec<u8>) -> Result<(), CartesiaError> {
        let format = self.shared.format;
        let Some(live) = self.live.as_mut() else {
            return Ok(());
        };
        let mut data = std::mem::take(&mut live.carry);
        data.extend_from_slice(&audio);
        let width = usize::try_from(format.encoding.width()).unwrap_or(1);
        let whole = data.len() - data.len() % width;
        live.carry = data.split_off(whole);
        if whole == 0 {
            return Ok(());
        }
        let sample_count = u32::try_from(whole / width)
            .map_err(|_| CartesiaError::Protocol(format!("an audio chunk of {whole} bytes")))?;
        let position = SampleClockPosition::new(self.next_sample).ok_or_else(|| {
            CartesiaError::Protocol("the sample clock passed i64::MAX".to_owned())
        })?;
        let frame = AudioFrame {
            format,
            position,
            sample_count,
            data,
        };
        let event = self.output.audio(live.segment_id, frame).map_err(refused)?;
        self.next_sample += u64::from(sample_count);
        self.delivered.push_back(event);
        Ok(())
    }

    /// Completes the segment being spoken at the end of its context, and retires the context.
    fn finish_segment(&mut self) -> Result<(), CartesiaError> {
        let Some(live) = self.live.take() else {
            return Ok(());
        };
        self.retired.insert(live.context_id);
        if !live.carry.is_empty() {
            return Err(CartesiaError::Protocol(format!(
                "the segment's audio ends in {} byte(s) of an incomplete sample",
                live.carry.len()
            )));
        }
        let events = self
            .output
            .complete(live.segment_id, Vec::new())
            .map_err(refused)?;
        self.delivered.extend(events);
        Ok(())
    }

    /// Cancels the context of the segment being spoken, if there is one: it is retired, so
    /// nothing more of it is emitted, and its cancel is sent, waiting at most [`CLOSE_BOUND`]. A
    /// cancel that cannot be written closes the connection; the next segment opens another.
    fn cancel_live(&mut self) {
        let Some(live) = self.live.take() else {
            return;
        };
        let message = protocol::cancel_request(&live.context_id);
        self.retired.insert(live.context_id);
        let Some(mut connection) = self.connection.take() else {
            return;
        };
        let Ok(task) = self.start(async move {
            let sent = connection.send_within(message, CLOSE_BOUND).await;
            (connection, sent)
        }) else {
            return;
        };
        let Some(Ok((connection, sent))) =
            wake::block_on_until(task, std::time::Instant::now().checked_add(CLOSE_BOUND * 2))
        else {
            return;
        };
        match sent {
            Ok(()) => self.connection = Some(connection),
            Err(_) => self.close_now(connection),
        }
    }

    /// Closes `connection` at once, waiting at most [`CLOSE_BOUND`].
    fn close_now(&mut self, connection: Connection) {
        if let Ok(task) = self.start(connection.close()) {
            let _ = wake::block_on_until(task, std::time::Instant::now().checked_add(CLOSE_BOUND));
        }
    }

    /// Closes the connection, if there is one, reading nothing more. The close is an exchange
    /// the next step waits for.
    fn close(&mut self) {
        if let Some(connection) = self.connection.take()
            && let Ok(task) = self.start(connection.close())
        {
            self.exchange = Some(Exchange::Close(task));
        }
    }

    /// Ends the synthesis as failed by `error`: its connection closed, its call marked failed,
    /// and nothing more spoken.
    fn fail(&mut self, error: CartesiaError) {
        self.live = None;
        self.close();
        self.pending.clear();
        self.output.mark_failed();
        self.failure = Some(error);
    }

    /// The next event: blocking while it waits for the server, or with `cx`, the async form's,
    /// `Pending` there instead.
    fn advance(&mut self, mut cx: Option<&mut Context<'_>>) -> Poll<Option<SynthesisEvent>> {
        loop {
            if let Some(event) = self.delivered.pop_front() {
                return Poll::Ready(Some(event));
            }
            if self.exchange.is_some() {
                ready!(self.settle(cx.as_deref_mut()));
                continue;
            }
            if self.failure.is_some() {
                self.take_input();
                return Poll::Ready(self.output.usage().ok());
            }
            if self.live.is_some() {
                if let Err(error) = self.read() {
                    self.fail(error);
                }
                continue;
            }
            self.take_input();
            let Some((segment_id, text)) = self.pending.pop_front() else {
                return Poll::Ready(self.output.usage().ok());
            };
            if self.state(segment_id) != Some(SegmentState::Queued) {
                continue;
            }
            self.contexts += 1;
            let context_id = format!("segment-{}", self.contexts);
            let Some(message) =
                protocol::generation_request(&self.shared.config, &context_id, &text)
            else {
                match self.output.complete(segment_id, Vec::new()) {
                    Ok(events) => self.delivered.extend(events),
                    Err(error) => self.fail(refused(error)),
                }
                continue;
            };
            self.send_request(segment_id, context_id, message);
        }
    }
}

/// An exchange with the server in flight on the synthesis's runtime; the connection travels
/// with it and comes back when it ends.
enum Exchange {
    /// A generation request is sent, on a connection opened first when there was none.
    Send(tokio::task::JoinHandle<Result<Connection, CartesiaError>>),
    /// One message is read.
    Read(tokio::task::JoinHandle<(Connection, Result<String, CartesiaError>)>),
    /// The connection is closed.
    Close(tokio::task::JoinHandle<()>),
}

impl Exchange {
    /// Stops the exchange; the connection it holds is dropped with it.
    fn abort(self) {
        match self {
            Exchange::Send(task) => task.abort(),
            Exchange::Read(task) => task.abort(),
            Exchange::Close(task) => task.abort(),
        }
    }
}

/// What `task` returned: blocking the calling thread without `cx`, `Pending` with it until the
/// task ends, which wakes `cx`.
fn join<T>(
    task: &mut tokio::task::JoinHandle<T>,
    cx: Option<&mut Context<'_>>,
) -> Poll<Result<T, CartesiaError>> {
    let joined = match cx {
        None => wake::block_on(&mut *task),
        Some(cx) => ready!(Pin::new(task).poll(cx)),
    };
    Poll::Ready(joined.map_err(|error| {
        CartesiaError::Runtime(format!("the task driving the connection ended: {error}"))
    }))
}

/// A refusal of the TTS contract, which a well-formed segment of this binding never meets.
fn refused(error: impl fmt::Display) -> CartesiaError {
    CartesiaError::Protocol(format!("the TTS contract refused the audio: {error}"))
}

impl Iterator for CartesiaSynthesis {
    type Item = SynthesisEvent;

    /// The next event. It blocks while a segment is being spoken, each read bounded by the read
    /// timeout; `None` once nothing more is delivered until more is offered or the utterance
    /// ends.
    fn next(&mut self) -> Option<SynthesisEvent> {
        match self.advance(None) {
            Poll::Ready(event) => event,
            Poll::Pending => None,
        }
    }
}

impl Drop for CartesiaSynthesis {
    /// Closes the connection, waiting at most [`CLOSE_BOUND`]; an exchange in flight other than
    /// a close is dropped with its connection.
    fn drop(&mut self) {
        let closing = match self.exchange.take() {
            Some(Exchange::Close(task)) => Some(task),
            Some(exchange) => {
                exchange.abort();
                None
            }
            None => None,
        };
        let closing = closing.or_else(|| {
            let connection = self.connection.take()?;
            self.start(connection.close()).ok()
        });
        if let Some(task) = closing {
            let _ = wake::block_on_until(task, std::time::Instant::now().checked_add(CLOSE_BOUND));
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl SynthesisHandle for CartesiaSynthesis {
    fn output(&self) -> &SynthesisOutput {
        &self.output
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        self.output.offer(segment)
    }

    fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        self.output.end_utterance()
    }

    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        self.output.acknowledge_playback(segment_id, receipt)
    }

    /// Takes the next event, `Pending` where iterating blocks: while an exchange with the
    /// server is in flight, which wakes `cx` when it ends.
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<SynthesisEvent>> {
        self.advance(Some(cx))
    }

    /// Discards the segment through [`SynthesisOutput::discard`] and drops every event of it not
    /// yet delivered. When it is the segment being spoken, a barge-in, the protocol's cancel for
    /// its context is sent, waiting at most [`CLOSE_BOUND`](crate::CLOSE_BOUND), and every later
    /// message of that context is dropped; the connection stays open for the next segment. When
    /// the async form left a request or a read in flight, which holds the connection, that
    /// exchange is dropped with the connection instead, without waiting, and the next segment
    /// opens another.
    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        let event = self.output.discard(segment_id)?;
        self.delivered
            .retain(|event| event.segment_id() != Some(segment_id));
        if self
            .live
            .as_ref()
            .is_some_and(|live| live.segment_id == segment_id)
        {
            if matches!(self.exchange, Some(Exchange::Send(_) | Exchange::Read(_))) {
                if let Some(exchange) = self.exchange.take() {
                    exchange.abort();
                }
                if let Some(live) = self.live.take() {
                    self.retired.insert(live.context_id);
                }
            } else {
                self.cancel_live();
            }
        }
        Ok(event)
    }
}
