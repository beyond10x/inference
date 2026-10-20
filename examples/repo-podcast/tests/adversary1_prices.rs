//! Adversary pass 1 on the price tables: per-second session pricing, a price with fewer than
//! six decimals written as JSON writes it, and session times at the edge.

use repo_podcast::{LIVE_PRICES, PriceTable, Rates};

#[test]
fn session_time_is_billed_in_whole_seconds_as_the_table_documents() {
    let table = PriceTable::parse(LIVE_PRICES).expect("the shipped table reads");
    let one = table.session_cost(1.0);
    let two = table.session_cost(2.0);
    let between = table.session_cost(1.5);
    assert!(
        between == one || between == two,
        "\"per minute of session, billed per second\": 1.5 s costs {between} USD, neither 1 s \
         ({one}) nor 2 s ({two})"
    );
}

#[test]
fn a_price_with_five_decimals_reads() {
    let text = r#"{"name": "n", "date": "d", "currency": "USD", "per_session_minute": 0.00001}"#;
    let table = PriceTable::parse(text);
    assert_eq!(
        table.map(|table| table.rates),
        Ok(Rates::SessionMinute(10)),
        "at most six decimals are allowed; five are refused because the JSON number prints as 1e-5"
    );
}

#[test]
fn a_huge_session_time_is_priced_without_a_panic() {
    let table = PriceTable::parse(LIVE_PRICES).expect("the shipped table reads");
    let million = table.session_cost(1.0e6);
    let huge = table.session_cost(1.0e20);
    assert!(huge >= million, "{huge} < {million}");
}

#[test]
fn session_cost_is_exact_on_a_fraction_of_a_second() {
    let table = PriceTable::parse(LIVE_PRICES).expect("the shipped table reads");
    // Session time is billed in whole seconds, rounded up (review finding 4): 2.3 s is 3 s.
    assert_eq!(
        table.session_cost(2.3).picos(),
        50_000 * 1_000_000 * 3_000 / 60_000
    );
}
