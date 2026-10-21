//! What usage costs, behind the feature `cost`: one [`CostRecorded`] per usage record and a
//! session cost total, priced with a price document the caller supplies.
//!
//! The prices are `llm-cost`'s: a [`PriceBook`] over a versioned price document (revision,
//! source, as-of time, currency, digest), exact decimal amounts and unknown-preserving
//! arithmetic. No provider price is looked up and no currency converted. [`Pricing`] holds the
//! book and a [`PriceBinding`] per adapter, which names the quantity kinds the adapter is charged
//! for and the price that charges them: a model price ([`ModelPriceBinding`]) for the four token
//! kinds, a compute price ([`ComputePriceBinding`]) for `local_compute_ms`.
//!
//! [`price`] prices one `UsageRecorded`, with or without a session, so a duplex caller prices
//! its records too:
//!
//! | the record's adapter | its lines |
//! |---|---|
//! | has no binding | one `price_unknown` line per quantity the record carries, or one `requests` line of no quantity when it carries none: priced as unknown, never skipped and never zero |
//! | is bound | one line per charged kind, in the order of [`QuantityKind::ALL`]; a kind it does not charge has none |
//!
//! A charged token kind is priced by `PriceBook::quote` over an `AttemptUsage` of the record's
//! counts, upstream model and `final`, under the binding's model price; `local_compute_ms` over a
//! `ComputeUsage` under its compute price. A missing quantity, rate, price, upstream model or
//! binding match stays unknown with `llm-cost`'s reason, and the counts of a record whose `final`
//! is false, its compute among them, give a lower bound marked `usage_incomplete`. The input line counts the input tokens neither
//! read from nor written to the cache, as `llm-cost` prices them, which a cache split known only
//! in part leaves unknown. A charged kind `llm-cost` has no unit for (audio tokens, audio
//! milliseconds, characters, requests), or a charged kind with no price of its family in the
//! binding, gets a `price_unknown` line. Quantities of one kind are added up, each checked first:
//! one unknown or negative value, or a sum beyond `i64`, makes the kind's count unknown, never
//! netted.
//!
//! A live session priced with [`crate::live::LiveSession::price_with`] prices every usage record
//! it reads with the pricing current then, and the records it read before any pricing at that
//! call, and adds them up in [`SessionCost`], beside its usage totals ([`crate::usage`]). Its
//! currency is that of the first document; a document in another currency is refused
//! ([`CurrencyMismatch`]).

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use inference_core::model::AdapterId;
use inference_core::usage::{
    CostLine, CostRecord, CostRecorded, CostTotal, PriceUsage, QuantityKind, UnknownReason,
    UsageRecordId, UsageRecorded,
};
pub use llm_core::{BillingKind, Id};
use llm_core::{Dispatch, Protocol, Provenance, Usage};
use llm_cost::{AccountingInput, Amount, AttemptUsage, ComputeUsage, Line, Observation, Unit};
pub use llm_cost::{CostError, PriceBook};

/// The model price an adapter's token kinds are charged under: the price's identity in the
/// document, the serving model and binding revision the price must match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPriceBinding {
    pub price_id: Id,
    pub serving_model_id: Id,
    pub binding_revision: Id,
    /// Metered use is priced as an estimate; subscription or self-hosted use as a reference
    /// valuation of the same work.
    pub billing: BillingKind,
}

/// The compute price an adapter's `local_compute_ms` is charged under: the price's identity in
/// the document and the hosting provider, account and resource class it must match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputePriceBinding {
    pub price_id: Id,
    pub hosting_provider: Id,
    pub account: Id,
    pub resource_class: Id,
}

/// How one adapter's usage is priced: the kinds it is charged for and the prices that charge
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PriceBinding {
    /// The quantity kinds the adapter is charged for; a kind not named gets no line.
    pub charges: Vec<QuantityKind>,
    pub model: Option<ModelPriceBinding>,
    pub compute: Option<ComputePriceBinding>,
}

