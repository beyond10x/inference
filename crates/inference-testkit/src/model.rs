//! The `inference.model` domain of `ess/domains/model.yaml`, held in memory.
//!
//! [`ModelDomain`] holds one store per entity, [`ModelStore`] and [`AdapterStore`], and records
//! every event it emits in order. Its commands run the constructors of `inference-core`,
//! [`Model::declare_as`] and [`Adapter::declare_as`], under identities the domain issues from a
//! counter rendered as a Uuid, so that two runs of the same commands report the same
//! identities. What only a store can decide is decided here:
//! `DeclareAdapter` refuses a `model_id` no declared model carries ([`ModelNotFound`], the
//! `unknown-model` outcome) before anything else. The views `inference.model.Models` and
//! `inference.model.Adapters` are [`ModelDomain::models_view`] and [`ModelDomain::adapters_view`];
//! both are `read_your_writes`, and since every row is read from the stores at the call, a write
//! is visible to the next read.

use std::error::Error;
use std::fmt;

use uuid::Uuid;

use inference_core::model::{
    Adapter, AdapterDeclared, AdapterId, Capability, DeclareAdapter, DeclareModel, ExecutionForm,
    Locality, Modality, Model, ModelDeclared, ModelId, ModelNotFound, NonPositiveConcurrency,
};

/// The store of `inference.model.Model`, in declaration order.
#[derive(Debug, Clone, Default)]
pub struct ModelStore {
    models: Vec<Model>,
}

impl ModelStore {
    /// The model carrying `model_id`, if one was declared.
    #[must_use]
    pub fn get(&self, model_id: ModelId) -> Option<&Model> {
        self.models
            .iter()
            .find(|model| model.model_id() == model_id)
    }

    /// Every declared model, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &Model> {
        self.models.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

/// The store of `inference.model.Adapter`, in declaration order.
#[derive(Debug, Clone, Default)]
pub struct AdapterStore {
    adapters: Vec<Adapter>,
}

impl AdapterStore {
    /// The adapter carrying `adapter_id`, if one was declared.
    #[must_use]
    pub fn get(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.adapter_id() == adapter_id)
    }

    /// Every declared adapter, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &Adapter> {
        self.adapters.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }
}

/// An event of `inference.model`, as the domain emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelEvent {
    /// `inference.model.ModelDeclared`.
    ModelDeclared(ModelDeclared),
    /// `inference.model.AdapterDeclared`.
    AdapterDeclared(AdapterDeclared),
}

/// Why `inference.model.DeclareAdapter` declared nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclareAdapterError {
    /// The `unknown-model` outcome: no declared model carries the `model_id` sent.
    UnknownModel(ModelNotFound),
    /// The `refused` outcome: the maximum concurrency sent was not positive.
    Refused(NonPositiveConcurrency),
}

impl fmt::Display for DeclareAdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownModel(error) => fmt::Display::fmt(error, f),
            Self::Refused(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for DeclareAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownModel(error) => Some(error),
            Self::Refused(error) => Some(error),
        }
    }
}

/// A row of the view `inference.model.Models`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelsRow {
    pub model_id: ModelId,
    pub family: String,
    pub checkpoint: String,
    pub code_license: String,
    pub weight_license: String,
    pub checksum: String,
    pub input_modalities: Vec<Modality>,
    pub output_modalities: Vec<Modality>,
    pub provenance: String,
    pub source_revision: String,
}

/// A row of the view `inference.model.Adapters`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptersRow {
    pub adapter_id: AdapterId,
    pub model_id: ModelId,
    pub backend: String,
    pub contract_revision: String,
    pub execution_form: ExecutionForm,
    pub locality: Locality,
    pub capabilities: Vec<Capability>,
    pub max_concurrency: i64,
}

/// The `inference.model` domain in memory: its two entity stores and the events it emitted.
#[derive(Debug, Clone, Default)]
pub struct ModelDomain {
    models: ModelStore,
    adapters: AdapterStore,
    events: Vec<ModelEvent>,
    issued: u128,
}

impl ModelDomain {
    /// An empty domain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `inference.model.DeclareModel`: stores the model [`Model::declare_as`] returns and
    /// records its [`ModelDeclared`].
    pub fn declare_model(&mut self, input: DeclareModel) -> ModelDeclared {
        let model_id = ModelId::from(self.issue());
        let (model, event) = Model::declare_as(model_id, input);
        self.models.models.push(model);
        self.events.push(ModelEvent::ModelDeclared(event.clone()));
        event
    }

