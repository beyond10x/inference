//! The `inference.model` domain of `ess/domains/model.yaml`, held in memory.
//!
//! [`ModelDomain`] holds one store per entity, [`ModelStore`], [`AdapterStore`] and
//! [`CapabilityRecordStore`], and records every event it emits in order. Its commands run the
//! constructors of `inference-core`, [`Model::declare_as`], [`Adapter::declare_as`] and
//! [`CapabilityRecord::declare_as`], under identities the domain issues from a counter rendered as
//! a Uuid in its own [`IDENTITY_SPACE`], so that two runs of the same commands report the same
//! identities and no other domain issues one of them. What only a store can decide is decided
//! here: `DeclareAdapter` refuses a `model_id` no declared model carries ([`ModelNotFound`], the
//! `unknown-model` outcome) before anything else, and `DeclareCapabilityRecord` refuses an
//! `adapter_id` no declared adapter carries ([`AdapterNotFound`], `unknown-adapter`) and then a
//! `model_id` no declared model carries (`unknown-model`). The views `inference.model.Models`,
//! `inference.model.Adapters` and `inference.model.CapabilityRecords` are
//! [`ModelDomain::models_view`], [`ModelDomain::adapters_view`] and
//! [`ModelDomain::capability_records_view`]; all are `read_your_writes`, and since every row is
//! read from the stores at the call, a write is visible to the next read.
//!
//! [`capability`] loads the record file an adapter crate commits and checks it against the
//! adapter's claims.

use std::error::Error;
use std::fmt;

use uuid::Uuid;

use inference_core::inference_audio::AudioFormat;
use inference_core::model::{
    Adapter, AdapterDeclared, AdapterId, AdapterNotFound, Capability, CapabilityEvidence,
    CapabilityRecord, CapabilityRecordDeclared, CapabilityRecordId, DeclareAdapter,
    DeclareCapabilityRecord, DeclareModel, ExecutionForm, Locality, Modality, Model, ModelDeclared,
    ModelId, ModelNotFound, NonPositiveConcurrency, ResourceProfile,
};

/// The capability record of an adapter, loaded from its JSON file and checked against the
/// adapter's claims.
#[path = "capability.rs"]
pub mod capability;

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

/// The store of `inference.model.CapabilityRecord`, in declaration order.
#[derive(Debug, Clone, Default)]
pub struct CapabilityRecordStore {
    records: Vec<CapabilityRecord>,
}

impl CapabilityRecordStore {
    /// The record carrying `capability_record_id`, if one was declared.
    #[must_use]
    pub fn get(&self, capability_record_id: CapabilityRecordId) -> Option<&CapabilityRecord> {
        self.records
            .iter()
            .find(|record| record.capability_record_id() == capability_record_id)
    }

    /// Every declared record, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &CapabilityRecord> {
        self.records.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// An event of `inference.model`, as the domain emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelEvent {
    /// `inference.model.ModelDeclared`.
    ModelDeclared(ModelDeclared),
    /// `inference.model.AdapterDeclared`.
    AdapterDeclared(AdapterDeclared),
    /// `inference.model.CapabilityRecordDeclared`.
    CapabilityRecordDeclared(CapabilityRecordDeclared),
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

/// Why `inference.model.DeclareCapabilityRecord` declared nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclareCapabilityRecordError {
    /// The `unknown-adapter` outcome: no declared adapter carries the `adapter_id` sent.
    UnknownAdapter(AdapterNotFound),
    /// The `unknown-model` outcome: no declared model carries the `model_id` sent.
    UnknownModel(ModelNotFound),
}

impl fmt::Display for DeclareCapabilityRecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAdapter(error) => fmt::Display::fmt(error, f),
            Self::UnknownModel(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for DeclareCapabilityRecordError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownAdapter(error) => Some(error),
            Self::UnknownModel(error) => Some(error),
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

/// A row of the view `inference.model.CapabilityRecords`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRecordsRow {
    pub capability_record_id: CapabilityRecordId,
    pub adapter_id: AdapterId,
    pub model_id: ModelId,
    pub hardware: String,
    pub languages: Vec<String>,
    pub audio_formats: Vec<AudioFormat>,
    pub evidence: Vec<CapabilityEvidence>,
    pub resource_profile: ResourceProfile,
    pub adapter_revision: String,
}

/// The first eight hex digits of every identity the domain issues, which no other domain of the
/// testkit issues under (see [`crate::policy::IDENTITY_SPACE`]).
pub const IDENTITY_SPACE: u32 = 1;

