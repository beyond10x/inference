//! The scripted mock recogniser of the STT contract of `inference-stt`.
//!
//! [`MockRecogniser`] is an `inference.model.Adapter` of one model, declared in one of two
//! [`MockConfiguration`]s, and replays a fixed script: each [`Cue`] names how many frames must
//! have been taken before its [`ScriptStep`], a hypothesis, a revision or a commit, runs. It
//! yields its enveloped events to the caller through the [`MockRecognition`] handle and records
//! nothing in any store of this crate. It asks the contract for input and output at every step
//! and does not consult its own claims, so how it behaves follows from what its configuration
//! claims:
//!
//! * claiming `incremental_input` and `output_streaming`, it takes each frame as it is offered
//!   and runs every cue whose frame count is reached, so hypotheses reach the caller before the
//!   end of input;
//! * claiming neither, it takes no frame and emits nothing before the end of input, then takes
//!   them all and runs the whole script.
//!
//! At the end of input every cue left runs, whatever its frame count. [`frames`] is a fixture of
//! silent frames and [`script`] a script fitting six of them; [`stream_envelope`] carries an
//! event of the mock into the [`EventLog`](crate::stream::EventLog) of the stream domain.

use std::collections::VecDeque;
use std::num::{NonZeroU16, NonZeroU32};

use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_stt::inference_text::{ReviseSpan, SpanId};
use inference_stt::{
    CONTRACT_REVISION, InputEnded, Recogniser, RecognitionHandle, RecognitionRequest,
    RevisionRefused, TranscriptEvent, TranscriptOutput,
};

use crate::stream::StreamEvent;

/// What a [`MockRecogniser`] claims, and so how it behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MockConfiguration {
    /// Claims `incremental_input` and `output_streaming`.
    Incremental,
    /// Claims neither: batch transcription, answering only after the end of input.
    Batch,
}

impl MockConfiguration {
    /// Every configuration.
    pub const ALL: [Self; 2] = [Self::Incremental, Self::Batch];

    /// The capabilities a recogniser in this configuration claims.
    #[must_use]
    pub fn capabilities(self) -> Vec<Capability> {
        match self {
            Self::Incremental => vec![Capability::IncrementalInput, Capability::OutputStreaming],
            Self::Batch => Vec::new(),
        }
    }
}

/// One move of the script. A span is named by its index among the spans the script emitted, 0
/// first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptStep {
    /// Emits a Hypothesis span carrying the text.
    Hypothesis(String),
    /// Revises the Hypothesis span `span` to read `text`.
    Revise { span: usize, text: String },
    /// Commits the span `span`.
    Commit { span: usize },
}

/// A step of the script and the number of frames that must have been taken before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub after_frames: usize,
    pub step: ScriptStep,
}

/// The script of the fixture: two spans over six frames, each emitted, revised and committed.
#[must_use]
pub fn script() -> Vec<Cue> {
    let cue = |after_frames, step| Cue { after_frames, step };
    vec![
        cue(1, ScriptStep::Hypothesis("hello".to_owned())),
        cue(
            2,
            ScriptStep::Revise {
                span: 0,
                text: "hello there".to_owned(),
            },
        ),
        cue(3, ScriptStep::Commit { span: 0 }),
        cue(4, ScriptStep::Hypothesis("how are".to_owned())),
        cue(
            5,
            ScriptStep::Revise {
                span: 1,
                text: "how are you".to_owned(),
            },
        ),
        cue(6, ScriptStep::Commit { span: 1 }),
    ]
}

/// The samples per channel of each fixture frame: 10 ms at 16 kHz.
pub const FRAME_SAMPLES: u32 = 160;

/// `count` consecutive frames of silence, 16 kHz mono `pcm_s16le`, each [`FRAME_SAMPLES`] long,
/// the first at sample 0.
///
/// # Panics
///
/// When the position of a frame would lie above `i64::MAX`.
#[must_use]
pub fn frames(count: usize) -> Vec<AudioFrame> {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("16000 is not zero"),
        channels: NonZeroU16::new(1).expect("1 is not zero"),
    };
    (0..count as u64)
        .map(|index| AudioFrame {
            format,
            position: SampleClockPosition::new(index * u64::from(FRAME_SAMPLES))
                .expect("the fixture stays within i64::MAX samples"),
            sample_count: FRAME_SAMPLES,
            data: vec![0; FRAME_SAMPLES as usize * 2],
        })
        .collect()
}

