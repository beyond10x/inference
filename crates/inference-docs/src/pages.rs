//! Checks over every page under `website/docs`, hand-written or generated.
//!
//! - Front matter opens the page and carries `title`, `description` and `sidebar_position`; a page
//!   under `concepts/` or `guides/` also carries `lede` and `source`, the files it was read from.
//! - No admonition carries a raw title (`:::caution Planned`): Docusaurus prints those as text.
//! - Every code fence is closed.
//! - A dependency line on this repository (`inference-x = { git = "…/inference", tag = "…",
//!   features = […] }`) names a workspace package, the workspace version as its tag, and only
//!   features that package has, so a reader who copies it gets a manifest that resolves.

use std::{fs, path::Path};

use crate::crates::{Package, metadata, packages, workspace_version};
use crate::{DOCS, Result};

/// The Git URL a dependency on this repository names.
const GIT: &str = "https://github.com/beyond10x/inference";

/// Whether `line` matches `^:::[a-z]+ +\S`: an admonition whose title is not in brackets.
fn is_raw_admonition(line: &str) -> bool {
    let Some(rest) = line.strip_prefix(":::") else {
        return false;
    };
    let kind = rest.bytes().take_while(u8::is_ascii_lowercase).count();
    let rest = &rest[kind..];
    let spaces = rest.bytes().take_while(|b| *b == b' ').count();
    kind > 0
        && spaces > 0
        && rest[spaces..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace())
}

/// The value of `key = "…"` in `line`.
fn quoted<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let start = line.find(&format!("{key} = \""))? + key.len() + 4;
    let rest = &line[start..];
    Some(&rest[..rest.find('"')?])
}

/// What is wrong with one dependency line on this repository, if anything.
fn dependency_problem(line: &str, packages: &[Package], version: &str) -> Option<String> {
    let Some((name, _)) = line.split_once(" = {") else {
        return Some(
            "a dependency on this repository not written `name = { git = …, tag = … }`".to_owned(),
        );
    };
    let name = name.trim();
    let Some(package) = packages.iter().find(|package| package.name == name) else {
        return Some(format!("{name} is not a package of the workspace"));
    };
    if quoted(line, "git") != Some(GIT) {
        return Some(format!("{name} does not name git = \"{GIT}\""));
    }
    match quoted(line, "tag") {
        Some(tag) if tag == version => {}
        Some(tag) => {
            return Some(format!(
                "{name} names tag {tag}, the workspace is at {version}"
            ));
        }
        None => return Some(format!("{name} names no tag")),
    }
    if let Some(start) = line.find("features = [") {
        let list = &line[start + "features = [".len()..];
        let Some(end) = list.find(']') else {
            return Some(format!("{name}: unclosed feature list"));
        };
        for feature in list[..end].split(',') {
            let feature = feature.trim().trim_matches('"');
            if !feature.is_empty() && !package.features.iter().any(|known| known == feature) {
                return Some(format!("{name} has no feature {feature}"));
            }
        }
    }
    None
}

/// The keys of the front matter that opens `page`, or why there is none.
fn front_matter_keys(page: &str) -> std::result::Result<Vec<&str>, &'static str> {
    let mut lines = page.lines();
    if lines.next() != Some("---") {
        return Err("no front matter");
    }
    let mut keys = Vec::new();
    for line in lines {
        if line == "---" {
            return Ok(keys);
        }
        if let Some((key, _)) = line.split_once(':')
            && !key.starts_with([' ', '#'])
        {
            keys.push(key);
        }
    }
    Err("unclosed front matter")
}

/// Every problem in one page: `page:line: problem`.
fn page_problems(name: &str, page: &str, packages: &[Package], version: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let relative = name.strip_prefix(&format!("{DOCS}/")).unwrap_or(name);
    match front_matter_keys(page) {
        Err(problem) => problems.push(format!("{name}:1: {problem}")),
        Ok(keys) => {
            let mut required = vec!["title", "description", "sidebar_position"];
            if relative.starts_with("concepts/") || relative.starts_with("guides/") {
                required.extend(["lede", "source"]);
            }
            for key in required {
                if !keys.contains(&key) {
                    problems.push(format!("{name}:1: front matter has no {key}"));
                }
            }
        }
    }
    let lines: Vec<&str> = page.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if is_raw_admonition(line) {
            problems.push(format!(
                "{name}:{}: raw admonition title (write `:::kind[Title]`)",
                index + 1
            ));
        }
        if line.trim_start().starts_with("```") {
            let indent = line.len() - line.trim_start().len();
            let opened = index;
            let end = lines[index + 1..]
                .iter()
                .position(|candidate| {
                    candidate.trim_end().len() == indent + 3 && candidate.trim() == "```"
                })
                .map(|offset| index + 1 + offset);
            let Some(end) = end else {
                problems.push(format!("{name}:{}: unclosed code block", opened + 1));
                break;
            };
            for (offset, code) in lines[opened + 1..end].iter().enumerate() {
                if code.contains(GIT)
                    && !code.trim_start().starts_with("git clone ")
                    && let Some(problem) = dependency_problem(code, packages, version)
                {
                    problems.push(format!("{name}:{}: {problem}", opened + 2 + offset));
                }
            }
            index = end;
        }
        index += 1;
    }
    problems
}