/// The `inference.model` domain in memory: its three entity stores and the events it emitted.
#[derive(Debug, Clone, Default)]
pub struct ModelDomain {
    models: ModelStore,
    adapters: AdapterStore,
    capability_records: CapabilityRecordStore,
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

    /// Runs `inference.model.DeclareCapabilityRecord`: stores the record
    /// [`CapabilityRecord::declare_as`] returns and records its [`CapabilityRecordDeclared`].
    /// Neither that the model is the adapter's nor that the adapter has no record for the
    /// hardware target yet is checked, as `ess/domains/model.yaml` declares.
    ///
    /// # Errors
    ///
    /// [`DeclareCapabilityRecordError::UnknownAdapter`] when no stored adapter carries
    /// `adapter_id`, decided first; otherwise [`DeclareCapabilityRecordError::UnknownModel`] when
    /// no stored model carries `model_id`. Either way nothing is stored, no event is recorded and
    /// no identity is taken.
    pub fn declare_capability_record(
        &mut self,
        input: DeclareCapabilityRecord,
    ) -> Result<CapabilityRecordDeclared, DeclareCapabilityRecordError> {
        if self.adapters.get(input.adapter_id).is_none() {
            return Err(DeclareCapabilityRecordError::UnknownAdapter(
                AdapterNotFound {
                    adapter_id: input.adapter_id,
                },
            ));
        }
        if self.models.get(input.model_id).is_none() {
            return Err(DeclareCapabilityRecordError::UnknownModel(ModelNotFound {
                model_id: input.model_id,
            }));
        }
        let capability_record_id = CapabilityRecordId::from(self.issue());
        let (record, event) = CapabilityRecord::declare_as(capability_record_id, input);
        self.capability_records.records.push(record);
        self.events
            .push(ModelEvent::CapabilityRecordDeclared(event.clone()));
        Ok(event)
    }

    /// The next identity of the counter; a refused command takes none.
    fn issue(&mut self) -> Uuid {
        self.issued += 1;
        Self::identity(self.issued)
    }

