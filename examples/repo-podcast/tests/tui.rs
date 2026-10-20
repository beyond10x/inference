//! The terminal UI against ratatui's test backend, and the plain lines of the same events.

use std::time::Duration;

use inference_duplex::{ResponseId, ResponseUsage, TokenCounts};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use realtime_voice::{Counts, Reported, Usage};
use repo_podcast::{
    Board, Directive, Event, LIVE_PRICES, PriceTable, REALTIME_PRICES, default_hosts, draw,
    plain_line,
};

const WIDTH: u16 = 140;
const HEIGHT: u16 = 34;

fn usage(input: (u64, u64, u64), output: (u64, u64)) -> ResponseUsage {
    ResponseUsage {
        input: TokenCounts {
            text_tokens: input.0,
            audio_tokens: input.1,
            cached_tokens: input.2,
        },
        output: TokenCounts {
            text_tokens: output.0,
            audio_tokens: output.1,
            cached_tokens: 0,
        },
    }
}

fn reported(host: usize, counts: ResponseUsage) -> [Event; 2] {
    let response_id = ResponseId::generate();
    [
        Event::Usage {
            host,
            usage: Usage::Response {
                response_id,
                usage: counts,
            },
        },
        Event::Ended { host, response_id },
    ]
}

/// A board five minutes long, 30 s in, Maya speaking, with usage of both hosts: Maya two
/// responses of 20/50/0 in and 8/30 out, Theo one of 100/0/40 in and 12/200 out.
fn board() -> Board {
    let table = PriceTable::parse(REALTIME_PRICES).expect("the shipped table");
    let mut board = Board::new(
        default_hosts(),
        Duration::from_secs(300),
        Some(table),
        Reported::OPENAI,
    );
    let mut events = vec![
        Event::Generating {
            host: 0,
            turn: 0,
            directive: Directive::Open,
        },
        Event::Said {
            host: 0,
            turn: 0,
            text: "Hello and welcome to the show".to_owned(),
        },
        Event::Playing {
            host: 0,
            turn: 0,
            queued: 0,
        },
        Event::Played {
            host: 0,
            turn: 0,
            audio: Duration::from_secs(10),
        },
        Event::Playing {
            host: 1,
            turn: 1,
            queued: 0,
        },
        Event::Played {
            host: 1,
            turn: 1,
            audio: Duration::from_secs(12),
        },
        Event::Said {
            host: 0,
            turn: 2,
            text: "So what surprised you most".to_owned(),
        },
        Event::Playing {
            host: 0,
            turn: 2,
            queued: 0,
        },
        Event::Tick {
            speaking: Some(0),
            level: 0.8,
            played: Duration::from_secs(30),
        },
    ];
    events.extend(reported(0, usage((20, 50, 0), (8, 30))));
    events.extend(reported(0, usage((20, 50, 0), (8, 30))));
    events.extend(reported(1, usage((100, 0, 40), (12, 200))));
    for event in &events {
        board.apply(event);
    }
    board
}

