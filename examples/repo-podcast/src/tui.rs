//! The terminal UI: a title, one panel per host (name, voice, role, a level meter that moves
//! with the audio it plays and the transcript of its turn), and a footer of running counters.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Sparkline, Wrap};

use crate::board::{Board, clock};
use crate::hosts::Role;

/// The lines of the footer inside its border.
const FOOTER_LINES: u16 = 6;

/// Draws `board` over the whole frame.
pub fn draw(frame: &mut Frame<'_>, board: &Board) {
    let [title, panels, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(FOOTER_LINES + 2),
    ])
    .areas(frame.area());
    let heading = if board.title().is_empty() {
        "repo-podcast".to_owned()
    } else {
        format!("repo-podcast · {}", board.title())
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            heading,
            Style::new().add_modifier(Modifier::BOLD),
        ))),
        title,
    );
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(panels);
    panel(frame, board, 0, left);
    panel(frame, board, 1, right);
    counters(frame, board, footer);
}

/// The panel of `host`.
fn panel(frame: &mut Frame<'_>, board: &Board, host: usize, area: Rect) {
    let who = &board.hosts()[host];
    let role = match who.role {
        Role::Host => "host",
        Role::Guest => "guest",
    };
    let speaking = board.speaking() == Some(host);
    let colour = if speaking {
        Color::Green
    } else {
        Color::DarkGray
    };
    let block = Block::bordered()
        .border_style(Style::new().fg(colour))
        .title(format!(" {} · {} · {role} ", who.name, who.voice));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [status, bar, wave, text] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(1),
    ])
    .areas(inner);
    let width = usize::from(bar.width);
    if speaking {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "▶ speaking",
                Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
            )),
            status,
        );
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let filled = ((board.meter(host) * width as f32).round() as usize).min(width);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("█".repeat(filled), Style::new().fg(Color::Green)),
                Span::styled("░".repeat(width - filled), Style::new().fg(Color::DarkGray)),
            ])),
            bar,
        );
        let history = board.history(host);
        let shown = &history[history.len().saturating_sub(usize::from(wave.width))..];
        frame.render_widget(
            Sparkline::default()
                .data(shown)
                .max(100)
                .style(Style::new().fg(Color::Green)),
            wave,
        );
        frame.render_widget(
            Paragraph::new(board.current_text().unwrap_or_default().to_owned())
                .wrap(Wrap { trim: true }),
            text,
        );
    } else {
        frame.render_widget(
            Paragraph::new(Span::styled("listening", Style::new().fg(Color::DarkGray))),
            status,
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                "·".repeat(width),
                Style::new().fg(Color::DarkGray),
            )),
            bar,
        );
    }
}

/// The footer: counters per host and in total, time, the price table and the state.
fn counters(frame: &mut Frame<'_>, board: &Board, area: Rect) {
    let hosts = board.hosts();
    let label = |name: &str| format!("{name:<7}");
    let mut lines = vec![
        Line::from(format!(
            "{}{}",
            label(&hosts[0].name),
            board.counter_line(Some(0))
        )),
        Line::from(format!(
            "{}{}",
            label(&hosts[1].name),
            board.counter_line(Some(1))
        )),
        Line::from(Span::styled(
            format!("{}{}", label("total"), board.counter_line(None)),
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "{} elapsed · {} left · cost {} · Enter ends",
            clock(board.played()),
            clock(board.remaining()),
            board.shown_cost(board.total_cost())
        )),
        Line::from(match board.prices() {
            Some(prices) => format!("prices: {prices}"),
            None => "prices: none given (--prices <file>)".to_owned(),
        }),
    ];
    let state = if let Some(problem) = board.problem() {
        Span::styled(format!("problem: {problem}"), Style::new().fg(Color::Red))
    } else if board.finished() {
        Span::raw("the show is over")
    } else if board.closing() {
        Span::raw("closing the show")
    } else {
        Span::raw("")
    };
    lines.push(Line::from(state));
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" tokens and cost ")),
        area,
    );
}
