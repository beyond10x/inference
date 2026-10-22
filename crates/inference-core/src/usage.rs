//! The `inference.usage` vocabulary of `ess/domains/usage.yaml`: the usage of one model call.
//!
//! A [`UsageRecord`] says what one STT, LLM, TTS or native speech call consumed, who made it and
//! when: the adapter and model that served it, the run, node, session, speech session and
//! response it was made for where there is one, its start and end on the wall clock, whether the
//! provider marked its counts final, and its [`Quantity`] list. Each quantity names its
//! [`QuantityKind`], its [`QuantitySource`] and a value that is `None` exactly when the source is
//! [`QuantitySource::Unknown`]; [`Quantity::new`] and the deserializer refuse one whose value and
//! source disagree ([`QuantityMismatch`]), so an unknown quantity is never read as zero. A record
//! is created only through [`UsageRecord::record`], which returns it with the [`UsageRecorded`]
//! event an envelope carries under [`UsageRecorded::KIND`]. Serialized names are the ESS names;
//! the Rust field `r#final` is `final` on the wire.
//!
//! The contract crate of a call fills the adapter, model, timing and quantities
//! ([`RecordUsage::new`]); the node that places the record on a run fills the run, node and
//! session ([`RecordUsage::placed`]). A call inside a subgraph names the child run it was made in.
//!
//! A [`UsageTotal`] adds records up, kind by kind ([`KindTotal`]): the sum of the known values,
//! the number of unknown quantities, the number of records that carried the kind and how many of
//! those were partial, their `final` false; a kind is complete only when none of its quantities
//! was unknown and none of its records partial. An unknown quantity is counted as unknown, never
//! added as zero, and a partial count is counted, never taken for a final one. A total its holder
//! could not count whole names the nodes whose usage it may miss, and is not complete. The total
//! is a value its holder keeps, such as a session over its runs; the
//! specification declares no entity for it.
//!
//! A [`CostRecord`] prices one usage record with one price document: the document's currency,
//! revision and digest, one [`CostLine`] per quantity kind the record's adapter is charged for,
//! each an exact decimal amount as a string or an [`UnknownReason`], and a [`CostTotal`] of the
//! known subtotal, the unknown lines and a complete total that is none unless no line is unknown.
//! It is created only through [`CostRecord::record`], from the [`PriceUsage`] a pricing crate
//! computed, and returned with the [`CostRecorded`] event. This crate computes no price: the
//! record holds what the pricing crate wrote, as a usage record holds what its contract crate
//! counted.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::{NodeId, ResponseId, RunId, SessionId, SpeechSessionId, uuid_newtype};
use crate::model::{AdapterId, ModelId};

uuid_newtype!(
    /// `inference.usage.UsageRecordId`: identifies a [`UsageRecord`].
    UsageRecordId
);

/// `inference.usage.QuantityKind`: what a [`Quantity`] counts. Cached and cache-creation input
/// tokens are a subset of the input tokens, never added to them. Serialized as written in ESS
/// (`input_text_tokens`, `local_compute_ms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuantityKind {
    /// Text tokens the model read.
    InputTextTokens,
    /// Text tokens the model wrote.
    OutputTextTokens,
    /// Input tokens the provider served from its cache, counted inside the input tokens.
    CachedInputTokens,
    /// Input tokens the provider wrote to its cache, counted inside the input tokens.
    CacheCreationInputTokens,
    /// Audio tokens the model read.
    InputAudioTokens,
    /// Audio tokens the model wrote.
    OutputAudioTokens,
    /// Milliseconds of audio the model read.
    InputAudioMs,
    /// Milliseconds of audio the model wrote.
    OutputAudioMs,
    /// Characters of text the model read, as a synthesiser is metered.
    InputCharacters,
    /// Requests made to the provider.
    Requests,
    /// Milliseconds a local backend computed.
    LocalComputeMs,
}

impl QuantityKind {
    /// Every kind, in the order `ess/domains/usage.yaml` lists them.
    pub const ALL: [Self; 11] = [
        Self::InputTextTokens,
        Self::OutputTextTokens,
        Self::CachedInputTokens,
        Self::CacheCreationInputTokens,
        Self::InputAudioTokens,
        Self::OutputAudioTokens,
        Self::InputAudioMs,
        Self::OutputAudioMs,
        Self::InputCharacters,
        Self::Requests,
        Self::LocalComputeMs,
    ];
}

/// `inference.usage.QuantitySource`: who counted a [`Quantity`]. Serialized as written in ESS
/// (`provider_reported`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuantitySource {
    /// The provider reported it in its response.
    ProviderReported,
    /// The adapter measured it.
    Measured,
    /// Nobody counted it; its value is unknown, never zero.
    Unknown,
}

impl QuantitySource {
    /// Every source, in the order `ess/domains/usage.yaml` lists them.
    pub const ALL: [Self; 3] = [Self::ProviderReported, Self::Measured, Self::Unknown];
}

/// A [`Quantity`] whose value and source disagree: a value with [`QuantitySource::Unknown`], or
/// none with a source that counted. Nothing is built from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantityMismatch {
    pub kind: QuantityKind,
    pub source: QuantitySource,
    pub value: Option<i64>,
}

