//! Usage and an estimate of its cost: the counts a session reports per response and per
//! transcription, their totals over a conversation, and a price table the caller supplies.
//!
//! A count the provider does not report is unknown, never zero. [`Reported`] says which counts a
//! provider reports at all; a response that ended without any usage reported has none, and the
//! totals leave it out and count it apart. Unknown counts print as `?`.
//!
//! [`Prices`] is a small table read from JSON, per million tokens, with its own name, date and
//! currency; no prices are built in:
//!
//! ```json
//! {
//!   "name": "list prices",
//!   "date": "2026-10-20",
//!   "currency": "USD",
//!   "per_million_tokens": {
//!     "text_in": 4.0, "cached_in": 0.4, "text_out": 16.0, "audio_in": 32.0, "audio_out": 64.0
//!   }
//! }
//! ```
//!
//! `cached_in` is optional. With it, cached tokens are priced at that rate, taken out of the text
//! input first and then out of the audio input, which errs high where audio costs more; without
//! it, every input token is priced at its own rate. Prices take at most six decimals and costs are
//! computed in integers, exactly.

use std::error::Error;
use std::fmt;

use inference_duplex::{ResponseId, ResponseUsage, TranscriptionUsage};
use serde_json::{Map, Value};

use crate::Note;

/// Usage the session reported, as the conversation hands it on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Usage {
    /// The usage of a response so far: a later one for the same response replaces it.
    Response {
        response_id: ResponseId,
        usage: ResponseUsage,
    },
    /// The usage of transcribing one turn of the input.
    Transcription(TranscriptionUsage),
}

/// Which counts of a response's usage a provider reports. The contract carries every count as a
/// number, so a count a provider does not report arrives as zero; this says which zeros are not
/// counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reported {
    pub input_text: bool,
    pub input_audio: bool,
    pub input_cached: bool,
    pub output_text: bool,
    pub output_audio: bool,
}

impl Reported {
    /// Every count.
    pub const ALL: Self = Self {
        input_text: true,
        input_audio: true,
        input_cached: true,
        output_text: true,
        output_audio: true,
    };
    /// What the OpenAI Realtime binding reports: every count.
    pub const OPENAI: Self = Self::ALL;
    /// What the GPT-Live binding reports: the tokens of its Responses backend, as text tokens
    /// with the cached ones inside them. Its voice model bills seconds, which no count holds.
    pub const OPENAI_LIVE: Self = Self {
        input_audio: false,
        output_audio: false,
        ..Self::ALL
    };
    /// What the Nova Sonic binding reports: text and speech tokens each way, no cached tokens.
    pub const NOVA: Self = Self {
        input_cached: false,
        ..Self::ALL
    };
}

/// Token counts of a response, or of several together; `None` is a count the provider did not
/// report. Cached tokens are input tokens served from the provider's cache, inside the text and
/// audio input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub input_text: Option<u64>,
    pub input_audio: Option<u64>,
    pub input_cached: Option<u64>,
    pub output_text: Option<u64>,
    pub output_audio: Option<u64>,
}

impl Counts {
    /// The counts of `usage`, those the provider does not report unknown.
    #[must_use]
    pub const fn of(usage: &ResponseUsage, reported: Reported) -> Self {
        const fn known(reported: bool, count: u64) -> Option<u64> {
            if reported { Some(count) } else { None }
        }
        Self {
            input_text: known(reported.input_text, usage.input.text_tokens),
            input_audio: known(reported.input_audio, usage.input.audio_tokens),
            input_cached: known(reported.input_cached, usage.input.cached_tokens),
            output_text: known(reported.output_text, usage.output.text_tokens),
            output_audio: known(reported.output_audio, usage.output.audio_tokens),
        }
    }

    /// The sum, count by count: the sum of what either knows, unknown where neither knows it.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self {
            input_text: add(self.input_text, other.input_text),
            input_audio: add(self.input_audio, other.input_audio),
            input_cached: add(self.input_cached, other.input_cached),
            output_text: add(self.output_text, other.output_text),
            output_audio: add(self.output_audio, other.output_audio),
        }
    }
}

fn add(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        (known, None) | (None, known) => known,
    }
}

/// A count as printed: the number, or `?` when it is not known.
struct Shown(Option<u64>);

impl fmt::Display for Shown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(count) => write!(f, "{count}"),
            None => f.write_str("?"),
        }
    }
}

impl fmt::Display for Counts {
    /// `in text 20 audio 50 cached 0, out text 8 audio 30`, `?` for a count not known.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "in text {} audio {} cached {}, out text {} audio {}",
            Shown(self.input_text),
            Shown(self.input_audio),
            Shown(self.input_cached),
            Shown(self.output_text),
            Shown(self.output_audio),
        )
    }
}

/// One response as the tally knows it.
#[derive(Debug, Clone, Copy)]
struct Entry {
    response_id: ResponseId,
    counts: Option<Counts>,
    ended: bool,
}

