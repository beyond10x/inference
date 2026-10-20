//! Price tables: per token, in the JSON form `realtime_voice::Prices` reads, or per minute of
//! session, billed per second (a started second counts whole), for a model that bills its voice
//! by time:
//!
//! ```json
//! {"name": "gpt-live-1 list prices", "date": "2026-10-20", "currency": "USD",
//!  "per_session_minute": 0.05}
//! ```
//!
//! The example ships one table per supported model under `prices/`, each naming its source URL
//! in its name and the date it was read. Prices take at most six decimals; costs are exact in
//! integers until they are shown, rounded half up to six decimals.

use std::error::Error;
use std::fmt;

use realtime_voice::{Prices, Tally};
use serde_json::Value;

/// The shipped table of `gpt-realtime-2.1`, per token.
pub const REALTIME_PRICES: &str = include_str!("../prices/gpt-realtime-2.1.json");

/// The shipped table of `gpt-live-1`, per session minute.
pub const LIVE_PRICES: &str = include_str!("../prices/gpt-live-1.json");

/// The fields of a table priced by session time.
const SESSION_FIELDS: [&str; 4] = ["name", "date", "currency", "per_session_minute"];

/// What a table prices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rates {
    /// Tokens, by the realtime-voice table.
    Tokens(Prices),
    /// Session time: millionths of the currency unit per minute, billed per second.
    SessionMinute(u64),
}

/// A price table with its name, date and currency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceTable {
    pub name: String,
    pub date: String,
    pub currency: String,
    pub rates: Rates,
}

impl PriceTable {
    /// Reads a table from its JSON text: per session minute when it has `per_session_minute`,
    /// otherwise the realtime-voice form per token.
    ///
    /// # Errors
    ///
    /// [`PriceTableError`] for text that is not JSON, a field missing, unknown or of the wrong
    /// type, an empty name, date or currency, or a price that is negative or has more than six
    /// decimals.
    pub fn parse(text: &str) -> Result<Self, PriceTableError> {
        let value: Value = serde_json::from_str(text)
            .map_err(|error| PriceTableError(format!("not a price table: {error}")))?;
        let Some(table) = value.as_object() else {
            return Err(PriceTableError(
                "the price table is not an object".to_owned(),
            ));
        };
        if !table.contains_key("per_session_minute") {
            let prices = Prices::parse(text).map_err(|error| PriceTableError(error.to_string()))?;
            return Ok(Self {
                name: prices.name.clone(),
                date: prices.date.clone(),
                currency: prices.currency.clone(),
                rates: Rates::Tokens(prices),
            });
        }
        if let Some(key) = table
            .keys()
            .find(|key| !SESSION_FIELDS.contains(&key.as_str()))
        {
            return Err(PriceTableError(format!(
                "the price table has an unknown field {key}"
            )));
        }
        let text_field = |field: &str| match table.get(field).and_then(Value::as_str) {
            Some(text) if !text.trim().is_empty() => Ok(text.to_owned()),
            _ => Err(PriceTableError(format!(
                "{field} is not a non-empty string"
            ))),
        };
        Ok(Self {
            name: text_field("name")?,
            date: text_field("date")?,
            currency: text_field("currency")?,
            rates: Rates::SessionMinute(micros(
                table.get("per_session_minute"),
                "per_session_minute",
            )?),
        })
    }

    /// The cost of one session of `seconds`, by a table priced per session minute: billed per
    /// second, so a started second is a whole one, and a time too large to count costs the most
    /// [`Money`] holds. Zero by a table per token, and for a time that is not a positive number.
    #[must_use]
    pub fn session_cost(&self, seconds: f64) -> Money {
        let Rates::SessionMinute(per_minute) = self.rates else {
            return Money::default();
        };
        Money(
            u128::from(per_minute)
                .saturating_mul(1_000_000)
                .saturating_mul(u128::from(billed_seconds(seconds)))
                / 60,
        )
    }

    /// The cost of the responses `tally` holds, by a table per token; `None` when none
    /// reported usage, a count the table needs is unknown, or the table prices session time.
    #[must_use]
    pub fn token_cost(&self, tally: &Tally) -> Option<Money> {
        match &self.rates {
            Rates::Tokens(prices) => tally.cost(prices).map(|cost| Money(cost.picos())),
            Rates::SessionMinute(_) => None,
        }
    }

    /// The cost of a host's part of the show: its tokens or its session seconds.
    #[must_use]
    pub fn cost(&self, tally: &Tally, seconds: f64) -> Option<Money> {
        match self.rates {
            Rates::Tokens(_) => self.token_cost(tally),
            Rates::SessionMinute(_) => Some(self.session_cost(seconds)),
        }
    }
}

impl fmt::Display for PriceTable {
    /// The table's name and date: `list prices (2026-10-20)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.name, self.date)
    }
}

/// The whole seconds a session of `seconds` is billed: rounded up, saturating at `u64::MAX`;
/// zero for a time that is not a positive number.
fn billed_seconds(seconds: f64) -> u64 {
    if !(seconds.is_finite() && seconds > 0.0) {
        return 0;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let whole = seconds.ceil() as u64;
    whole
}

/// A price in millionths, read from the JSON number's decimal digits so nothing is rounded.
fn micros(value: Option<&Value>, field: &str) -> Result<u64, PriceTableError> {
    let refused = || {
        PriceTableError(format!(
            "{field} is not a price: a number of at least zero with at most six decimals"
        ))
    };
    let Some(Value::Number(number)) = value else {
        return Err(refused());
    };
    let digits = number.to_string();
    let (whole, fraction) = digits.split_once('.').unwrap_or((&digits, ""));
    let decimal = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if whole.is_empty() || !decimal(whole) || !decimal(fraction) || fraction.len() > 6 {
        return Err(refused());
    }
    let whole: u64 = whole.parse().map_err(|_| refused())?;
    let fraction: u64 = format!("{fraction:0<6}").parse().map_err(|_| refused())?;
    whole
        .checked_mul(1_000_000)
        .and_then(|whole| whole.checked_add(fraction))
        .ok_or_else(refused)
}

/// Why a price table was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceTableError(String);

impl fmt::Display for PriceTableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for PriceTableError {}

/// An amount, exact in millionths of a millionth of the currency unit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Money(u128);

impl Money {
    /// The amount of `picos` millionths of a millionth of the currency unit.
    #[must_use]
    pub const fn from_picos(picos: u128) -> Self {
        Self(picos)
    }

    /// The amount in millionths of a millionth of the currency unit.
    #[must_use]
    pub const fn picos(self) -> u128 {
        self.0
    }
}

impl std::ops::Add for Money {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl fmt::Display for Money {
    /// The amount in the currency unit, rounded half up to six decimals: `0.003792`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let micros = (self.0 + 500_000) / 1_000_000;
        write!(f, "{}.{:06}", micros / 1_000_000, micros % 1_000_000)
    }
}
