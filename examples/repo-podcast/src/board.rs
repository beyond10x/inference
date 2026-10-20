//! What the screen shows, fed with the show's events: who speaks and how loud, the transcript
//! of the turn playing, and running counters per host and in total, with the cost by a price
//! table. [`plain_line`] gives the same events as lines.
//!
//! The counters are read from the session ledger's usage records ([`Event::Recorded`]), one
//! per response, with what the provider reported ([`Event::Usage`]) standing in until a
//! response's record arrives. A count nobody counted is unknown, never zero: a response whose
//! record knows no count at all, such as one that stalled or was stopped, is left out of the
//! sums and counted apart, `(1 without usage)`, a count one response left unknown makes the sum
//! of that count `?`, and a cost that leaves out such a response is shown as a lower bound.

use std::collections::{BTreeMap, VecDeque};
use std::ops::Add;
use std::time::Duration;

use inference_duplex::ResponseId;
use realtime_voice::{Counts, Reported, Usage};

use crate::hosts::{Directive, Host};
use crate::prices::{Money, PriceTable, Rates};
use crate::show::Event;

/// The meter history kept per host.
const HISTORY: usize = 96;

/// One response of a host as the board knows it.
#[derive(Debug, Clone, Copy)]
struct Response {
    response_id: ResponseId,
    /// Its counts; `None` while nothing reported any, or when its record knows none.
    counts: Option<Counts>,
    /// Whether the ledger's record of it arrived, which nothing replaces.
    recorded: bool,
}

/// One host's side of the board.
#[derive(Debug, Clone)]
struct Panel {
    level: f32,
    history: VecDeque<u64>,
    responses: Vec<Response>,
    turns: usize,
    seconds: f64,
    /// What the sessions billed by time cost, each priced on its own, and how many.
    session_cost: Money,
    sessions: usize,
}

impl Panel {
    fn response(&mut self, response_id: ResponseId) -> &mut Response {
        let at = if let Some(at) = self
            .responses
            .iter()
            .position(|response| response.response_id == response_id)
        {
            at
        } else {
            self.responses.push(Response {
                response_id,
                counts: None,
                recorded: false,
            });
            self.responses.len() - 1
        };
        &mut self.responses[at]
    }
}

/// An estimated cost: the sum of what could be priced, and how many responses or sessions were
/// priced and how many could not be, which makes the sum a lower bound.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Estimate {
    pub amount: Money,
    pub priced: usize,
    pub unpriced: usize,
}

impl Estimate {
    /// Whether the amount leaves something out.
    #[must_use]
    pub const fn lower_bound(&self) -> bool {
        self.unpriced > 0
    }
}

impl Add for Estimate {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            amount: self.amount + other.amount,
            priced: self.priced + other.priced,
            unpriced: self.unpriced + other.unpriced,
        }
    }
}

/// What the screen shows.
#[derive(Debug, Clone)]
pub struct Board {
    hosts: [Host; 2],
    title: String,
    limit: Duration,
    prices: Option<PriceTable>,
    reported: Reported,
    panels: [Panel; 2],
    texts: BTreeMap<usize, String>,
    playing: Option<(usize, usize)>,
    played: Duration,
    closing: bool,
    finished: bool,
    problem: Option<String>,
}

impl Board {
    /// An empty board of a show of `limit` audio, its cost estimated by `prices`, its counts
    /// those a provider that reports `reported` reports.
    #[must_use]
    pub fn new(
        hosts: [Host; 2],
        limit: Duration,
        prices: Option<PriceTable>,
        reported: Reported,
    ) -> Self {
        let panel = || Panel {
            level: 0.0,
            history: VecDeque::with_capacity(HISTORY),
            responses: Vec::new(),
            turns: 0,
            seconds: 0.0,
            session_cost: Money::default(),
            sessions: 0,
        };
        Self {
            hosts,
            title: String::new(),
            limit,
            prices,
            reported,
            panels: [panel(), panel()],
            texts: BTreeMap::new(),
            playing: None,
            played: Duration::ZERO,
            closing: false,
            finished: false,
            problem: None,
        }
    }

