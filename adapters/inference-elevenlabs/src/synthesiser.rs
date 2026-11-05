//! The binding of the TTS contract: [`ElevenLabsSynthesiser`] and its synthesis handle.

use std::collections::VecDeque;
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
use crate::connection::Connection;
use crate::credential::{CredentialSource, ELEVENLABS_KEY_NAME, EnvironmentSource};
use crate::error::{ConfigError, ElevenLabsError};
use crate::protocol::{self, END_OF_INPUT};

/// The backend name of the adapter record.
pub const BACKEND: &str = "elevenlabs-stream-input";

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
    init: String,
    source: Arc<dyn CredentialSource>,
    credential: String,
}

/// ElevenLabs' single-stream text-to-speech endpoint as a synthesiser of the TTS contract, at
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
                init: protocol::init_message(config.voice_settings.as_ref()),
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

/// The connection of the segment being spoken, and the byte of a sample split across chunks.
struct Live {
    segment_id: SegmentId,
    connection: Connection,
    carry: Vec<u8>,
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
    live: Option<Live>,
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

    /// Opens the connection of one segment and sends its whole input: the opening message, the
    /// text message and the end of input.
    fn open(&mut self, message: String) -> Result<Connection, ElevenLabsError> {
        let shared = Arc::clone(&self.shared);
        run(&mut self.runtime, async move {
            let endpoint = &shared.endpoint;
            let mut connection = Connection::open(
                &shared.url,
                shared.source.as_ref(),
                &shared.credential,
                endpoint.connect_timeout,
                endpoint.read_timeout,
            )
            .await?;
            connection.send(shared.init.clone()).await?;
            connection.send(message).await?;
            connection.send(END_OF_INPUT.to_owned()).await?;
            Ok(connection)
        })?
    }

    /// Reads one message of the segment being spoken and acts on it.
    fn step(&mut self) -> Result<(), ElevenLabsError> {
        let Some(live) = self.live.as_mut() else {
            return Ok(());
        };
        let text = run(&mut self.runtime, live.connection.read())??;
        let message = protocol::parse_server_message(&text)?;
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

    /// Completes the segment being spoken on its final message and closes its connection.
    fn finish_segment(&mut self) -> Result<(), ElevenLabsError> {
        let Some(live) = self.live.as_ref() else {
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
        self.close_live();
        Ok(())
    }

    /// Closes the connection of the segment being spoken, if there is one, reading nothing more.
    fn close_live(&mut self) {
        if let Some(live) = self.live.take() {
            let _ = run(&mut self.runtime, live.connection.close());
        }
    }

    /// Ends the synthesis as failed by `error`: its connection closed, its call marked failed,
    /// and nothing more spoken.
    fn fail(&mut self, error: ElevenLabsError) {
        self.close_live();
        self.pending.clear();
        self.output.mark_failed();
        self.failure = Some(error);
    }
}

/// Runs `future` to its end on the synthesis's own runtime, built on first use. Refused on a
/// thread that runs an async runtime, where blocking would panic.
fn run<F: Future>(
    runtime: &mut Option<tokio::runtime::Runtime>,
    future: F,
) -> Result<F::Output, ElevenLabsError> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(ElevenLabsError::InsideRuntime);
    }
    let runtime = match runtime {
        Some(runtime) => runtime,
        None => runtime.insert(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| ElevenLabsError::Runtime(error.to_string()))?,
        ),
    };
    Ok(runtime.block_on(future))
}

/// A refusal of the TTS contract, which a well-formed segment of this binding never meets.
fn refused(error: impl fmt::Display) -> ElevenLabsError {
    ElevenLabsError::Protocol(format!("the TTS contract refused the audio: {error}"))
}

impl Iterator for ElevenLabsSynthesis {
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
            let Some(message) = protocol::text_message(&text) else {
                match self.output.complete(segment_id, Vec::new()) {
                    Ok(events) => self.delivered.extend(events),
                    Err(error) => self.fail(refused(error)),
                }
                continue;
            };
            match self.open(message) {
                Ok(connection) => {
                    self.live = Some(Live {
                        segment_id,
                        connection,
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
        self.close_live();
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
    /// yet delivered. When it is the segment being spoken, a barge-in, its connection is closed:
    /// the protocol has no cancel message, so the binding sends a close frame, waiting at most
    /// [`CLOSE_BOUND`](crate::CLOSE_BOUND), and reads nothing more of it.
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
