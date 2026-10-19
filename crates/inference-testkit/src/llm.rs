//! The scripted mock generator of the LLM contract of `inference-llm`.
//!
//! [`MockGenerator`] is an `inference.model.Adapter` of one model, declared in one of three
//! [`MockConfiguration`]s, and generates the text pieces of its script in order. It yields its
//! events to the caller through the [`MockGeneration`] handle and records nothing in any store of
//! this crate. How it behaves follows from what its configuration claims:
//!
//! * claiming `output_streaming`, it emits and commits one segment per piece, each delivered while
//!   the generation is Active, then completes;
//! * claiming neither, it emits the whole script as one segment, commits it and completes before
//!   delivering anything after `GenerationStarted`;
//! * claiming `output_streaming` and `concurrent_input_output`, it streams, and input offered
//!   while the generation is Active is emitted as a later segment of its own.

use std::collections::VecDeque;

use inference_core::generation::{
    GenerationCancelled, GenerationState, GenerationStateConflict, GenerationSuperseded,
};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_llm::{
    AdmittedInput, CONTRACT_REVISION, GenerationEvent, GenerationHandle, GenerationOutput,
    GenerationRequest, Generator, InputRefused,
};

/// What a [`MockGenerator`] claims, and so how it behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MockConfiguration {
    /// Claims `output_streaming` only.
    StreamingOnly,
    /// Claims neither `output_streaming` nor `concurrent_input_output`.
    Neither,
    /// Claims `output_streaming` and `concurrent_input_output`.
    StreamingAndConcurrentInput,
}

impl MockConfiguration {
    /// Every configuration.
    pub const ALL: [Self; 3] = [
        Self::StreamingOnly,
        Self::Neither,
        Self::StreamingAndConcurrentInput,
    ];

    /// The capabilities a generator in this configuration claims.
    #[must_use]
    pub fn capabilities(self) -> Vec<Capability> {
        match self {
            Self::StreamingOnly => vec![Capability::OutputStreaming],
            Self::Neither => Vec::new(),
            Self::StreamingAndConcurrentInput => vec![
                Capability::OutputStreaming,
                Capability::ConcurrentInputOutput,
            ],
        }
    }
}

/// The scripted mock generator: an in-process adapter of one model with a maximum concurrency of
/// one, generating the same script for every request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockGenerator {
    adapter: Adapter,
    script: Vec<String>,
}

impl MockGenerator {
    /// Declares a mock generator of the model `model_id` in `configuration`, generating `script`
    /// piece by piece.
    #[must_use]
    pub fn new<I, S>(configuration: MockConfiguration, model_id: ModelId, script: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id,
            backend: "inference-testkit mock generator".to_owned(),
            contract_revision: CONTRACT_REVISION.to_owned(),
            execution_form: ExecutionForm::InProcess,
            locality: Locality::SameProcess,
            capabilities: configuration.capabilities(),
            max_concurrency: 1,
        })
        .expect("a maximum concurrency of one is positive");
        Self {
            adapter,
            script: script.into_iter().map(Into::into).collect(),
        }
    }
}

impl Generator for MockGenerator {
    type Handle = MockGeneration;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(&mut self, request: GenerationRequest) -> MockGeneration {
        let (output, started) = GenerationOutput::start(&self.adapter, &request);
        MockGeneration {
            streams: output.capabilities().contains(&Capability::OutputStreaming),
            output,
            pending: self.script.iter().cloned().collect(),
            delivered: VecDeque::from([GenerationEvent::Started(started)]),
        }
    }
}

/// One generation of a [`MockGenerator`]. It is not `Clone`, as the `GenerationOutput` it holds
/// is not: a copy taken while the generation is Active would deliver output for it after it was
/// superseded or cancelled.
///
/// ```compile_fail
/// fn cloneable<T: Clone>() {}
/// cloneable::<inference_testkit::llm::MockGeneration>();
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct MockGeneration {
    streams: bool,
    output: GenerationOutput,
    pending: VecDeque<String>,
    delivered: VecDeque<GenerationEvent>,
}

impl Iterator for MockGeneration {
    type Item = GenerationEvent;

    fn next(&mut self) -> Option<GenerationEvent> {
        if let Some(event) = self.delivered.pop_front() {
            return Some(event);
        }
        if self.output.generation().state() != GenerationState::Active {
            return None;
        }
        if self.streams {
            match self.pending.pop_front() {
                Some(piece) => self.deliver_segment(piece),
                None => self.deliver_completion(),
            }
        } else {
            let whole = self.pending.drain(..).collect();
            self.deliver_segment(whole);
            self.deliver_completion();
        }
        self.delivered.pop_front()
    }
}

impl MockGeneration {
    /// Emits `text` as one segment and commits it, queueing both events for delivery.
    fn deliver_segment(&mut self, text: String) {
        let delta = self
            .output
            .emit(text)
            .expect("the generation was checked Active");
        self.delivered.push_back(GenerationEvent::TextDelta(delta));
        let committed = self
            .output
            .commit_tentative()
            .expect("only Tentative segments are committed");
        self.delivered
            .extend(committed.into_iter().map(GenerationEvent::SegmentCommitted));
    }

    /// Completes the generation, queueing its event for delivery.
    fn deliver_completion(&mut self) {
        let completed = self
            .output
            .complete()
            .expect("the generation was checked Active");
        self.delivered
            .push_back(GenerationEvent::Completed(completed));
    }
}

impl GenerationHandle for MockGeneration {
    fn output(&self) -> &GenerationOutput {
        &self.output
    }

    fn accept_input(&mut self, input: AdmittedInput) -> Result<(), InputRefused> {
        let text = self.output.release(input)?;
        self.pending.push_back(text);
        Ok(())
    }

    fn supersede(&mut self) -> Result<GenerationSuperseded, GenerationStateConflict> {
        let (event, _) = self.output.supersede()?;
        self.delivered.clear();
        Ok(event)
    }

    fn cancel(&mut self) -> Result<GenerationCancelled, GenerationStateConflict> {
        let (event, _) = self.output.cancel()?;
        self.delivered.clear();
        Ok(event)
    }
}