/// A price book and the binding of each adapter priced with it.
#[derive(Debug, Clone)]
pub struct Pricing {
    book: PriceBook,
    bindings: BTreeMap<AdapterId, PriceBinding>,
}

impl Pricing {
    /// Prices with `book`, no adapter bound yet.
    #[must_use]
    pub const fn new(book: PriceBook) -> Self {
        Self {
            book,
            bindings: BTreeMap::new(),
        }
    }

    /// The same pricing, the adapter `adapter_id` priced under `binding`.
    #[must_use]
    pub fn bind(mut self, adapter_id: AdapterId, binding: PriceBinding) -> Self {
        self.bindings.insert(adapter_id, binding);
        self
    }

    #[must_use]
    pub const fn book(&self) -> &PriceBook {
        &self.book
    }

    /// The three-letter code of the price document's currency.
    #[must_use]
    pub fn currency(&self) -> &str {
        self.book.document().currency.as_str()
    }
}

/// Why a usage record has no cost record: `llm-cost` refused its counts, such as cache counts
/// beyond the input tokens, or an amount overflowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceError {
    Cost(CostError),
}

impl fmt::Display for PriceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cost(error) => write!(f, "the usage record cannot be priced: {error}"),
        }
    }
}

impl Error for PriceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Cost(error) => Some(error),
        }
    }
}

impl From<CostError> for PriceError {
    fn from(error: CostError) -> Self {
        Self::Cost(error)
    }
}

/// Prices the usage record `recorded` with `pricing`, as the module documentation says, and
/// returns its cost record as `inference.usage.CostRecorded`.
///
/// # Errors
///
/// [`PriceError::Cost`] when `llm-cost` refuses the record's counts or an amount overflows.
pub fn price(pricing: &Pricing, recorded: &UsageRecorded) -> Result<CostRecorded, PriceError> {
    let lines = match pricing.bindings.get(&recorded.adapter_id) {
        None if recorded.quantities.is_empty() => {
            vec![price_unknown(QuantityKind::Requests, None)]
        }
        None => recorded
            .quantities
            .iter()
            .map(|quantity| price_unknown(quantity.kind(), quantity.value()))
            .collect(),
        Some(binding) => bound_lines(&pricing.book, binding, recorded)?,
    };
    let document = pricing.book.document();
    let input = PriceUsage {
        usage_record_id: recorded.usage_record_id,
        currency: document.currency.as_str().to_owned(),
        price_revision: document.revision.as_str().to_owned(),
        price_digest: pricing.book.digest().as_str().to_owned(),
        total: Sum::of(&lines)?.total(),
        lines,
    };
    Ok(CostRecord::record(input).1)
}

/// The line of a charged kind no price covers.
const fn price_unknown(kind: QuantityKind, quantity: Option<i64>) -> CostLine {
    CostLine {
        kind,
        quantity,
        amount: None,
        unknown: Some(UnknownReason::PriceUnknown),
    }
}

/// The token kinds a model price charges, and the `llm-cost` unit of each.
const TOKEN_UNITS: [(QuantityKind, Unit); 4] = [
    (QuantityKind::InputTextTokens, Unit::InputToken),
    (QuantityKind::OutputTextTokens, Unit::OutputToken),
    (QuantityKind::CachedInputTokens, Unit::CachedInputToken),
    (
        QuantityKind::CacheCreationInputTokens,
        Unit::CacheCreationInputToken,
    ),
];

