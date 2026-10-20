//! The name of the project the show is about, for the title: the repository's name when the
//! directory is in a git checkout, read from the checkout's files alone (no `git` process is run
//! in a directory the show only reads).
//!
//! The name is the last path segment of the `origin` remote's URL without `.git`, or else the
//! name of the checkout's top-level directory; a directory below the top level adds its path
//! from there. A worktree's `.git` file is followed to its common directory, where the remote
//! is configured.

use std::path::{Path, PathBuf};

/// The name of `dir` for the title: `inference` for the top level of a checkout of a
/// repository whose `origin` is `…/inference.git`, `inference/examples` for a directory below
/// it, and the directory's own name outside a checkout.
#[must_use]
pub fn project_name(dir: &Path) -> String {
    let own = || {
        dir.file_name().map_or_else(
            || dir.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    let Some(top) = dir.ancestors().find(|at| at.join(".git").exists()) else {
        return own();
    };
    let name = git_dir(top)
        .and_then(|git| std::fs::read_to_string(common_dir(&git).join("config")).ok())
        .and_then(|config| origin_url(&config))
        .and_then(|url| repository_name(&url))
        .or_else(|| {
            top.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        });
    let Some(name) = name else {
        return own();
    };
    match dir.strip_prefix(top) {
        Ok(below) if !below.as_os_str().is_empty() => {
            format!("{name}/{}", below.display())
        }
        _ => name,
    }
}

/// The git directory of the checkout at `top`: its `.git` directory, or where its `.git` file
/// points.
fn git_dir(top: &Path) -> Option<PathBuf> {
    let dot_git = top.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let target = text.lines().find_map(|line| line.strip_prefix("gitdir:"))?;
    Some(top.join(target.trim()))
}

/// The common directory of the git directory `git`: the one its `commondir` file names, a
/// worktree's, or `git` itself.
fn common_dir(git: &Path) -> PathBuf {
    std::fs::read_to_string(git.join("commondir"))
        .ok()
        .map_or_else(|| git.to_owned(), |common| git.join(common.trim()))
}

/// The URL of the `origin` remote in the git config text `config`.
fn origin_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line.replace(' ', "") == "[remote\"origin\"]";
            continue;
        }
        if in_origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().to_owned());
        }
    }
    None
}

/// The repository's name in a remote URL: its last path segment without `.git`, for
/// `ssh://git@host/owner/name.git`, `git@host:owner/name.git` and `https://host/owner/name/`.
#[must_use]
pub fn repository_name(url: &str) -> Option<String> {
    let segment = url.trim().trim_end_matches('/').rsplit(['/', ':']).next()?;
    let name = segment.strip_suffix(".git").unwrap_or(segment);
    (!name.is_empty()).then(|| name.to_owned())
}
