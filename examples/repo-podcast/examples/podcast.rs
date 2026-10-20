//! A podcast about a repository, played on the default output: Claude distils the directory
//! into a brief, then Maya, the host, and Theo, the guest, talk about it until `--minutes` of
//! audio have played, and Maya closes the show.
//!
//! ```text
//! cargo run -p repo-podcast --example podcast
//! cargo run -p repo-podcast --example podcast -- --dir ../other-repo --minutes 10
//! cargo run -p repo-podcast --example podcast -- --save-context brief.md
//! cargo run -p repo-podcast --example podcast -- --context-file brief.md --plain
//! cargo run -p repo-podcast --example podcast -- --provider openai-live
//! ```
//!
//! The brief comes from the Claude Code CLI (`claude -p`, read-only tools, run in `--dir`,
//! which defaults to this repository), or from `--context-file`, which skips that step;
//! `--save-context` keeps it for the next run. `--claude` names another CLI binary. The title
//! names the repository (its `origin` remote's name, or its top-level directory's), or
//! `--title <name>`. `--speed` sets how fast the OpenAI Realtime voices speak, 0.9 of the
//! model's own pace by default.
//!
//! OpenAI, both the Realtime API (`openai`, `INFERENCE_OPENAI_REALTIME_MODEL` names another
//! model) and GPT-Live (`openai-live`), reads its key from the OS keychain, secret
//! `openai-api-key`, or with `INFERENCE_OPENAI_KEY_SOURCE=env` from `OPENAI_API_KEY`. The key is
//! never printed.
//!
//! The terminal UI shows both hosts, a level meter on the one speaking, the transcript of the
//! turn playing, and running counters of tokens, turns, time and estimated cost. The cost is
//! estimated by the shipped price table of the provider's default model (`prices/`), or by
//! `--prices <file>`. `--plain` prints the transcript and usage as lines instead. Enter (or q,
//! Esc, Ctrl-C in the terminal UI) ends the show early; the usage per host and in total is
//! printed at the end.

use std::error::Error;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use inference_audio_device::CpalHost;
use inference_duplex::NativeSpeechModel;
use inference_openai::duplex::OpenAiRealtime;
use inference_openai::live_duplex::{OpenAiLive, OpenAiLiveSession};
use inference_openai::{
    CredentialSource, DEFAULT_MODEL, Endpoint, EnvironmentSource, KeychainSource, OPENAI_KEY_NAME,
};
use ratatui::crossterm::event::{self, Event as Terminal, KeyCode, KeyEventKind, KeyModifiers};
use repo_podcast::{
    BRIEF_LIMIT, Board, DISTIL_CUT, DISTIL_LIMIT, LIVE_PRICES, PriceTable, REALTIME_PRICES,
    Reported, Settings, Show, brief, default_hosts, draw, plain_line, project_name,
};

/// The variable that switches the OpenAI key from the keychain to `OPENAI_API_KEY`.
const KEY_SOURCE_VARIABLE: &str = "INFERENCE_OPENAI_KEY_SOURCE";

/// How often the terminal UI redraws.
const FRAME: Duration = Duration::from_millis(40);

