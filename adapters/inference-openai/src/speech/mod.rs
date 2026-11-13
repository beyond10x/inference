//! OpenAI's streamed speech endpoint as a synthesiser of the `inference-tts` contract.
//!
//! [`OpenAiSpeech`] implements [`inference_tts::Synthesiser`] over `POST /v1/audio/speech` at
//! locality `external_provider`: the text of every segment leaves the machine. [`protocol`] says
//! what it sends and reads, the documentation it was taken from with its date, and what that
//! documentation leaves open.
//!
//! A synthesis takes the segments offered to it in order. Each segment is one request, its text
//! sent as given; the response's body, 24 kHz 16-bit PCM, is read chunk by chunk and emitted as
//! frames of that segment while it is still Queued, on one sample clock across the synthesis, and
//! the end of the body completes it. A segment is sent as soon as the handle is iterated after it
//! was offered, before the utterance ends. So the binding claims `incremental_input` and
//! `output_streaming` ([`claims`]), and `records/claims-speech.json` holds the same claims.
//!
//! The frames are in the format the [`SpeechConfig`] names: mono `pcm_s16le`, `pcm_mulaw` or
//! `pcm_alaw` at 8, 16, 24 or 48 kHz, resampled from 24 kHz by `inference-audio`'s anti-aliased
//! resampler and encoded by its G.711 codec. A segment whose text is longer than the
//! 4096 characters ([`MAX_INPUT_CHARACTERS`]) one request takes fails the synthesis with
//! [`SpeechError::InputTooLong`] before anything of it is sent; a blank segment is completed with
//! no audio and no request.
//!
//! A cancel is a discard of the segment being spoken, as a barge-in makes it: the endpoint has no
//! cancel request, so the binding drops the response and closes its connection, and emits nothing
//! more of the segment. The utterance still ends with its one usage record, which counts the
//! audio emitted before the cancel. Dropping the handle closes its connection the same way. A
//! request that is refused, stalls or answers outside the protocol fails the synthesis with a
//! [`SpeechError`] ([`OpenAiSpeechSynthesis::failure`]), never a panic or a hang: making the
//! connection is bounded by the endpoint's connect timeout, the wait for the response's head and
//! for each chunk of its body by its read timeout.
//!
//! The handle drives its requests on a runtime of its own, with one thread. Iterated, as the
//! contract defines it, it blocks the calling thread while it waits for the server, on any
//! thread, an async worker included, which it then holds up. Its async form,
//! [`SynthesisHandle::event`], waits without blocking, so an async caller awaits that instead.
//!
//! The key is never read by the binding itself: the caller injects a
//! [`CredentialSource`](crate::CredentialSource) and names the secret
//! ([`EnvironmentSource::openai`](crate::EnvironmentSource::openai) reads `OPENAI_API_KEY`; with
//! feature `keychain`, `KeychainSource` reads the OS keychain). It is resolved for every request,
//! sent in its `Authorization` header, marked sensitive, and never kept, printed, logged or put
//! in an error.
//!
//! No field of the binding holds the key. Every copy, for each request:
//!
//! - the [`Credential`](crate::Credential) the source returns, zeroized on drop;
//! - one zeroizing buffer holding `Bearer <key>`, built from the credential, which is dropped at
//!   once. The header value is that buffer itself, not a copy of it, and the buffer is zeroized
//!   when its last holder drops. `reqwest`'s pending request keeps a clone of the headers, which
//!   shares that buffer, until the response head arrives, so the buffer lives that long, or until
//!   the request fails;
//! - the request head hyper, the HTTP client below `reqwest`, writes the header into: a buffer of
//!   the connection that is not zeroized and is reused, so the key stays in it until the next
//!   request on that connection overwrites it or the idle connection is closed, after 90 seconds
//!   by default, and the smaller buffers it outgrew may hold part of it too. The binding cannot
//!   reach that buffer short of an HTTP client of its own; it is memory of this process, which
//!   matters only to someone who can already read this process's memory or a core dump of it,
//!   and the binding accepts that;
//! - for `https://`, the TLS record rustls copies the head into and encrypts in place, so what it
//!   keeps is ciphertext.

mod config;
pub mod protocol;
mod transcode;

use std::collections::VecDeque;
use std::fmt;
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
use reqwest::header::{AUTHORIZATION, HeaderValue};

pub use config::{
    DEFAULT_MODEL, DEFAULT_URL, MAX_SPEED, MIN_SPEED, SpeechConfig, SpeechConfigError,
    SpeechEndpoint, SpeechError, SpeechVoice,
};
pub use protocol::MAX_INPUT_CHARACTERS;