impl fmt::Display for QuantityMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.value {
            Some(value) => write!(
                f,
                "the {:?} quantity is {:?} and carries the value {value}; an unknown quantity has none",
                self.kind, self.source
            ),
            None => write!(
                f,
                "the {:?} quantity is {:?} and carries no value; only an unknown quantity has none",
                self.kind, self.source
            ),
        }
    }
}

impl Error for QuantityMismatch {}

/// `inference.usage.Quantity`: one counted quantity of a call. `value` is `None` exactly when
/// `source` is [`QuantitySource::Unknown`]; [`Quantity::new`] and the deserializer refuse
/// anything else. A field the specification does not declare is refused, and the wire carries a
/// `None` as `null` or leaves it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "QuantityWire")]
pub struct Quantity {
    kind: QuantityKind,
    source: QuantitySource,
    value: Option<i64>,
}

/// The wire form of a [`Quantity`], read before its value and source are checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuantityWire {
    kind: QuantityKind,
    source: QuantitySource,
    #[serde(default)]
    value: Option<i64>,
}

impl TryFrom<QuantityWire> for Quantity {
    type Error = QuantityMismatch;

    fn try_from(wire: QuantityWire) -> Result<Self, Self::Error> {
        Self::new(wire.kind, wire.source, wire.value)
    }
}

impl Quantity {
    /// The quantity `kind` counted by `source` as `value`.
    ///
    /// # Errors
    ///
    /// [`QuantityMismatch`] when `value` is `None` and `source` is not
    /// [`QuantitySource::Unknown`], or `value` is some and `source` is.
    pub fn new(
        kind: QuantityKind,
        source: QuantitySource,
        value: Option<i64>,
    ) -> Result<Self, QuantityMismatch> {
        if value.is_some() == (source == QuantitySource::Unknown) {
            return Err(QuantityMismatch {
                kind,
                source,
                value,
            });
        }
        Ok(Self {
            kind,
            source,
            value,
        })
    }

    /// The quantity `kind` the provider reported as `value`.
    #[must_use]
    pub const fn provider_reported(kind: QuantityKind, value: i64) -> Self {
        Self {
            kind,
            source: QuantitySource::ProviderReported,
            value: Some(value),
        }
    }

    /// The quantity `kind` the adapter measured as `value`.
    #[must_use]
    pub const fn measured(kind: QuantityKind, value: i64) -> Self {
        Self {
            kind,
            source: QuantitySource::Measured,
            value: Some(value),
        }
    }

    /// The quantity `kind` that nobody counted.
    #[must_use]
    pub const fn unknown(kind: QuantityKind) -> Self {
        Self {
            kind,
            source: QuantitySource::Unknown,
            value: None,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> QuantityKind {
        self.kind
    }

    #[must_use]
    pub const fn source(&self) -> QuantitySource {
        self.source
    }

    /// The count; `None` exactly when the source is [`QuantitySource::Unknown`].
    #[must_use]
    pub const fn value(&self) -> Option<i64> {
        self.value
    }
}

/// `inference.usage.RecordUsage`: the input that records the usage of one ended call. The
/// times are Unix epoch milliseconds on the `wall_time` timebase of
/// [`Timebase`](crate::envelope::Timebase). A field the specification does not declare is
/// refused; an `Optional` field may be left out or sent as `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordUsage {
    #[serde(default)]
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub node_id: Option<NodeId>,
    #[serde(default)]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub speech_session_id: Option<SpeechSessionId>,
    #[serde(default)]
    pub response_id: Option<ResponseId>,
    pub adapter_id: AdapterId,
    pub model_id: ModelId,
    /// The model the provider reported serving, which routing may make differ from `model_id`.
    #[serde(default)]
    pub upstream_model: Option<String>,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    /// False when the provider marked its counts partial.
    pub r#final: bool,
    pub quantities: Vec<Quantity>,
}

impl RecordUsage {
    /// The usage of a call the adapter `adapter_id` of the model `model_id` served from
    /// `started_at_ms` to `ended_at_ms`, its counts final and placed on no run, node, session,
    /// speech session or response yet; what a contract crate fills.
    #[must_use]
    pub const fn new(
        adapter_id: AdapterId,
        model_id: ModelId,
        started_at_ms: i64,
        ended_at_ms: i64,
        quantities: Vec<Quantity>,
    ) -> Self {
        Self {
            run_id: None,
            node_id: None,
            session_id: None,
            speech_session_id: None,
            response_id: None,
            adapter_id,
            model_id,
            upstream_model: None,
            started_at_ms,
            ended_at_ms,
            r#final: true,
            quantities,
        }
    }

