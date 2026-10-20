//! The project's name in the title, read from a git checkout's files alone.

use std::path::{Path, PathBuf};

use repo_podcast::{project_name, repository_name};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("repo-podcast-title-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the test directory is made");
    dir
}

#[test]
fn the_repository_name_is_the_last_segment_of_the_remote_url() {
    for (url, name) in [
        ("ssh://git@github.com/beyond10x/inference.git", "inference"),
        ("git@github.com:beyond10x/inference.git", "inference"),
        ("https://github.com/beyond10x/inference/", "inference"),
        ("https://example.org/a/b/tool", "tool"),
        ("../local", "local"),
    ] {
        assert_eq!(repository_name(url).as_deref(), Some(name), "{url}");
    }
    assert_eq!(repository_name(".git"), None);
}

#[test]
fn a_worktree_is_named_by_its_origin_remote() {
    let root = scratch("worktree");
    let main = root.join("main-checkout");
    let common = main.join(".git");
    let linked = common.join("worktrees").join("review");
    std::fs::create_dir_all(&linked).expect("made");
    std::fs::write(
        common.join("config"),
        "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@host:x/other.git\n\
         [remote \"origin\"]\n\turl = ssh://git@github.com/beyond10x/inference.git\n",
    )
    .expect("written");
    std::fs::write(linked.join("commondir"), "../..\n").expect("written");
    let tree = root.join("inference-review-podcast");
    std::fs::create_dir_all(tree.join("examples")).expect("made");
    std::fs::write(tree.join(".git"), format!("gitdir: {}\n", linked.display())).expect("written");
    assert_eq!(project_name(&tree), "inference");
    assert_eq!(project_name(&tree.join("examples")), "inference/examples");
    assert_eq!(project_name(&main), "inference");
}

#[test]
fn a_checkout_without_origin_is_named_by_its_top_level_directory() {
    let root = scratch("no-origin");
    let top = root.join("my-project");
    std::fs::create_dir_all(top.join(".git")).expect("made");
    std::fs::write(top.join(".git").join("config"), "[core]\n\tbare = false\n").expect("written");
    assert_eq!(project_name(&top), "my-project");
    let plain = root.join("not-a-checkout");
    std::fs::create_dir_all(&plain).expect("made");
    assert_ne!(project_name(&plain), "");
}

#[test]
fn this_repository_is_named_inference_from_its_origin() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root");
    let has_origin = std::process::Command::new("git")
        .args([
            "-C",
            &root.display().to_string(),
            "remote",
            "get-url",
            "origin",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok());
    let Some(url) = has_origin else {
        eprintln!("not a checkout with an origin remote; nothing to compare");
        return;
    };
    assert_eq!(
        Some(project_name(&root)),
        repository_name(&url),
        "the title of a run on {}",
        root.display()
    );
}