/// A podcast about a repository: Claude distils it, two voice models discuss it.
#[derive(Debug, Parser)]
struct Args {
    /// The directory the show is about; this repository by default.
    #[arg(long, value_name = "PATH")]
    dir: Option<PathBuf>,
    /// The minutes of audio the show runs before the host closes it.
    #[arg(long, default_value_t = 5.0)]
    minutes: f64,
    /// A brief saved before; nothing is distilled.
    #[arg(long, value_name = "PATH")]
    context_file: Option<PathBuf>,
    /// Where to save the brief for later runs.
    #[arg(long, value_name = "PATH")]
    save_context: Option<PathBuf>,
    /// The voice models.
    #[arg(long, value_enum, default_value_t = Provider::Openai)]
    provider: Provider,
    /// A JSON price table; the provider's shipped table by default.
    #[arg(long, value_name = "FILE")]
    prices: Option<PathBuf>,
    /// Print lines instead of the terminal UI.
    #[arg(long)]
    plain: bool,
    /// The Claude Code CLI to distil with.
    #[arg(long, value_name = "PATH", default_value = "claude")]
    claude: PathBuf,
    /// The name in the title; the repository's name by default (its `origin` remote's, or its
    /// top-level directory's).
    #[arg(long, value_name = "NAME")]
    title: Option<String>,
    /// How fast the OpenAI Realtime voices speak, as a multiple of the model's own pace, 0.25
    /// to 1.5 (GPT-Live takes no speed).
    #[arg(long, default_value_t = 0.9)]
    speed: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Provider {
    /// The OpenAI Realtime API.
    Openai,
    /// OpenAI GPT-Live.
    OpenaiLive,
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), Box<dyn Error>> {
    if !(args.minutes.is_finite() && args.minutes > 0.0) {
        return Err("--minutes must be a positive number".into());
    }
    if !(0.25..=1.5).contains(&args.speed) {
        return Err("--speed must be from 0.25 to 1.5".into());
    }
    let dir = match &args.dir {
        Some(dir) => dir.clone(),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
    };
    let dir = dir
        .canonicalize()
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    let endpoint = Endpoint::from_env();
    let prices = prices(&args, &endpoint)?;
    if args.context_file.is_none() {
        eprintln!(
            "distilling {} with {} (read-only tools); this takes a minute or two",
            dir.display(),
            args.claude.display()
        );
    }
    let brief = brief(
        args.context_file.as_deref(),
        args.save_context.as_deref(),
        &args.claude,
        &dir,
    )?;
    if let Some(saved) = &args.save_context {
        eprintln!("the brief is saved in {}", saved.display());
    }
    if brief.ends_with(DISTIL_CUT) {
        eprintln!(
            "claude printed more than {} KiB; the brief is cut there",
            DISTIL_LIMIT / 1024
        );
    }
    if brief.trim().len() > BRIEF_LIMIT {
        eprintln!(
            "the brief has {} bytes; the hosts are given its first {} KiB",
            brief.trim().len(),
            BRIEF_LIMIT / 1024
        );
    }
    let name = args.title.clone().unwrap_or_else(|| project_name(&dir));
    let settings = Settings {
        limit: Duration::from_secs_f64(args.minutes * 60.0),
        ..Settings::default()
    };
    let host = CpalHost::new();
    match args.provider {
        Provider::Openai => {
            let title = format!("{name} · OpenAI {}", endpoint.model);
            let model = OpenAiRealtime::new(endpoint, openai_key()?, OPENAI_KEY_NAME)
                .with_input_transcription(None)
                .with_output_speed(Some(args.speed));
            let board = Board::new(default_hosts(), settings.limit, prices, Reported::OPENAI)
                .with_title(title);
            play(model, brief, &host, settings, board, args.plain, |_| None)
        }
        Provider::OpenaiLive => {
            let model = OpenAiLive::hosted(openai_key()?, OPENAI_KEY_NAME);
            let board = Board::new(
                default_hosts(),
                settings.limit,
                prices,
                Reported::OPENAI_LIVE,
            )
            .with_title(format!("{name} · OpenAI GPT-Live"));
            play(
                model,
                brief,
                &host,
                settings,
                board,
                args.plain,
                OpenAiLiveSession::voice_seconds,
            )
        }
    }
}

/// The price table: `--prices`, or the shipped table of the provider's default model.
fn prices(args: &Args, endpoint: &Endpoint) -> Result<Option<PriceTable>, Box<dyn Error>> {
    let text = match (&args.prices, args.provider) {
        (Some(path), _) => std::fs::read_to_string(path)
            .map_err(|error| format!("the price table {}: {error}", path.display()))?,
        (None, Provider::Openai) if endpoint.model == DEFAULT_MODEL => REALTIME_PRICES.to_owned(),
        (None, Provider::Openai) => return Ok(None),
        (None, Provider::OpenaiLive) => LIVE_PRICES.to_owned(),
    };
    Ok(Some(PriceTable::parse(&text)?))
}

/// Where the OpenAI key is read: the keychain, or the environment.
fn openai_key() -> Result<Box<dyn CredentialSource>, Box<dyn Error>> {
    Ok(
        if std::env::var(KEY_SOURCE_VARIABLE).as_deref() == Ok("env") {
            Box::new(EnvironmentSource::openai())
        } else {
            Box::new(KeychainSource::native()?)
        },
    )
}

/// Runs the show to its end or until it is stopped, then prints the usage.
fn play<M: NativeSpeechModel>(
    model: M,
    brief: String,
    host: &CpalHost,
    settings: Settings,
    mut board: Board,
    plain: bool,
    billed: fn(&M::Session) -> Option<f64>,
) -> Result<(), Box<dyn Error>> {
    let mut show = Show::open(model, default_hosts(), brief, host, settings, billed)?;
    if plain {
        println!(
            "playing on {:?}; Enter ends the show",
            show.speaker().device().name
        );
        let enter = enter_pressed();
        while !show.is_over() {
            let events = if enter.load(Ordering::Relaxed) {
                show.stop()
            } else {
                show.step()
            };
            for event in &events {
                board.apply(event);
                if let Some(line) = plain_line(&board, event) {
                    println!("{line}");
                }
            }
        }
    } else {
        let outcome = {
            let mut screen = Screen(ratatui::init());
            tui(&mut show, &mut board, &mut screen.0)
        };
        if outcome.is_err() {
            let _ = show.stop();
        }
        outcome?;
        if let Some(problem) = board.problem() {
            println!("problem: {problem}");
        }
    }
    for line in board.summary() {
        println!("{line}");
    }
    Ok(())
}

/// The terminal in the UI's mode, given back to the shell when it is dropped, on every way out
/// of the UI: the end of the show, a draw or key-poll error, or a panic.
struct Screen(ratatui::DefaultTerminal);

impl Drop for Screen {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// Runs the show in the terminal UI until it is over or stopped, and shows the last frame
/// for a moment.
fn tui<M: NativeSpeechModel>(
    show: &mut Show<M>,
    board: &mut Board,
    terminal: &mut ratatui::DefaultTerminal,
) -> Result<(), Box<dyn Error>> {
    let mut drawn = Instant::now() - FRAME;
    loop {
        let events = if stop_pressed()? {
            show.stop()
        } else {
            show.step()
        };
        for event in &events {
            board.apply(event);
        }
        if show.is_over() || drawn.elapsed() >= FRAME {
            terminal.draw(|frame| draw(frame, board))?;
            drawn = Instant::now();
        }
        if show.is_over() {
            std::thread::sleep(Duration::from_millis(800));
            return Ok(());
        }
    }
}

/// Whether Enter, q, Esc or Ctrl-C was pressed in the terminal UI.
fn stop_pressed() -> Result<bool, Box<dyn Error>> {
    while event::poll(Duration::ZERO)? {
        if let Terminal::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            let ctrl_c =
                key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
            if ctrl_c || matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// A flag that turns on when a line is read from standard input.
fn enter_pressed() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let set = Arc::clone(&flag);
    std::thread::spawn(move || {
        let mut line = String::new();
        if let Ok(read) = std::io::stdin().lock().read_line(&mut line)
            && read > 0
        {
            set.store(true, Ordering::Relaxed);
        }
    });
    flag
}
