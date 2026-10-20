//! The price tables the example ships and how they price a show.

use realtime_voice::Prices;
use repo_podcast::{LIVE_PRICES, PriceTable, REALTIME_PRICES, Rates};

#[test]
fn the_realtime_table_is_in_the_realtime_voice_format_with_the_list_prices() {
    let table = PriceTable::parse(REALTIME_PRICES).expect("the shipped table reads");
    let prices = Prices::parse(REALTIME_PRICES).expect("realtime-voice reads it as well");
    assert_eq!(table.rates, Rates::Tokens(prices.clone()));
    assert_eq!(
        (
            prices.text_in,
            prices.cached_in,
            prices.text_out,
            prices.audio_in,
            prices.audio_out
        ),
        (4_000_000, Some(400_000), 24_000_000, 32_000_000, 64_000_000)
    );
    assert!(
        table
            .name
            .contains("https://developers.openai.com/api/docs/models/gpt-realtime-2.1")
    );
    assert_eq!(table.date, "2026-10-20");
    assert_eq!(table.currency, "USD");
}

#[test]
fn the_live_table_prices_session_time_per_second() {
    let table = PriceTable::parse(LIVE_PRICES).expect("the shipped table reads");
    assert_eq!(table.rates, Rates::SessionMinute(50_000));
    assert!(
        table
            .name
            .contains("https://developers.openai.com/api/docs/models/gpt-live-1")
    );
    assert_eq!(table.date, "2026-10-20");
    assert_eq!(table.session_cost(90.0).to_string(), "0.075000");
    assert_eq!(table.session_cost(1.0).to_string(), "0.000833");
}

#[test]
fn a_table_with_an_unknown_field_or_a_bad_price_is_refused() {
    for text in [
        r#"{"name": "n", "date": "d", "currency": "USD", "per_session_minute": 0.05, "x": 1}"#,
        r#"{"name": "n", "date": "d", "currency": "USD", "per_session_minute": -1}"#,
        r#"{"name": "n", "date": "d", "currency": "USD", "per_session_minute": 0.0000001}"#,
        r#"{"name": "", "date": "d", "currency": "USD", "per_session_minute": 0.05}"#,
        r#"{"name": "n", "date": "d", "currency": "USD"}"#,
        "not json",
    ] {
        assert!(PriceTable::parse(text).is_err(), "{text}");
    }
}
