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

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        Quantity, QuantityKind, QuantityMismatch, QuantitySource, RecordUsage, UsageRecord,
        UsageRecordId, UsageRecorded,
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
    fn a_contract_crate_places_nothing_and_counts_are_final() {
        let usage = input();
        assert_eq!(usage.run_id, None);
        assert_eq!(usage.node_id, None);
        assert_eq!(usage.session_id, None);
        assert!(usage.r#final);
    }
}
