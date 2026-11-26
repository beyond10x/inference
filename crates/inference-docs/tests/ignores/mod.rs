//! Every `#[ignore]` of the workspace's Rust sources gives a reason of a kind `AGENTS.md` admits.
//!
//! An ignore is a line of a `.rs` file that begins with `#[ignore`, or a `#[cfg_attr(` attribute,
//! read over as many lines as its brackets span, with an `ignore` item; its reason is the string
//! after `ignore = `. The kinds, in the order they are tried:
//!
//! - billed: the reason holds the word `billed`, not right after `not` or `never` (a live test
//!   against a hosted provider);
//! - an ignore in a test target gated on feature `live` (`required-features` of its `[[test]]`,
//!   or `#![cfg(feature = "live")]` in its file) is refused without `billed`, unless its package
//!   is a local adapter (its `description` begins `Local adapter`);
//! - a reason that speaks of a hosted service, a key, credentials or money, or names a
//!   `*_API_KEY` variable, without `billed` is refused;
//! - limit: the reason begins `limit:` and names a path of the repository that exists, the file
//!   whose documentation states the limit;
//! - child: the reason begins `a child of`, and the ignored function's name stands elsewhere in
//!   its file as a whole identifier or inside a string literal, outside comments, the ignore and
//!   the function's own declaration, where the parent case runs it;
//! - by hand: the reason holds `by hand` and names the device the test opens, a microphone or a
//!   speaker;
//! - local: the reason names `INFERENCE_*` variables, and the test's package (the nearest
//!   directory above it with a `Cargo.toml`) holds each whole name inside a string literal
//!   outside comments and ignores.
//!
//! Not read: directories whose name begins with `.`, `node_modules`, the build directory
//! `<root>/target` and any directory that holds a `CACHEDIR.TAG`.

mod adversary;

use std::cell::OnceCell;
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

/// One ignore: its first and last line (from 1), its reason if it gives one, and the function it
/// ignores with the line that declares it.
struct Ignore {
    line: usize,
    end: usize,
    reason: Option<String>,
    function: Option<(usize, String)>,
}

/// What a package says about its ignores: whether it is a local adapter, the names of its test
/// targets gated on feature `live`, and where its string literals are read from.
struct Package {
    local_adapter: bool,
    live_targets: Vec<String>,
    root: PathBuf,
    directory: PathBuf,
    literals: OnceCell<String>,
}

/// What one ignore is judged against.
struct Context<'a> {
    root: &'a Path,
    package: &'a Package,
    live: bool,
    file: &'a str,
    ignores: &'a [Ignore],
}

