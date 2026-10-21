//! The `inference.usage` domain of `ess/domains/usage.yaml`, held in memory.
//!
//! [`UsageDomain`] holds the store of [`UsageRecord`]s, [`UsageRecordStore`], and of
//! [`CostRecord`]s, [`CostRecordStore`], and records every event it emits in order, both kinds in
//! one sequence. `RecordUsage` runs the constructor of `inference-core`,
//! [`UsageRecord::record_as`], under identities the domain issues from a counter rendered as a
//! Uuid in its own [`IDENTITY_SPACE`], `00000007-0000-4000-8000-000000000001` first; `PriceUsage`
//! runs [`CostRecord::record_as`] under identities of a second counter in the group `0001` of that
//! space, `00000007-0001-4000-8000-000000000001` first. What only a store can decide is decided
//! here, reading the `inference.model` store: `RecordUsage` refuses an `adapter_id` no declared
//! adapter carries ([`AdapterNotFound`], the `unknown-adapter` outcome) and then a `model_id` no
//! declared model carries ([`ModelNotFound`], `unknown-model`), in the order the outcomes are
//! declared; `PriceUsage` refuses a `usage_record_id` no usage record carries
//! ([`UsageRecordNotFound`], `unknown-usage`). The run, node, session, speech session and response
//! a record names are not checked, as `ess/domains/usage.yaml` declares, and neither are a cost
//! record's lines and totals, which are the pricing crate's. The specification declares no view of
//! the records; [`UsageDomain::records`] and [`UsageDomain::costs`] are the stores themselves.

use std::error::Error;
use std::fmt;

use uuid::Uuid;

use inference_core::model::{AdapterNotFound, ModelNotFound};
use inference_core::usage::{
    CostRecord, CostRecordId, CostRecorded, PriceUsage, RecordUsage, UsageRecord, UsageRecordId,
    UsageRecordNotFound, UsageRecorded,
};

use crate::model::ModelDomain;

/// The first eight hex digits of every identity the domain issues, which no other domain of the
/// testkit issues under (see [`crate::policy::IDENTITY_SPACE`]).
pub const IDENTITY_SPACE: u32 = 7;

/// The store of `inference.usage.UsageRecord`, in the order the records were made.
#[derive(Debug, Clone, Default)]
pub struct UsageRecordStore {
    records: Vec<UsageRecord>,
}

impl UsageRecordStore {
    /// The record carrying `usage_record_id`, if one was made.
    #[must_use]
    pub fn get(&self, usage_record_id: UsageRecordId) -> Option<&UsageRecord> {
        self.records
            .iter()
            .find(|record| record.usage_record_id() == usage_record_id)
    }

    /// Every record, in the order the records were made.
    pub fn iter(&self) -> impl Iterator<Item = &UsageRecord> {
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

/// The store of `inference.usage.CostRecord`, in the order the records were made.
#[derive(Debug, Clone, Default)]
pub struct CostRecordStore {
    records: Vec<CostRecord>,
}

impl CostRecordStore {
    /// The record carrying `cost_record_id`, if one was made.
    #[must_use]
    pub fn get(&self, cost_record_id: CostRecordId) -> Option<&CostRecord> {
        self.records
            .iter()
            .find(|record| record.cost_record_id() == cost_record_id)
    }

    /// Every record, in the order the records were made.
    pub fn iter(&self) -> impl Iterator<Item = &CostRecord> {
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

/// An event of `inference.usage`, as the domain emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageEvent {
    /// `inference.usage.UsageRecorded`.
    UsageRecorded(UsageRecorded),
    /// `inference.usage.CostRecorded`.
    CostRecorded(CostRecorded),
}

/// Why `inference.usage.RecordUsage` recorded nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordUsageError {
    /// The `unknown-adapter` outcome: no declared adapter carries the `adapter_id` sent.
    UnknownAdapter(AdapterNotFound),
    /// The `unknown-model` outcome: no declared model carries the `model_id` sent.
    UnknownModel(ModelNotFound),
}

impl fmt::Display for RecordUsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAdapter(error) => fmt::Display::fmt(error, f),
            Self::UnknownModel(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for RecordUsageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownAdapter(error) => Some(error),
            Self::UnknownModel(error) => Some(error),
        }
    }
}

/// The `inference.usage` domain in memory: its record stores and the events it emitted.
#[derive(Debug, Clone, Default)]
pub struct UsageDomain {
    records: UsageRecordStore,
    costs: CostRecordStore,
    events: Vec<UsageEvent>,
    issued: u128,
    costs_issued: u128,
}

