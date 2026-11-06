//! The binding of the TTS contract: [`ElevenLabsSynthesiser`] and its synthesis handle.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use inference_tts::inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_tts::inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued, SegmentState,
};
use inference_tts::{
    CONTRACT_REVISION, ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt,
    PlaybackRefused, SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};

use crate::config::{Endpoint, VoiceConfig, VoiceSettings};
use crate::connection::Connection;
use crate::credential::{CredentialSource, ELEVENLABS_KEY_NAME, EnvironmentSource};
use crate::error::{ConfigError, ElevenLabsError};
use crate::protocol::{self, CLOSE_SOCKET};

/// The backend name of the adapter record.
pub const BACKEND: &str = "elevenlabs-multi-stream-input";

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
    format: AudioFormat,
    settings: Option<VoiceSettings>,
    source: Arc<dyn CredentialSource>,
    credential: String,
}

/// ElevenLabs' multi-stream text-to-speech endpoint as a synthesiser of the TTS contract, at
/// locality `external_provider`. See the crate documentation.
pub struct ElevenLabsSynthesiser {
    adapter: Adapter,
    config: VoiceConfig,
    shared: Arc<Shared>,
    running: Arc<AtomicUsize>,
}

impl fmt::Debug for ElevenLabsSynthesiser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ElevenLabsSynthesiser")
            .field("adapter", &self.adapter.adapter_id())
            .field("config", &self.config)
            .field("endpoint", &self.shared.endpoint)
            .field("credential", &self.shared.credential)
            .finish_non_exhaustive()
    }
}