/// The usage of a conversation, fed with its notes: the latest usage of each response, the
/// responses that ended, and the transcriptions.
#[derive(Debug, Clone)]
pub struct Tally {
    reported: Reported,
    responses: Vec<Entry>,
    heard: usize,
    transcriptions: usize,
    transcription_input: u64,
    transcription_output: u64,
}

impl Tally {
    /// An empty tally of a provider that reports `reported`.
    #[must_use]
    pub const fn new(reported: Reported) -> Self {
        Self {
            reported,
            responses: Vec::new(),
            heard: 0,
            transcriptions: 0,
            transcription_input: 0,
            transcription_output: 0,
        }
    }

    /// Takes in what `note` says about usage: a response's usage replaces the one before, an end
    /// marks its response ended, a transcript and a transcription's usage are counted.
    pub fn record(&mut self, note: &Note) {
        match note {
            Note::Usage(Usage::Response { response_id, usage }) => {
                let counts = Counts::of(usage, self.reported);
                self.entry(*response_id).counts = Some(counts);
            }
            Note::Usage(Usage::Transcription(usage)) => {
                self.transcriptions += 1;
                self.transcription_input =
                    self.transcription_input.saturating_add(usage.input_tokens);
                self.transcription_output = self
                    .transcription_output
                    .saturating_add(usage.output_tokens);
            }
            Note::Ended { response_id, .. } => self.entry(*response_id).ended = true,
            Note::Heard(_) => self.heard += 1,
            _ => {}
        }
    }

    fn entry(&mut self, response_id: ResponseId) -> &mut Entry {
        let at = match self
            .responses
            .iter()
            .position(|entry| entry.response_id == response_id)
        {
            Some(at) => at,
            None => {
                self.responses.push(Entry {
                    response_id,
                    counts: None,
                    ended: false,
                });
                self.responses.len() - 1
            }
        };
        &mut self.responses[at]
    }

    fn find(&self, response_id: ResponseId) -> Option<&Entry> {
        self.responses
            .iter()
            .find(|entry| entry.response_id == response_id)
    }

    /// The latest counts of `response_id`; `None` while it has reported no usage.
    #[must_use]
    pub fn response(&self, response_id: ResponseId) -> Option<Counts> {
        self.find(response_id).and_then(|entry| entry.counts)
    }

    /// Whether `response_id` ended.
    #[must_use]
    pub fn ended(&self, response_id: ResponseId) -> bool {
        self.find(response_id).is_some_and(|entry| entry.ended)
    }

    /// The totals of every response and transcription so far.
    #[must_use]
    pub fn totals(&self) -> Totals {
        let reported = || self.responses.iter().filter_map(|entry| entry.counts);
        let counted = self.transcriptions > 0;
        Totals {
            responses: self.responses.len(),
            unreported: self.responses.len() - reported().count(),
            counts: reported().fold(Counts::default(), Counts::plus),
            heard: self.heard,
            transcriptions: self.transcriptions,
            transcription_input: counted.then_some(self.transcription_input),
            transcription_output: counted.then_some(self.transcription_output),
        }
    }

    /// The estimated cost of every response that reported usage, by `prices`; `None` when none
    /// did or one lacks a count the table needs. Transcription is not priced.
    #[must_use]
    pub fn cost(&self, prices: &Prices) -> Option<Cost> {
        let mut costs = self
            .responses
            .iter()
            .filter_map(|entry| entry.counts)
            .map(|counts| prices.cost(&counts))
            .peekable();
        costs.peek()?;
        costs.sum()
    }
}

/// The usage of a conversation in total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    /// The responses that ended or reported usage.
    pub responses: usize,
    /// Those of them that reported no usage, left out of `counts`.
    pub unreported: usize,
    /// The sum of the latest counts of each response that reported usage.
    pub counts: Counts,
    /// The user turns transcribed.
    pub heard: usize,
    /// The transcriptions that reported usage.
    pub transcriptions: usize,
    /// Their input tokens; `None` when none reported.
    pub transcription_input: Option<u64>,
    /// Their output tokens; `None` when none reported.
    pub transcription_output: Option<u64>,
}

impl fmt::Display for Totals {
    /// `responses 2 (1 without usage): <counts>; transcription in 20 out 6 (2 of 2 turns
    /// reported)`, the part in parentheses after `responses` only when one reported nothing.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "responses {}", self.responses)?;
        if self.unreported > 0 {
            write!(f, " ({} without usage)", self.unreported)?;
        }
        write!(
            f,
            ": {}; transcription in {} out {} ({} of {} turns reported)",
            self.counts,
            Shown(self.transcription_input),
            Shown(self.transcription_output),
            self.transcriptions,
            self.heard,
        )
    }
}

/// A price table, per million tokens, each price in millionths of the currency unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prices {
    /// The table's own name.
    pub name: String,
    /// The date the table states its prices for.
    pub date: String,
    /// The currency of every price.
    pub currency: String,
    pub text_in: u64,
    /// The price of a cached input token; `None` prices cached tokens at their own rate.
    pub cached_in: Option<u64>,
    pub text_out: u64,
    pub audio_in: u64,
    pub audio_out: u64,
}