    /// The `count`th identity: a version 4, RFC 4122 variant Uuid whose low bits are `count` and
    /// whose first eight hex digits are [`IDENTITY_SPACE`], `00000001-0000-4000-8000-000000000001`
    /// first.
    fn identity(count: u128) -> Uuid {
        Uuid::from_u128(
            (u128::from(IDENTITY_SPACE) << 96) | 0x0000_0000_0000_4000_8000_0000_0000_0000 | count,
        )
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

    /// The declared capability records.
    #[must_use]
    pub const fn capability_records(&self) -> &CapabilityRecordStore {
        &self.capability_records
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

    /// The view `inference.model.CapabilityRecords`: one row per declared record, in declaration
    /// order.
    #[must_use]
    pub fn capability_records_view(&self) -> Vec<CapabilityRecordsRow> {
        self.capability_records
            .iter()
            .map(|record| CapabilityRecordsRow {
                capability_record_id: record.capability_record_id(),
                adapter_id: record.adapter_id(),
                model_id: record.model_id(),
                hardware: record.hardware().to_owned(),
                languages: record.languages().to_vec(),
                audio_formats: record.audio_formats().to_vec(),
                evidence: record.evidence().to_vec(),
                resource_profile: record.resource_profile(),
                adapter_revision: record.adapter_revision().to_owned(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use inference_core::model::{
        AdapterId, AdapterNotFound, Capability, CapabilityEvidence, DeclareAdapter,
        DeclareCapabilityRecord, DeclareModel, ExecutionForm, Locality, Modality, ModelId,
        ModelNotFound, NonPositiveConcurrency, ResourceProfile,
    };

    use super::{
        AdaptersRow, CapabilityRecordsRow, DeclareAdapterError, DeclareCapabilityRecordError,
        ModelDomain, ModelEvent, ModelsRow,
    };

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
        assert_eq!(model_id.to_string(), "00000001-0000-4000-8000-000000000001");
        assert!(refused);
        assert_eq!(
            adapter_id.to_string(),
            "00000001-0000-4000-8000-000000000002"
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

    fn declare_record(adapter_id: AdapterId, model_id: ModelId) -> DeclareCapabilityRecord {
        DeclareCapabilityRecord {
            adapter_id,
            model_id,
            hardware: "hardware".to_owned(),
            languages: vec!["en".to_owned()],
            audio_formats: Vec::new(),
            evidence: vec![CapabilityEvidence {
                capability: Capability::OutputStreaming,
                tests: vec!["test".to_owned()],
            }],
            resource_profile: ResourceProfile {
                peak_resident_memory_mib: 1,
                load_time_ms: 2,
                cpu_real_time_factor_milli: Some(3),
                gpu_memory_mib: None,
            },
            adapter_revision: "adapter_revision".to_owned(),
        }
    }

    fn unknown_adapter_id() -> AdapterId {
        serde_json::from_value(serde_json::json!("00000000-0000-4000-8000-eaaf599fc0a2"))
            .expect("a canonical Uuid")
    }

    #[test]
    fn a_record_of_a_declared_adapter_and_model_is_stored_recorded_and_in_its_view() {
        let mut domain = ModelDomain::new();
        let model = domain.declare_model(declare_model());
        let adapter = domain
            .declare_adapter(declare_adapter(model.model_id, 1))
            .expect("declared");
        let input = declare_record(adapter.adapter_id, model.model_id);

        let event = domain
            .declare_capability_record(input.clone())
            .expect("the adapter and the model are declared");

        assert_eq!(
            event.capability_record_id.to_string(),
            "00000001-0000-4000-8000-000000000003",
            "the record's identity is the domain counter's next"
        );
        assert_eq!(event.adapter_id, adapter.adapter_id);
        assert_eq!(event.model_id, model.model_id);
        assert_eq!(event.hardware, "hardware");
        assert!(
            domain
                .capability_records()
                .get(event.capability_record_id)
                .is_some()
        );
        assert_eq!(domain.capability_records().len(), 1);
        assert_eq!(
            domain.events().last(),
            Some(&ModelEvent::CapabilityRecordDeclared(event.clone()))
        );
        assert_eq!(
            domain.capability_records_view(),
            [CapabilityRecordsRow {
                capability_record_id: event.capability_record_id,
                adapter_id: input.adapter_id,
                model_id: input.model_id,
                hardware: input.hardware,
                languages: input.languages,
                audio_formats: input.audio_formats,
                evidence: input.evidence,
                resource_profile: input.resource_profile,
                adapter_revision: input.adapter_revision,
            }]
        );
    }

    #[test]
    fn a_record_of_an_undeclared_adapter_is_refused_before_its_model() {
        let mut domain = ModelDomain::new();
        let unknown_adapter = unknown_adapter_id();
        let unknown_model = unknown_model_id();

        assert_eq!(
            domain.declare_capability_record(declare_record(unknown_adapter, unknown_model)),
            Err(DeclareCapabilityRecordError::UnknownAdapter(
                AdapterNotFound {
                    adapter_id: unknown_adapter
                }
            ))
        );
        assert!(domain.capability_records().is_empty());
        assert!(domain.capability_records_view().is_empty());
        assert!(domain.events().is_empty());
    }

    #[test]
    fn a_record_of_an_undeclared_model_is_refused_and_takes_no_identity() {
        let mut domain = ModelDomain::new();
        let model = domain.declare_model(declare_model());
        let adapter = domain
            .declare_adapter(declare_adapter(model.model_id, 1))
            .expect("declared");
        let unknown = unknown_model_id();

        assert_eq!(
            domain.declare_capability_record(declare_record(adapter.adapter_id, unknown)),
            Err(DeclareCapabilityRecordError::UnknownModel(ModelNotFound {
                model_id: unknown
            }))
        );
        assert!(domain.capability_records().is_empty());
        assert_eq!(domain.events().len(), 2);

        let declared = domain
            .declare_capability_record(declare_record(adapter.adapter_id, model.model_id))
            .expect("declared");
        assert_eq!(
            declared.capability_record_id.to_string(),
            "00000001-0000-4000-8000-000000000003",
            "the refusal took no identity"
        );
    }

    #[test]
    fn a_record_may_name_a_model_other_than_its_adapters_as_the_specification_declares() {
        let mut domain = ModelDomain::new();
        let first = domain.declare_model(declare_model());
        let second = domain.declare_model(declare_model());
        let adapter = domain
            .declare_adapter(declare_adapter(second.model_id, 1))
            .expect("declared");

        let event = domain
            .declare_capability_record(declare_record(adapter.adapter_id, first.model_id))
            .expect("both identities are declared");
        assert_eq!(event.model_id, first.model_id);
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
