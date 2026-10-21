//! Adversary: `UsageTotal` properties (`story:usage-session-totals`).

use inference_core::usage::{KindTotal, Quantity, QuantityKind, UsageTotal};

#[test]
fn a_record_carrying_one_kind_twice_counts_once_as_a_record_and_keeps_both_values() {
    let mut total = UsageTotal::new();
    total.add(&[
        Quantity::measured(QuantityKind::OutputAudioMs, 20),
        Quantity::unknown(QuantityKind::OutputAudioMs),
        Quantity::provider_reported(QuantityKind::OutputAudioMs, 5),
    ]);
    assert_eq!(
        total.kind(QuantityKind::OutputAudioMs),
        Some(KindTotal {
            known: 25,
            unknown: 1,
            records: 1,
        })
    );
    assert_eq!(total.records(), 1);
}

#[test]
fn a_record_without_quantities_is_counted_and_carries_no_kind() {
    let mut total = UsageTotal::new();
    total.add(&[]);
    assert_eq!(total.records(), 1);
    assert_eq!(total.kinds().count(), 0);
    assert!(total.complete());
}

#[test]
fn the_order_records_are_added_in_does_not_change_the_total() {
    let records = [
        vec![
            Quantity::provider_reported(QuantityKind::InputTextTokens, 7),
            Quantity::unknown(QuantityKind::OutputTextTokens),
        ],
        vec![Quantity::measured(QuantityKind::Requests, 1)],
        vec![
            Quantity::unknown(QuantityKind::InputTextTokens),
            Quantity::provider_reported(QuantityKind::OutputTextTokens, 3),
        ],
    ];
    let mut forward = UsageTotal::new();
    let mut backward = UsageTotal::new();
    for record in &records {
        forward.add(record);
    }
    for record in records.iter().rev() {
        backward.add(record);
    }
    assert_eq!(forward, backward);
}

#[test]
fn an_unknown_never_reads_as_a_complete_zero() {
    let mut total = UsageTotal::new();
    total.add(&[Quantity::unknown(QuantityKind::InputCharacters)]);
    let kind = total.kind(QuantityKind::InputCharacters).expect("carried");
    assert_eq!(kind.known, 0);
    assert!(
        !kind.complete(),
        "a known sum of 0 beside an unknown is not complete"
    );
    assert!(!total.complete());
}

#[test]
fn the_extremes_of_i64_sum_without_overflow() {
    let mut total = UsageTotal::new();
    for _ in 0..4 {
        total.add(&[Quantity::measured(QuantityKind::LocalComputeMs, i64::MAX)]);
        total.add(&[Quantity::measured(QuantityKind::InputAudioMs, i64::MIN)]);
    }
    assert_eq!(
        total.kind(QuantityKind::LocalComputeMs).map(|k| k.known),
        Some(4 * i128::from(i64::MAX))
    );
    assert_eq!(
        total.kind(QuantityKind::InputAudioMs).map(|k| k.known),
        Some(4 * i128::from(i64::MIN))
    );
}