fn scan(root: &Path) -> Scan {
    let mut files = Vec::new();
    rust_files(root, root, &mut files);
    files.sort();
    let mut packages = BTreeMap::new();
    let mut scan = Scan {
        read: 0,
        refusals: Vec::new(),
    };
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        let found = ignores(&text);
        if found.is_empty() {
            continue;
        }
        let shown = file.strip_prefix(root).unwrap().display().to_string();
        let directory = package_of(root, &file);
        let package = &*packages
            .entry(directory.clone())
            .or_insert_with(|| package(root, &directory));
        let context = Context {
            root,
            live: live_gated(&directory, &file, &text, package),
            package,
            file: &text,
            ignores: &found,
        };
        for ignore in &found {
            scan.read += 1;
            if let Err(why) = admitted(ignore, &context) {
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

/// The `.rs` files under `dir`, leaving out hidden directories, `node_modules`, `<root>/target`
/// and directories that hold a `CACHEDIR.TAG`.
fn rust_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            let skipped = name.starts_with('.')
                || name == "node_modules"
                || path == root.join("target")
                || path.join("CACHEDIR.TAG").is_file();
            if !skipped {
                rust_files(root, &path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

/// The source with comments and the contents of string literals replaced by spaces, lines kept.
struct Lexed {
    /// Comments and the contents of string literals blanked.
    bare: String,
    /// The contents of the string literals, one per line.
    literals: String,
}

fn lex(text: &str) -> Lexed {
    let chars: Vec<char> = text.chars().collect();
    let mut lexed = Lexed {
        bare: String::new(),
        literals: String::new(),
    };
    let blank = |character: char| if character == '\n' { '\n' } else { ' ' };
    let mut at = 0;
    while at < chars.len() {
        let character = chars[at];
        let next = chars.get(at + 1).copied();
        let after_identifier = at > 0 && is_identifier(chars[at - 1]);
        if character == '/' && next == Some('/') {
            while at < chars.len() && chars[at] != '\n' {
                lexed.bare.push(' ');
                at += 1;
            }
            continue;
        }
        if character == '/' && next == Some('*') {
            let mut depth = 0;
            while at < chars.len() {
                if chars[at] == '/' && chars.get(at + 1) == Some(&'*') {
                    depth += 1;
                    lexed.bare.push_str("  ");
                    at += 2;
                } else if chars[at] == '*' && chars.get(at + 1) == Some(&'/') {
                    depth -= 1;
                    lexed.bare.push_str("  ");
                    at += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    lexed.bare.push(blank(chars[at]));
                    at += 1;
                }
            }
            continue;
        }
        let raw_start = match (character, next) {
            ('r', _) if !after_identifier => Some(at + 1),
            ('b', Some('r')) if !after_identifier => Some(at + 2),
            _ => None,
        };
        if let Some(mut open) = raw_start {
            let mut hashes = 0;
            while chars.get(open) == Some(&'#') {
                hashes += 1;
                open += 1;
            }
            if chars.get(open) == Some(&'"') {
                let mut close = open + 1;
                while close < chars.len()
                    && !(chars[close] == '"'
                        && (0..hashes).all(|hash| chars.get(close + 1 + hash) == Some(&'#')))
                {
                    close += 1;
                }
                let close = close.min(chars.len());
                let end = (close + 1 + hashes).min(chars.len());
                for &prefix in &chars[at..=open] {
                    lexed.bare.push(prefix);
                }
                for &inside in &chars[open + 1..close] {
                    lexed.bare.push(blank(inside));
                    lexed.literals.push(inside);
                }
                lexed.literals.push('\n');
                for &suffix in &chars[close..end] {
                    lexed.bare.push(suffix);
                }
                at = end;
                continue;
            }
        }
        if character == '"' {
            lexed.bare.push('"');
            at += 1;
            while at < chars.len() && chars[at] != '"' {
                let step = if chars[at] == '\\' { 2 } else { 1 };
                for &inside in chars.iter().skip(at).take(step) {
                    lexed.bare.push(blank(inside));
                    lexed.literals.push(inside);
                }
                at += step;
            }
            lexed.literals.push('\n');
            if at < chars.len() {
                lexed.bare.push('"');
                at += 1;
            }
            continue;
        }
        if character == '\'' {
            let length = match (next, chars.get(at + 2)) {
                (Some('\\'), _) => chars[at + 1..]
                    .iter()
                    .skip(2)
                    .position(|&inside| inside == '\'')
                    .map(|position| position + 4),
                (Some(_), Some('\'')) => Some(3),
                _ => None,
            };
            if let Some(length) = length {
                for &inside in chars.iter().skip(at).take(length) {
                    lexed.bare.push(blank(inside));
                }
                at += length;
                continue;
            }
        }
        lexed.bare.push(character);
        at += 1;
    }
    lexed
}

fn is_identifier(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Whether `word` stands in `text` with no letter, digit or underscore on either side.
fn has_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_identifier) && !after.is_some_and(is_identifier)
    })
}

fn ignores(text: &str) -> Vec<Ignore> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim_start();
        let attribute = if trimmed.starts_with("#[ignore") {
            Some((index, trimmed.to_owned()))
        } else if trimmed.starts_with("#[cfg_attr(") {
            let (end, joined) = balanced(&lines, index);
            has_word(&lex(&joined).bare, "ignore").then_some((end, joined))
        } else {
            None
        };
        if let Some((end, attribute)) = attribute {
            found.push(Ignore {
                line: index + 1,
                end: end + 1,
                reason: reason(&attribute),
                function: lines
                    .iter()
                    .enumerate()
                    .skip(end + 1)
                    .find_map(|(at, line)| function(line).map(|name| (at, name))),
            });
            index = end;
        }
        index += 1;
    }
    found
}

/// The attribute that begins on line `start`, joined with spaces up to the line where its
/// brackets balance, and the index of that line.
fn balanced(lines: &[&str], start: usize) -> (usize, String) {
    let mut joined = String::new();
    for (index, line) in lines.iter().enumerate().skip(start) {
        joined.push_str(line.trim());
        joined.push(' ');
        let bare = lex(&joined).bare;
        let open = bare.matches(['(', '[']).count();
        let close = bare.matches([')', ']']).count();
        if open <= close {
            return (index, joined);
        }
    }
    (lines.len().saturating_sub(1), joined)
}

