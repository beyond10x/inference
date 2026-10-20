//! Adversary pass 1 on distilling: a directory whose name looks like an option reaches the
//! command only as its working directory. No test runs `claude`; a stand-in prints what it got.

use std::path::{Path, PathBuf};

use repo_podcast::{DISTIL_PROMPT, distil};

fn scratch(name: &str) -> PathBuf {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("repo-podcast-adversary1-{name}"));
    std::fs::create_dir_all(&dir).expect("the test directory is made");
    dir
}

#[test]
fn a_directory_named_like_an_option_is_only_the_working_directory() {
    use std::os::unix::fs::PermissionsExt;

    let root = scratch("injection");
    let fake = root.join("fake-claude");
    std::fs::write(
        &fake,
        "#!/bin/sh\npwd\nfor arg in \"$@\"; do printf '[%s]\\n' \"$arg\"; done\n",
    )
    .expect("written");
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("executable");
    let dir = root.join("--dangerously-skip-permissions --tools Bash");
    std::fs::create_dir_all(&dir).expect("made");
    let printed = distil(&fake, &dir).expect("the stand-in prints");
    let mut lines = printed.lines();
    assert_eq!(
        lines.next().map(Path::new),
        Some(dir.canonicalize().expect("exists").as_path())
    );
    let args: Vec<&str> = lines.collect();
    let prompt = format!("[{DISTIL_PROMPT}]");
    assert_eq!(
        args,
        vec![
            "[-p]",
            prompt.as_str(),
            "[--tools]",
            "[Read,Glob,Grep]",
            "[--allowedTools]",
            "[Read,Glob,Grep]",
            "[--permission-prompts]",
            "[none]",
            "[--setting-sources]",
            "[user]",
            "[--strict-mcp-config]",
            "[--output-format]",
            "[text]",
        ]
    );
}