fn walk(
    root: &Path,
    dir: &Path,
    packages: &[Package],
    version: &str,
    problems: &mut Vec<String>,
) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|error| format!("reading {}: {error}", dir.display()))?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(root, &path, packages, version, problems)?;
        } else if path
            .extension()
            .is_some_and(|ext| ext == "md" || ext == "mdx")
        {
            let page = fs::read_to_string(&path)
                .map_err(|error| format!("reading {}: {error}", path.display()))?;
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            problems.extend(page_problems(&name, &page, packages, version));
        }
    }
    Ok(())
}

/// Every problem in the pages under `website/docs` of the repository at `root`.
pub fn check(root: &Path) -> Result<Vec<String>> {
    let metadata = metadata(root)?;
    let packages = packages(&metadata)?;
    let version = workspace_version(&metadata)?;
    let docs = root.join(DOCS);
    if !docs.is_dir() {
        return Ok(vec![format!("{DOCS} does not exist")]);
    }
    let mut problems = Vec::new();
    walk(root, &docs, &packages, &version, &mut problems)?;
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> (Vec<Package>, String) {
        let metadata = metadata(&crate::repository_root()).unwrap();
        (
            packages(&metadata).unwrap(),
            workspace_version(&metadata).unwrap(),
        )
    }

    #[test]
    fn raw_admonition_titles_are_found_and_bracketed_ones_pass() {
        for raw in [":::caution Planned", ":::note  These"] {
            assert!(is_raw_admonition(raw), "{raw}");
        }
        for fine in [":::caution[Planned]", ":::", ":::note", "::: note T"] {
            assert!(!is_raw_admonition(fine), "{fine}");
        }
    }

    #[test]
    fn a_dependency_line_must_name_a_package_the_release_and_its_features() {
        let (packages, version) = workspace();
        let line = |name: &str, tag: &str, features: &str| {
            format!("{name} = {{ git = \"{GIT}\", tag = \"{tag}\", features = [{features}] }}")
        };
        assert_eq!(
            dependency_problem(
                &line("inference-openai", &version, "\"duplex\", \"gpt-live\""),
                &packages,
                &version
            ),
            None
        );
        assert_eq!(
            dependency_problem(&line("inference-stt", &version, ""), &packages, &version),
            None
        );
        let problem = |text: String| dependency_problem(&text, &packages, &version).unwrap();
        assert!(problem(line("inference-sdk", &version, "")).contains("not a package"));
        assert!(problem(line("inference-openai", "0.1.0", "")).contains("names tag 0.1.0"));
        assert!(
            problem(line("inference-openai", &version, "\"teleport\""))
                .contains("no feature teleport")
        );
        assert!(
            problem(format!("inference-openai = {{ git = \"{GIT}\" }}")).contains("names no tag")
        );
        assert!(problem(format!("git = \"{GIT}\"")).contains("not written"));
    }

    #[test]
    fn a_clone_command_in_a_code_block_is_no_dependency_line() {
        let (packages, version) = workspace();
        let page = format!(
            "---\ntitle: T\ndescription: D\nsidebar_position: 1\n---\n\n```sh\ngit clone {GIT}\ncd inference\n```\n"
        );
        assert!(
            page_problems(
                "website/docs/getting-started.md",
                &page,
                &packages,
                &version
            )
            .is_empty()
        );
    }

    #[test]
    fn front_matter_lede_and_source_and_fences_are_checked() {
        let (packages, version) = workspace();
        let good = "---\ntitle: T\ndescription: D\nsidebar_position: 1\nlede: L\nsource: S\n---\n\n```bash\nls\n```\n";
        assert!(page_problems("website/docs/guides/a.md", good, &packages, &version).is_empty());
        let bare = "---\ntitle: T\ndescription: D\nsidebar_position: 1\n---\n\n```bash\nls\n";
        let problems = page_problems("website/docs/concepts/a.md", bare, &packages, &version);
        assert_eq!(
            problems,
            [
                "website/docs/concepts/a.md:1: front matter has no lede",
                "website/docs/concepts/a.md:1: front matter has no source",
                "website/docs/concepts/a.md:7: unclosed code block",
            ]
        );
        assert!(
            page_problems(
                "website/docs/index.md",
                bare.trim_end_matches("```bash\nls\n"),
                &packages,
                &version
            )
            .is_empty()
        );
        assert_eq!(
            page_problems("website/docs/x.md", "# x\n", &packages, &version),
            ["website/docs/x.md:1: no front matter"]
        );
    }
}