use crate::credential::{CredentialSource, EnvironmentSource, OPENAI_KEY_NAME};
use transcode::Transcoder;

/// The backend name of the adapter record.
pub const BACKEND: &str = "openai-audio-speech";

/// The capabilities an adapter record of the binding claims. `incremental_input`: a segment is
/// sent as soon as the handle is iterated after it was offered, before the utterance ends.
/// `output_streaming`: each chunk of the response is emitted as frames while its segment is
/// still Queued. Nothing else: a segment offered while another speaks waits for it, and the
/// binding declares no latency for a cancel.
pub fn claims() -> Vec<Capability> {
    vec![Capability::OutputStreaming, Capability::IncrementalInput]
}

/// What every synthesis of one synthesiser shares.
struct Shared {
    endpoint: SpeechEndpoint,
    config: SpeechConfig,
    source: Arc<dyn CredentialSource>,
    credential: String,
}

/// OpenAI's speech endpoint as a synthesiser of the TTS contract, at locality
/// `external_provider`. See the module documentation.
pub struct OpenAiSpeech {
    adapter: Adapter,
    shared: Arc<Shared>,
    running: Arc<AtomicUsize>,
}

impl fmt::Debug for OpenAiSpeech {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiSpeech")
            .field("adapter", &self.adapter.adapter_id())
            .field("config", &self.shared.config)
            .field("endpoint", &self.shared.endpoint)
            .field("credential", &self.shared.credential)
            .finish_non_exhaustive()
    }
}

