//! Every `#[ignore]` of the workspace's Rust sources gives a reason of a kind `AGENTS.md` admits.
//!
//! An ignore is a line of a `.rs` file that begins with `#[ignore`, or a `#[cfg_attr(` line that
//! ignores; its reason is the string after `ignore = `. The kinds, in the order they are tried:
//!
//! - billed: the reason holds the word `billed` (a live test against a hosted provider);
//! - a reason that speaks of a hosted service, a key, credentials or money, or names a
//!   `*_API_KEY` variable, without `billed` is refused;
//! - limit: the reason begins `limit:` (a case that shows a limit the code states);
//! - child: the reason begins `a child of`, and the ignored function's name stands elsewhere in
//!   its file, where the parent case runs it;
//! - by hand: the reason holds `by hand` (a test that opens a device of the machine);
//! - local: the reason names an `INFERENCE_*` variable, and the test's package (the nearest
//!   directory above it with a `Cargo.toml`) reads every one it names.
//!
//! Directories whose name begins with `.`, `target` and `node_modules` are not read.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::root;

/// Where a refusal sends its reader.
const RULE: &str = "the kinds are the bullet \"Every `#[ignore]` gives a reason of one of five kinds\" under Rules in AGENTS.md";

/// What the check found: how many ignores it read and the refusals among them.
struct Scan {
    read: usize,
    refusals: Vec<String>,
}

/// One ignore: its line (from 1), its reason if it gives one, and the function it ignores.
struct Ignore {
    line: usize,
    reason: Option<String>,
    function: Option<String>,
}

fn scan(root: &Path) -> Scan {
    let mut files = Vec::new();
    rust_files(root, &mut files);
    files.sort();
    let mut packages = BTreeMap::new();
    let mut scan = Scan {
        read: 0,
        refusals: Vec::new(),
    };
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        let shown = file.strip_prefix(root).unwrap().display().to_string();
        for ignore in ignores(&text) {
            scan.read += 1;
            let package = package_of(root, &file);
            let reads = packages
                .entry(package.clone())
                .or_insert_with(|| package_text(&package));
            if let Err(why) = admitted(&ignore, &text, reads) {
                let reason = ignore
                    .reason
                    .as_deref()
                    .map_or_else(|| "none".to_owned(), |reason| format!("{reason:?}"));
                scan.refusals.push(format!(
                    "{shown}:{}: the reason {reason} is refused: {why}; {RULE}",
                    ignore.line
                ));
            }
        }
    }
    scan
}

/// The `.rs` files under `dir`, leaving out hidden directories, `target` and `node_modules`.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                rust_files(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

/// Whether a line, trimmed, is an attribute that ignores a test.
fn is_ignore(trimmed: &str) -> bool {
    trimmed.starts_with("#[ignore")
        || (trimmed.starts_with("#[cfg_attr(")
            && (trimmed.contains(", ignore") || trimmed.contains("(ignore")))
}

fn ignores(text: &str) -> Vec<Ignore> {
    let lines: Vec<&str> = text.lines().collect();
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_ignore(line.trim_start()))
        .map(|(index, line)| Ignore {
            line: index + 1,
            reason: reason(line),
            function: lines[index + 1..].iter().find_map(|line| function(line)),
        })
        .collect()
}

/// The string after `ignore = ` on `line`, up to its closing quote.
fn reason(line: &str) -> Option<String> {
    let start = line.find("ignore = \"")? + "ignore = \"".len();
    let rest = &line[start..];
    let mut end = None;
    let mut escaped = false;
    for (at, character) in rest.char_indices() {
        match character {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => {
                end = Some(at);
                break;
            }
            _ => escaped = false,
        }
    }
    Some(rest[..end?].to_owned())
}

