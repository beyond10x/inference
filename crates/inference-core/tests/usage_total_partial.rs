//! A partial record leaves a usage total incomplete even when it carries no quantity
//! (`story:contract-usage-and-naming-drift`): the provider may still add counts to it, so the
//! total is not settled, as the module documentation says of every partial record.

use inference_core::usage::{Quantity, QuantityKind, UsageTotal};

#[test]
fn a_partial_record_with_no_quantities_leaves_the_total_incomplete() {
    let mut total = UsageTotal::new();
    total.add(&[], false);
    assert_eq!(total.records(), 1);
    assert_eq!(total.kinds().count(), 0, "no kind was carried");
    assert!(
        !total.complete(),
        "a partial record with no quantities is still partial (story:contract-usage-and-naming-drift)"
    );
}

#[test]
fn a_final_record_with_no_quantities_leaves_the_total_complete() {
    let mut total = UsageTotal::new();
    total.add(&[], true);
    assert_eq!(total.records(), 1);
    assert!(total.complete(), "nothing is unknown and nothing partial");
}

#[test]
fn an_empty_partial_record_among_final_ones_still_makes_the_total_incomplete() {
    let mut total = UsageTotal::new();
    total.add(&[Quantity::measured(QuantityKind::Requests, 1)], true);
    total.add(&[], false);
    total.add(&[Quantity::measured(QuantityKind::Requests, 1)], true);
    assert_eq!(
        total.kind(QuantityKind::Requests).map(|kind| kind.known),
        Some(2)
    );
    assert!(
        total
            .kind(QuantityKind::Requests)
            .is_some_and(|kind| kind.complete()),
        "the kind's own records are final"
    );
    assert!(!total.complete(), "the empty partial record is not settled");
}