    /// The same usage placed on the run `run_id` by its node `node_id`, in the session
    /// `session_id` if the run has one; what the node that places the record fills.
    #[must_use]
    pub const fn placed(
        mut self,
        run_id: RunId,
        node_id: NodeId,
        session_id: Option<SessionId>,
    ) -> Self {
        self.run_id = Some(run_id);
        self.node_id = Some(node_id);
        self.session_id = session_id;
        self
    }
}

/// `inference.usage.UsageRecorded`: the whole record of one call's usage, so that a meter or a
/// price document needs nothing else to read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageRecorded {
    pub usage_record_id: UsageRecordId,
    #[serde(default)]
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub node_id: Option<NodeId>,
    #[serde(default)]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub speech_session_id: Option<SpeechSessionId>,
    #[serde(default)]
    pub response_id: Option<ResponseId>,
    pub adapter_id: AdapterId,
    pub model_id: ModelId,
    #[serde(default)]
    pub upstream_model: Option<String>,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub r#final: bool,
    pub quantities: Vec<Quantity>,
}

impl UsageRecorded {
    /// The `kind` of an [`EventEnvelope`](crate::envelope::EventEnvelope) carrying this event: its
    /// declared name.
    pub const KIND: &'static str = "inference.usage.UsageRecorded";
}

/// `inference.usage.UsageRecord`: the usage of one model call. It references its adapter and
/// model, and optionally the run, node, session, speech session and response it was made for;
/// it owns nothing and nothing owns it. Its one state is `Declared`: it never changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageRecord {
    usage_record_id: UsageRecordId,
    run_id: Option<RunId>,
    node_id: Option<NodeId>,
    session_id: Option<SessionId>,
    speech_session_id: Option<SpeechSessionId>,
    response_id: Option<ResponseId>,
    adapter_id: AdapterId,
    model_id: ModelId,
    upstream_model: Option<String>,
    started_at_ms: i64,
    ended_at_ms: i64,
    r#final: bool,
    quantities: Vec<Quantity>,
}

impl UsageRecord {
    /// Runs `inference.usage.RecordUsage`: generates a [`UsageRecordId`] and returns the record
    /// with the input's fields and the [`UsageRecorded`] event.
    #[must_use]
    pub fn record(input: RecordUsage) -> (Self, UsageRecorded) {
        Self::record_as(UsageRecordId::generate(), input)
    }

    /// Runs `inference.usage.RecordUsage` under `usage_record_id` instead of a generated one, and
    /// otherwise as [`UsageRecord::record`]. It is for a store that assigns the identities
    /// itself, such as the counter of `inference-testkit`, whose runs must report the same
    /// identities each time.
    #[must_use]
    pub fn record_as(usage_record_id: UsageRecordId, input: RecordUsage) -> (Self, UsageRecorded) {
        let record = Self {
            usage_record_id,
            run_id: input.run_id,
            node_id: input.node_id,
            session_id: input.session_id,
            speech_session_id: input.speech_session_id,
            response_id: input.response_id,
            adapter_id: input.adapter_id,
            model_id: input.model_id,
            upstream_model: input.upstream_model,
            started_at_ms: input.started_at_ms,
            ended_at_ms: input.ended_at_ms,
            r#final: input.r#final,
            quantities: input.quantities,
        };
        let event = record.recorded();
        (record, event)
    }

    /// The [`UsageRecorded`] event that carries this record.
    #[must_use]
    pub fn recorded(&self) -> UsageRecorded {
        UsageRecorded {
            usage_record_id: self.usage_record_id,
            run_id: self.run_id,
            node_id: self.node_id,
            session_id: self.session_id,
            speech_session_id: self.speech_session_id,
            response_id: self.response_id,
            adapter_id: self.adapter_id,
            model_id: self.model_id,
            upstream_model: self.upstream_model.clone(),
            started_at_ms: self.started_at_ms,
            ended_at_ms: self.ended_at_ms,
            r#final: self.r#final,
            quantities: self.quantities.clone(),
        }
    }

    #[must_use]
    pub const fn usage_record_id(&self) -> UsageRecordId {
        self.usage_record_id
    }

    /// The run the call was made in; a call inside a subgraph names the child run.
    #[must_use]
    pub const fn run_id(&self) -> Option<RunId> {
        self.run_id
    }

    /// The node that made the call.
    #[must_use]
    pub const fn node_id(&self) -> Option<NodeId> {
        self.node_id
    }

    #[must_use]
    pub const fn session_id(&self) -> Option<SessionId> {
        self.session_id
    }

    #[must_use]
    pub const fn speech_session_id(&self) -> Option<SpeechSessionId> {
        self.speech_session_id
    }

    #[must_use]
    pub const fn response_id(&self) -> Option<ResponseId> {
        self.response_id
    }

    #[must_use]
    pub const fn adapter_id(&self) -> AdapterId {
        self.adapter_id
    }

    #[must_use]
    pub const fn model_id(&self) -> ModelId {
        self.model_id
    }

    /// The model the provider reported serving, if it reported one.
    #[must_use]
    pub fn upstream_model(&self) -> Option<&str> {
        self.upstream_model.as_deref()
    }

    /// When the call started, in Unix epoch milliseconds on the wall clock.
    #[must_use]
    pub const fn started_at_ms(&self) -> i64 {
        self.started_at_ms
    }

    /// When the call ended, in Unix epoch milliseconds on the wall clock.
    #[must_use]
    pub const fn ended_at_ms(&self) -> i64 {
        self.ended_at_ms
    }

    /// The ESS field `final`: false when the provider marked its counts partial.
    #[must_use]
    pub const fn is_final(&self) -> bool {
        self.r#final
    }

