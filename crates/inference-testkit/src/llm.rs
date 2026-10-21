//! The scripted mock model of the LLM contract of `inference-llm`.
//!
//! [`MockGenerator`] is an `inference.model.Adapter` of one model, declared in one of three
//! [`MockConfiguration`]s, implemented by a scripted [`llm_core::Model`] and wrapped by
//! `inference-llm`'s [`Generator`]. Its generations are [`MockGeneration`]s, the wrapper's
//! handles; it records nothing in any store of this crate. With a script, the model answers the
//! first turn of a conversation with it, one delta per piece; every other turn, and the first of
//! a model without a script, it answers with the text of each caller item after its last answer,
//! one delta each: the user text of an input, or the text of a tool result. A model that does not
//! stream shows no delta and returns that text in the turn's outcome only, as `llm` defines one.
//! Each turn answered so is a model request. A turn reports no usage unless the generator was
//! scripted to ([`MockGenerator::reporting`]), so the wrapper's record of it counts every token
//! unknown. Scripted to hold a turn ([`MockGenerator::holding`]), the model streams that turn's
//! text and then waits for its cancel, failing with the turn's usage as counts not final, as a
//! call cut short in the middle of its output does. The model declares to `llm` that it streams
//! exactly when its configuration claims `output_streaming` ([`MockConfiguration::declared`]), so
//! the wrapper accepts the claims. What a generation
//! delivers follows from what its configuration claims, as the wrapper decides it:
//!
//! * claiming `output_streaming`, each piece is one segment, emitted and committed while the
//!   generation is Active, and the generation completes once nothing is left to answer;
//! * claiming neither, the whole script is one segment, committed, and the generation completes
//!   before anything after `GenerationStarted` is delivered;
//! * claiming `output_streaming` and `concurrent_input_output`, it streams, and input offered
//!   while the generation is Active is answered by a later turn, one segment of its own.
//!
//! It declares a maximum concurrency of one, which the wrapper holds to: while one generation of a
//! generator, or of a clone of it, runs, a second start is refused with
//! [`ConcurrencyLimitReached`]. A generation runs until its handle is dropped.
//!
//! A generator scripted for tools ([`MockGenerator::proposing`]) also claims `tool_calling`, and
//! its model declares tools: each of its first `proposals` model requests ends with the
//! same [`ToolProposal`], an `llm` tool call of the tool named as
//! [`tool_name`](inference_llm::tools::tool_name) names it. Scripted with a round
//! ([`MockGenerator::proposing_round`]), each of those turns ends with every proposal of the
//! round, in the round's order. The text of a turn answering tool results contains each: the
//! result's JSON, the failure's detail or the rejection's reason.
//!
//! A caller offers its tool set on the request, `llm`'s tool specifications; the model records the
//! set the first turn of each conversation carries, when it carries one
//! ([`MockGenerator::offered`]).
//!
//! Behind the `run` feature, [`ModelNode`] is the model node of a text-only pipeline the
//! runtime's run controller drives: one generation per run, one committed segment per input,
//! given tools, the tool loop of that generation and, given a turns input, a generation started at
//! the completion of the first turn, and, given a usage port, the usage record of each of its
//! turns on that port. It answers on any `inference-llm` [`Generator`], the mock's
//! among them.

use std::sync::{Arc, Mutex, PoisonError};

use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_llm::llm_core::{
    self, BoxFuture, CallId, Cancel, Capabilities, Dispatch, Id, Item, Protocol, Provenance,
    StopReason, StreamEvent, StreamSink, ToolCall, ToolSpec, TurnObservation, TurnOutcome,
    TurnRequest, Usage,
};
use inference_llm::tools::{ToolOutcome, tool_name};
use inference_llm::{
    CONTRACT_REVISION, ConcurrencyLimitReached, GenerationHandle, GenerationRequest, Generator,
};
use inference_tools::{Json, RejectionReason, ToolId};

/// The mock model node of a text-only pipeline, behind the `run` feature: a [`ModelNode`] answers
/// each input on one generation of a [`Generator`] per run.
#[cfg(feature = "run")]
mod node;

