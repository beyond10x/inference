//! The brief: distilled by the Claude Code CLI, or read from `--context-file`. No test runs
//! `claude`; each names a command that does not exist or a stand-in.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use repo_podcast::{
    DISTIL_CUT, DISTIL_LIMIT, DISTIL_PROMPT, DistilError, brief, distil, distil_command,
};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("repo-podcast-{name}"));
    std::fs::create_dir_all(&dir).expect("the test directory is made");
    dir
}

/// [`distil`] on a stand-in script this test has just written. A test that forks while that
/// file is open for writing makes exec fail with ETXTBSY until that child execs; such a start is
/// retried.
fn distil_written(fake: &Path, dir: &Path) -> Result<String, DistilError> {
    let mut attempt = 0;
    loop {
        match distil(fake, dir) {
            Err(DistilError::Start { error, .. })
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 50 =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            other => return other,
        }
    }
}

fn missing() -> PathBuf {
    scratch("missing").join("no-such-claude")
}

#[test]
fn a_missing_claude_is_an_error_naming_the_context_file_alternative() {
    let error = distil(&missing(), Path::new(".")).expect_err("no such command");
    assert!(matches!(error, DistilError::Missing { .. }), "{error:?}");
    let shown = error.to_string();
    assert!(shown.contains("--context-file"), "{shown}");
    assert!(shown.contains("no-such-claude"), "{shown}");
}

#[test]
fn a_context_file_skips_distilling() {
    let dir = scratch("context");
    let file = dir.join("brief.md");
    std::fs::write(&file, "a saved brief\n").expect("written");
    let text = brief(Some(&file), None, &missing(), &dir).expect("read from the file");
    assert_eq!(text, "a saved brief\n");
}

#[test]
fn claude_runs_in_the_directory_printing_with_read_only_tools() {
    let dir = scratch("command");
    let command = distil_command(Path::new("claude"), &dir);
    assert_eq!(command.get_program(), OsStr::new("claude"));
    assert_eq!(command.get_current_dir(), Some(dir.as_path()));
    let args: Vec<&OsStr> = command.get_args().collect();
    assert_eq!(
        args,
        vec![
            OsStr::new("-p"),
            OsStr::new(DISTIL_PROMPT),
            OsStr::new("--tools"),
            OsStr::new("Read,Glob,Grep"),
            OsStr::new("--allowedTools"),
            OsStr::new("Read,Glob,Grep"),
            OsStr::new("--permission-prompts"),
            OsStr::new("none"),
            OsStr::new("--setting-sources"),
            OsStr::new("user"),
            OsStr::new("--strict-mcp-config"),
            OsStr::new("--output-format"),
            OsStr::new("text"),
        ]
    );
    for asked in [
        "what the project is",
        "why it exists",
        "surprising",
        "open questions",
    ] {
        assert!(DISTIL_PROMPT.contains(asked), "the prompt asks {asked:?}");
    }
}

#[test]
fn the_brief_is_what_the_command_prints_and_is_saved_when_asked() {
    let dir = scratch("saved");
    let saved = dir.join("saved-brief.md");
    let _ = std::fs::remove_file(&saved);
    let text = brief(None, Some(&saved), Path::new("echo"), &dir).expect("echo prints");
    assert!(text.starts_with("-p "), "{text}");
    assert!(text.contains("--allowedTools Read,Glob,Grep"), "{text}");
    assert_eq!(std::fs::read_to_string(&saved).expect("saved"), text);
}

#[test]
fn a_failing_command_is_an_error_with_its_status() {
    let error = distil(Path::new("false"), Path::new(".")).expect_err("false fails");
    assert!(matches!(error, DistilError::Failed { .. }), "{error:?}");
}

#[test]
fn a_failure_the_command_prints_on_standard_output_is_in_the_error() {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch("stdout-failure");
    let fake = dir.join("fake-claude");
    std::fs::write(&fake, "#!/bin/sh\necho 'Invalid API key'\nexit 1\n").expect("written");
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("executable");
    let error = distil_written(&fake, &dir).expect_err("the fake fails");
    assert!(error.to_string().contains("Invalid API key"), "{error}");
}

#[test]
fn what_the_command_prints_is_kept_up_to_64_kib_and_marked_when_cut() {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch("flood");
    let fake = dir.join("fake-claude");
    std::fs::write(
        &fake,
        "#!/bin/sh\nyes 'a brief that never ends' | head -c 1000000\nyes noise | head -c 1000000 >&2\n",
    )
    .expect("written");
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("executable");
    let text = distil_written(&fake, &dir).expect("the fake prints");
    assert!(text.ends_with(DISTIL_CUT), "{}", &text[text.len() - 100..]);
    assert_eq!(text.len(), DISTIL_LIMIT + DISTIL_CUT.len());
    assert!(DISTIL_CUT.contains("cut"), "{DISTIL_CUT}");
}

#[test]
fn an_empty_brief_is_an_error() {
    let error = distil(Path::new("true"), Path::new(".")).expect_err("true prints nothing");
    assert!(matches!(error, DistilError::Empty), "{error:?}");
}