    #[must_use]
    pub fn quantities(&self) -> &[Quantity] {
        &self.quantities
    }
}

/// The total of one [`QuantityKind`] over the records a [`UsageTotal`] took.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KindTotal {
    /// The sum of the known values of the kind. Wider than a value, so that no sum overflows.
    pub known: i128,
    /// How many quantities of the kind were [`QuantitySource::Unknown`]: counted here, never
    /// added to `known`.
    pub unknown: u64,
    /// How many records carried a quantity of the kind.
    pub records: u64,
    /// How many of those records were partial, their `final` false: the provider may still
    /// change their counts, so `known` and `unknown` may change with them.
    pub partial: u64,
}

impl KindTotal {
    /// Whether `known` is the whole and settled total of the kind: no quantity of it was unknown
    /// and no record carrying it was partial.
    #[must_use]
    pub const fn complete(&self) -> bool {
        self.unknown == 0 && self.partial == 0
    }
}

/// The usage of several calls added up, kind by kind; see the module documentation. A kind no
/// record carried has no [`KindTotal`]. A total is complete only when every kind is: no quantity
/// unknown and no record partial, so a total over counts the provider may still change never
/// reads as settled; and only when it names no node whose usage its holder could not count
/// ([`UsageTotal::mark_uncounted`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageTotal {
    records: u64,
    kinds: BTreeMap<QuantityKind, KindTotal>,
    uncounted: BTreeSet<NodeId>,
}

impl UsageTotal {
    /// The total of no record.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the record whose quantities are `quantities` and whose ESS field `final` is
    /// `is_final`: each known value to its kind's sum, each unknown quantity to its kind's unknown
    /// count, and the record to the count of every kind it carries and, when it is not final, to
    /// the partial count of each.
    pub fn add(&mut self, quantities: &[Quantity], is_final: bool) {
        self.records += 1;
        let mut carried: Vec<QuantityKind> = Vec::with_capacity(quantities.len());
        for quantity in quantities {
            let total = self.kinds.entry(quantity.kind()).or_default();
            match quantity.value() {
                Some(value) => total.known += i128::from(value),
                None => total.unknown += 1,
            }
            if !carried.contains(&quantity.kind()) {
                carried.push(quantity.kind());
                total.records += 1;
                if !is_final {
                    total.partial += 1;
                }
            }
        }
    }

    /// How many records the total took.
    #[must_use]
    pub const fn records(&self) -> u64 {
        self.records
    }

    /// The total of `kind`; none when no record carried it.
    #[must_use]
    pub fn kind(&self, kind: QuantityKind) -> Option<KindTotal> {
        self.kinds.get(&kind).copied()
    }

    /// The total of every kind some record carried, in the order of [`QuantityKind::ALL`].
    pub fn kinds(&self) -> impl Iterator<Item = (QuantityKind, KindTotal)> + '_ {
        self.kinds.iter().map(|(kind, total)| (*kind, *total))
    }

    /// Marks the total partial: the usage of the node `node_id` may be missing from it, since its
    /// holder had no way to read it, such as a subgraph node with no `usage` port over a child
    /// whose model node's `usage` port has no edge. Marking a node twice marks it once.
    pub fn mark_uncounted(&mut self, node_id: NodeId) {
        self.uncounted.insert(node_id);
    }

    /// The nodes whose usage the total may miss ([`UsageTotal::mark_uncounted`]), in identity
    /// order.
    pub fn uncounted(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.uncounted.iter().copied()
    }

    /// Whether every kind is complete: no quantity the total took was unknown and no record it
    /// took was partial; and whether the total names no node whose usage it may miss.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.uncounted.is_empty() && self.kinds.values().all(KindTotal::complete)
    }
}

uuid_newtype!(
    /// `inference.usage.CostRecordId`: identifies a [`CostRecord`].
    CostRecordId
);

/// `inference.usage.UsageRecordNotFound`: no usage record carries the identity a cost record
/// named; nothing was priced. It is the error of `PriceUsage`'s `unknown-usage` outcome, which
/// only a store holding the usage records can decide; [`CostRecord::record`] never returns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageRecordNotFound {
    pub usage_record_id: UsageRecordId,
}

impl fmt::Display for UsageRecordNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no usage record carries the identity {}",
            self.usage_record_id
        )
    }
}

impl Error for UsageRecordNotFound {}

/// `inference.usage.UnknownReason`: why a [`CostLine`] has no amount, or only a lower bound.
/// Serialized as written in ESS (`price_unknown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    /// The quantity was not counted.
    QuantityUnknown,
    /// The price names no rate for the quantity.
    RateUnknown,
    /// No price covers the kind, or the adapter has no price at all.
    PriceUnknown,
    /// The price's binding is not the one the call was made over.
    BindingMismatch,
    /// The provider reported no upstream model, so no model price can be matched.
    ModelUnknown,
    /// The upstream model the provider reported is not the one the price is for.
    ModelMismatch,
    /// A recorded charge carried no amount.
    ChargeUnknown,
    /// The provider marked the counts partial: the amount is a lower bound.
    UsageIncomplete,
}