#[cfg(feature = "run")]
pub use node::{Carried, ModelHandle, ModelNode, ModelPayload};

/// What a [`MockGenerator`] claims, and so how its generations deliver.
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

    /// The `llm` declaration of the mock model of a generator in this configuration: `llm`'s
    /// text model, which takes no input during output, streaming its output only when the
    /// configuration claims `output_streaming`. `concurrent_input_output` needs no declaration:
    /// the wrapper takes the input as the next turn's text.
    #[must_use]
    pub fn declared(self) -> Capabilities {
        Capabilities::text(1 << 20, 1 << 16)
            .with_output_streaming(self.capabilities().contains(&Capability::OutputStreaming))
    }
}

/// A proposal of the mock's tool script: the tool and the arguments each scripted turn proposes
/// to call it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolProposal {
    pub tool_id: ToolId,
    pub arguments: Json,
}

/// One generation of a [`MockGenerator`]: `inference-llm`'s handle over the mock model; it runs
/// until it is dropped. It is not `Clone`: a copy taken while the generation is Active would
/// deliver output for it after it was superseded or cancelled.
///
/// ```compile_fail
/// fn cloneable<T: Clone>() {}
/// cloneable::<inference_testkit::llm::MockGeneration>();
/// ```
pub type MockGeneration = GenerationHandle;

/// The scripted mock generator: an in-process adapter of one model with a maximum concurrency of
/// one, whose model answers every conversation with the same script.
#[derive(Debug, Clone)]
pub struct MockGenerator {
    generator: Generator,
    model: Arc<MockModel>,
}

/// What a generator scripted for tools proposes: every proposal of `round`, at the end of each
/// of its first `proposals` model requests.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolScript {
    round: Vec<ToolProposal>,
    proposals: usize,
}

/// The scripted [`llm_core::Model`] behind a [`MockGenerator`].
#[derive(Debug)]
struct MockModel {
    provenance: Provenance,
    capabilities: Capabilities,
    script: Vec<String>,
    tools: Option<ToolScript>,
    offered: Offered,
    /// The usage the n-th turn of a conversation reports; a turn past the list reports none.
    usage: Vec<Option<Usage>>,
    /// The turn of a conversation, counted from 0, that holds until it is cancelled.
    holding: Option<usize>,
}

/// The tool sets offered to the model of one generator, shared by its clones.
#[derive(Debug, Clone, Default)]
struct Offered(Arc<Mutex<Vec<Vec<ToolSpec>>>>);