impl UsageDomain {
    /// An empty domain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `inference.usage.RecordUsage`: stores the record [`UsageRecord::record_as`] returns
    /// and records its [`UsageRecorded`].
    ///
    /// # Errors
    ///
    /// [`RecordUsageError::UnknownAdapter`] when no adapter of `model` carries `adapter_id`,
    /// decided first; otherwise [`RecordUsageError::UnknownModel`] when no model of `model`
    /// carries `model_id`. Either way nothing is stored, no event is recorded and no identity is
    /// taken.
    pub fn record_usage(
        &mut self,
        model: &ModelDomain,
        input: RecordUsage,
    ) -> Result<UsageRecorded, RecordUsageError> {
        if model.adapters().get(input.adapter_id).is_none() {
            return Err(RecordUsageError::UnknownAdapter(AdapterNotFound {
                adapter_id: input.adapter_id,
            }));
        }
        if model.models().get(input.model_id).is_none() {
            return Err(RecordUsageError::UnknownModel(ModelNotFound {
                model_id: input.model_id,
            }));
        }
        let usage_record_id = UsageRecordId::from(self.issue());
        let (record, event) = UsageRecord::record_as(usage_record_id, input);
        self.records.records.push(record);
        self.events.push(UsageEvent::UsageRecorded(event.clone()));
        Ok(event)
    }

    /// The next identity of the counter; a refused command takes none.
    fn issue(&mut self) -> Uuid {
        self.issued += 1;
        Uuid::from_u128(
            (u128::from(IDENTITY_SPACE) << 96)
                | 0x0000_0000_0000_4000_8000_0000_0000_0000
                | self.issued,
        )
    }

    /// Runs `inference.usage.PriceUsage`: stores the record [`CostRecord::record_as`] returns and
    /// records its [`CostRecorded`].
    ///
    /// # Errors
    ///
    /// [`UsageRecordNotFound`] when no usage record of this domain carries `usage_record_id`;
    /// nothing is stored, no event is recorded and no identity is taken.
    pub fn price_usage(&mut self, input: PriceUsage) -> Result<CostRecorded, UsageRecordNotFound> {
        if self.records.get(input.usage_record_id).is_none() {
            return Err(UsageRecordNotFound {
                usage_record_id: input.usage_record_id,
            });
        }
        self.costs_issued += 1;
        let cost_record_id = CostRecordId::from(Uuid::from_u128(
            (u128::from(IDENTITY_SPACE) << 96)
                | (1 << 80)
                | 0x0000_0000_0000_4000_8000_0000_0000_0000
                | self.costs_issued,
        ));
        let (record, event) = CostRecord::record_as(cost_record_id, input);
        self.costs.records.push(record);
        self.events.push(UsageEvent::CostRecorded(event.clone()));
        Ok(event)
    }

    /// The usage records made.
    #[must_use]
    pub const fn records(&self) -> &UsageRecordStore {
        &self.records
    }

    /// The cost records made.
    #[must_use]
    pub const fn costs(&self) -> &CostRecordStore {
        &self.costs
    }