const TABLE_FIELDS: [&str; 4] = ["name", "date", "currency", "per_million_tokens"];
const PRICE_FIELDS: [&str; 5] = ["text_in", "cached_in", "text_out", "audio_in", "audio_out"];

impl Prices {
    /// Reads a price table from its JSON text.
    ///
    /// # Errors
    ///
    /// [`PricesError`] for text that is not JSON, a field missing, unknown or of the wrong type,
    /// an empty name, date or currency, or a price that is negative or has more than six
    /// decimals.
    pub fn parse(text: &str) -> Result<Self, PricesError> {
        let value: Value = serde_json::from_str(text)
            .map_err(|error| PricesError(format!("not a price table: {error}")))?;
        let table = object(&value, "the price table")?;
        only(table, &TABLE_FIELDS, "the price table")?;
        let rates = object(
            table
                .get("per_million_tokens")
                .ok_or_else(|| missing("per_million_tokens"))?,
            "per_million_tokens",
        )?;
        only(rates, &PRICE_FIELDS, "per_million_tokens")?;
        let price = |field: &str| rates.get(field).ok_or_else(|| missing(field));
        Ok(Self {
            name: text_field(table, "name")?,
            date: text_field(table, "date")?,
            currency: text_field(table, "currency")?,
            text_in: micros(price("text_in")?, "text_in")?,
            cached_in: rates
                .get("cached_in")
                .map(|value| micros(value, "cached_in"))
                .transpose()?,
            text_out: micros(price("text_out")?, "text_out")?,
            audio_in: micros(price("audio_in")?, "audio_in")?,
            audio_out: micros(price("audio_out")?, "audio_out")?,
        })
    }

    /// The estimated cost of `counts`; `None` when a count it needs is not known (the cached
    /// count only when the table prices cached tokens).
    #[must_use]
    pub fn cost(&self, counts: &Counts) -> Option<Cost> {
        let mut text_in = counts.input_text?;
        let mut audio_in = counts.input_audio?;
        let text_out = counts.output_text?;
        let audio_out = counts.output_audio?;
        let mut cached = 0;
        if self.cached_in.is_some() {
            let reported = counts.input_cached?;
            let from_text = reported.min(text_in);
            let from_audio = (reported - from_text).min(audio_in);
            text_in -= from_text;
            audio_in -= from_audio;
            cached = from_text + from_audio;
        }
        let priced = |tokens: u64, price: u64| u128::from(tokens) * u128::from(price);
        Some(Cost {
            picos: priced(text_in, self.text_in)
                + priced(audio_in, self.audio_in)
                + priced(cached, self.cached_in.unwrap_or(0))
                + priced(text_out, self.text_out)
                + priced(audio_out, self.audio_out),
        })
    }
}

impl fmt::Display for Prices {
    /// The table's name and date: `list prices (2026-10-20)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.name, self.date)
    }
}

fn missing(field: &str) -> PricesError {
    PricesError(format!("the price table has no {field}"))
}

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, PricesError> {
    value
        .as_object()
        .ok_or_else(|| PricesError(format!("{what} is not an object")))
}

fn only(object: &Map<String, Value>, fields: &[&str], what: &str) -> Result<(), PricesError> {
    match object.keys().find(|key| !fields.contains(&key.as_str())) {
        Some(key) => Err(PricesError(format!("{what} has an unknown field {key}"))),
        None => Ok(()),
    }
}

fn text_field(table: &Map<String, Value>, field: &str) -> Result<String, PricesError> {
    match table.get(field).ok_or_else(|| missing(field))?.as_str() {
        Some(text) if !text.trim().is_empty() => Ok(text.to_owned()),
        _ => Err(PricesError(format!("{field} is not a non-empty string"))),
    }
}

/// A price in millionths, read from the JSON number's decimal digits so nothing is rounded.
fn micros(value: &Value, field: &str) -> Result<u64, PricesError> {
    let refused = || {
        PricesError(format!(
            "{field} is not a price: a number of at least zero with at most six decimals"
        ))
    };
    let Value::Number(number) = value else {
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
pub struct PricesError(String);

impl fmt::Display for PricesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for PricesError {}

/// An estimated cost, exact in millionths of a millionth of the currency unit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cost {
    picos: u128,
}

impl Cost {
    /// The cost in millionths of a millionth of the currency unit.
    #[must_use]
    pub const fn picos(self) -> u128 {
        self.picos
    }
}

impl std::iter::Sum for Cost {
    fn sum<I: Iterator<Item = Self>>(costs: I) -> Self {
        Self {
            picos: costs.map(Self::picos).sum(),
        }
    }
}

impl fmt::Display for Cost {
    /// The cost in the currency unit, rounded half up to six decimals: `0.003692`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let micros = (self.picos + 500_000) / 1_000_000;
        write!(f, "{}.{:06}", micros / 1_000_000, micros % 1_000_000)
    }
}