impl OpenAiSpeech {
    /// A synthesiser of `config` at `endpoint`, resolving the secret `credential` names through
    /// `source` for every request, under an adapter record of its own claiming [`claims`] with a
    /// maximum concurrency of one.
    ///
    /// # Errors
    /// The [`SpeechConfigError`] of [`SpeechConfig::validate`], or
    /// [`SpeechConfigError::InvalidEndpoint`] when the URL is not `https://`, or `http://` to a
    /// loopback host.
    pub fn new(
        endpoint: SpeechEndpoint,
        config: SpeechConfig,
        source: Arc<dyn CredentialSource>,
        credential: impl Into<String>,
    ) -> Result<Self, SpeechConfigError> {
        config.validate()?;
        check_url(&endpoint.url)?;
        Ok(Self {
            adapter: declare(1)?,
            shared: Arc::new(Shared {
                endpoint,
                config,
                source,
                credential: credential.into(),
            }),
            running: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// [`OpenAiSpeech::new`] with the key from the environment variable `OPENAI_API_KEY`, read at
    /// every request.
    ///
    /// # Errors
    /// As [`OpenAiSpeech::new`].
    pub fn from_env(
        endpoint: SpeechEndpoint,
        config: SpeechConfig,
    ) -> Result<Self, SpeechConfigError> {
        Self::new(
            endpoint,
            config,
            Arc::new(EnvironmentSource::openai()),
            OPENAI_KEY_NAME,
        )
    }

    /// This synthesiser under a new adapter record that allows `max_concurrency` syntheses at
    /// once, as the account's limits allow.
    ///
    /// # Errors
    /// [`SpeechConfigError::MaxConcurrency`] when it is not positive.
    pub fn with_max_concurrency(mut self, max_concurrency: i64) -> Result<Self, SpeechConfigError> {
        self.adapter = declare(max_concurrency)?;
        Ok(self)
    }

    /// The configuration it synthesises with.
    pub fn config(&self) -> &SpeechConfig {
        &self.shared.config
    }

    /// The format of every frame it emits.
    pub fn format(&self) -> AudioFormat {
        self.shared.config.output
    }
}

fn declare(max_concurrency: i64) -> Result<Adapter, SpeechConfigError> {
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
    .map_err(|_| SpeechConfigError::MaxConcurrency(max_concurrency))
}

/// Refuses a URL that is not `https://`, or `http://` to a loopback host: `http://` would send
/// the key in the clear. The scheme is read without regard to case.
fn check_url(url: &str) -> Result<(), SpeechConfigError> {
    let refused = |detail: &str| Err(SpeechConfigError::InvalidEndpoint(detail.to_owned()));
    let Some((scheme, rest)) = url.split_once("://") else {
        return refused(
            "the URL has no scheme; it must be https://, or http:// to a loopback host",
        );
    };
    if scheme.eq_ignore_ascii_case("https") {
        Ok(())
    } else if scheme.eq_ignore_ascii_case("http") {
        if crate::client::is_loopback(rest) {
            Ok(())
        } else {
            refused(
                "http:// would send the key in the clear; it is taken only to a loopback host, \
                 use https://",
            )
        }
    } else {
        refused("the scheme must be https://, or http:// to a loopback host")
    }
}

impl Synthesiser for OpenAiSpeech {
    type Handle = OpenAiSpeechSynthesis;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(&mut self) -> Result<OpenAiSpeechSynthesis, ConcurrencyLimitReached> {
        let max_concurrency = self.adapter.max_concurrency();
        let limit = usize::try_from(max_concurrency).unwrap_or(usize::MAX);
        let slot = Slot(Arc::clone(&self.running));
        if self.running.fetch_add(1, Ordering::AcqRel) >= limit {
            drop(slot);
            return Err(ConcurrencyLimitReached { max_concurrency });
        }
        Ok(OpenAiSpeechSynthesis {
            output: SynthesisOutput::start(&self.adapter),
            shared: Arc::clone(&self.shared),
            transport: None,
            exchange: None,
            live: None,
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

/// The runtime a synthesis drives its requests on and the HTTP client whose connections live on
/// it. The two are made and dropped together: a connection's task runs on the runtime, so a
/// client kept past its runtime would hand out dead connections.
struct Transport {
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
}

impl Transport {
    fn new(endpoint: &SpeechEndpoint) -> Result<Self, SpeechError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("inference-openai-speech")
            .enable_all()
            .build()
            .map_err(|error| SpeechError::Runtime(error.to_string()))?;
        let client = reqwest::Client::builder()
            .connect_timeout(endpoint.connect_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|error| SpeechError::Runtime(error.to_string()))?;
        Ok(Self { runtime, client })
    }
}

/// The segment being spoken and its response: `None` until the response's head arrives, and
/// while a read of its body is in flight, which holds it.
struct Live {
    segment_id: SegmentId,
    response: Option<reqwest::Response>,
    transcoder: Transcoder,
}

/// A request in flight on the synthesis's runtime.
enum Exchange {
    /// The request of the segment being spoken is sent and its response's head awaited.
    Open(tokio::task::JoinHandle<Result<reqwest::Response, SpeechError>>),
    /// One chunk of the response's body is read; the response comes back with it.
    Read(tokio::task::JoinHandle<(reqwest::Response, Result<Option<bytes::Bytes>, SpeechError>)>),
}

/// One synthesis of an [`OpenAiSpeech`]; it runs until it is dropped.
///
/// Iterating it blocks while it waits for the server, each wait bounded by the endpoint's read
/// timeout; its async form, [`SynthesisHandle::poll_event`], returns `Pending` there instead and
/// is woken when the request's step ends. A failure ends the iteration of the segments it took;
/// [`OpenAiSpeechSynthesis::failure`] says why, the usage record then is not final, and the
/// segments offered after it are taken and not spoken.
pub struct OpenAiSpeechSynthesis {
    output: SynthesisOutput,
    shared: Arc<Shared>,
    transport: Option<Transport>,
    /// The request in flight, if there is one.
    exchange: Option<Exchange>,
    live: Option<Live>,
    pending: VecDeque<(SegmentId, String)>,
    delivered: VecDeque<SynthesisEvent>,
    next_sample: u64,
    failure: Option<SpeechError>,
    _slot: Slot,
}

impl fmt::Debug for OpenAiSpeechSynthesis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiSpeechSynthesis")
            .field("format", &self.shared.config.output)
            .field("segments", &self.output.segments().len())
            .field("speaking", &self.live.as_ref().map(|live| live.segment_id))
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl OpenAiSpeechSynthesis {
    /// Why the synthesis failed, once it has.
    pub fn failure(&self) -> Option<&SpeechError> {
        self.failure.as_ref()
    }

    /// The format of every frame it emits.
    pub fn format(&self) -> AudioFormat {
        self.shared.config.output
    }

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

    /// Starts the request that speaks `text`, which ends with its response's head.
    fn open(
        &mut self,
        text: &str,
    ) -> Result<tokio::task::JoinHandle<Result<reqwest::Response, SpeechError>>, SpeechError> {
        if self.transport.is_none() {
            self.transport = Some(Transport::new(&self.shared.endpoint)?);
        }
        let Some(transport) = self.transport.as_ref() else {
            return Err(SpeechError::Runtime("no transport".to_owned()));
        };
        let shared = Arc::clone(&self.shared);
        let body = protocol::request_body(&shared.config, text).to_string();
        let client = transport.client.clone();
        Ok(transport.runtime.spawn(async move {
            let authorization = authorization(shared.source.as_ref(), &shared.credential).await?;
            let request = client
                .post(&shared.endpoint.url)
                .header(AUTHORIZATION, authorization)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send();
            let read_timeout = shared.endpoint.read_timeout;
            let response = within(read_timeout, request)
                .await
                .ok_or(SpeechError::Stalled(read_timeout))?
                .map_err(|error| SpeechError::Connect(describe(error)))?;
            let status = response.status();
            if !status.is_success() {
                return Err(SpeechError::Status(status.as_u16()));
            }
            Ok(response)
        }))
    }

    /// Starts reading one chunk of the response being spoken.
    fn read(&mut self) -> Result<(), SpeechError> {
        let read_timeout = self.shared.endpoint.read_timeout;
        let response = self.live.as_mut().and_then(|live| live.response.take());
        let (Some(mut response), Some(transport)) = (response, self.transport.as_ref()) else {
            return Err(SpeechError::Runtime("no response to read".to_owned()));
        };
        let task = transport.runtime.spawn(async move {
            let chunk = match within(read_timeout, response.chunk()).await {
                None => Err(SpeechError::Stalled(read_timeout)),
                Some(chunk) => chunk.map_err(|error| SpeechError::Transport(describe(error))),
            };
            (response, chunk)
        });
        self.exchange = Some(Exchange::Read(task));
        Ok(())
    }

    /// Waits for the request in flight to end its step and acts on how it ended: blocking
    /// without `cx`, `Pending` with it until the step ends, which wakes `cx`.
    fn settle(&mut self, cx: Option<&mut Context<'_>>) -> Poll<()> {
        let (response, chunk) = match self.exchange.as_mut() {
            None => return Poll::Ready(()),
            Some(Exchange::Open(task)) => match ready!(join(task, cx)).and_then(|opened| opened) {
                Ok(response) => (Some(response), Ok(None)),
                Err(error) => (None, Err(error)),
            },
            Some(Exchange::Read(task)) => match ready!(join(task, cx)) {
                Ok((response, chunk)) => (Some(response), chunk.map(Some)),
                Err(error) => (None, Err(error)),
            },
        };
        self.exchange = None;
        if let Some(live) = self.live.as_mut() {
            live.response = response;
        }
        let outcome = match chunk {
            Ok(Some(chunk)) => self.receive(chunk),
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = outcome {
            self.fail(error);
        }
        Poll::Ready(())
    }

    /// Acts on `chunk`, the next chunk of the response being spoken, `None` at its end.
    fn receive(&mut self, chunk: Option<bytes::Bytes>) -> Result<(), SpeechError> {
        let Some(live) = self.live.as_mut() else {
            return Ok(());
        };
        match chunk {
            Some(bytes) => {
                let (count, data) = live.transcoder.push(&bytes);
                self.emit(count, data)
            }
            None => self.finish_segment(),
        }
    }

    /// Emits `count` samples of `data` as one frame of the segment being spoken at the next
    /// position on the synthesis's sample clock.
    fn emit(&mut self, count: u32, data: Vec<u8>) -> Result<(), SpeechError> {
        let Some(live) = self.live.as_ref() else {
            return Ok(());
        };
        if count == 0 {
            return Ok(());
        }
        let position = SampleClockPosition::new(self.next_sample)
            .ok_or_else(|| SpeechError::Protocol("the sample clock passed i64::MAX".to_owned()))?;
        let frame = AudioFrame {
            format: self.shared.config.output,
            position,
            sample_count: count,
            data,
        };
        let event = self.output.audio(live.segment_id, frame).map_err(refused)?;
        self.next_sample += u64::from(count);
        self.delivered.push_back(event);
        Ok(())
    }

    /// Emits what the filter still holds and completes the segment being spoken at the end of
    /// its response's body.
    fn finish_segment(&mut self) -> Result<(), SpeechError> {
        let Some(live) = self.live.as_mut() else {
            return Ok(());
        };
        let (count, data) = live.transcoder.finish().map_err(|left| {
            SpeechError::Protocol(format!(
                "the response's audio ends in {left} byte(s) of an incomplete sample"
            ))
        })?;
        self.emit(count, data)?;
        if let Some(live) = self.live.take() {
            let events = self
                .output
                .complete(live.segment_id, Vec::new())
                .map_err(refused)?;
            self.delivered.extend(events);
        }
        Ok(())
    }

    /// Drops the response being spoken, if there is one, and with it the request in flight, the
    /// runtime and the client, so its connection is closed at once instead of when the runtime
    /// next runs.
    fn close_live(&mut self) {
        if let Some(live) = self.live.take() {
            drop(live);
            if let Some(exchange) = self.exchange.take() {
                exchange.abort();
            }
            if let Some(transport) = self.transport.take() {
                drop(transport.client);
                transport.runtime.shutdown_background();
            }
        }
    }

    /// Ends the synthesis as failed by `error`: its connection closed, its call marked failed,
    /// and nothing more spoken.
    fn fail(&mut self, error: SpeechError) {
        self.close_live();
        self.pending.clear();
        self.output.mark_failed();
        self.failure = Some(error);
    }
}

/// The `Authorization` header of one request, marked sensitive, from the credential `name`
/// resolves to through `source`.
async fn authorization(
    source: &dyn CredentialSource,
    name: &str,
) -> Result<HeaderValue, SpeechError> {
    let resolved = source
        .resolve(name)
        .await
        .map_err(SpeechError::Credential)?;
    resolved
        .into_header(b"Bearer ")
        .map_err(SpeechError::Credential)
}

/// `future`'s output, or `None` when `bound` passes first. A bound too large for an `Instant` is
/// no bound.
async fn within<F: Future>(bound: std::time::Duration, future: F) -> Option<F::Output> {
    crate::client::within(tokio::time::Instant::now(), bound, future).await
}

/// What a failed request says, without its URL, which names no secret but is the caller's.
fn describe(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut detail = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        detail.push_str(": ");
        detail.push_str(&cause.to_string());
        source = cause.source();
    }
    detail
}

impl Exchange {
    fn abort(self) {
        match self {
            Exchange::Open(task) => task.abort(),
            Exchange::Read(task) => task.abort(),
        }
    }
}

/// What `task` returned: blocking the calling thread without `cx`, `Pending` with it until the
/// task ends, which wakes `cx`.
fn join<T>(
    task: &mut tokio::task::JoinHandle<T>,
    cx: Option<&mut Context<'_>>,
) -> Poll<Result<T, SpeechError>> {
    let joined = match cx {
        None => crate::wake::block_on(&mut *task),
        Some(cx) => ready!(Pin::new(task).poll(cx)),
    };
    Poll::Ready(joined.map_err(|error| {
        SpeechError::Runtime(format!("the task driving the request ended: {error}"))
    }))
}

/// A refusal of the TTS contract, which a well-formed segment of this binding never meets.
fn refused(error: impl fmt::Display) -> SpeechError {
    SpeechError::Protocol(format!("the TTS contract refused the audio: {error}"))
}

impl Iterator for OpenAiSpeechSynthesis {
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

impl OpenAiSpeechSynthesis {
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
            let characters = text.chars().count();
            if characters > MAX_INPUT_CHARACTERS {
                self.fail(SpeechError::InputTooLong {
                    characters,
                    limit: MAX_INPUT_CHARACTERS,
                });
                continue;
            }
            if text.trim().is_empty() {
                match self.output.complete(segment_id, Vec::new()) {
                    Ok(events) => self.delivered.extend(events),
                    Err(error) => self.fail(refused(error)),
                }
                continue;
            }
            let transcoder = match Transcoder::new(self.shared.config.output) {
                Ok(transcoder) => transcoder,
                Err(error) => {
                    self.fail(SpeechError::Protocol(error.to_string()));
                    continue;
                }
            };
            self.live = Some(Live {
                segment_id,
                response: None,
                transcoder,
            });
            match self.open(&text) {
                Ok(task) => self.exchange = Some(Exchange::Open(task)),
                Err(error) => self.fail(error),
            }
        }
    }
}

impl Drop for OpenAiSpeechSynthesis {
    fn drop(&mut self) {
        self.live = None;
        if let Some(exchange) = self.exchange.take() {
            exchange.abort();
        }
        if let Some(transport) = self.transport.take() {
            drop(transport.client);
            transport.runtime.shutdown_background();
        }
    }
}

impl SynthesisHandle for OpenAiSpeechSynthesis {
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

    /// Takes the next event, `Pending` where iterating blocks: while a request's step is in
    /// flight, which wakes `cx` when it ends.
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<SynthesisEvent>> {
        self.advance(Some(cx))
    }

    /// Discards the segment through [`SynthesisOutput::discard`] and drops every event of it not
    /// yet delivered. When it is the segment being spoken, a barge-in, its response is dropped and
    /// its connection closed: the endpoint has no cancel request, so nothing more of it is read.
    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        let event = self.output.discard(segment_id)?;
        self.delivered
            .retain(|event| event.segment_id() != Some(segment_id));
        if self
            .live
            .as_ref()
            .is_some_and(|live| live.segment_id == segment_id)
        {
            self.close_live();
        }
        Ok(event)
    }
}