    /// The same board with `title` above the panels.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Takes in what `event` says.
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Generating { directive, .. } => {
                if matches!(directive, Directive::FinalWords | Directive::Close) {
                    self.closing = true;
                }
            }
            Event::Said { turn, text, .. } => {
                self.texts.entry(*turn).or_default().push_str(text);
            }
            Event::Generated {
                turn, transcript, ..
            } => {
                self.texts.insert(*turn, transcript.clone());
            }
            Event::Playing { host, turn, .. } => {
                self.playing = Some((*host, *turn));
                self.texts.retain(|kept, _| kept >= turn);
            }
            Event::Played { host, .. } => {
                if let Some(panel) = self.panels.get_mut(*host) {
                    panel.turns += 1;
                }
                self.playing = None;
            }
            Event::Tick {
                speaking,
                level,
                played,
            } => {
                self.played = *played;
                for (at, panel) in self.panels.iter_mut().enumerate() {
                    panel.level = if *speaking == Some(at) { *level } else { 0.0 };
                    if panel.history.len() == HISTORY {
                        panel.history.pop_front();
                    }
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    panel
                        .history
                        .push_back((meter(panel.level) * 100.0).round() as u64);
                }
            }
            Event::Usage {
                host,
                usage: Usage::Response { response_id, usage },
            } => {
                let counts = Counts::of(usage, self.reported);
                if let Some(panel) = self.panels.get_mut(*host) {
                    let response = panel.response(*response_id);
                    if !response.recorded {
                        response.counts = Some(counts);
                    }
                }
            }
            Event::Usage { .. } => {}
            Event::Recorded {
                host,
                response_id,
                counts,
            } => {
                let counts = masked(*counts, self.reported);
                if let Some(panel) = self.panels.get_mut(*host) {
                    let response = panel.response(*response_id);
                    response.counts = (counts != Counts::default()).then_some(counts);
                    response.recorded = true;
                }
            }
            Event::Ended { host, response_id } => {
                if let Some(panel) = self.panels.get_mut(*host) {
                    panel.response(*response_id);
                }
            }
            Event::Billed { host, seconds } => {
                let cost = self
                    .prices
                    .as_ref()
                    .map(|prices| prices.session_cost(*seconds))
                    .unwrap_or_default();
                if let Some(panel) = self.panels.get_mut(*host) {
                    panel.seconds += seconds;
                    panel.session_cost = panel.session_cost + cost;
                    panel.sessions += 1;
                }
            }
            Event::Problem { host, detail } => {
                self.problem = Some(match host.and_then(|host| self.hosts.get(host)) {
                    Some(host) => format!("{}: {detail}", host.name),
                    None => detail.clone(),
                });
            }
            Event::Finished { played } => {
                self.played = *played;
                self.finished = true;
                self.playing = None;
            }
        }
    }

    /// The hosts.
    #[must_use]
    pub const fn hosts(&self) -> &[Host; 2] {
        &self.hosts
    }

    /// The title above the panels.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The host whose turn is playing.
    #[must_use]
    pub fn speaking(&self) -> Option<usize> {
        self.playing.map(|(host, _)| host)
    }

    /// The transcript so far of the turn playing.
    #[must_use]
    pub fn current_text(&self) -> Option<&str> {
        let (_, turn) = self.playing?;
        self.texts.get(&turn).map(String::as_str)
    }

    /// How loud `host` is now, from 0 to 1, as the meter shows it.
    #[must_use]
    pub fn meter(&self, host: usize) -> f32 {
        self.panels
            .get(host)
            .map_or(0.0, |panel| meter(panel.level))
    }

    /// The recent meter readings of `host`, in percent.
    #[must_use]
    pub fn history(&self, host: usize) -> Vec<u64> {
        self.panels
            .get(host)
            .map(|panel| panel.history.iter().copied().collect())
            .unwrap_or_default()
    }

    /// The turns `host` played.
    #[must_use]
    pub fn turns(&self, host: usize) -> usize {
        self.panels.get(host).map_or(0, |panel| panel.turns)
    }

    /// The responses of `host` that have counts.
    fn counted(&self, host: usize) -> impl Iterator<Item = Counts> + '_ {
        self.panels.get(host).into_iter().flat_map(|panel| {
            panel
                .responses
                .iter()
                .filter_map(|response| response.counts)
        })
    }

    /// The token counts of `host`'s responses: each count the sum over the responses that have
    /// counts, unknown when one of them does not know it. Responses without usage are left
    /// out; [`Board::without_usage`] counts them.
    #[must_use]
    pub fn counts(&self, host: usize) -> Counts {
        self.counted(host).reduce(sum_known).unwrap_or_default()
    }

    /// The token counts of both hosts, summed as [`Board::counts`] sums.
    #[must_use]
    pub fn total_counts(&self) -> Counts {
        self.counted(0)
            .chain(self.counted(1))
            .reduce(sum_known)
            .unwrap_or_default()
    }

    /// The responses of `host` that ended or were recorded without any count known.
    #[must_use]
    pub fn without_usage(&self, host: usize) -> usize {
        self.panels.get(host).map_or(0, |panel| {
            panel
                .responses
                .iter()
                .filter(|response| response.counts.is_none())
                .count()
        })
    }

    /// The estimated cost of `host`'s part; `None` without a price table. By a table per token
    /// each response with counts it can price is priced, and every other response makes the
    /// estimate a lower bound; by a table per session minute each session is priced.
    #[must_use]
    pub fn cost(&self, host: usize) -> Option<Estimate> {
        let prices = self.prices.as_ref()?;
        let panel = self.panels.get(host)?;
        Some(match &prices.rates {
            Rates::Tokens(table) => panel.responses.iter().fold(
                Estimate::default(),
                |estimate, response| match response.counts.and_then(|counts| table.cost(&counts)) {
                    Some(cost) => Estimate {
                        amount: estimate.amount + Money::from_picos(cost.picos()),
                        priced: estimate.priced + 1,
                        ..estimate
                    },
                    None => Estimate {
                        unpriced: estimate.unpriced + 1,
                        ..estimate
                    },
                },
            ),
            Rates::SessionMinute(_) => Estimate {
                amount: panel.session_cost,
                priced: panel.sessions,
                unpriced: 0,
            },
        })
    }

    /// The estimated cost of the show so far, both hosts' estimates summed.
    #[must_use]
    pub fn total_cost(&self) -> Option<Estimate> {
        Some(self.cost(0)? + self.cost(1)?)
    }

    /// The price table.
    #[must_use]
    pub const fn prices(&self) -> Option<&PriceTable> {
        self.prices.as_ref()
    }

    /// The audio played so far.
    #[must_use]
    pub const fn played(&self) -> Duration {
        self.played
    }

    /// The audio still to play before the show closes.
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.limit.saturating_sub(self.played)
    }

    /// Whether the closing turns have begun.
    #[must_use]
    pub const fn closing(&self) -> bool {
        self.closing
    }

    /// Whether the show is over.
    #[must_use]
    pub const fn finished(&self) -> bool {
        self.finished
    }

    /// The last problem reported.
    #[must_use]
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The cost as shown: the amount and currency, `at least` before it when it is a lower
    /// bound, `?` when nothing was priced yet.
    #[must_use]
    pub fn shown_cost(&self, cost: Option<Estimate>) -> String {
        match (&self.prices, cost) {
            (Some(prices), Some(cost)) if cost.lower_bound() => {
                format!("at least {} {}", cost.amount, prices.currency)
            }
            (Some(prices), Some(cost)) if cost.priced > 0 => {
                format!("{} {}", cost.amount, prices.currency)
            }
            (Some(prices), _) => format!("? {}", prices.currency),
            (None, _) => "no price table".to_owned(),
        }
    }

    /// One counter line of `host`, or of both hosts with `None`: the counts, `(n without
    /// usage)` after them when responses have none, the turns and the cost.
    #[must_use]
    pub fn counter_line(&self, host: Option<usize>) -> String {
        let (counts, without, turns, cost) = match host {
            Some(host) => (
                self.counts(host),
                self.without_usage(host),
                self.turns(host),
                self.cost(host),
            ),
            None => (
                self.total_counts(),
                self.without_usage(0) + self.without_usage(1),
                self.turns(0) + self.turns(1),
                self.total_cost(),
            ),
        };
        let without = if without > 0 {
            format!(" ({without} without usage)")
        } else {
            String::new()
        };
        format!(
            "{counts}{without} · turns {turns} · {}",
            self.shown_cost(cost)
        )
    }

    /// The usage of the show, per host and in total, with the price table, as lines.
    #[must_use]
    pub fn summary(&self) -> Vec<String> {
        let mut lines: Vec<String> = (0..2)
            .map(|host| {
                format!(
                    "{}: {} · session {:.1} s",
                    self.hosts[host].name,
                    self.counter_line(Some(host)),
                    self.panels[host].seconds
                )
            })
            .collect();
        lines.push(format!("total: {}", self.counter_line(None)));
        lines.push(format!("audio played: {}", clock(self.played)));
        if let Some(prices) = &self.prices {
            lines.push(format!("cost estimated by {prices}"));
        }
        if let Some(cost) = self.total_cost().filter(Estimate::lower_bound) {
            lines.push(format!(
                "the cost is a lower bound: {} responses have no usage or a count the price \
                 table needs unknown",
                cost.unpriced
            ));
        }
        lines
    }
}

