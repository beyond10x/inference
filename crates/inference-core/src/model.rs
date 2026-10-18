//! The `inference.model` vocabulary of `ess/domains/model.yaml`: what computation can be
//! performed and what it requires.
//!
//! A [`Model`] is a trained inference artifact or hosted endpoint. An [`Adapter`] is the concrete
//! implementation that connects one model or backend to the framework's contracts; it references
//! its model by [`ModelId`], and a model stands on its own. Both are created only through their
//! declaring commands, [`Model::declare`] and [`Adapter::declare`], which return the entity and the
//! event the declaration emits. Serialized names are the ESS names.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::uuid_newtype;

uuid_newtype!(
    /// `inference.model.ModelId`: identifies a [`Model`].
    ModelId
);
uuid_newtype!(
    /// `inference.model.AdapterId`: identifies an [`Adapter`].
    AdapterId
);

/// `inference.model.Capability`: the core streaming capability matrix. An adapter claims each one
/// it supports and must pass the conformance tests for it; a persistent connection proves none of
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Emits partial results before completion.
    OutputStreaming,
    /// Accepts input pieces before the logical input is finished.
    IncrementalInput,
    /// Accepts additional input while emitting output.
    ConcurrentInputOutput,
    /// The model and serving stack genuinely compute on simultaneous inbound and outbound streams.
    NativeDuplex,
    /// Can handle provisional source corrections under an agreed policy.
    RevisableInput,
    /// Can request structured tools and accept correlated results.
    ToolCalling,
    /// Can stop or deactivate work within a declared latency contract.
    Interruptible,
    /// Restores equivalent opaque model state after interruption.
    ResumableModelState,
    /// Reports temporal relationships between output audio and text.
    AudioTextAlignment,
}

impl Capability {
    /// Every capability, in the order `ess/domains/model.yaml` lists them.
    pub const ALL: [Self; 9] = [
        Self::OutputStreaming,
        Self::IncrementalInput,
        Self::ConcurrentInputOutput,
        Self::NativeDuplex,
        Self::RevisableInput,
        Self::ToolCalling,
        Self::Interruptible,
        Self::ResumableModelState,
        Self::AudioTextAlignment,
    ];
}

/// `inference.model.Modality`: a first-class media modality. Serialized as written in ESS
/// (`Text`, `Control`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Modality {
    Text,
    Audio,
    Structured,
    Image,
    Video,
    Control,
}

impl Modality {
    /// Every modality, in the order `ess/domains/model.yaml` lists them.
    pub const ALL: [Self; 6] = [
        Self::Text,
        Self::Audio,
        Self::Structured,
        Self::Image,
        Self::Video,
        Self::Control,
    ];
}

/// `inference.model.Locality`: where an adapter is allowed to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Locality {
    SameProcess,
    LocalWorker,
    SelfHostedCluster,
    ExternalProvider,
}

impl Locality {
    /// Every locality, in the order `ess/domains/model.yaml` lists them.
    pub const ALL: [Self; 4] = [
        Self::SameProcess,
        Self::LocalWorker,
        Self::SelfHostedCluster,
        Self::ExternalProvider,
    ];
}

/// `inference.model.ExecutionForm`: the form a backend adapter runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionForm {
    InProcess,
    NativeFfi,
    Subprocess,
    LocalService,
    HostedService,
}

impl ExecutionForm {
    /// Every execution form, in the order `ess/domains/model.yaml` lists them.
    pub const ALL: [Self; 5] = [
        Self::InProcess,
        Self::NativeFfi,
        Self::Subprocess,
        Self::LocalService,
        Self::HostedService,
    ];
}

/// `inference.model.DeclareModel`: the input that declares a [`Model`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclareModel {
    pub family: String,
    pub checkpoint: String,
    pub code_license: String,
    pub weight_license: String,
    pub checksum: String,
    pub input_modalities: Vec<Modality>,
    pub output_modalities: Vec<Modality>,
}

/// `inference.model.ModelDeclared`: a model was declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDeclared {
    pub model_id: ModelId,
    pub family: String,
    pub checkpoint: String,
}

/// `inference.model.Model`: a trained inference artifact or hosted model endpoint, with its code
/// and weight licences as separate entries and its artifact checksum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Model {
    model_id: ModelId,
    family: String,
    checkpoint: String,
    code_license: String,
    weight_license: String,
    checksum: String,
    input_modalities: Vec<Modality>,
    output_modalities: Vec<Modality>,
}