impl Offered {
    fn record(&self, tools: Vec<ToolSpec>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tools);
    }

    fn read(&self) -> Vec<Vec<ToolSpec>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl MockGenerator {
    /// Declares a mock generator of the model `model_id` in `configuration`, whose model answers
    /// the first turn of a conversation with `script`, piece by piece.
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
        let id = |value: &str| Id::new(value).expect("a printable identifier");
        let model = MockModel {
            provenance: Provenance {
                protocol: Protocol::Responses,
                provider: id("inference-testkit"),
                account: id("mock"),
                endpoint: id("in-process"),
                model: id("mock"),
                binding_revision: id("1"),
            },
            capabilities: configuration.declared(),
            script: script.into_iter().map(Into::into).collect(),
            tools: None,
            offered: Offered::default(),
            usage: Vec::new(),
            holding: None,
        };
        Self::over(adapter, model)
    }

    /// The same generator scripted for tools: it claims `tool_calling` besides what it claimed,
    /// and each of the first `proposals` model requests of a conversation ends with a
    /// proposal to call `tool_id` with `arguments`. Its adapter is declared again with the added
    /// claim, under a new identity.
    #[must_use]
    pub fn proposing(self, tool_id: ToolId, arguments: Json, proposals: usize) -> Self {
        self.proposing_round([ToolProposal { tool_id, arguments }], proposals)
    }

    /// As [`MockGenerator::proposing`], each of the first `proposals` model requests
    /// ending with every proposal of `round`, in its order: the calls of one tool round.
    #[must_use]
    pub fn proposing_round(
        self,
        round: impl IntoIterator<Item = ToolProposal>,
        proposals: usize,
    ) -> Self {
        let adapter = self.generator.adapter();
        let mut capabilities = adapter.capabilities().to_vec();
        if !capabilities.contains(&Capability::ToolCalling) {
            capabilities.push(Capability::ToolCalling);
        }
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id: adapter.model_id(),
            backend: adapter.backend().to_owned(),
            contract_revision: adapter.contract_revision().to_owned(),
            execution_form: adapter.execution_form(),
            locality: adapter.locality(),
            capabilities,
            max_concurrency: adapter.max_concurrency(),
        })
        .expect("the declaration it repeats was admitted");
        let model = MockModel {
            provenance: self.model.provenance.clone(),
            capabilities: Capabilities {
                tools: true,
                tool_choice: true,
                ..self.model.capabilities.clone()
            },
            script: self.model.script.clone(),
            tools: Some(ToolScript {
                round: round.into_iter().collect(),
                proposals,
            }),
            offered: self.model.offered.clone(),
            usage: self.model.usage.clone(),
            holding: self.model.holding,
        };
        Self::over(adapter, model)
    }

    /// The same generator, whose model reports `usage` for its turns: the n-th turn of a
    /// conversation reports the n-th entry, `None` reporting no usage, and a turn past the list
    /// reports none, as every turn of a generator does by default. Its adapter is the same.
    #[must_use]
    pub fn reporting(self, usage: impl IntoIterator<Item = Option<Usage>>) -> Self {
        let model = MockModel {
            provenance: self.model.provenance.clone(),
            capabilities: self.model.capabilities.clone(),
            script: self.model.script.clone(),
            tools: self.model.tools.clone(),
            offered: self.model.offered.clone(),
            usage: usage.into_iter().collect(),
            holding: self.model.holding,
        };
        Self::over(self.generator.adapter().clone(), model)
    }

    /// The same generator, whose model holds the `turn`-th turn of a conversation, counted from
    /// 0 as [`MockGenerator::reporting`] counts: it streams that turn's pieces, if it streams,
    /// and then neither ends nor fails until the turn is cancelled, as a call cut short in the
    /// middle of its output. Cancelled, it fails with `llm`'s cancelled error, dispatch
    /// accepted, carrying the usage scripted for that turn as counts not final, as `llm`'s
    /// clients keep the partial usage of an interrupted stream; no usage scripted, it carries
    /// none. Its adapter is the same.
    #[must_use]
    pub fn holding(self, turn: usize) -> Self {
        let model = MockModel {
            provenance: self.model.provenance.clone(),
            capabilities: self.model.capabilities.clone(),
            script: self.model.script.clone(),
            tools: self.model.tools.clone(),
            offered: self.model.offered.clone(),
            usage: self.model.usage.clone(),
            holding: Some(turn),
        };
        Self::over(self.generator.adapter().clone(), model)
    }

    fn over(adapter: Adapter, model: MockModel) -> Self {
        let model = Arc::new(model);
        let generator = Generator::new(adapter, Arc::clone(&model) as Arc<dyn llm_core::Model>)
            .expect("the mock model declares what its configuration claims");
        Self { generator, model }
    }

    /// The adapter record this generator was declared with, the capabilities it claims among it.
    #[must_use]
    pub const fn adapter(&self) -> &Adapter {
        self.generator.adapter()
    }

    /// The `inference-llm` generator over the mock model, which a [`ModelNode`] answers on.
    #[must_use]
    pub const fn generator(&self) -> &Generator {
        &self.generator
    }

    /// Starts a generation for `request`, as [`Generator::start`] does.
    ///
    /// # Errors
    ///
    /// The [`ConcurrencyLimitReached`] of [`Generator::start`]; nothing starts.
    pub fn start(
        &mut self,
        request: GenerationRequest,
    ) -> Result<MockGeneration, ConcurrencyLimitReached> {
        self.generator.start(request)
    }

    /// Every tool set the first turn of a conversation of the generator's model, or of a clone's,
    /// carried, in the order those turns ran, each in the order it was offered; a first turn
    /// carrying no tools records none.
    #[must_use]
    pub fn offered(&self) -> Vec<Vec<ToolSpec>> {
        self.model.offered.read()
    }
}

