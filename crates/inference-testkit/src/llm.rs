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
//!
//! It declares a maximum concurrency of one and holds to it: while one generation of a generator,
//! or of a clone of it, runs, a second start is refused with [`ConcurrencyLimitReached`]. A
//! generation runs until its handle is dropped. [`MockGeneration::emit`] emits a delta outside
//! the script, which a generation no longer Active refuses as stale.
//!
//! A generator scripted for tools ([`MockGenerator::proposing`]) also claims `tool_calling` and
//! implements [`ToolCalling`]: each input offered and each outcome integrated is a model request
//! of its own, whose text is one segment, and each of its first `proposals` model requests ends
//! with the same [`ToolProposal`], taken once that request's segment is delivered. Scripted with a
//! round ([`MockGenerator::proposing_round`]), each of those requests ends with every proposal of
//! the round, taken one by one in the round's order. The text of a request that integrates an
//! outcome contains it: the result's JSON, the failure's detail or the rejection's reason.
//!
//! Behind the `run` feature, [`ModelNode`] is the model node of a text-only pipeline the
//! runtime's run controller drives: one generation per run, one committed segment per input, and,
//! given tools, the tool loop of that generation.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use inference_core::generation::{
    GenerationCancelled, GenerationState, GenerationStateConflict, GenerationSuperseded,
};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_llm::tools::{ToolCalling, ToolOutcome, ToolProposal};
use inference_llm::{
    AdmittedInput, CONTRACT_REVISION, ConcurrencyLimitReached, GenerationEvent, GenerationHandle,
    GenerationOutput, GenerationRequest, Generator, InputRefused,
};
use inference_text::TextDelta;
use inference_tools::{Json, ToolId};

/// The mock model node of a text-only pipeline, behind the `run` feature: a [`ModelNode`] answers
/// each input on one generation of a [`MockGenerator`] per run.
#[cfg(feature = "run")]
mod node;

#[cfg(feature = "run")]
pub use node::{Carried, ModelHandle, ModelNode, ModelPayload};

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

/// The generations of one generator that run, shared by its clones, which are the same adapter.
#[derive(Debug, Clone, Default)]
struct Running(Arc<AtomicUsize>);

impl PartialEq for Running {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Running {}

impl Running {
    /// Counts one more running generation, if fewer than `max_concurrency` run.
    fn admit(&self, max_concurrency: i64) -> Result<Slot, ConcurrencyLimitReached> {
        let limit = usize::try_from(max_concurrency).unwrap_or(usize::MAX);
        let slot = Slot(Arc::clone(&self.0));
        if self.0.fetch_add(1, Ordering::AcqRel) < limit {
            Ok(slot)
        } else {
            drop(slot);
            Err(ConcurrencyLimitReached { max_concurrency })
        }
    }
}

/// The place of one running generation among [`Running`], given back when it is dropped.
#[derive(Debug)]
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl PartialEq for Slot {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Slot {}

/// The scripted mock generator: an in-process adapter of one model with a maximum concurrency of
/// one, which it holds to, generating the same script for every request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockGenerator {
    adapter: Adapter,
    script: Vec<String>,
    tools: Option<ToolScript>,
    running: Running,
}

/// What a generator scripted for tools proposes: every proposal of `round`, at the end of each
/// of its first `proposals` model requests.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolScript {
    round: Vec<ToolProposal>,
    proposals: usize,
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
            tools: None,
            running: Running::default(),
        }
    }

    /// The same generator scripted for tools: it claims `tool_calling` besides what it claimed,
    /// and each of the first `proposals` model requests of a generation ends with a proposal to
    /// call `tool_id` with `arguments`. Its adapter is declared again with the added claim, under
    /// a new identity.
    #[must_use]
    pub fn proposing(self, tool_id: ToolId, arguments: Json, proposals: usize) -> Self {
        self.proposing_round([ToolProposal { tool_id, arguments }], proposals)
    }

    /// As [`MockGenerator::proposing`], each of the first `proposals` model requests of a
    /// generation ending with every proposal of `round`, in its order: the calls of one tool
    /// round.
    #[must_use]
    pub fn proposing_round(
        self,
        round: impl IntoIterator<Item = ToolProposal>,
        proposals: usize,
    ) -> Self {
        let mut capabilities = self.adapter.capabilities().to_vec();
        if !capabilities.contains(&Capability::ToolCalling) {
            capabilities.push(Capability::ToolCalling);
        }
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id: self.adapter.model_id(),
            backend: self.adapter.backend().to_owned(),
            contract_revision: self.adapter.contract_revision().to_owned(),
            execution_form: self.adapter.execution_form(),
            locality: self.adapter.locality(),
            capabilities,
            max_concurrency: self.adapter.max_concurrency(),
        })
        .expect("the declaration it repeats was admitted");
        Self {
            adapter,
            tools: Some(ToolScript {
                round: round.into_iter().collect(),
                proposals,
            }),
            ..self
        }
    }
}