/// The envelope of an event of the mock with its payload as a [`StreamEvent`], for the
/// [`EventLog`](crate::stream::EventLog).
#[must_use]
pub fn stream_envelope(envelope: EventEnvelope<TranscriptEvent>) -> EventEnvelope<StreamEvent> {
    let EventEnvelope {
        schema_version,
        event_id,
        run_id,
        session_id,
        stream_id,
        sequence,
        producer,
        port,
        kind,
        generation_id,
        timestamp,
        timebase,
        causation_id,
        payload,
    } = envelope;
    EventEnvelope {
        schema_version,
        event_id,
        run_id,
        session_id,
        stream_id,
        sequence,
        producer,
        port,
        kind,
        generation_id,
        timestamp,
        timebase,
        causation_id,
        payload: payload.into(),
    }
}

impl From<TranscriptEvent> for StreamEvent {
    fn from(event: TranscriptEvent) -> Self {
        match event {
            TranscriptEvent::Hypothesis(event) => Self::TranscriptHypothesis(event),
            TranscriptEvent::Revision(event) => Self::TranscriptRevision(event),
            TranscriptEvent::Committed(event) => Self::TranscriptCommitted(event),
        }
    }
}

/// The scripted mock recogniser: an in-process adapter of one model with a maximum concurrency of
/// one, replaying the same script for every request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockRecogniser {
    adapter: Adapter,
    script: Vec<Cue>,
}

impl MockRecogniser {
    /// Declares a mock recogniser of the model `model_id` in `configuration`, replaying `script`.
    #[must_use]
    pub fn new(configuration: MockConfiguration, model_id: ModelId, script: Vec<Cue>) -> Self {
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id,
            backend: "inference-testkit mock recogniser".to_owned(),
            contract_revision: CONTRACT_REVISION.to_owned(),
            execution_form: ExecutionForm::InProcess,
            locality: Locality::SameProcess,
            capabilities: configuration.capabilities(),
            max_concurrency: 1,
        })
        .expect("a maximum concurrency of one is positive");
        Self { adapter, script }
    }
}

impl Recogniser for MockRecogniser {
    type Handle = MockRecognition;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(&mut self, request: RecognitionRequest) -> MockRecognition {
        MockRecognition {
            output: TranscriptOutput::start(&self.adapter, request),
            pending: self.script.iter().cloned().collect(),
            emitted: Vec::new(),
            delivered: VecDeque::new(),
        }
    }
}

/// One recognition of a [`MockRecogniser`].
#[derive(Debug, PartialEq, Eq)]
pub struct MockRecognition {
    output: TranscriptOutput,
    pending: VecDeque<Cue>,
    emitted: Vec<SpanId>,
    delivered: VecDeque<EventEnvelope<TranscriptEvent>>,
}

impl Iterator for MockRecognition {
    type Item = EventEnvelope<TranscriptEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.delivered.is_empty() {
            self.advance();
        }
        self.delivered.pop_front()
    }
}

impl MockRecognition {
    /// Takes the input the contract gives up and runs every cue that is due, stopping at the
    /// first the contract refuses until more input is taken or input ends.
    ///
    /// # Panics
    ///
    /// When the script revises or commits a span it did not emit, or one the contract refuses to
    /// move: the script is wrong.
    fn advance(&mut self) {
        let _ = self.output.take_input();
        let ended = self.output.input_ended();
        while let Some(cue) = self.pending.front() {
            if !ended && cue.after_frames > self.output.frames_taken() {
                return;
            }
            let envelope = match &cue.step {
                ScriptStep::Hypothesis(text) => match self.output.hypothesis(text.clone()) {
                    Ok(envelope) => {
                        self.emitted.push(envelope.payload.span_id());
                        envelope
                    }
                    Err(_) => return,
                },
                ScriptStep::Revise { span, text } => self
                    .output
                    .revise(ReviseSpan {
                        span_id: self.script_span(*span),
                        text: text.clone(),
                    })
                    .expect("the script revises a Hypothesis it emitted"),
                ScriptStep::Commit { span } => self
                    .output
                    .commit(self.script_span(*span))
                    .expect("the script commits a Hypothesis it emitted"),
            };
            self.pending.pop_front();
            self.delivered.push_back(envelope);
        }
    }

    fn script_span(&self, index: usize) -> SpanId {
        *self
            .emitted
            .get(index)
            .expect("the script names a span it emitted earlier")
    }
}

impl RecognitionHandle for MockRecognition {
    fn output(&self) -> &TranscriptOutput {
        &self.output
    }

    fn offer_frame(&mut self, frame: AudioFrame) -> Result<(), InputEnded> {
        self.output.offer(frame)
    }

    fn end_input(&mut self) -> Result<(), InputEnded> {
        self.output.end_input()
    }

    fn revise(&mut self, input: ReviseSpan) -> Result<(), RevisionRefused> {
        let envelope = self.output.revise(input)?;
        self.delivered.push_back(envelope);
        Ok(())
    }
}