/// `counts` with the counts a provider that reports `reported` does not report unknown.
const fn masked(counts: Counts, reported: Reported) -> Counts {
    const fn known(reported: bool, count: Option<u64>) -> Option<u64> {
        if reported { count } else { None }
    }
    Counts {
        input_text: known(reported.input_text, counts.input_text),
        input_audio: known(reported.input_audio, counts.input_audio),
        input_cached: known(reported.input_cached, counts.input_cached),
        output_text: known(reported.output_text, counts.output_text),
        output_audio: known(reported.output_audio, counts.output_audio),
    }
}

/// The sum, count by count, known only where both know it.
fn sum_known(left: Counts, right: Counts) -> Counts {
    let add = |left: Option<u64>, right: Option<u64>| Some(left?.saturating_add(right?));
    Counts {
        input_text: add(left.input_text, right.input_text),
        input_audio: add(left.input_audio, right.input_audio),
        input_cached: add(left.input_cached, right.input_cached),
        output_text: add(left.output_text, right.output_text),
        output_audio: add(left.output_audio, right.output_audio),
    }
}

/// A level as the meter shows it: speech is quiet in root mean square, so it is scaled up.
fn meter(level: f32) -> f32 {
    (level * 4.0).clamp(0.0, 1.0)
}

/// `mm:ss`.
#[must_use]
pub(crate) fn clock(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// `event` as a line for plain mode, `None` for an event that makes none: each turn's
/// transcript when it is generated, each response's usage, the closing turns, problems and the
/// end.
#[must_use]
pub fn plain_line(board: &Board, event: &Event) -> Option<String> {
    let name = |host: usize| board.hosts.get(host).map_or("?", |host| host.name.as_str());
    match event {
        Event::Generating {
            host, directive, ..
        } => match directive {
            Directive::Open => Some(format!("[{} opens the show]", name(*host))),
            Directive::FinalWords => Some(format!("[time is up: {}'s final words]", name(*host))),
            Directive::Close => Some(format!("[{} closes the show]", name(*host))),
            Directive::Reply => None,
        },
        Event::Generated {
            host, transcript, ..
        } if !transcript.is_empty() => Some(format!("{}: {transcript}", name(*host))),
        Event::Ended { host, response_id } => {
            let panel = board.panels.get(*host)?;
            let counts = panel
                .responses
                .iter()
                .find(|response| response.response_id == *response_id)?
                .counts?;
            let cost = board
                .prices
                .as_ref()
                .and_then(|prices| match &prices.rates {
                    Rates::Tokens(table) => table.cost(&counts),
                    Rates::SessionMinute(_) => None,
                });
            Some(match (cost, &board.prices) {
                (Some(cost), Some(prices)) => format!(
                    "[{} usage: {counts}; cost {cost} {}]",
                    name(*host),
                    prices.currency
                ),
                _ => format!("[{} usage: {counts}]", name(*host)),
            })
        }
        Event::Problem { host, detail } => Some(match host {
            Some(host) => format!("[problem, {}: {detail}]", name(*host)),
            None => format!("[problem: {detail}]"),
        }),
        Event::Finished { played } => Some(format!(
            "[the show is over: {} of audio played]",
            clock(*played)
        )),
        _ => None,
    }
}