impl UnknownReason {
    /// Every reason, in the order `ess/domains/usage.yaml` lists them.
    pub const ALL: [Self; 8] = [
        Self::QuantityUnknown,
        Self::RateUnknown,
        Self::PriceUnknown,
        Self::BindingMismatch,
        Self::ModelUnknown,
        Self::ModelMismatch,
        Self::ChargeUnknown,
        Self::UsageIncomplete,
    ];
}

/// `inference.usage.CostLine`: one charged quantity kind priced. `quantity` is the count priced,
/// none when unknown; `amount` an exact decimal string in the record's currency, never a float,
/// none when it cannot be known; `unknown` why the amount is none, or why it is only a lower
/// bound. A field the specification does not declare is refused; an `Optional` field may be left
/// out or sent as `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostLine {
    pub kind: QuantityKind,
    #[serde(default)]
    pub quantity: Option<i64>,
    #[serde(default)]
    pub amount: Option<String>,
    #[serde(default)]
    pub unknown: Option<UnknownReason>,
}

/// `inference.usage.CostTotal`: the lines of a [`CostRecord`] added up. `known_subtotal` is the
/// sum of the known amounts, `unknown_lines` the number of lines whose amount is none or only a
/// lower bound, and `complete_total` the whole, none unless no line is unknown. Amounts are exact
/// decimal strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostTotal {
    pub known_subtotal: String,
    pub unknown_lines: u64,
    #[serde(default)]
    pub complete_total: Option<String>,
}

/// `inference.usage.PriceUsage`: the input that records the cost of the usage record
/// `usage_record_id` under one price document, as the pricing crate computed it. A field the
/// specification does not declare is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceUsage {
    pub usage_record_id: UsageRecordId,
    /// The three-letter code of the price document's currency.
    pub currency: String,
    pub price_revision: String,
    /// The content digest of the price document.
    pub price_digest: String,
    pub lines: Vec<CostLine>,
    pub total: CostTotal,
}

/// `inference.usage.CostRecorded`: the whole cost record, so that a meter needs nothing else to
/// read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostRecorded {
    pub cost_record_id: CostRecordId,
    pub usage_record_id: UsageRecordId,
    pub currency: String,
    pub price_revision: String,
    pub price_digest: String,
    pub lines: Vec<CostLine>,
    pub total: CostTotal,
}

impl CostRecorded {
    /// The `kind` of an [`EventEnvelope`](crate::envelope::EventEnvelope) carrying this event: its
    /// declared name.
    pub const KIND: &'static str = "inference.usage.CostRecorded";
}

/// `inference.usage.CostRecord`: one usage record priced with one price document. It references
/// the usage record; a usage record has one cost record per price document it is priced with.
/// Its one state is `Declared`: a new price document makes a new record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CostRecord {
    cost_record_id: CostRecordId,
    usage_record_id: UsageRecordId,
    currency: String,
    price_revision: String,
    price_digest: String,
    lines: Vec<CostLine>,
    total: CostTotal,
}

impl CostRecord {
    /// Runs `inference.usage.PriceUsage`: generates a [`CostRecordId`] and returns the record
    /// with the input's fields and the [`CostRecorded`] event.
    #[must_use]
    pub fn record(input: PriceUsage) -> (Self, CostRecorded) {
        Self::record_as(CostRecordId::generate(), input)
    }

    /// Runs `inference.usage.PriceUsage` under `cost_record_id` instead of a generated one, and
    /// otherwise as [`CostRecord::record`]; for a store that assigns the identities itself.
    #[must_use]
    pub fn record_as(cost_record_id: CostRecordId, input: PriceUsage) -> (Self, CostRecorded) {
        let record = Self {
            cost_record_id,
            usage_record_id: input.usage_record_id,
            currency: input.currency,
            price_revision: input.price_revision,
            price_digest: input.price_digest,
            lines: input.lines,
            total: input.total,
        };
        let event = record.recorded();
        (record, event)
    }

    /// The [`CostRecorded`] event that carries this record.
    #[must_use]
    pub fn recorded(&self) -> CostRecorded {
        CostRecorded {
            cost_record_id: self.cost_record_id,
            usage_record_id: self.usage_record_id,
            currency: self.currency.clone(),
            price_revision: self.price_revision.clone(),
            price_digest: self.price_digest.clone(),
            lines: self.lines.clone(),
            total: self.total.clone(),
        }
    }

    #[must_use]
    pub const fn cost_record_id(&self) -> CostRecordId {
        self.cost_record_id
    }

    /// The usage record priced.
    #[must_use]
    pub const fn usage_record_id(&self) -> UsageRecordId {
        self.usage_record_id
    }

    #[must_use]
    pub fn currency(&self) -> &str {
        &self.currency
    }

    #[must_use]
    pub fn price_revision(&self) -> &str {
        &self.price_revision
    }

    #[must_use]
    pub fn price_digest(&self) -> &str {
        &self.price_digest
    }

    #[must_use]
    pub fn lines(&self) -> &[CostLine] {
        &self.lines
    }