impl Model {
    /// Runs `inference.model.DeclareModel`: generates a [`ModelId`] and returns the model with the
    /// input's fields and the [`ModelDeclared`] event.
    #[must_use]
    pub fn declare(input: DeclareModel) -> (Self, ModelDeclared) {
        let model_id = ModelId::generate();
        let event = ModelDeclared {
            model_id,
            family: input.family.clone(),
            checkpoint: input.checkpoint.clone(),
        };
        let model = Self {
            model_id,
            family: input.family,
            checkpoint: input.checkpoint,
            code_license: input.code_license,
            weight_license: input.weight_license,
            checksum: input.checksum,
            input_modalities: input.input_modalities,
            output_modalities: input.output_modalities,
        };
        (model, event)
    }

    #[must_use]
    pub const fn model_id(&self) -> ModelId {
        self.model_id
    }

    #[must_use]
    pub fn family(&self) -> &str {
        &self.family
    }

    #[must_use]
    pub fn checkpoint(&self) -> &str {
        &self.checkpoint
    }

    #[must_use]
    pub fn code_license(&self) -> &str {
        &self.code_license
    }

    #[must_use]
    pub fn weight_license(&self) -> &str {
        &self.weight_license
    }

    #[must_use]
    pub fn checksum(&self) -> &str {
        &self.checksum
    }

    #[must_use]
    pub fn input_modalities(&self) -> &[Modality] {
        &self.input_modalities
    }

    #[must_use]
    pub fn output_modalities(&self) -> &[Modality] {
        &self.output_modalities
    }
}

/// `inference.model.DeclareAdapter`: the input that declares an [`Adapter`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclareAdapter {
    pub model_id: ModelId,
    pub backend: String,
    pub contract_revision: String,
    pub execution_form: ExecutionForm,
    pub locality: Locality,
    pub capabilities: Vec<Capability>,
    pub max_concurrency: i64,
}

/// `inference.model.AdapterDeclared`: an adapter was declared with the capabilities it claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterDeclared {
    pub adapter_id: AdapterId,
    pub model_id: ModelId,
    pub capabilities: Vec<Capability>,
}

/// `inference.model.NonPositiveConcurrency`: an adapter's maximum concurrency must be at least
/// one. `submitted` is the value that was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonPositiveConcurrency {
    pub submitted: i64,
}

impl fmt::Display for NonPositiveConcurrency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "an adapter's maximum concurrency must be at least one, got {}",
            self.submitted
        )
    }
}

impl Error for NonPositiveConcurrency {}

/// `inference.model.Adapter`: the concrete implementation that connects one model or backend to
/// the contracts. It references its [`Model`] by `model_id`; dropping an adapter changes nothing
/// about its model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Adapter {
    adapter_id: AdapterId,
    model_id: ModelId,
    backend: String,
    contract_revision: String,
    execution_form: ExecutionForm,
    locality: Locality,
    capabilities: Vec<Capability>,
    max_concurrency: i64,
}

impl Adapter {
    /// Runs `inference.model.DeclareAdapter`.
    ///
    /// # Errors
    ///
    /// [`NonPositiveConcurrency`] carrying the submitted value when `max_concurrency` is not
    /// greater than zero; nothing is declared.
    pub fn declare(
        input: DeclareAdapter,
    ) -> Result<(Self, AdapterDeclared), NonPositiveConcurrency> {
        if input.max_concurrency <= 0 {
            return Err(NonPositiveConcurrency {
                submitted: input.max_concurrency,
            });
        }
        let adapter_id = AdapterId::generate();
        let event = AdapterDeclared {
            adapter_id,
            model_id: input.model_id,
            capabilities: input.capabilities.clone(),
        };
        let adapter = Self {
            adapter_id,
            model_id: input.model_id,
            backend: input.backend,
            contract_revision: input.contract_revision,
            execution_form: input.execution_form,
            locality: input.locality,
            capabilities: input.capabilities,
            max_concurrency: input.max_concurrency,
        };
        Ok((adapter, event))
    }

    #[must_use]
    pub const fn adapter_id(&self) -> AdapterId {
        self.adapter_id
    }

    #[must_use]
    pub const fn model_id(&self) -> ModelId {
        self.model_id
    }

    #[must_use]
    pub fn backend(&self) -> &str {
        &self.backend
    }

    #[must_use]
    pub fn contract_revision(&self) -> &str {
        &self.contract_revision
    }

    #[must_use]
    pub const fn execution_form(&self) -> ExecutionForm {
        self.execution_form
    }

    #[must_use]
    pub const fn locality(&self) -> Locality {
        self.locality
    }

    #[must_use]
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// Always at least one: [`Adapter::declare`] refuses anything less.
    #[must_use]
    pub const fn max_concurrency(&self) -> i64 {
        self.max_concurrency
    }
}