impl ElevenLabsSynthesiser {
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
        let url = protocol::request_url(&endpoint, &config)?;
        let adapter = declare(1)?;
        Ok(Self {
            adapter,
            shared: Arc::new(Shared {
                url,
                endpoint,
                format: config.output_format.audio_format(),
                settings: config.voice_settings,
                source,
                credential: credential.into(),
            }),
            config,
            running: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// [`ElevenLabsSynthesiser::new`] with the key from the environment variable
    /// `ELEVENLABS_API_KEY`, read at every connection.
    ///
    /// # Errors
    /// As [`ElevenLabsSynthesiser::new`].
    pub fn from_env(endpoint: Endpoint, config: VoiceConfig) -> Result<Self, ConfigError> {
        Self::new(
            endpoint,
            config,
            Arc::new(EnvironmentSource::elevenlabs()),
            ELEVENLABS_KEY_NAME,
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
        &self.config
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
    })
    .map(|(adapter, _)| adapter)
    .map_err(|_| ConfigError::MaxConcurrency(max_concurrency))
}

impl Synthesiser for ElevenLabsSynthesiser {
    type Handle = ElevenLabsSynthesis;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(&mut self) -> Result<ElevenLabsSynthesis, ConcurrencyLimitReached> {
        let max_concurrency = self.adapter.max_concurrency();
        let limit = usize::try_from(max_concurrency).unwrap_or(usize::MAX);
        let slot = Slot(Arc::clone(&self.running));
        if self.running.fetch_add(1, Ordering::AcqRel) >= limit {
            drop(slot);
            return Err(ConcurrencyLimitReached { max_concurrency });
        }
        Ok(ElevenLabsSynthesis {
            output: SynthesisOutput::start(&self.adapter),
            shared: Arc::clone(&self.shared),
            runtime: None,
            connection: None,
            live: None,
            cancelled: HashSet::new(),
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

/// The segment being spoken: its context on the connection and the text message sent to it, the
/// instant its read timeout ends, moved on each message of the context, whether a message of the
/// context has arrived, whether its connection was already reopened once for it, and the byte of
/// a sample split across chunks.
struct Live {
    segment_id: SegmentId,
    context_id: String,
    message: String,
    deadline: Option<tokio::time::Instant>,
    heard: bool,
    reopened: bool,
    carry: Vec<u8>,
}

/// Whether `error` says the connection ended, by a close or a transport failure, as an idle
/// timeout of the endpoint would end it; the one kind of failure a segment reopens the
/// connection for.
fn ended(error: &ElevenLabsError) -> bool {
    matches!(
        error,
        ElevenLabsError::Closed { .. } | ElevenLabsError::Transport(_)
    )
}

/// One synthesis of an [`ElevenLabsSynthesiser`]; it runs until it is dropped.
///
/// Iterating it blocks while it waits for the server, each wait bounded by the endpoint's read
/// timeout. A failure ends the iteration of the segments it took; [`ElevenLabsSynthesis::failure`]
/// says why, the usage record then is not final, and the segments offered after it are taken and
/// not spoken.
pub struct ElevenLabsSynthesis {
    output: SynthesisOutput,
    shared: Arc<Shared>,
    runtime: Option<tokio::runtime::Runtime>,
    connection: Option<Connection>,
    live: Option<Live>,
    cancelled: HashSet<String>,
    contexts: u64,
    pending: VecDeque<(SegmentId, String)>,
    delivered: VecDeque<SynthesisEvent>,
    next_sample: u64,
    failure: Option<ElevenLabsError>,
    _slot: Slot,
}

impl fmt::Debug for ElevenLabsSynthesis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ElevenLabsSynthesis")
            .field("format", &self.shared.format)
            .field("segments", &self.output.segments().len())
            .field("connected", &self.connection.is_some())
            .field("speaking", &self.live.as_ref().map(|live| live.segment_id))
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl ElevenLabsSynthesis {
    /// Why the synthesis failed, once it has.
    pub fn failure(&self) -> Option<&ElevenLabsError> {
        self.failure.as_ref()
    }

    /// The format of every frame it emits.
    pub fn format(&self) -> AudioFormat {
        self.shared.format
    }
}

impl ElevenLabsSynthesis {
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

    /// Speaks `message`, the text message of the context `context_id`, as [`Self::send_context`]
    /// does. When the connection turns out to have ended, as the endpoint's idle timeout ends it
    /// between segments, it reopens the connection once and sends the context again. Returns the
    /// instant the read timeout of the context's first message ends, and whether it reopened.
    fn speak(
        &mut self,
        context_id: &str,
        message: &str,
    ) -> Result<(Option<tokio::time::Instant>, bool), ElevenLabsError> {
        match self.send_context(context_id, message) {
            Ok(deadline) => Ok((deadline, false)),
            Err(error) if ended(&error) => {
                self.cancelled.clear();
                Ok((self.send_context(context_id, message)?, true))
            }
            Err(error) => Err(error),
        }
    }

    /// Reopens the connection for the segment being spoken, whose connection ended before any
    /// message of its context arrived, and sends its context again on the new one.
    fn reopen(&mut self) -> Result<(), ElevenLabsError> {
        let Some((context_id, message)) = self
            .live
            .as_ref()
            .map(|live| (live.context_id.clone(), live.message.clone()))
        else {
            return Ok(());
        };
        self.connection = None;
        self.cancelled.clear();
        let deadline = self.send_context(&context_id, &message)?;
        if let Some(live) = self.live.as_mut() {
            live.deadline = deadline;
            live.reopened = true;
        }
        Ok(())
    }

    /// Sends the context `context_id` with its text message `message` on the synthesis's
    /// connection, opening it first when there is none: the context's opening message, the text
    /// and the close of the context. Returns the instant the read timeout of its first message
    /// ends. On a failure the connection is dropped.
    fn send_context(
        &mut self,
        context_id: &str,
        message: &str,
    ) -> Result<Option<tokio::time::Instant>, ElevenLabsError> {
        let shared = Arc::clone(&self.shared);
        let connection = self.connection.take();
        let opening = protocol::context_message(context_id, shared.settings.as_ref());
        let message = message.to_owned();
        let close = protocol::close_context_message(context_id);
        let connection = run(&mut self.runtime, async move {
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
            connection.send(opening).await?;
            connection.send(message).await?;
            connection.send(close).await?;
            Ok::<_, ElevenLabsError>(connection)
        })??;
        let deadline = connection.deadline();
        self.connection = Some(connection);
        Ok(deadline)
    }

    /// Reads one message of the connection and acts on it: audio and the final message of the
    /// segment being spoken, or a message of a cancelled context, which is dropped. A connection
    /// that ends before any message of the context being spoken arrived is reopened once for it.
    fn step(&mut self) -> Result<(), ElevenLabsError> {
        let (Some(live), Some(connection)) = (self.live.as_ref(), self.connection.as_mut()) else {
            return Ok(());
        };
        let may_reopen = !live.heard && !live.reopened;
        let text = match run(&mut self.runtime, connection.read_until(live.deadline))? {
            Ok(text) => text,
            Err(error) if may_reopen && ended(&error) => return self.reopen(),
            Err(error) => return Err(error),
        };
        let (Some(live), Some(connection)) = (self.live.as_ref(), self.connection.as_mut()) else {
            return Ok(());
        };
        let message = protocol::parse_server_message(&text)?;
        let Some(context_id) = message.context_id else {
            return Err(ElevenLabsError::Protocol(
                "a message without `contextId`".to_owned(),
            ));
        };
        if context_id != live.context_id {
            if !self.cancelled.contains(&context_id) {
                return Err(ElevenLabsError::Protocol(
                    "a message of a context the binding is not speaking".to_owned(),
                ));
            }
            if message.is_final {
                self.cancelled.remove(&context_id);
            }
            return Ok(());
        }
        let deadline = connection.deadline();
        if let Some(live) = self.live.as_mut() {
            live.deadline = deadline;
            live.heard = true;
        }
        if let Some(audio) = message.audio {
            self.emit(audio)?;
        }
        if message.is_final {
            self.finish_segment()?;
        }
        Ok(())
    }

    /// Emits the whole samples of `audio`, after the bytes kept from the chunk before, as one
    /// frame of the segment being spoken at the next position on the synthesis's sample clock.
    /// The bytes of a sample split across chunks are kept for the next.
    fn emit(&mut self, audio: Vec<u8>) -> Result<(), ElevenLabsError> {
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
            .map_err(|_| ElevenLabsError::Protocol(format!("an audio chunk of {whole} bytes")))?;
        let position = SampleClockPosition::new(self.next_sample).ok_or_else(|| {
            ElevenLabsError::Protocol("the sample clock passed i64::MAX".to_owned())
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

    /// Completes the segment being spoken on the final message of its context. The connection
    /// stays open for the next segment.
    fn finish_segment(&mut self) -> Result<(), ElevenLabsError> {
        let Some(live) = self.live.take() else {
            return Ok(());
        };
        if !live.carry.is_empty() {
            return Err(ElevenLabsError::Protocol(format!(
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

    /// Closes the connection, if there is one: `close_socket`, then a close frame, waiting at
    /// most [`CLOSE_BOUND`](crate::CLOSE_BOUND), reading nothing more.
    fn close_connection(&mut self) {
        self.live = None;
        if let Some(connection) = self.connection.take() {
            let _ = run(&mut self.runtime, connection.close(Some(CLOSE_SOCKET)));
        }
    }

    /// Ends the synthesis as failed by `error`: its connection closed, its call marked failed,
    /// and nothing more spoken.
    fn fail(&mut self, error: ElevenLabsError) {
        self.close_connection();
        self.pending.clear();
        self.output.mark_failed();
        self.failure = Some(error);
    }
}

/// Runs `future` to its end on the synthesis's own runtime, built on first use, and blocks the
/// calling thread until it has.
///
/// On a thread where a runtime handle is current, blocking on another runtime panics on an
/// async worker and not on a `spawn_blocking` thread, and tokio gives no public way to tell the
/// two apart. So on any such thread the synthesis's runtime is driven on a scoped thread of its
/// own while the caller waits, and no thread is refused. The caller's thread is still blocked:
/// on an async worker that holds up the worker's other tasks.
fn run<F>(
    runtime: &mut Option<tokio::runtime::Runtime>,
    future: F,
) -> Result<F::Output, ElevenLabsError>
where
    F: Future + Send,
    F::Output: Send,
{
    let runtime = match runtime {
        Some(runtime) => runtime,
        None => runtime.insert(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| ElevenLabsError::Runtime(error.to_string()))?,
        ),
    };
    if tokio::runtime::Handle::try_current().is_err() {
        return Ok(runtime.block_on(future));
    }
    let runtime = &*runtime;
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("inference-elevenlabs".to_owned())
            .spawn_scoped(scope, move || runtime.block_on(future))
            .map_err(|error| ElevenLabsError::Runtime(error.to_string()))?
            .join()
            .map_err(|_| {
                ElevenLabsError::Runtime("the thread driving the connection panicked".to_owned())
            })
    })
}

/// A refusal of the TTS contract, which a well-formed segment of this binding never meets.
fn refused(error: impl fmt::Display) -> ElevenLabsError {
    ElevenLabsError::Protocol(format!("the TTS contract refused the audio: {error}"))
}

impl Iterator for ElevenLabsSynthesis {
    type Item = SynthesisEvent;

    /// The next event. It blocks while a segment is being spoken, each read bounded by the read
    /// timeout; `None` once nothing more is delivered until more is offered or the utterance
    /// ends. Once the utterance has ended and every segment is spoken, the connection is closed
    /// with `close_socket` before the usage record is returned.
    fn next(&mut self) -> Option<SynthesisEvent> {
        loop {
            if let Some(event) = self.delivered.pop_front() {
                return Some(event);
            }
            if self.failure.is_some() {
                self.take_input();
                return self.output.usage().ok();
            }
            if self.live.is_some() {
                if let Err(error) = self.step() {
                    self.fail(error);
                }
                continue;
            }
            self.take_input();
            let Some((segment_id, text)) = self.pending.pop_front() else {
                if self.output.utterance_ended() {
                    self.close_connection();
                }
                return self.output.usage().ok();
            };
            if self.state(segment_id) != Some(SegmentState::Queued) {
                continue;
            }
            let context_id = format!("s{}", self.contexts);
            let Some(message) = protocol::text_message(&context_id, &text) else {
                match self.output.complete(segment_id, Vec::new()) {
                    Ok(events) => self.delivered.extend(events),
                    Err(error) => self.fail(refused(error)),
                }
                continue;
            };
            self.contexts += 1;
            match self.speak(&context_id, &message) {
                Ok((deadline, reopened)) => {
                    self.live = Some(Live {
                        segment_id,
                        context_id,
                        message,
                        deadline,
                        heard: false,
                        reopened,
                        carry: Vec::new(),
                    });
                }
                Err(error) => self.fail(error),
            }
        }
    }
}

impl Drop for ElevenLabsSynthesis {
    fn drop(&mut self) {
        self.close_connection();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl SynthesisHandle for ElevenLabsSynthesis {
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

    /// Discards the segment through [`SynthesisOutput::discard`] and drops every event of it not
    /// yet delivered. When it is the segment being spoken, a barge-in, its context ends for the
    /// binding: nothing is sent, since its `close_context` went with its text, every later
    /// message of the context is dropped unread, and the connection stays open for the next
    /// segment. The discard does no I/O and does not wait for the server; it stops nothing at
    /// the server, which still generates and sends the whole segment.
    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        let event = self.output.discard(segment_id)?;
        self.delivered
            .retain(|event| event.segment_id() != Some(segment_id));
        if self
            .live
            .as_ref()
            .is_some_and(|live| live.segment_id == segment_id)
            && let Some(live) = self.live.take()
        {
            self.cancelled.insert(live.context_id);
        }
        Ok(event)
    }
}