/// The lines of `recorded` under `binding`: one per charged kind, in the order of
/// [`QuantityKind::ALL`].
fn bound_lines(
    book: &PriceBook,
    binding: &PriceBinding,
    recorded: &UsageRecorded,
) -> Result<Vec<CostLine>, PriceError> {
    let charged = |kind: &QuantityKind| binding.charges.contains(kind);
    let tokens = match &binding.model {
        Some(model) if TOKEN_UNITS.iter().any(|(kind, _)| charged(kind)) => {
            quote(book, Observation::Attempt(attempt(model, recorded)))?
        }
        _ => Vec::new(),
    };
    let compute = match &binding.compute {
        Some(compute) if charged(&QuantityKind::LocalComputeMs) => {
            let mut lines = quote(book, Observation::Compute(compute_usage(compute, recorded)))?;
            // `llm-cost` reads no finality of a compute observation; a partial record's count is
            // a lower bound, marked as the token lines of a partial attempt are.
            for line in &mut lines {
                if !recorded.r#final && line.unknown.is_none() {
                    line.unknown = Some(llm_cost::UnknownReason::UsageIncomplete);
                }
            }
            lines
        }
        _ => Vec::new(),
    };
    QuantityKind::ALL
        .into_iter()
        .filter(charged)
        .map(|kind| {
            let unit = TOKEN_UNITS
                .iter()
                .find(|(token, _)| *token == kind)
                .map(|(_, unit)| *unit)
                .or_else(|| {
                    (kind == QuantityKind::LocalComputeMs).then_some(Unit::ResourceMillisecond)
                });
            let quoted =
                unit.and_then(|unit| tokens.iter().chain(&compute).find(|line| line.unit == unit));
            match quoted {
                Some(line) => cost_line(kind, line),
                None => Ok(price_unknown(kind, count(recorded, kind))),
            }
        })
        .collect()
}

/// The lines `llm-cost` quotes for `observation` alone.
fn quote(book: &PriceBook, observation: Observation) -> Result<Vec<Line>, PriceError> {
    let report = book.quote(&AccountingInput::new(vec![observation]))?;
    Ok(report
        .records
        .into_iter()
        .flat_map(|record| record.lines)
        .collect())
}

/// `llm-cost`'s line of `kind` as an `inference.usage.CostLine`.
fn cost_line(kind: QuantityKind, line: &Line) -> Result<CostLine, PriceError> {
    Ok(CostLine {
        kind,
        quantity: line
            .quantity
            .map(i64::try_from)
            .transpose()
            .map_err(|_| CostError::Overflow)?,
        amount: line.amount.map(|amount| amount.to_string()),
        unknown: line.unknown.map(reason),
    })
}

const fn reason(reason: llm_cost::UnknownReason) -> UnknownReason {
    match reason {
        llm_cost::UnknownReason::QuantityUnknown => UnknownReason::QuantityUnknown,
        llm_cost::UnknownReason::RateUnknown => UnknownReason::RateUnknown,
        llm_cost::UnknownReason::PriceUnknown => UnknownReason::PriceUnknown,
        llm_cost::UnknownReason::BindingMismatch => UnknownReason::BindingMismatch,
        llm_cost::UnknownReason::ModelUnknown => UnknownReason::ModelUnknown,
        llm_cost::UnknownReason::ModelMismatch => UnknownReason::ModelMismatch,
        llm_cost::UnknownReason::ChargeUnknown => UnknownReason::ChargeUnknown,
        llm_cost::UnknownReason::UsageIncomplete => UnknownReason::UsageIncomplete,
    }
}

/// The value of `kind` in `recorded`: the sum of its quantities of that kind, none when it
/// carries none, one of them is unknown or negative, or the sum overflows. Every value is checked
/// before any is added, so a negative count is never netted against another.
fn count(recorded: &UsageRecorded, kind: QuantityKind) -> Option<i64> {
    let values = recorded
        .quantities
        .iter()
        .filter(|quantity| quantity.kind() == kind)
        .map(|quantity| quantity.value().filter(|value| *value >= 0))
        .collect::<Option<Vec<i64>>>()?;
    if values.is_empty() {
        return None;
    }
    values
        .into_iter()
        .try_fold(0_i64, |sum, value| sum.checked_add(value))
}

/// [`count`] as `llm-cost` counts it.
fn unsigned(recorded: &UsageRecorded, kind: QuantityKind) -> Option<u64> {
    count(recorded, kind).and_then(|value| u64::try_from(value).ok())
}

/// An identity of this crate as an identifier `llm-cost` takes: a Uuid is printable ASCII.
fn identity(value: impl fmt::Display) -> Id {
    Id::new(value.to_string()).expect("a Uuid is 36 printable ASCII bytes")
}