    #[must_use]
    pub const fn total(&self) -> &CostTotal {
        &self.total
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        CostLine, CostRecord, CostRecordId, CostRecorded, CostTotal, KindTotal, PriceUsage,
        Quantity, QuantityKind, QuantityMismatch, QuantitySource, RecordUsage, UnknownReason,
        UsageRecord, UsageRecordId, UsageRecorded, UsageTotal,
    };
    use crate::id::{NodeId, RunId, SessionId};
    use crate::model::{AdapterId, ModelId};

    fn input() -> RecordUsage {
        RecordUsage::new(
            AdapterId::from(Uuid::from_u128(1)),
            ModelId::from(Uuid::from_u128(2)),
            1_000,
            1_250,
            vec![
                Quantity::provider_reported(QuantityKind::InputTextTokens, 12),
                Quantity::measured(QuantityKind::LocalComputeMs, 250),
                Quantity::unknown(QuantityKind::CachedInputTokens),
            ],
        )
    }

    #[test]
    fn a_value_with_an_unknown_source_is_refused() {
        assert_eq!(
            Quantity::new(
                QuantityKind::InputTextTokens,
                QuantitySource::Unknown,
                Some(0)
            ),
            Err(QuantityMismatch {
                kind: QuantityKind::InputTextTokens,
                source: QuantitySource::Unknown,
                value: Some(0),
            })
        );
    }

    #[test]
    fn no_value_with_a_counting_source_is_refused() {
        for source in [QuantitySource::ProviderReported, QuantitySource::Measured] {
            assert_eq!(
                Quantity::new(QuantityKind::Requests, source, None),
                Err(QuantityMismatch {
                    kind: QuantityKind::Requests,
                    source,
                    value: None,
                }),
                "{source:?}"
            );
        }
    }

    #[test]
    fn a_value_and_source_that_agree_are_accepted() {
        assert_eq!(
            Quantity::new(
                QuantityKind::InputAudioMs,
                QuantitySource::Measured,
                Some(40)
            ),
            Ok(Quantity::measured(QuantityKind::InputAudioMs, 40))
        );
        assert_eq!(
            Quantity::new(QuantityKind::InputAudioMs, QuantitySource::Unknown, None),
            Ok(Quantity::unknown(QuantityKind::InputAudioMs))
        );
    }

    #[test]
    fn the_wire_refuses_a_disagreeing_quantity_and_an_undeclared_field() {
        let mismatched = serde_json::from_value::<Quantity>(
            json!({"kind": "requests", "source": "unknown", "value": 1}),
        );
        assert!(mismatched.is_err(), "{mismatched:?}");
        let missing =
            serde_json::from_value::<Quantity>(json!({"kind": "requests", "source": "measured"}));
        assert!(missing.is_err(), "{missing:?}");
        let extra = serde_json::from_value::<Quantity>(
            json!({"kind": "requests", "source": "unknown", "unit": "x"}),
        );
        assert!(extra.is_err(), "{extra:?}");
        assert_eq!(
            serde_json::from_value::<Quantity>(json!({"kind": "requests", "source": "unknown"}))
                .expect("an unknown quantity without a value"),
            Quantity::unknown(QuantityKind::Requests)
        );
    }

    #[test]
    fn a_record_usage_with_a_disagreeing_quantity_is_not_read() {
        let mut wire = serde_json::to_value(input()).expect("serializes");
        wire["quantities"][0]["value"] = serde_json::Value::Null;
        let read = serde_json::from_value::<RecordUsage>(wire);
        assert!(read.is_err(), "{read:?}");
    }

    #[test]
    fn serialized_names_are_the_ess_names() {
        assert_eq!(
            serde_json::to_value(QuantityKind::ALL).expect("serializes"),
            json!([
                "input_text_tokens",
                "output_text_tokens",
                "cached_input_tokens",
                "cache_creation_input_tokens",
                "input_audio_tokens",
                "output_audio_tokens",
                "input_audio_ms",
                "output_audio_ms",
                "input_characters",
                "requests",
                "local_compute_ms"
            ])
        );
        assert_eq!(
            serde_json::to_value(QuantitySource::ALL).expect("serializes"),
            json!(["provider_reported", "measured", "unknown"])
        );
        let wire = serde_json::to_value(input()).expect("serializes");
        assert_eq!(wire["final"], json!(true));
        assert_eq!(
            wire["quantities"][2],
            json!({"kind": "cached_input_tokens", "source": "unknown", "value": null})
        );
        assert_eq!(UsageRecorded::KIND, "inference.usage.UsageRecorded");
    }

    #[test]
    fn a_record_carries_its_input_and_its_event_carries_the_record() {
        let run_id = RunId::from(Uuid::from_u128(3));
        let node_id = NodeId::from(Uuid::from_u128(4));
        let session_id = SessionId::from(Uuid::from_u128(5));
        let mut usage = input().placed(run_id, node_id, Some(session_id));
        usage.r#final = false;
        usage.upstream_model = Some("served".to_owned());
        let id = UsageRecordId::from(Uuid::from_u128(6));

        let (record, event) = UsageRecord::record_as(id, usage.clone());

        assert_eq!(record.usage_record_id(), id);
        assert_eq!(record.run_id(), Some(run_id));
        assert_eq!(record.node_id(), Some(node_id));
        assert_eq!(record.session_id(), Some(session_id));
        assert_eq!(record.speech_session_id(), None);
        assert_eq!(record.response_id(), None);
        assert_eq!(record.adapter_id(), usage.adapter_id);
        assert_eq!(record.model_id(), usage.model_id);
        assert_eq!(record.upstream_model(), Some("served"));
        assert_eq!(record.started_at_ms(), 1_000);
        assert_eq!(record.ended_at_ms(), 1_250);
        assert!(!record.is_final());
        assert_eq!(record.quantities(), usage.quantities.as_slice());
        assert_eq!(event, record.recorded());
        assert_eq!(event.usage_record_id, id);
        assert_eq!(event.quantities, usage.quantities);

        let read: UsageRecorded =
            serde_json::from_value(serde_json::to_value(&event).expect("serializes"))
                .expect("the event reads back");
        assert_eq!(read, event);
    }