impl From<MockGenerator> for Generator {
    fn from(mock: MockGenerator) -> Self {
        mock.generator
    }
}

impl MockModel {
    /// What the model answers `request` with: the pieces it streams, and the calls the turn ends
    /// with.
    fn answer(&self, request: &TurnRequest) -> (Vec<String>, Vec<ToolCall>) {
        let answered = request
            .items
            .iter()
            .filter(|item| matches!(item, Item::AssistantText { .. }))
            .count();
        let last = request
            .items
            .iter()
            .rposition(|item| matches!(item, Item::AssistantText { .. } | Item::ToolCall(_)));
        if last.is_none() && !request.tools.is_empty() {
            self.offered.record(request.tools.clone());
        }
        let scripted = last.is_none() && !self.script.is_empty();
        let pieces = if scripted {
            self.script.clone()
        } else {
            request.items[last.map_or(0, |at| at + 1)..]
                .iter()
                .filter_map(caller_text)
                .collect()
        };
        let number = answered + usize::from(self.script.is_empty());
        let calls = match &self.tools {
            Some(script) if !scripted && number >= 1 && number <= script.proposals => script
                .round
                .iter()
                .enumerate()
                .map(|(index, proposal)| ToolCall {
                    call_id: CallId::new(format!("call-{number}-{index}"))
                        .expect("a printable identifier"),
                    name: tool_name(proposal.tool_id),
                    arguments: proposal.arguments.clone(),
                })
                .collect(),
            _ => Vec::new(),
        };
        (pieces, calls)
    }
}

/// The text the model answers a caller item with: user text as it is, a tool result as its
/// JSON, a failure's detail or a rejection's reason.
fn caller_text(item: &Item) -> Option<String> {
    match item {
        Item::UserText { text } => Some(text.clone()),
        Item::ToolResult {
            output,
            failed: false,
            ..
        } => Some(output.to_string()),
        Item::ToolResult { output, .. } => Some(
            match RejectionReason::ALL
                .into_iter()
                .find(|reason| ToolOutcome::Rejected(*reason).output() == *output)
            {
                Some(reason) => format!("the tool call was rejected: {reason:?}"),
                None => format!(
                    "the tool failed: {}",
                    output
                        .pointer("/failure/detail")
                        .and_then(Json::as_str)
                        .map_or_else(|| output.to_string(), str::to_owned)
                ),
            },
        ),
        _ => None,
    }
}

impl llm_core::Model for MockModel {
    fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn turn<'a>(
        &'a self,
        request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, llm_core::Error>> {
        Box::pin(async move {
            let (pieces, calls) = self.answer(request);
            if self.capabilities.output_streaming {
                for piece in &pieces {
                    sink.emit(StreamEvent::TextDelta {
                        text: piece.clone(),
                    })
                    .await?;
                }
            }
            let turn = request
                .items
                .iter()
                .filter(|item| matches!(item, Item::AssistantText { .. }))
                .count();
            let usage = self.usage.get(turn).cloned().flatten();
            if self.holding == Some(turn) {
                cancel.cancelled().await;
                return Err(llm_core::Error::cancelled()
                    .with_dispatch(Dispatch::Accepted)
                    .with_observation(TurnObservation {
                        usage,
                        final_usage: false,
                        ..TurnObservation::new(self.provenance.clone())
                    }));
            }
            let stop_reason = if calls.is_empty() {
                StopReason::EndTurn
            } else {
                StopReason::ToolCalls
            };
            let mut items = vec![Item::assistant(pieces.concat())];
            items.extend(calls.into_iter().map(Item::ToolCall));
            Ok(TurnOutcome {
                stop_reason,
                items,
                observation: TurnObservation {
                    usage,
                    final_usage: true,
                    ..TurnObservation::new(self.provenance.clone())
                },
            })
        })
    }
}
