//! Adversary cases for `story:realtime-usage-cached-audio-input`, core side: the new kind's
//! place among the kinds, and what the user documentation tells a consumer about the kinds.

use inference_core::usage::{Quantity, QuantityKind, UsageTotal};

/// The usage concept page, the user documentation's list of quantity kinds.
const USAGE_CONCEPT: &str = include_str!("../../../website/docs/concepts/usage.md");

/// `UsageTotal::kinds` promises the order of `QuantityKind::ALL`; it iterates a `BTreeMap` keyed
/// by the derived `Ord`, which is the enum's declaration order. Inserting a variant into `ALL`
/// and the enum at different places breaks the promise; a total over every kind, added in
/// reverse, must still come out in the order of `ALL`.
#[test]
fn usage_total_kinds_come_out_in_the_order_of_all_with_cached_input_audio() {
    let mut total = UsageTotal::new();
    let quantities: Vec<Quantity> = QuantityKind::ALL
        .iter()
        .rev()
        .map(|kind| Quantity::provider_reported(*kind, 1))
        .collect();
    total.add(&quantities, true);
    let kinds: Vec<QuantityKind> = total.kinds().map(|(kind, _)| kind).collect();
    assert_eq!(kinds, QuantityKind::ALL.to_vec());
    let at = |kind| kinds.iter().position(|k| *k == kind).expect("present");
    assert_eq!(
        at(QuantityKind::CachedInputAudioTokens),
        at(QuantityKind::CachedInputTokens) + 1
    );
}

/// The concept page's kinds table is the consumer's list of what a quantity can count. The unit
/// added `cached_input_audio_tokens`; the table must name it, or a consumer is told the list is
/// complete without it.
#[test]
fn the_usage_concept_page_names_the_cached_input_audio_kind() {
    let row = USAGE_CONCEPT
        .lines()
        .find(|line| line.starts_with("| input and output text tokens"))
        .expect("the kinds row of the usage concept page");
    assert!(
        row.contains("cached input audio tokens") || row.contains("cached audio input tokens"),
        "the kinds row does not name the cached input audio kind: {row}"
    );
}