/// The name of the function `line` declares, if it declares one.
fn function(line: &str) -> Option<String> {
    let at = line.find("fn ")?;
    let before = line[..at].trim();
    if !before
        .split_whitespace()
        .all(|word| ["pub", "pub(crate)", "async", "unsafe", "const"].contains(&word))
    {
        return None;
    }
    let name: String = line[at + 3..]
        .chars()
        .take_while(|character| character.is_alphanumeric() || *character == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// The nearest directory above `file`, up to `root`, that holds a `Cargo.toml`.
fn package_of(root: &Path, file: &Path) -> PathBuf {
    file.ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(root))
        .find(|dir| dir.join("Cargo.toml").is_file())
        .unwrap_or(root)
        .to_path_buf()
}

/// The lines of the package's `.rs` files that are not ignores: what the package reads.
fn package_text(package: &Path) -> String {
    let mut files = Vec::new();
    rust_files(package, &mut files);
    files
        .iter()
        .flat_map(|file| {
            fs::read_to_string(file)
                .unwrap()
                .lines()
                .filter(|line| !is_ignore(line.trim_start()))
                .map(|line| format!("{line}\n"))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The words of `reason`: its runs of letters, digits and underscores.
fn words(reason: &str) -> impl Iterator<Item = &str> {
    reason
        .split(|character: char| !(character.is_alphanumeric() || character == '_'))
        .filter(|word| !word.is_empty())
}

/// Whether `ignore` gives a reason of an admitted kind; why not, if it does not.
fn admitted(ignore: &Ignore, file: &str, package: &str) -> Result<(), String> {
    let Some(reason) = ignore.reason.as_deref() else {
        return Err("it gives no reason".to_owned());
    };
    if words(reason).any(|word| word == "billed") {
        return Ok(());
    }
    if words(reason).any(|word| {
        ["hosted", "key", "keys", "credentials", "money"].contains(&word)
            || word.ends_with("_API_KEY")
    }) {
        return Err(
            "it speaks of a hosted service, a key, credentials or money without the word `billed`"
                .to_owned(),
        );
    }
    if reason.starts_with("limit:") || reason.contains("by hand") {
        return Ok(());
    }
    if reason.starts_with("a child of") {
        let name = ignore.function.as_deref().unwrap_or("");
        return if !name.is_empty() && file.matches(name).count() > 1 {
            Ok(())
        } else {
            Err(format!(
                "it is a child case, but its function `{name}` stands nowhere else in its file, so no parent runs it"
            ))
        };
    }
    let variables: Vec<&str> = words(reason)
        .filter(|word| word.starts_with("INFERENCE_"))
        .collect();
    if variables.is_empty() {
        return Err("it is of no admitted kind".to_owned());
    }
    match variables
        .iter()
        .find(|variable| !package.contains(**variable))
    {
        Some(variable) => Err(format!(
            "it names `{variable}`, which its package does not read"
        )),
        None => Ok(()),
    }
}

#[test]
fn every_ignore_of_the_workspace_gives_a_reason_of_an_admitted_kind() {
    let scan = scan(&root());
    assert!(
        scan.read > 0,
        "the check read no #[ignore] in the workspace"
    );
    assert!(
        scan.refusals.is_empty(),
        "{} of {} ignored tests give no reason of an admitted kind:\n{}",
        scan.refusals.len(),
        scan.read,
        scan.refusals.join("\n")
    );
}

/// A tree of one package, `crates/planted`, whose test file holds `source`.
fn planted(source: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let tree = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "ignores-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&tree);
    let package = tree.join("crates/planted");
    fs::create_dir_all(package.join("tests")).unwrap();
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("Cargo.toml"),
        "[package]\nname = \"planted\"\n",
    )
    .unwrap();
    fs::write(
        package.join("src/lib.rs"),
        "pub fn checkpoint() -> Option<String> {\n    std::env::var(\"INFERENCE_PLANTED_DIR\").ok()\n}\n",
    )
    .unwrap();
    fs::write(package.join("tests/planted.rs"), source).unwrap();
    tree
}

/// The refusals of a tree whose one test is ignored with the attribute `ignore`.
fn refusals_of(ignore: &str) -> Vec<String> {
    let tree = planted(&format!(
        "#[test]\n{ignore}\nfn planted_case() {{}}\n\n#[test]\nfn parent() {{\n    let _ = \"planted_case\";\n}}\n"
    ));
    let refusals = scan(&tree).refusals;
    let _ = fs::remove_dir_all(&tree);
    refusals
}

#[test]
fn an_ignore_without_a_reason_is_refused_with_its_file_line_and_rule() {
    let refusals = refusals_of("#[ignore]");
    assert_eq!(refusals.len(), 1, "{refusals:?}");
    let refusal = &refusals[0];
    assert!(
        refusal.starts_with("crates/planted/tests/planted.rs:2: "),
        "{refusal}"
    );
    assert!(refusal.contains("gives no reason"), "{refusal}");
    assert!(refusal.contains("AGENTS.md"), "{refusal}");
}

#[test]
fn a_reason_of_no_admitted_kind_is_refused_with_its_file_line_reason_and_rule() {
    let refusals = refusals_of("#[ignore = \"flaky on a busy runner\"]");
    assert_eq!(refusals.len(), 1, "{refusals:?}");
    let refusal = &refusals[0];
    assert!(
        refusal.starts_with("crates/planted/tests/planted.rs:2: "),
        "{refusal}"
    );
    assert!(refusal.contains("\"flaky on a busy runner\""), "{refusal}");
    assert!(refusal.contains("AGENTS.md"), "{refusal}");
}

#[test]
fn each_admitted_kind_passes_and_its_near_misses_are_refused() {
    for admitted in [
        "#[ignore = \"talks to a hosted provider and is billed; needs PLANTED_API_KEY\"]",
        "#[ignore = \"needs the checkpoint in INFERENCE_PLANTED_DIR\"]",
        "#[ignore = \"opens the default microphone; run it by hand\"]",
        "#[ignore = \"a child of the parent cases of this file, run with PLANTED set\"]",
        "#[ignore = \"limit: red only on a shared processor, as the module comment states\"]",
    ] {
        assert_eq!(refusals_of(admitted), Vec::<String>::new(), "{admitted}");
    }
    for (refused, why) in [
        (
            "#[ignore = \"calls the hosted API; needs a key\"]",
            "billed",
        ),
        (
            "#[ignore = \"needs a key in PLANTED_API_KEY and the clip in INFERENCE_PLANTED_DIR\"]",
            "billed",
        ),
        (
            "#[ignore = \"needs the checkpoint in INFERENCE_PLANTED_MODEL\"]",
            "INFERENCE_PLANTED_MODEL",
        ),
        ("#[ignore = \"needs AWS_REGION\"]", "no admitted kind"),
        (
            "#[cfg_attr(not(feature = \"x\"), ignore)]",
            "gives no reason",
        ),
    ] {
        let refusals = refusals_of(refused);
        assert_eq!(refusals.len(), 1, "{refused}: {refusals:?}");
        assert!(refusals[0].contains(why), "{refused}: {}", refusals[0]);
    }
    let orphan = planted(
        "#[test]\n#[ignore = \"a child of the parent cases of this file\"]\nfn orphan() {}\n",
    );
    let refusals = scan(&orphan).refusals;
    let _ = fs::remove_dir_all(&orphan);
    assert_eq!(refusals.len(), 1, "{refusals:?}");
    assert!(refusals[0].contains("orphan"), "{}", refusals[0]);
}