    /// Every event emitted, in the order it was emitted.
    #[must_use]
    pub fn events(&self) -> &[UsageEvent] {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use inference_core::model::{
        AdapterId, AdapterNotFound, DeclareAdapter, DeclareModel, ExecutionForm, Locality,
        Modality, ModelId, ModelNotFound,
    };
    use inference_core::usage::{
        CostLine, CostTotal, PriceUsage, Quantity, QuantityKind, RecordUsage, UsageRecordId,
        UsageRecordNotFound,
    };
    use uuid::Uuid;

    use super::{RecordUsageError, UsageDomain, UsageEvent};
    use crate::model::ModelDomain;

    /// A model domain with one model and one adapter of it.
    fn declared() -> (ModelDomain, AdapterId, ModelId) {
        let mut model = ModelDomain::new();
        let model_id = model
            .declare_model(DeclareModel {
                family: "family".to_owned(),
                checkpoint: "checkpoint".to_owned(),
                code_license: "code_license".to_owned(),
                weight_license: "weight_license".to_owned(),
                checksum: "checksum".to_owned(),
                input_modalities: vec![Modality::Text],
                output_modalities: vec![Modality::Text],
                provenance: "provenance".to_owned(),
                source_revision: "source_revision".to_owned(),
            })
            .model_id;
        let adapter_id = model
            .declare_adapter(DeclareAdapter {
                model_id,
                backend: "backend".to_owned(),
                contract_revision: "1".to_owned(),
                execution_form: ExecutionForm::InProcess,
                locality: Locality::SameProcess,
                capabilities: Vec::new(),
                max_concurrency: 1,
            })
            .expect("declared")
            .adapter_id;
        (model, adapter_id, model_id)
    }

    fn usage(adapter_id: AdapterId, model_id: ModelId) -> RecordUsage {
        RecordUsage::new(
            adapter_id,
            model_id,
            10,
            20,
            vec![
                Quantity::provider_reported(QuantityKind::OutputTextTokens, 7),
                Quantity::unknown(QuantityKind::CachedInputTokens),
            ],
        )
    }

    fn unknown<T: From<Uuid>>() -> T {
        T::from(Uuid::from_u128(0x0000_0000_0000_4000_8000_50f4_08d7_384c))
    }

    #[test]
    fn a_record_of_a_declared_adapter_and_model_is_stored_and_recorded() {
        let (model, adapter_id, model_id) = declared();
        let mut domain = UsageDomain::new();

        let event = domain
            .record_usage(&model, usage(adapter_id, model_id))
            .expect("the adapter and the model are declared");

        assert_eq!(
            event.usage_record_id.to_string(),
            "00000007-0000-4000-8000-000000000001"
        );
        assert_eq!(event.adapter_id, adapter_id);
        assert_eq!(event.model_id, model_id);
        let record = domain.records().get(event.usage_record_id).expect("stored");
        assert_eq!(record.recorded(), event);
        assert_eq!(domain.events(), [UsageEvent::UsageRecorded(event)]);
    }

    #[test]
    fn an_undeclared_adapter_is_refused_before_the_model() {
        let (model, _, _) = declared();
        let mut domain = UsageDomain::new();
        let adapter_id = unknown::<AdapterId>();

        assert_eq!(
            domain.record_usage(&model, usage(adapter_id, unknown())),
            Err(RecordUsageError::UnknownAdapter(AdapterNotFound {
                adapter_id
            }))
        );
        assert!(domain.records().is_empty());
        assert!(domain.events().is_empty());
    }

    fn price(usage_record_id: UsageRecordId) -> PriceUsage {
        PriceUsage {
            usage_record_id,
            currency: "EUR".to_owned(),
            price_revision: "1".to_owned(),
            price_digest: "prices-sha256:00".to_owned(),
            lines: vec![CostLine {
                kind: QuantityKind::OutputTextTokens,
                quantity: Some(7),
                amount: Some("0.000014".to_owned()),
                unknown: None,
            }],
            total: CostTotal {
                known_subtotal: "0.000014".to_owned(),
                unknown_lines: 0,
                complete_total: Some("0.000014".to_owned()),
            },
        }
    }

    #[test]
    fn a_cost_of_a_recorded_usage_is_stored_and_recorded_after_it() {
        let (model, adapter_id, model_id) = declared();
        let mut domain = UsageDomain::new();
        let usage = domain
            .record_usage(&model, usage(adapter_id, model_id))
            .expect("declared");

        let cost = domain
            .price_usage(price(usage.usage_record_id))
            .expect("the usage record exists");

        assert_eq!(
            cost.cost_record_id.to_string(),
            "00000007-0001-4000-8000-000000000001",
            "the cost counter's first identity, apart from the usage records'"
        );
        assert_eq!(cost.usage_record_id, usage.usage_record_id);
        assert_eq!(cost.lines, price(usage.usage_record_id).lines);
        assert_eq!(cost.total, price(usage.usage_record_id).total);
        let record = domain.costs().get(cost.cost_record_id).expect("stored");
        assert_eq!(record.recorded(), cost);
        assert_eq!(domain.costs().len(), 1);
        assert_eq!(
            domain.events(),
            [
                UsageEvent::UsageRecorded(usage),
                UsageEvent::CostRecorded(cost)
            ],
            "one sequence, in the order emitted"
        );
    }

    #[test]
    fn a_cost_of_an_unknown_usage_is_refused_and_takes_no_identity() {
        let (model, adapter_id, model_id) = declared();
        let mut domain = UsageDomain::new();
        let missing = unknown::<UsageRecordId>();

        assert_eq!(
            domain.price_usage(price(missing)),
            Err(UsageRecordNotFound {
                usage_record_id: missing
            })
        );
        assert!(domain.costs().is_empty());
        assert!(domain.events().is_empty());

        let usage = domain
            .record_usage(&model, usage(adapter_id, model_id))
            .expect("declared");
        let cost = domain
            .price_usage(price(usage.usage_record_id))
            .expect("exists");
        assert_eq!(
            cost.cost_record_id.to_string(),
            "00000007-0001-4000-8000-000000000001",
            "the refusal took no identity"
        );
    }

    #[test]
    fn an_undeclared_model_is_refused_and_takes_no_identity() {
        let (model, adapter_id, model_id) = declared();
        let mut domain = UsageDomain::new();
        let unknown_model = unknown::<ModelId>();

        assert_eq!(
            domain.record_usage(&model, usage(adapter_id, unknown_model)),
            Err(RecordUsageError::UnknownModel(ModelNotFound {
                model_id: unknown_model
            }))
        );
        assert!(domain.records().is_empty());
        assert!(domain.events().is_empty());

        let event = domain
            .record_usage(&model, usage(adapter_id, model_id))
            .expect("declared");
        assert_eq!(
            event.usage_record_id.to_string(),
            "00000007-0000-4000-8000-000000000001",
            "the refusal took no identity"
        );
    }
}