/// The observation of one model call under `model`. Pricing reads the binding revision of its
/// provenance and nothing else of it; the rest names the adapter, as this crate knows no
/// protocol, provider, account or endpoint of the call.
fn attempt(model: &ModelPriceBinding, recorded: &UsageRecorded) -> AttemptUsage {
    let call = identity(recorded.usage_record_id);
    let adapter = identity(recorded.adapter_id);
    AttemptUsage {
        id: call.clone(),
        usage_record_id: call.clone(),
        request_id: call,
        route_target_id: adapter.clone(),
        serving_model_id: model.serving_model_id.clone(),
        price_id: model.price_id.clone(),
        binding: Provenance {
            protocol: Protocol::Responses,
            provider: adapter.clone(),
            account: adapter.clone(),
            endpoint: adapter,
            model: model.serving_model_id.clone(),
            binding_revision: model.binding_revision.clone(),
        },
        upstream_model: recorded
            .upstream_model
            .as_deref()
            .and_then(|upstream| Id::new(upstream).ok()),
        billing: model.billing,
        dispatch: Dispatch::Accepted,
        failed: false,
        final_usage: recorded.r#final,
        usage: Usage {
            input_tokens: unsigned(recorded, QuantityKind::InputTextTokens),
            output_tokens: unsigned(recorded, QuantityKind::OutputTextTokens),
            cached_input_tokens: unsigned(recorded, QuantityKind::CachedInputTokens),
            cache_creation_input_tokens: unsigned(recorded, QuantityKind::CacheCreationInputTokens),
            reasoning_output_tokens: None,
        },
    }
}

/// The local computation of one call under `compute`: the resource is the adapter.
fn compute_usage(compute: &ComputePriceBinding, recorded: &UsageRecorded) -> ComputeUsage {
    let adapter = identity(recorded.adapter_id);
    ComputeUsage {
        id: identity(recorded.usage_record_id),
        price_id: compute.price_id.clone(),
        hosting_provider: compute.hosting_provider.clone(),
        account: compute.account.clone(),
        resource_class: compute.resource_class.clone(),
        resource_id: adapter,
        duration_ms: unsigned(recorded, QuantityKind::LocalComputeMs),
    }
}

/// Lines added up as `llm-cost` adds them: a known amount to the subtotal, a line with no amount
/// or with a reason to the unknown lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Sum {
    known: Amount,
    unknown_lines: u64,
}

impl Sum {
    fn of(lines: &[CostLine]) -> Result<Self, PriceError> {
        let mut sum = Self::default();
        for line in lines {
            if let Some(amount) = &line.amount {
                sum.known = add(sum.known, Amount::parse(amount)?)?;
            }
            if line.amount.is_none() || line.unknown.is_some() {
                sum.unknown_lines += 1;
            }
        }
        Ok(sum)
    }

    fn add(&mut self, other: Self) -> Result<(), PriceError> {
        self.known = add(self.known, other.known)?;
        self.unknown_lines = self
            .unknown_lines
            .checked_add(other.unknown_lines)
            .ok_or(CostError::Overflow)?;
        Ok(())
    }

    fn total(self) -> CostTotal {
        CostTotal {
            known_subtotal: self.known.to_string(),
            unknown_lines: self.unknown_lines,
            complete_total: (self.unknown_lines == 0).then(|| self.known.to_string()),
        }
    }
}

fn add(left: Amount, right: Amount) -> Result<Amount, PriceError> {
    left.nanos()
        .checked_add(right.nanos())
        .map(Amount::from_nanos)
        .ok_or(PriceError::Cost(CostError::Overflow))
}

/// A price document a session priced with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceDocumentRef {
    pub revision: String,
    pub digest: String,
}

/// A usage record the session read and could not price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpriced {
    pub usage_record_id: UsageRecordId,
    pub error: PriceError,
}

