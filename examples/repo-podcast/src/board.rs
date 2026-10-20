//! What the screen shows, fed with the show's events: who speaks and how loud, the transcript
//! of the turn playing, and running counters per host and in total, with the cost by a price
//! table. [`plain_line`] gives the same events as lines.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use realtime_voice::{Counts, Ending, Note, Reported, Tally};

use crate::hosts::{Directive, Host};
use crate::prices::{Money, PriceTable};
use crate::show::Event;

/// The meter history kept per host.
const HISTORY: usize = 96;

/// One host's side of the board.
#[derive(Debug, Clone)]
struct Panel {
    level: f32,
    history: VecDeque<u64>,
    tally: Tally,
    turns: usize,
    seconds: f64,
}

/// What the screen shows.
#[derive(Debug, Clone)]
pub struct Board {
    hosts: [Host; 2],
    title: String,
    limit: Duration,
    prices: Option<PriceTable>,
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
            tally: Tally::new(reported),
            turns: 0,
            seconds: 0.0,
        };
        Self {
            hosts,
            title: String::new(),
            limit,
            prices,
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
            Event::Usage { host, usage } => {
                if let Some(panel) = self.panels.get_mut(*host) {
                    panel.tally.record(&Note::Usage(*usage));
                }
            }
            Event::Ended { host, response_id } => {
                if let Some(panel) = self.panels.get_mut(*host) {
                    panel.tally.record(&Note::Ended {
                        response_id: *response_id,
                        how: Ending::Completed,
                    });
                }
            }
            Event::Billed { host, seconds } => {
                if let Some(panel) = self.panels.get_mut(*host) {
                    panel.seconds += seconds;
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

    /// The token counts of `host`'s responses.
    #[must_use]
    pub fn counts(&self, host: usize) -> Counts {
        self.panels
            .get(host)
            .map(|panel| panel.tally.totals().counts)
            .unwrap_or_default()
    }

    /// The token counts of both hosts.
    #[must_use]
    pub fn total_counts(&self) -> Counts {
        self.counts(0).plus(self.counts(1))
    }

    /// The estimated cost of `host`'s part; `None` without a price table or with a count it
    /// needs unknown.
    #[must_use]
    pub fn cost(&self, host: usize) -> Option<Money> {
        let prices = self.prices.as_ref()?;
        let panel = self.panels.get(host)?;
        prices.cost(&panel.tally, panel.seconds)
    }

    /// The estimated cost of the show so far: the sum of the hosts' costs that are known.
    #[must_use]
    pub fn total_cost(&self) -> Option<Money> {
        match (self.cost(0), self.cost(1)) {
            (Some(left), Some(right)) => Some(left + right),
            (known, None) | (None, known) => known,
        }
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

    /// The cost as shown: the amount and currency, `?` when unknown.
    #[must_use]
    pub fn shown_cost(&self, cost: Option<Money>) -> String {
        match (&self.prices, cost) {
            (Some(prices), Some(cost)) => format!("{cost} {}", prices.currency),
            (Some(prices), None) => format!("? {}", prices.currency),
            (None, _) => "no price table".to_owned(),
        }
    }

    /// One counter line of `host`, or of both hosts with `None`.
    #[must_use]
    pub fn counter_line(&self, host: Option<usize>) -> String {
        match host {
            Some(host) => format!(
                "{} · turns {} · {}",
                self.counts(host),
                self.turns(host),
                self.shown_cost(self.cost(host))
            ),
            None => format!(
                "{} · turns {} · {}",
                self.total_counts(),
                self.turns(0) + self.turns(1),
                self.shown_cost(self.total_cost())
            ),
        }
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
        lines
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
            let counts = panel.tally.response(*response_id)?;
            let cost = board
                .prices
                .as_ref()
                .and_then(|prices| match &prices.rates {
                    crate::prices::Rates::Tokens(table) => table.cost(&counts),
                    crate::prices::Rates::SessionMinute(_) => None,
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
