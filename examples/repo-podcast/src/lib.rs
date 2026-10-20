//! A podcast about a repository: Claude distils the repository into a brief, and two voice
//! models, a curious host and an expert guest, discuss it over the speaker while a terminal UI
//! shows both sides, the transcript, the tokens and the cost.
//!
//! ```text
//! cargo run -p repo-podcast --example podcast
//! cargo run -p repo-podcast --example podcast -- --dir <path> --minutes 10 --provider openai-live
//! cargo run -p repo-podcast --example podcast -- --context-file brief.md --plain
//! ```
//!
//! [`brief`] runs the Claude Code CLI (`claude -p`, read-only tools) in the directory and keeps
//! what it prints, or reads a brief saved before. [`Show`] holds the conversation and is generic
//! over the speech model and the audio host, so the tests drive it with the testkit's duplex
//! mock and the device adapter's fake host. [`Board`] is what the screen shows, fed with the
//! show's [`Event`]s; [`draw`] renders it with ratatui and [`plain_line`] prints it as lines.
//!
//! Each turn is one `inference-duplex` session of its host, opened with the host's voice and
//! persona, the brief and the conversation so far as text in its instructions; the duplex
//! contract takes no text input, so the instructions carry the other host's transcript. The
//! next turn is generated while the current one plays, and the speaker plays one turn at a
//! time.

mod board;
mod brief;
mod hosts;
mod prices;
mod show;
mod tui;

pub use board::{Board, plain_line};
pub use brief::{DISTIL_PROMPT, DistilError, brief, distil, distil_command};
pub use hosts::{Directive, Host, Line, Role, default_hosts, instructions, turn_config};
pub use prices::{LIVE_PRICES, Money, PriceTable, PriceTableError, REALTIME_PRICES, Rates};
pub use realtime_voice::{Counts, Reported, Usage};
pub use show::{Event, Settings, Show, level};
pub use tui::draw;