    #[test]
    fn a_total_marked_uncounted_names_the_node_once_and_is_not_complete() {
        let mut total = UsageTotal::new();
        total.add(
            &[Quantity::provider_reported(
                QuantityKind::InputTextTokens,
                3,
            )],
            true,
        );
        assert!(total.complete());
        let node = NodeId::generate();
        total.mark_uncounted(node);
        total.mark_uncounted(node);
        assert_eq!(total.uncounted().collect::<Vec<_>>(), [node]);
        assert!(!total.complete(), "a total that may miss a node's usage");
        assert!(
            total
                .kind(QuantityKind::InputTextTokens)
                .is_some_and(|kind| kind.complete()),
            "what it counted stays as counted"
        );
    }

    #[test]
    fn a_total_sums_known_values_and_counts_unknowns_apart_from_zero() {
        let mut total = UsageTotal::new();
        assert_eq!(total.records(), 0);
        assert!(total.complete(), "no record is a complete total of nothing");
        assert_eq!(total.kind(QuantityKind::Requests), None);

        total.add(
            &[
                Quantity::provider_reported(QuantityKind::InputTextTokens, 12),
                Quantity::measured(QuantityKind::Requests, 1),
                Quantity::unknown(QuantityKind::LocalComputeMs),
            ],
            true,
        );
        total.add(
            &[
                Quantity::unknown(QuantityKind::InputTextTokens),
                Quantity::measured(QuantityKind::Requests, 1),
                Quantity::measured(QuantityKind::LocalComputeMs, 0),
            ],
            true,
        );
        total.add(
            &[Quantity::provider_reported(
                QuantityKind::InputTextTokens,
                30,
            )],
            true,
        );

        assert_eq!(total.records(), 3);
        assert_eq!(
            total.kind(QuantityKind::InputTextTokens),
            Some(KindTotal {
                known: 42,
                unknown: 1,
                records: 3,
                partial: 0,
            }),
            "the unknown count of the second record is not added as zero"
        );
        assert_eq!(
            total.kind(QuantityKind::Requests),
            Some(KindTotal {
                known: 2,
                unknown: 0,
                records: 2,
                partial: 0,
            })
        );
        assert_eq!(
            total.kind(QuantityKind::LocalComputeMs),
            Some(KindTotal {
                known: 0,
                unknown: 1,
                records: 2,
                partial: 0,
            }),
            "a measured zero is known; the unknown beside it stays unknown"
        );
        assert_eq!(total.kind(QuantityKind::OutputTextTokens), None);
        assert!(
            !total
                .kind(QuantityKind::InputTextTokens)
                .expect("carried")
                .complete()
        );
        assert!(
            total
                .kind(QuantityKind::Requests)
                .expect("carried")
                .complete()
        );
        assert!(!total.complete());
        assert_eq!(
            total.kinds().map(|(kind, _)| kind).collect::<Vec<_>>(),
            [
                QuantityKind::InputTextTokens,
                QuantityKind::Requests,
                QuantityKind::LocalComputeMs
            ],
            "kinds in the order the specification lists them"
        );
    }

    #[test]
    fn a_total_of_unknowns_only_knows_nothing_and_a_sum_past_i64_does_not_overflow() {
        let mut unknown = UsageTotal::new();
        unknown.add(&[Quantity::unknown(QuantityKind::OutputAudioMs)], true);
        unknown.add(&[Quantity::unknown(QuantityKind::OutputAudioMs)], true);
        assert_eq!(
            unknown.kind(QuantityKind::OutputAudioMs),
            Some(KindTotal {
                known: 0,
                unknown: 2,
                records: 2,
                partial: 0,
            })
        );
        assert!(!unknown.complete());

        let mut large = UsageTotal::new();
        large.add(
            &[Quantity::measured(QuantityKind::InputAudioMs, i64::MAX)],
            true,
        );
        large.add(
            &[Quantity::measured(QuantityKind::InputAudioMs, i64::MAX)],
            true,
        );
        assert_eq!(
            large
                .kind(QuantityKind::InputAudioMs)
                .map(|kind| kind.known),
            Some(2 * i128::from(i64::MAX))
        );
        assert!(large.complete());
    }

