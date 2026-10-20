//! The brief the hosts discuss: distilled from a directory by the Claude Code CLI, or read from
//! a file saved before.
//!
//! The CLI is an external process the example talks to: `claude -p` with [`DISTIL_PROMPT`],
//! run in the directory, its tools limited to reading (`Read`, `Glob`, `Grep`), no permission
//! prompts, only the user's own settings loaded (`--setting-sources user`, so the directory's
//! project settings and hooks are not) and no MCP servers, plain text out. What it prints is
//! the brief, its first 64 KiB.

use std::error::Error;
use std::fmt;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The tools the CLI may use: reading only.
const READ_ONLY_TOOLS: &str = "Read,Glob,Grep";

/// The most bytes of the CLI's output kept, on each stream.
pub const DISTIL_LIMIT: usize = 64 * 1024;

/// What a brief cut at [`DISTIL_LIMIT`] ends in.
pub const DISTIL_CUT: &str = "\n[the brief is cut here: claude printed more than 64 KiB]\n";

/// What `claude -p` is asked.
pub const DISTIL_PROMPT: &str = "You are preparing the brief for a podcast episode about the \
     project in the current directory. Read the project (its README, its design documents, its \
     source layout and a few central source files) and write a brief of at most 900 words, in \
     plain prose and short lists, no code blocks and no Markdown tables, covering: what the \
     project is; why it exists and who it is for; its parts and how they fit together; three \
     surprising details a listener would enjoy, each with the file it is in; and open questions \
     the project has not settled. Print only the brief.";

/// Why no brief was made.
#[derive(Debug)]
pub enum DistilError {
    /// The command is not there.
    Missing { command: PathBuf },
    /// The command did not start for another reason.
    Start {
        command: PathBuf,
        error: std::io::Error,
    },
    /// The command ran and failed; what it printed on either stream, the CLI printing some
    /// failures (an invalid API key) on standard output.
    Failed { status: String, printed: String },
    /// The command printed nothing.
    Empty,
    /// A brief file did not read or write.
    File {
        path: PathBuf,
        error: std::io::Error,
    },
}

impl fmt::Display for DistilError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { command } => write!(
                f,
                "{} was not found: install the Claude Code CLI, or give a brief with \
                 --context-file <path>",
                command.display()
            ),
            Self::Start { command, error } => {
                write!(f, "{} did not start: {error}", command.display())
            }
            Self::Failed { status, printed } => {
                write!(f, "distilling failed ({status}): {}", printed.trim())
            }
            Self::Empty => f.write_str("distilling printed no brief"),
            Self::File { path, error } => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl Error for DistilError {}

/// The command that distils `dir`: `command -p <prompt>` in `dir` with read-only tools, loading
/// the user's settings only, so the directory's own project and local settings (their hooks,
/// permissions and plugins) and its MCP servers are not loaded.
#[must_use]
pub fn distil_command(command: &Path, dir: &Path) -> Command {
    let mut process = Command::new(command);
    process
        .arg("-p")
        .arg(DISTIL_PROMPT)
        .args(["--tools", READ_ONLY_TOOLS])
        .args(["--allowedTools", READ_ONLY_TOOLS])
        .args(["--permission-prompts", "none"])
        .args(["--setting-sources", "user"])
        .arg("--strict-mcp-config")
        .args(["--output-format", "text"])
        .current_dir(dir);
    process
}

/// Reads `stream` to its end, keeping its first `limit` bytes; whether it held more.
fn read_capped(mut stream: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let cap = u64::try_from(limit).unwrap_or(u64::MAX);
    let _ = stream.by_ref().take(cap).read_to_end(&mut kept);
    let more = std::io::copy(&mut stream, &mut std::io::sink()).unwrap_or(0) > 0;
    (kept, more)
}

/// Runs `command` in `dir` and answers what it printed, its first [`DISTIL_LIMIT`] bytes; a
/// longer output is cut there, ending in [`DISTIL_CUT`].
///
/// # Errors
///
/// [`DistilError::Missing`] when the command is not there, [`DistilError::Start`] when it did
/// not start otherwise, [`DistilError::Failed`] when it failed, [`DistilError::Empty`] when it
/// printed nothing.
pub fn distil(command: &Path, dir: &Path) -> Result<String, DistilError> {
    let mut child = distil_command(command, dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| match error.kind() {
            ErrorKind::NotFound => DistilError::Missing {
                command: command.to_owned(),
            },
            _ => DistilError::Start {
                command: command.to_owned(),
                error,
            },
        })?;
    let stderr = child
        .stderr
        .take()
        .map(|stderr| std::thread::spawn(move || read_capped(stderr, DISTIL_LIMIT)));
    let (stdout, cut) = child
        .stdout
        .take()
        .map(|stdout| read_capped(stdout, DISTIL_LIMIT))
        .unwrap_or_default();
    let (stderr, _) = stderr
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let status = child.wait().map_err(|error| DistilError::Start {
        command: command.to_owned(),
        error,
    })?;
    if !status.success() {
        let printed = [&stdout, &stderr]
            .map(|stream| String::from_utf8_lossy(stream).trim().to_owned())
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(DistilError::Failed {
            status: status.to_string(),
            printed,
        });
    }
    let mut text = String::from_utf8_lossy(&stdout).into_owned();
    if text.trim().is_empty() {
        return Err(DistilError::Empty);
    }
    if cut {
        if text.ends_with('\u{FFFD}') {
            text.pop();
        }
        text.push_str(DISTIL_CUT);
    }
    Ok(text)
}

/// The brief: read from `context_file` when one is named, distilling nothing, or else
/// distilled from `dir` by `command`; saved to `save` when that is named.
///
/// # Errors
///
/// [`DistilError::File`] when a file does not read or write, and the errors of [`distil`].
pub fn brief(
    context_file: Option<&Path>,
    save: Option<&Path>,
    command: &Path,
    dir: &Path,
) -> Result<String, DistilError> {
    let text = match context_file {
        Some(path) => std::fs::read_to_string(path).map_err(|error| DistilError::File {
            path: path.to_owned(),
            error,
        })?,
        None => distil(command, dir)?,
    };
    if let Some(path) = save {
        std::fs::write(path, &text).map_err(|error| DistilError::File {
            path: path.to_owned(),
            error,
        })?;
    }
    Ok(text)
}