fn rendered(board: &Board) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("a test terminal");
    terminal
        .draw(|frame| draw(frame, board))
        .expect("the board draws");
    let buffer = terminal.backend().buffer().clone();
    (0..HEIGHT)
        .map(|y| {
            (0..WIDTH)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect()
}

fn halves(rows: &[String]) -> (String, String) {
    let half = usize::from(WIDTH / 2);
    let mut left = String::new();
    let mut right = String::new();
    for row in rows {
        let chars: Vec<char> = row.chars().collect();
        left.extend(&chars[..half]);
        left.push('\n');
        right.extend(&chars[half..]);
        right.push('\n');
    }
    (left, right)
}

#[test]
fn both_panels_show_their_host_the_speaker_active_and_the_listener_idle() {
    let rows = rendered(&board());
    let screen = rows.join("\n");
    let (left, right) = halves(&rows);
    assert!(left.contains("Maya") && left.contains("marin"), "{screen}");
    assert!(
        right.contains("Theo") && right.contains("cedar"),
        "{screen}"
    );
    assert!(left.contains("speaking"), "{screen}");
    assert!(right.contains("listening"), "{screen}");
    assert!(
        left.matches('█').count() > 10,
        "the speaker's meter is active:\n{screen}"
    );
    assert_eq!(
        right.matches('█').count(),
        0,
        "the listener's meter is idle:\n{screen}"
    );
    assert!(
        left.contains("So what surprised you most"),
        "the current turn's transcript under its speaker:\n{screen}"
    );
    assert!(
        !left.contains("Hello and welcome"),
        "an earlier turn's transcript is gone:\n{screen}"
    );
}

#[test]
fn the_counters_are_the_sums_of_the_usage_events() {
    let screen = rendered(&board()).join("\n");
    for expected in [
        "in text 40 audio 100 cached 0, out text 16 audio 60",
        "in text 100 audio 0 cached 40, out text 12 audio 200",
        "in text 140 audio 100 cached 40, out text 28 audio 260",
        "0.007584 USD",
        "0.013344 USD",
        "0.020928 USD",
        "(2026-10-20)",
        "00:30",
        "04:30 left",
    ] {
        assert!(screen.contains(expected), "{expected:?} missing:\n{screen}");
    }
    let board = board();
    assert_eq!(board.turns(0), 1);
    assert_eq!(board.turns(1), 1);
}

#[test]
fn session_time_is_priced_per_second() {
    let table = PriceTable::parse(LIVE_PRICES).expect("the shipped table");
    let mut board = Board::new(
        default_hosts(),
        Duration::from_secs(300),
        Some(table),
        Reported::OPENAI_LIVE,
    );
    board.apply(&Event::Billed {
        host: 0,
        seconds: 30.0,
    });
    board.apply(&Event::Billed {
        host: 1,
        seconds: 60.0,
    });
    let screen = rendered(&board).join("\n");
    assert!(screen.contains("0.025000 USD"), "{screen}");
    assert!(screen.contains("0.050000 USD"), "{screen}");
    assert!(screen.contains("0.075000 USD"), "{screen}");
}

#[test]
fn plain_mode_prints_the_same_events_as_lines() {
    let board = board();
    assert_eq!(
        plain_line(
            &board,
            &Event::Generated {
                host: 1,
                turn: 3,
                transcript: "It surprised me".to_owned(),
            }
        ),
        Some("Theo: It surprised me".to_owned())
    );
    let line = plain_line(
        &board,
        &Event::Generating {
            host: 0,
            turn: 9,
            directive: Directive::Close,
        },
    )
    .expect("the close is announced");
    assert!(line.contains("Maya") && line.contains("close"), "{line}");
    assert_eq!(
        plain_line(
            &board,
            &Event::Tick {
                speaking: Some(0),
                level: 0.5,
                played: Duration::from_secs(31),
            }
        ),
        None,
        "the meter is not a line"
    );
    let summary = board.summary().join("\n");
    assert!(
        summary.contains("Maya: in text 40 audio 100 cached 0, out text 16 audio 60"),
        "{summary}"
    );
    assert!(summary.contains("0.020928 USD"), "{summary}");
}

/// The counts of a usage record: `None` for a count nobody counted.
fn counts(known: [Option<u64>; 5]) -> Counts {
    Counts {
        input_text: known[0],
        input_audio: known[1],
        input_cached: known[2],
        output_text: known[3],
        output_audio: known[4],
    }
}

#[test]
fn a_record_replaces_reported_usage_and_one_without_counts_is_apart_and_a_lower_bound() {
    let mut board = board();
    let response_id = ResponseId::generate();
    for event in reported(1, usage((1, 1, 0), (1, 1))) {
        board.apply(&event);
    }
    let stalled = ResponseId::generate();
    board.apply(&Event::Usage {
        host: 1,
        usage: Usage::Response {
            response_id: stalled,
            usage: usage((5, 5, 0), (5, 5)),
        },
    });
    board.apply(&Event::Recorded {
        host: 1,
        response_id: stalled,
        counts: counts([None; 5]),
    });
    board.apply(&Event::Recorded {
        host: 0,
        response_id,
        counts: counts([Some(10), None, Some(0), Some(2), Some(3)]),
    });
    let theo = board.counter_line(Some(1));
    assert!(theo.contains("(1 without usage)"), "{theo}");
    assert!(theo.contains("at least"), "{theo}");
    assert!(
        theo.contains("in text 101 audio 1 cached 40, out text 13 audio 201"),
        "the stalled response's reported usage gave way to its record: {theo}"
    );
    let maya = board.counter_line(Some(0));
    assert!(
        maya.contains("in text 50 audio ? cached 0, out text 18 audio 63"),
        "a count one response left unknown is unknown in the sum: {maya}"
    );
    assert!(maya.contains("at least"), "{maya}");
    let total = board.counter_line(None);
    assert!(total.contains("(1 without usage)"), "{total}");
    assert!(total.contains("at least"), "{total}");
    let summary = board.summary().join("\n");
    assert!(summary.contains("the cost is a lower bound"), "{summary}");
    let screen = rendered(&board).join("\n");
    assert!(screen.contains("without usage"), "{screen}");
}

#[test]
fn each_session_is_billed_in_whole_seconds_rounded_up() {
    let table = PriceTable::parse(LIVE_PRICES).expect("the shipped table");
    let mut board = Board::new(
        default_hosts(),
        Duration::from_secs(300),
        Some(table),
        Reported::OPENAI_LIVE,
    );
    for seconds in [0.4, 0.4, 0.4] {
        board.apply(&Event::Billed { host: 0, seconds });
    }
    let line = board.counter_line(Some(0));
    assert!(
        line.contains("0.002500 USD"),
        "three sessions of 0.4 s are billed 1 s each: {line}"
    );
}
