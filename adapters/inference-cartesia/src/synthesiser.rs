//! The binding of the TTS contract: [`CartesiaSynthesiser`] and its synthesis handle.

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

use crate::config::{Endpoint, VoiceConfig};
use crate::connection::{CLOSE_BOUND, Connection};
use crate::credential::{CARTESIA_KEY_NAME, CredentialSource, EnvironmentSource};
use crate::error::{CartesiaError, ConfigError};
use crate::protocol::{self, ServerMessage};

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
/// Iterating it blocks while it waits for the server, each wait bounded by the endpoint's read
/// timeout. A failure ends the iteration of the segments it took; [`CartesiaSynthesis::failure`]
/// says why, the usage record then is not final, and the segments offered after it are taken and
/// not spoken.
pub struct CartesiaSynthesis {
    output: SynthesisOutput,
    shared: Arc<Shared>,
    runtime: Option<tokio::runtime::Runtime>,
    connection: Option<Connection>,
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

    /// Sends the generation request of one segment on a new context, opening the connection
    /// first when the synthesis has none.
    fn send_request(&mut self, message: String) -> Result<(), CartesiaError> {
        let shared = Arc::clone(&self.shared);
        let connection = self.connection.take();
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
            connection.send(message).await?;
            Ok::<_, CartesiaError>(connection)
        })??;
        self.connection = Some(connection);
        Ok(())
    }

    /// Reads one message of the connection and acts on it.
    fn step(&mut self) -> Result<(), CartesiaError> {
        let Some(connection) = self.connection.as_mut() else {
            return Err(CartesiaError::Closed {
                code: None,
                reason: String::new(),
            });
        };
        let text = run(&mut self.runtime, connection.read())??;
        let message = protocol::parse_server_message(&text)?;
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
        let sent = run(&mut self.runtime, async move {
            let sent = connection.send_within(message, CLOSE_BOUND).await;
            (connection, sent)
        });
        match sent {
            Ok((connection, Ok(()))) => self.connection = Some(connection),
            Ok((connection, Err(_))) => {
                let _ = run(&mut self.runtime, connection.close());
            }
            Err(_) => {}
        }
    }

    /// Closes the connection, if there is one, reading nothing more.
    fn close(&mut self) {
        if let Some(connection) = self.connection.take() {
            let _ = run(&mut self.runtime, connection.close());
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
) -> Result<F::Output, CartesiaError>
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
                .map_err(|error| CartesiaError::Runtime(error.to_string()))?,
        ),
    };
    if tokio::runtime::Handle::try_current().is_err() {
        return Ok(runtime.block_on(future));
    }
    let runtime = &*runtime;
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("inference-cartesia".to_owned())
            .spawn_scoped(scope, move || runtime.block_on(future))
            .map_err(|error| CartesiaError::Runtime(error.to_string()))?
            .join()
            .map_err(|_| {
                CartesiaError::Runtime("the thread driving the connection panicked".to_owned())
            })
    })
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
                return self.output.usage().ok();
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
            match self.send_request(message) {
                Ok(()) => {
                    self.live = Some(Live {
                        segment_id,
                        context_id,
                        carry: Vec::new(),
                    });
                }
                Err(error) => self.fail(error),
            }
        }
    }
}

impl Drop for CartesiaSynthesis {
    fn drop(&mut self) {
        self.close();
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

    /// Discards the segment through [`SynthesisOutput::discard`] and drops every event of it not
    /// yet delivered. When it is the segment being spoken, a barge-in, the protocol's cancel for
    /// its context is sent, waiting at most [`CLOSE_BOUND`](crate::CLOSE_BOUND), and every later
    /// message of that context is dropped; the connection stays open for the next segment.
    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        let event = self.output.discard(segment_id)?;
        self.delivered
            .retain(|event| event.segment_id() != Some(segment_id));
        if self
            .live
            .as_ref()
            .is_some_and(|live| live.segment_id == segment_id)
        {
            self.cancel_live();
        }
        Ok(event)
    }
}