impl Generator for MockGenerator {
    type Handle = MockGeneration;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(
        &mut self,
        request: GenerationRequest,
    ) -> Result<MockGeneration, ConcurrencyLimitReached> {
        let slot = self.running.admit(self.adapter.max_concurrency())?;
        let (output, started) = GenerationOutput::start(&self.adapter, &request);
        Ok(MockGeneration {
            streams: output.capabilities().contains(&Capability::OutputStreaming),
            output,
            pending: self.script.iter().cloned().collect(),
            delivered: VecDeque::from([GenerationEvent::Started(started)]),
            tools: self.tools.clone(),
            staged: VecDeque::new(),
            _slot: slot,
        })
    }
}

/// One generation of a [`MockGenerator`]; it runs until it is dropped. It is not `Clone`, as the
/// `GenerationOutput` it holds is not: a copy taken while the generation is Active would deliver
/// output for it after it was superseded or cancelled.
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
    /// What is left of the generator's tool script.
    tools: Option<ToolScript>,
    /// The proposals the latest model request ends with, until each is taken.
    staged: VecDeque<ToolProposal>,
    _slot: Slot,
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
    /// Runs `inference.stream.EmitTextDelta` for `text` now, outside the script, and queues the
    /// `TextDelta` for delivery; the segment stays Tentative. Computation may outlive the
    /// cancellation of delivery (`docs/design/inference.md:641`), so a caller may call it on a
    /// generation that is no longer Active, as a model still computing does.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the generation's state when it is not Active, the
    /// `stale-generation` outcome of `EmitTextDelta`: no segment is created and nothing is
    /// delivered.
    pub fn emit(&mut self, text: impl Into<String>) -> Result<TextDelta, GenerationStateConflict> {
        let delta = self.output.emit(text.into())?;
        self.delivered
            .push_back(GenerationEvent::TextDelta(delta.clone()));
        Ok(delta)
    }

    /// Issues one model request whose text is `text`; scripted for tools and with proposals left,
    /// the request ends with the next one.
    fn request(&mut self, text: String) {
        self.pending.push_back(text);
        if let Some(script) = self.tools.as_mut()
            && script.proposals > 0
        {
            script.proposals -= 1;
            self.staged = script.round.iter().cloned().collect();
        }
    }

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
        self.request(text);
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

impl ToolCalling for MockGeneration {
    fn take_proposal(&mut self) -> Option<ToolProposal> {
        if self.pending.is_empty() && self.delivered.is_empty() {
            self.staged.pop_front()
        } else {
            None
        }
    }

    fn integrate(&mut self, outcome: ToolOutcome) -> Result<(), GenerationStateConflict> {
        let current = self.output.generation().state();
        if current != GenerationState::Active {
            return Err(GenerationStateConflict { current });
        }
        let text = match outcome {
            ToolOutcome::Result(result) => result.to_string(),
            ToolOutcome::Failed(failure) => format!("the tool failed: {}", failure.detail),
            ToolOutcome::Rejected(reason) => format!("the tool call was rejected: {reason:?}"),
        };
        self.request(text);
        Ok(())
    }
}