    /// Runs `inference.model.DeclareAdapter`: stores the adapter [`Adapter::declare_as`] returns and
    /// records its [`AdapterDeclared`].
    ///
    /// # Errors
    ///
    /// [`DeclareAdapterError::UnknownModel`] when no stored model carries `model_id`, decided
    /// before the concurrency; otherwise [`DeclareAdapterError::Refused`] when `max_concurrency`
    /// is not positive. Either way nothing is stored and no event is recorded.
    pub fn declare_adapter(
        &mut self,
        input: DeclareAdapter,
    ) -> Result<AdapterDeclared, DeclareAdapterError> {
        if self.models.get(input.model_id).is_none() {
            return Err(DeclareAdapterError::UnknownModel(ModelNotFound {
                model_id: input.model_id,
            }));
        }
        let adapter_id = AdapterId::from(Self::identity(self.issued + 1));
        let (adapter, event) =
            Adapter::declare_as(adapter_id, input).map_err(DeclareAdapterError::Refused)?;
        self.issue();
        self.adapters.adapters.push(adapter);
        self.events.push(ModelEvent::AdapterDeclared(event.clone()));
        Ok(event)
    }

    /// The next identity of the counter; a refused command takes none.
    fn issue(&mut self) -> Uuid {
        self.issued += 1;
        Self::identity(self.issued)
    }

