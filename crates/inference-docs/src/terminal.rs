//! The landing's terminal, `website/data/terminal/stt-only.json` (`b10x-terminal/1`): the command
//! that runs the `stt-only` example and the lines it prints, taken from `stt_only::run()`, the
//! function the example prints. `examples/stt-only/tests/output.rs` fixes those lines.

use serde_json::json;

use crate::Result;

/// Where the terminal session lands, relative to the repository root.
pub const DATA: &str = "website/data/terminal/stt-only.json";

/// The command the landing shows.
pub const COMMAND: &str = "cargo run -p stt-only --example stt_only";

/// The terminal session of the `stt-only` example.
pub fn data() -> Result<String> {
    let session = json!({
        "format": "b10x-terminal/1",
        "title": "stt-only",
        "entries": [{
            "command": COMMAND,
            "output": stt_only::run().join("\n"),
            "exitCode": 0,
        }],
        "recordedWith": "inference-docs, from stt_only::run(), fixed by examples/stt-only/tests/output.rs",
    });
    serde_json::to_string_pretty(&session)
        .map(|text| text + "\n")
        .map_err(|error| format!("serializing the terminal session: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_shows_the_committed_spans_of_the_example() {
        let session: serde_json::Value = serde_json::from_str(&data().unwrap()).unwrap();
        assert_eq!(session["format"], "b10x-terminal/1");
        assert_eq!(
            session["entries"][0]["output"],
            "committed: hello there\ncommitted: how are you"
        );
        assert_eq!(session["entries"][0]["exitCode"], 0);
    }
}
