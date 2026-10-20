//! Adversary pass 1 on the terminal UI: every terminal size from nothing up, with a host
//! speaking loud and a long transcript, draws without a panic.

use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use realtime_voice::Reported;
use repo_podcast::{Board, Directive, Event, PriceTable, REALTIME_PRICES, default_hosts, draw};

fn speaking_board() -> Board {
    let table = PriceTable::parse(REALTIME_PRICES).expect("the shipped table");
    let mut board = Board::new(
        default_hosts(),
        Duration::from_secs(300),
        Some(table),
        Reported::OPENAI,
    )
    .with_title("a title long enough to be cut on a narrow terminal ".repeat(4));
    for event in [
        Event::Generating {
            host: 1,
            turn: 0,
            directive: Directive::Close,
        },
        Event::Said {
            host: 1,
            turn: 0,
            text: "a long turn ".repeat(200),
        },
        Event::Playing {
            host: 1,
            turn: 0,
            queued: 0,
        },
        Event::Problem {
            host: Some(1),
            detail: "a problem ".repeat(30),
        },
    ] {
        board.apply(&event);
    }
    for _ in 0..200 {
        board.apply(&Event::Tick {
            speaking: Some(1),
            level: 1.0,
            played: Duration::from_secs(400),
        });
    }
    board
}

#[test]
fn every_small_terminal_size_draws() {
    let board = speaking_board();
    for width in 0..=40_u16 {
        for height in 0..=20_u16 {
            let mut terminal =
                Terminal::new(TestBackend::new(width, height)).expect("the test backend");
            terminal
                .draw(|frame| draw(frame, &board))
                .unwrap_or_else(|error| panic!("{width}x{height}: {error}"));
        }
    }
}