/// The string after `ignore = ` in `attribute`, up to its closing quote.
fn reason(attribute: &str) -> Option<String> {
    let start = attribute.find("ignore = \"")? + "ignore = \"".len();
    let rest = &attribute[start..];
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
        .take_while(|character| is_identifier(*character))
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

/// `text` with the lines of `ignores` blanked.
fn without_ignores(text: &str, ignores: &[Ignore]) -> String {
    text.lines()
        .enumerate()
        .map(|(index, line)| {
            if ignores
                .iter()
                .any(|ignore| (ignore.line..=ignore.end).contains(&(index + 1)))
            {
                "\n".to_owned()
            } else {
                format!("{line}\n")
            }
        })
        .collect()
}

fn package(root: &Path, directory: &Path) -> Package {
    let manifest: toml::Table = fs::read_to_string(directory.join("Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let local_adapter = manifest
        .get("package")
        .and_then(|package| package.get("description"))
        .and_then(toml::Value::as_str)
        .is_some_and(|description| description.starts_with("Local adapter"));
    let live_targets = manifest
        .get("test")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|target| {
            target
                .get("required-features")
                .and_then(toml::Value::as_array)
                .is_some_and(|features| {
                    features
                        .iter()
                        .any(|feature| feature.as_str() == Some("live"))
                })
        })
        .flat_map(|target| {
            ["name", "path"]
                .into_iter()
                .filter_map(|key| {
                    target
                        .get(key)
                        .and_then(toml::Value::as_str)
                        .map(str::to_owned)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    Package {
        local_adapter,
        live_targets,
        root: root.to_path_buf(),
        directory: directory.to_path_buf(),
        literals: OnceCell::new(),
    }
}

impl Package {
    /// The contents of the string literals of the package's `.rs` files, comments and ignores
    /// left out; read when a reason first needs them.
    fn literals(&self) -> &str {
        self.literals.get_or_init(|| {
            let mut files = Vec::new();
            rust_files(&self.root, &self.directory, &mut files);
            files
                .iter()
                .map(|file| {
                    let text = fs::read_to_string(file).unwrap();
                    lex(&without_ignores(&text, &ignores(&text))).literals
                })
                .collect()
        })
    }
}

/// Whether `file` of the package in `directory` is built only with feature `live`: its test
/// target requires the feature, or the file says `#![cfg(feature = "live")]`.
fn live_gated(directory: &Path, file: &Path, text: &str, package: &Package) -> bool {
    let relative = file.strip_prefix(directory).unwrap();
    let target = relative
        .strip_prefix("tests")
        .ok()
        .and_then(|inside| inside.components().next())
        .map(|first| {
            let first = first.as_os_str().to_string_lossy();
            first.strip_suffix(".rs").unwrap_or(&first).to_owned()
        });
    let by_manifest = package
        .live_targets
        .iter()
        .any(|named| Some(named) == target.as_ref() || Path::new(named) == relative);
    let by_file = text.lines().any(|line| {
        let line = line.trim();
        line.starts_with("#![cfg(")
            && line.contains("feature = \"live\"")
            && !line.contains("not(feature = \"live\")")
    });
    by_manifest || by_file
}

/// The words of `reason`: its runs of letters, digits and underscores.
fn words(reason: &str) -> impl Iterator<Item = &str> {
    reason
        .split(|character: char| !is_identifier(character))
        .filter(|word| !word.is_empty())
}

/// Whether `reason` says `billed`, other than right after `not` or `never`.
fn says_billed(reason: &str) -> bool {
    let words: Vec<&str> = words(reason).collect();
    words.iter().enumerate().any(|(at, word)| {
        *word == "billed" && !(at > 0 && ["not", "never"].contains(&words[at - 1]))
    })
}

/// Whether `ignore` gives a reason of an admitted kind; why not, if it does not.
fn admitted(ignore: &Ignore, context: &Context) -> Result<(), String> {
    let Some(reason) = ignore.reason.as_deref() else {
        return Err("it gives no reason".to_owned());
    };
    if says_billed(reason) {
        return Ok(());
    }
    if context.live && !context.package.local_adapter {
        return Err(
            "its test target is built only with feature `live`, which a hosted live test is, and the reason does not say `billed`"
                .to_owned(),
        );
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
    if let Some(statement) = reason.strip_prefix("limit:") {
        return limit(statement, context.root);
    }
    if reason.starts_with("a child of") {
        return child(ignore, context);
    }
    if reason.contains("by hand") {
        return if words(reason)
            .any(|word| ["microphone", "microphones", "speaker", "speakers"].contains(&word))
        {
            Ok(())
        } else {
            Err(
                "it says `by hand` but names no device it opens, a microphone or a speaker"
                    .to_owned(),
            )
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
        .find(|variable| !has_word(context.package.literals(), variable))
    {
        Some(variable) => Err(format!(
            "it names `{variable}`, which no string literal of its package holds"
        )),
        None => Ok(()),
    }
}

/// A `limit:` reason names a path of the repository that exists.
fn limit(statement: &str, root: &Path) -> Result<(), String> {
    let paths: Vec<&str> = statement
        .split_whitespace()
        .map(|word| {
            word.trim_start_matches(['(', '`', '\''])
                .trim_end_matches([',', ';', ':', ')', '.', '`', '\''])
        })
        .filter(|word| word.contains('/'))
        .collect();
    if paths.is_empty() {
        return Err(
            "it begins `limit:` but names no path of the file that states the limit".to_owned(),
        );
    }
    match paths
        .iter()
        .find(|path| path.split('/').any(|part| part == "..") || !root.join(path).is_file())
    {
        Some(path) => Err(format!(
            "it begins `limit:` and names `{path}`, which is no file of the repository"
        )),
        None => Ok(()),
    }
}

/// A child case is named by its file outside comments, its ignore and its own declaration.
fn child(ignore: &Ignore, context: &Context) -> Result<(), String> {
    let Some((declared, name)) = ignore.function.as_ref() else {
        return Err("it begins `a child of` but ignores no function".to_owned());
    };
    let rest: String = without_ignores(context.file, context.ignores)
        .lines()
        .enumerate()
        .map(|(index, line)| {
            if index == *declared {
                "\n".to_owned()
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    let lexed = lex(&rest);
    if has_word(&lexed.bare, name) || has_word(&lexed.literals, name) {
        Ok(())
    } else {
        Err(format!(
            "it is a child case, but its function `{name}` stands nowhere else in its file, so no parent runs it"
        ))
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
        "#[ignore = \"limit: crates/planted/src/lib.rs states it: red only on a shared processor\"]",
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

/// The refusals of a planted tree whose test file is `source` and whose manifest is `manifest`.
fn refusals_in(manifest: &str, source: &str) -> Vec<String> {
    let tree = planted(source);
    fs::write(tree.join("crates/planted/Cargo.toml"), manifest).unwrap();
    let refusals = scan(&tree).refusals;
    let _ = fs::remove_dir_all(&tree);
    refusals
}

#[test]
fn a_target_built_only_with_feature_live_needs_billed_unless_its_package_is_a_local_adapter() {
    let local = "#[test]\n#[ignore = \"needs the checkpoint in INFERENCE_PLANTED_DIR\"]\nfn planted_case() {}\n";
    let gated = format!("#![cfg(feature = \"live\")]\n{local}");
    let hosted =
        "[package]\nname = \"planted\"\ndescription = \"Planted for the inference framework\"\n";
    let required =
        format!("{hosted}\n[[test]]\nname = \"planted\"\nrequired-features = [\"live\"]\n");
    for (manifest, source) in [(hosted, gated.as_str()), (required.as_str(), local)] {
        let refusals = refusals_in(manifest, source);
        assert_eq!(refusals.len(), 1, "{manifest}{source}: {refusals:?}");
        assert!(refusals[0].contains("feature `live`"), "{}", refusals[0]);
    }
    let adapter = "[package]\nname = \"planted\"\ndescription = \"Local adapter of the inference framework\"\n";
    assert_eq!(refusals_in(adapter, &gated), Vec::<String>::new());
    assert_eq!(refusals_in(hosted, local), Vec::<String>::new());
}

#[test]
fn a_limit_that_names_no_file_of_the_repository_is_refused() {
    let refusals = refusals_of("#[ignore = \"limit: crates/planted/src/gone.rs states it\"]");
    assert_eq!(refusals.len(), 1, "{refusals:?}");
    assert!(
        refusals[0].contains("`crates/planted/src/gone.rs`"),
        "{}",
        refusals[0]
    );
}

#[test]
fn the_build_directories_are_not_read_and_other_directories_named_target_are() {
    let tree = planted("");
    let ignored = "#[test]\n#[ignore]\nfn planted_case() {}\n";
    for (directory, tag) in [("target", false), ("crates/planted/build", true)] {
        fs::create_dir_all(tree.join(directory)).unwrap();
        fs::write(tree.join(directory).join("built.rs"), ignored).unwrap();
        if tag {
            fs::write(tree.join(directory).join("CACHEDIR.TAG"), "").unwrap();
        }
    }
    fs::create_dir_all(tree.join("crates/planted/target")).unwrap();
    fs::write(tree.join("crates/planted/target/mod.rs"), ignored).unwrap();
    let found = scan(&tree);
    let _ = fs::remove_dir_all(&tree);
    assert_eq!(found.read, 1, "{:?}", found.refusals);
    assert!(
        found.refusals[0].starts_with("crates/planted/target/mod.rs:2: "),
        "{}",
        found.refusals[0]
    );
}
