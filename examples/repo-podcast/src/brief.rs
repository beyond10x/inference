//! The brief the hosts discuss: distilled from a directory by the Claude Code CLI, or read from
//! a file saved before.
//!
//! The CLI is an external process the example talks to: `claude -p` with [`DISTIL_PROMPT`],
//! run in the directory, its tools limited to reading (`Read`, `Glob`, `Grep`), no permission
//! prompts, plain text out. What it prints is the brief.

use std::error::Error;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The tools the CLI may use: reading only.
const READ_ONLY_TOOLS: &str = "Read,Glob,Grep";

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

/// The command that distils `dir`: `command -p <prompt>` in `dir` with read-only tools.
#[must_use]
pub fn distil_command(command: &Path, dir: &Path) -> Command {
    let mut process = Command::new(command);
    process
        .arg("-p")
        .arg(DISTIL_PROMPT)
        .args(["--tools", READ_ONLY_TOOLS])
        .args(["--allowedTools", READ_ONLY_TOOLS])
        .args(["--permission-prompts", "none"])
        .args(["--output-format", "text"])
        .current_dir(dir);
    process
}

/// Runs `command` in `dir` and answers what it printed.
///
/// # Errors
///
/// [`DistilError::Missing`] when the command is not there, [`DistilError::Start`] when it did
/// not start otherwise, [`DistilError::Failed`] when it failed, [`DistilError::Empty`] when it
/// printed nothing.
pub fn distil(command: &Path, dir: &Path) -> Result<String, DistilError> {
    let output = distil_command(command, dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            ErrorKind::NotFound => DistilError::Missing {
                command: command.to_owned(),
            },
            _ => DistilError::Start {
                command: command.to_owned(),
                error,
            },
        })?;
    if !output.status.success() {
        let printed = [&output.stdout, &output.stderr]
            .map(|stream| String::from_utf8_lossy(stream).trim().to_owned())
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(DistilError::Failed {
            status: output.status.to_string(),
            printed,
        });
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if text.trim().is_empty() {
        return Err(DistilError::Empty);
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