/// What a session's usage cost: the cost record of each usage record it read and priced, in the
/// order it read them, and their total. A record read before any pricing is pending until the
/// session is priced: the session keeps every such record, with no bound, until
/// [`crate::live::LiveSession::price_with`] is called, so a session that is never priced holds all
/// it read. The total is complete only once a price document is set, and then only when no line
/// of any record is unknown, no record is pending and none was refused; a session never priced
/// has no complete total, not even of nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCost {
    /// The currency of the first price document; none before the session is priced.
    pub currency: Option<String>,
    /// Each price document the session priced with, in the order it was given.
    pub documents: Vec<PriceDocumentRef>,
    pub records: Vec<CostRecorded>,
    pub unpriced: Vec<Unpriced>,
    /// How many usage records wait for a price document.
    pub pending: u64,
    pub total: CostTotal,
    sum: Sum,
}

impl Default for SessionCost {
    fn default() -> Self {
        Self {
            currency: None,
            documents: Vec::new(),
            records: Vec::new(),
            unpriced: Vec::new(),
            pending: 0,
            total: CostTotal {
                complete_total: None,
                ..Sum::default().total()
            },
            sum: Sum::default(),
        }
    }
}

/// A price document in another currency than the session's: no amount is converted, so the
/// session refuses it and keeps its pricing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrencyMismatch {
    pub session: String,
    pub document: String,
}

impl fmt::Display for CurrencyMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the price document is in {} and the session in {}; no currency is converted",
            self.document, self.session
        )
    }
}

impl Error for CurrencyMismatch {}

/// The session's pricing, if it has one, and the usage records it read before it had any.
#[derive(Debug, Default)]
pub(crate) struct Pricer {
    pricing: Option<Pricing>,
    pending: Vec<UsageRecorded>,
}

impl Pricer {
    /// Prices `recorded` into `cost` now, or holds it until the session is priced.
    pub(crate) fn read(&mut self, cost: &mut SessionCost, recorded: &UsageRecorded) {
        match &self.pricing {
            Some(pricing) => cost.add(price(pricing, recorded), recorded.usage_record_id),
            None => self.pending.push(recorded.clone()),
        }
        cost.pending = self.pending.len() as u64;
        cost.refresh();
    }

    /// From now on prices with `pricing`, and prices the records held until now.
    pub(crate) fn price_with(
        &mut self,
        cost: &mut SessionCost,
        pricing: Pricing,
    ) -> Result<(), CurrencyMismatch> {
        let currency = pricing.currency().to_owned();
        if let Some(session) = &cost.currency
            && *session != currency
        {
            return Err(CurrencyMismatch {
                session: session.clone(),
                document: currency,
            });
        }
        cost.currency = Some(currency);
        cost.documents.push(PriceDocumentRef {
            revision: pricing.book.document().revision.as_str().to_owned(),
            digest: pricing.book.digest().as_str().to_owned(),
        });
        for recorded in std::mem::take(&mut self.pending) {
            cost.add(price(&pricing, &recorded), recorded.usage_record_id);
        }
        self.pricing = Some(pricing);
        cost.pending = 0;
        cost.refresh();
        Ok(())
    }
}

impl SessionCost {
    /// Adds the cost record of the usage record `usage_record_id` to the records and the sum, or
    /// why it has none to the unpriced: its pricing failed, or the sum would overflow with it.
    fn add(&mut self, priced: Result<CostRecorded, PriceError>, usage_record_id: UsageRecordId) {
        let added = priced.and_then(|record| {
            let mut sum = self.sum;
            sum.add(Sum::of(&record.lines)?)?;
            Ok((record, sum))
        });
        match added {
            Ok((record, sum)) => {
                self.records.push(record);
                self.sum = sum;
            }
            Err(error) => self.unpriced.push(Unpriced {
                usage_record_id,
                error,
            }),
        }
    }

    /// The total of the records, complete only once a price document is set and when no line is
    /// unknown, no record pending and none unpriced.
    fn refresh(&mut self) {
        let mut total = self.sum.total();
        if self.documents.is_empty() || self.pending > 0 || !self.unpriced.is_empty() {
            total.complete_total = None;
        }
        self.total = total;
    }
}