    #[test]
    fn a_partial_record_keeps_its_kinds_incomplete_however_well_they_are_counted() {
        let mut total = UsageTotal::new();
        total.add(
            &[
                Quantity::provider_reported(QuantityKind::OutputTextTokens, 9),
                Quantity::measured(QuantityKind::Requests, 1),
            ],
            false,
        );
        total.add(&[Quantity::measured(QuantityKind::Requests, 1)], true);

        assert_eq!(
            total.kind(QuantityKind::OutputTextTokens),
            Some(KindTotal {
                known: 9,
                unknown: 0,
                records: 1,
                partial: 1,
            }),
            "every count known, but the provider marked it partial"
        );
        assert_eq!(
            total.kind(QuantityKind::Requests),
            Some(KindTotal {
                known: 2,
                unknown: 0,
                records: 2,
                partial: 1,
            })
        );
        assert!(
            !total
                .kind(QuantityKind::OutputTextTokens)
                .expect("carried")
                .complete()
        );
        assert!(
            !total.complete(),
            "a total over partial counts is not complete"
        );

        let mut final_only = UsageTotal::new();
        final_only.add(&[Quantity::measured(QuantityKind::Requests, 1)], true);
        assert!(final_only.complete());
    }

    fn price_usage() -> PriceUsage {
        PriceUsage {
            usage_record_id: UsageRecordId::from(Uuid::from_u128(6)),
            currency: "EUR".to_owned(),
            price_revision: "2026-10".to_owned(),
            price_digest: "prices-sha256:00".to_owned(),
            lines: vec![
                CostLine {
                    kind: QuantityKind::OutputTextTokens,
                    quantity: Some(9),
                    amount: Some("0.000018".to_owned()),
                    unknown: None,
                },
                CostLine {
                    kind: QuantityKind::InputAudioMs,
                    quantity: Some(300),
                    amount: None,
                    unknown: Some(UnknownReason::PriceUnknown),
                },
            ],
            total: CostTotal {
                known_subtotal: "0.000018".to_owned(),
                unknown_lines: 1,
                complete_total: None,
            },
        }
    }

    #[test]
    fn a_cost_record_carries_its_input_and_its_event_carries_the_record() {
        let id = CostRecordId::from(Uuid::from_u128(7));
        let (record, event) = CostRecord::record_as(id, price_usage());

        assert_eq!(record.cost_record_id(), id);
        assert_eq!(record.usage_record_id(), price_usage().usage_record_id);
        assert_eq!(record.currency(), "EUR");
        assert_eq!(record.price_revision(), "2026-10");
        assert_eq!(record.price_digest(), "prices-sha256:00");
        assert_eq!(record.lines(), price_usage().lines.as_slice());
        assert_eq!(record.total(), &price_usage().total);
        assert_eq!(event, record.recorded());
        assert_eq!(event.cost_record_id, id);
        let read: CostRecorded =
            serde_json::from_value(serde_json::to_value(&event).expect("serializes"))
                .expect("the event reads back");
        assert_eq!(read, event);
        assert_ne!(
            CostRecord::record(price_usage()).1.cost_record_id,
            CostRecord::record(price_usage()).1.cost_record_id,
            "a generated identity per record"
        );
    }

    #[test]
    fn cost_names_are_the_ess_names_and_amounts_are_strings() {
        assert_eq!(
            serde_json::to_value(UnknownReason::ALL).expect("serializes"),
            json!([
                "quantity_unknown",
                "rate_unknown",
                "price_unknown",
                "binding_mismatch",
                "model_unknown",
                "model_mismatch",
                "charge_unknown",
                "usage_incomplete"
            ])
        );
        let wire = serde_json::to_value(price_usage()).expect("serializes");
        assert_eq!(
            wire["lines"][0],
            json!({"kind": "output_text_tokens", "quantity": 9, "amount": "0.000018", "unknown": null})
        );
        assert_eq!(
            wire["total"],
            json!({"known_subtotal": "0.000018", "unknown_lines": 1, "complete_total": null})
        );
        assert_eq!(CostRecorded::KIND, "inference.usage.CostRecorded");
    }

    #[test]
    fn a_cost_line_and_total_read_without_their_optional_fields_and_refuse_undeclared_ones() {
        assert_eq!(
            serde_json::from_value::<CostLine>(json!({"kind": "requests"})).expect("read"),
            CostLine {
                kind: QuantityKind::Requests,
                quantity: None,
                amount: None,
                unknown: None,
            }
        );
        assert_eq!(
            serde_json::from_value::<CostTotal>(json!({"known_subtotal": "0", "unknown_lines": 0}))
                .expect("read"),
            CostTotal {
                known_subtotal: "0".to_owned(),
                unknown_lines: 0,
                complete_total: None,
            }
        );
        let extra =
            serde_json::from_value::<CostLine>(json!({"kind": "requests", "unit": "request"}));
        assert!(extra.is_err(), "{extra:?}");
        let float = serde_json::from_value::<CostLine>(json!({"kind": "requests", "amount": 0.5}));
        assert!(float.is_err(), "an amount is never a float: {float:?}");
    }

    #[test]
    fn a_contract_crate_places_nothing_and_counts_are_final() {
        let usage = input();
        assert_eq!(usage.run_id, None);
        assert_eq!(usage.node_id, None);
        assert_eq!(usage.session_id, None);
        assert!(usage.r#final);
    }
}