    /// The `count`th identity: a version 4, RFC 4122 variant Uuid whose low bits are `count`,
    /// `00000000-0000-4000-8000-000000000001` first.
    const fn identity(count: u128) -> Uuid {
        Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | count)
    }

    /// The declared models.
    #[must_use]
    pub const fn models(&self) -> &ModelStore {
        &self.models
    }

    /// The declared adapters.
    #[must_use]
    pub const fn adapters(&self) -> &AdapterStore {
        &self.adapters
    }

    /// Every event emitted, in the order it was emitted.
    #[must_use]
    pub fn events(&self) -> &[ModelEvent] {
        &self.events
    }

    /// The view `inference.model.Models`: one row per declared model, in declaration order.
    #[must_use]
    pub fn models_view(&self) -> Vec<ModelsRow> {
        self.models
            .iter()
            .map(|model| ModelsRow {
                model_id: model.model_id(),
                family: model.family().to_owned(),
                checkpoint: model.checkpoint().to_owned(),
                code_license: model.code_license().to_owned(),
                weight_license: model.weight_license().to_owned(),
                checksum: model.checksum().to_owned(),
                input_modalities: model.input_modalities().to_vec(),
                output_modalities: model.output_modalities().to_vec(),
                provenance: model.provenance().to_owned(),
                source_revision: model.source_revision().to_owned(),
            })
            .collect()
    }

    /// The view `inference.model.Adapters`: one row per declared adapter, in declaration order.
    #[must_use]
    pub fn adapters_view(&self) -> Vec<AdaptersRow> {
        self.adapters
            .iter()
            .map(|adapter| AdaptersRow {
                adapter_id: adapter.adapter_id(),
                model_id: adapter.model_id(),
                backend: adapter.backend().to_owned(),
                contract_revision: adapter.contract_revision().to_owned(),
                execution_form: adapter.execution_form(),
                locality: adapter.locality(),
                capabilities: adapter.capabilities().to_vec(),
                max_concurrency: adapter.max_concurrency(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use inference_core::model::{
        Capability, DeclareAdapter, DeclareModel, ExecutionForm, Locality, Modality, ModelId,
        ModelNotFound, NonPositiveConcurrency,
    };

    use super::{AdaptersRow, DeclareAdapterError, ModelDomain, ModelEvent, ModelsRow};

    fn declare_model() -> DeclareModel {
        DeclareModel {
            family: "family".to_owned(),
            checkpoint: "checkpoint".to_owned(),
            code_license: "code_license".to_owned(),
            weight_license: "weight_license".to_owned(),
            checksum: "checksum".to_owned(),
            input_modalities: vec![Modality::Text],
            output_modalities: vec![Modality::Audio],
            provenance: "provenance".to_owned(),
            source_revision: "source_revision".to_owned(),
        }
    }

    fn declare_adapter(model_id: ModelId, max_concurrency: i64) -> DeclareAdapter {
        DeclareAdapter {
            model_id,
            backend: "backend".to_owned(),
            contract_revision: "contract_revision".to_owned(),
            execution_form: ExecutionForm::InProcess,
            locality: Locality::SameProcess,
            capabilities: vec![Capability::OutputStreaming],
            max_concurrency,
        }
    }

    fn unknown_model_id() -> ModelId {
        serde_json::from_value(serde_json::json!("00000000-0000-4000-8000-50f408d7384c"))
            .expect("a canonical Uuid")
    }

    #[test]
    fn identities_come_from_a_counter_and_a_refusal_takes_none() {
        let run = || {
            let mut domain = ModelDomain::new();
            let model = domain.declare_model(declare_model());
            let refused = domain.declare_adapter(declare_adapter(model.model_id, 0));
            let adapter = domain
                .declare_adapter(declare_adapter(model.model_id, 1))
                .expect("declared");
            (model.model_id, refused.is_err(), adapter.adapter_id)
        };
        let (model_id, refused, adapter_id) = run();
        assert_eq!(model_id.to_string(), "00000000-0000-4000-8000-000000000001");
        assert!(refused);
        assert_eq!(
            adapter_id.to_string(),
            "00000000-0000-4000-8000-000000000002"
        );
        assert_eq!(run(), (model_id, refused, adapter_id));
    }

    #[test]
    fn a_declared_model_is_stored_recorded_and_in_the_models_view() {
        let mut domain = ModelDomain::new();
        let event = domain.declare_model(declare_model());

        assert_eq!(event.family, "family");
        assert_eq!(event.checkpoint, "checkpoint");
        assert!(domain.models().get(event.model_id).is_some());
        assert_eq!(domain.events(), [ModelEvent::ModelDeclared(event.clone())]);
        assert_eq!(
            domain.models_view(),
            [ModelsRow {
                model_id: event.model_id,
                family: "family".to_owned(),
                checkpoint: "checkpoint".to_owned(),
                code_license: "code_license".to_owned(),
                weight_license: "weight_license".to_owned(),
                checksum: "checksum".to_owned(),
                input_modalities: vec![Modality::Text],
                output_modalities: vec![Modality::Audio],
                provenance: "provenance".to_owned(),
                source_revision: "source_revision".to_owned(),
            }]
        );
    }

    #[test]
    fn an_adapter_of_a_declared_model_is_stored_recorded_after_it_and_in_the_adapters_view() {
        let mut domain = ModelDomain::new();
        let model = domain.declare_model(declare_model());
        let event = domain
            .declare_adapter(declare_adapter(model.model_id, 2))
            .expect("the model is declared and the concurrency positive");

        assert_eq!(event.model_id, model.model_id);
        assert_eq!(event.capabilities, [Capability::OutputStreaming]);
        assert!(domain.adapters().get(event.adapter_id).is_some());
        assert_eq!(
            domain.events(),
            [
                ModelEvent::ModelDeclared(model.clone()),
                ModelEvent::AdapterDeclared(event.clone())
            ]
        );
        assert_eq!(
            domain.adapters_view(),
            [AdaptersRow {
                adapter_id: event.adapter_id,
                model_id: model.model_id,
                backend: "backend".to_owned(),
                contract_revision: "contract_revision".to_owned(),
                execution_form: ExecutionForm::InProcess,
                locality: Locality::SameProcess,
                capabilities: vec![Capability::OutputStreaming],
                max_concurrency: 2,
            }]
        );
    }

    #[test]
    fn an_adapter_of_an_undeclared_model_is_refused_and_nothing_is_stored() {
        let mut domain = ModelDomain::new();
        domain.declare_model(declare_model());
        let unknown = unknown_model_id();

        assert_eq!(
            domain.declare_adapter(declare_adapter(unknown, 1)),
            Err(DeclareAdapterError::UnknownModel(ModelNotFound {
                model_id: unknown
            }))
        );
        assert!(domain.adapters().is_empty());
        assert!(domain.adapters_view().is_empty());
        assert_eq!(domain.events().len(), 1);
    }

    #[test]
    fn an_undeclared_model_is_decided_before_the_concurrency() {
        let mut domain = ModelDomain::new();
        let unknown = unknown_model_id();

        assert_eq!(
            domain.declare_adapter(declare_adapter(unknown, 0)),
            Err(DeclareAdapterError::UnknownModel(ModelNotFound {
                model_id: unknown
            }))
        );
    }

    #[test]
    fn a_non_positive_concurrency_is_refused_and_nothing_is_stored() {
        let mut domain = ModelDomain::new();
        let model = domain.declare_model(declare_model());

        for submitted in [0, -1] {
            assert_eq!(
                domain.declare_adapter(declare_adapter(model.model_id, submitted)),
                Err(DeclareAdapterError::Refused(NonPositiveConcurrency {
                    submitted
                }))
            );
        }
        assert!(domain.adapters().is_empty());
        assert_eq!(domain.events().len(), 1);
    }
}
