//! Adversary cases for `story:realtime-usage-cached-audio-input`, pricing side: a Realtime
//! response record that carries its cached audio share, priced under a model price.

#![cfg(feature = "cost")]

use inference_core::model::{AdapterId, ModelId};
use inference_core::usage::{
    CostLine, Quantity, QuantityKind, RecordUsage, UnknownReason, UsageRecord, UsageRecorded,
};
use inference_facade::cost::{
    BillingKind, Id, ModelPriceBinding, PriceBinding, PriceBook, Pricing, price,
};

const UPSTREAM: &str = "mock-served";

fn book() -> PriceBook {
    let document = format!(
        r#"{{
            "format": "llm.prices/1",
            "revision": "1",
            "source": "inference facade adversary tests",
            "as_of_unix_ms": 1792540800000,
            "currency": "EUR",
            "models": [{{
                "id": "mock-llm",
                "serving_model_id": "mock",
                "binding_revision": "1",
                "upstream_model": "{UPSTREAM}",
                "input": {{"amount": "2", "per_units": 1000000}},
                "cached_input": {{"amount": "0.5", "per_units": 1000000}},
                "output": {{"amount": "8", "per_units": 1000000}}
            }}],
            "compute": []
        }}"#
    );
    PriceBook::parse_json(&document).expect("a valid price document")
}

fn id(value: &str) -> Id {
    Id::new(value).expect("a printable identifier")
}

/// A binding that charges the text token kinds a Realtime record carries and its cached audio.
fn realtime_binding() -> PriceBinding {
    PriceBinding {
        charges: vec![
            QuantityKind::InputTextTokens,
            QuantityKind::OutputTextTokens,
            QuantityKind::CachedInputTokens,
            QuantityKind::CachedInputAudioTokens,
        ],
        model: Some(ModelPriceBinding {
            price_id: id("mock-llm"),
            serving_model_id: id("mock"),
            binding_revision: id("1"),
            billing: BillingKind::Metered,
        }),
        compute: None,
    }
}

/// The quantities the Realtime duplex binding records for the unit's own `response.done` in
/// `realtime_duplex_records_cached_audio_input_tokens`: input text 5, audio 20, cached 16 of
/// which 12 audio (so 4 cached text), output text 3, audio 12.
fn realtime_record(adapter_id: AdapterId) -> UsageRecorded {
    let quantities = vec![
        Quantity::provider_reported(QuantityKind::InputTextTokens, 5),
        Quantity::provider_reported(QuantityKind::InputAudioTokens, 20),
        Quantity::provider_reported(QuantityKind::CachedInputTokens, 16),
        Quantity::provider_reported(QuantityKind::CachedInputAudioTokens, 12),
        Quantity::provider_reported(QuantityKind::OutputTextTokens, 3),
        Quantity::provider_reported(QuantityKind::OutputAudioTokens, 12),
        Quantity::measured(QuantityKind::Requests, 1),
    ];
    let mut input = RecordUsage::new(adapter_id, ModelId::generate(), 0, 10, quantities);
    input.upstream_model = Some(UPSTREAM.to_owned());
    UsageRecord::record(input).1
}

/// The record is consistent with the specification: the cached audio (12) is inside both the
/// cached input (16) and the input audio (20), and the cached text (16 - 12 = 4) is inside the
/// input text (5). Pricing it must not refuse it as contradictory counts; the cached audio line
/// is `price_unknown` with its count, as the unit's cost change says.
#[test]
fn a_realtime_record_whose_cached_input_is_mostly_audio_is_priced_not_refused() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book()).bind(adapter, realtime_binding());
    let cost = price(&pricing, &realtime_record(adapter)).expect(
        "a record the Realtime binding records, cached text 4 within input text 5, is priced",
    );
    let cached_audio: Vec<&CostLine> = cost
        .lines
        .iter()
        .filter(|line| line.kind == QuantityKind::CachedInputAudioTokens)
        .collect();
    assert_eq!(
        cached_audio,
        [&CostLine {
            kind: QuantityKind::CachedInputAudioTokens,
            quantity: Some(12),
            amount: None,
            unknown: Some(UnknownReason::PriceUnknown),
        }]
    );
    assert_eq!(cost.total.complete_total, None, "unknown, never zero");
}
