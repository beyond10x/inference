//! Adversary pass 1 on `story:llm-0-8-3`: the source scan of `llm_release` reads text, so a
//! resolver implemented through a qualified path (`impl llm_credentials::SecretResolver for`), or
//! `Arc::new(AnonymousResolver)` named only in a comment, passes it. This reads the code lines
//! alone, every file under `src/`, and fails when it reads none.

use std::fs;
use std::path::{Path, PathBuf};

fn sources(dir: &Path, into: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("a source directory") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            sources(&path, into);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            into.push(path);
        }
    }
}

/// The lines of `source` that are code: comment lines and doc lines dropped.
fn code(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_code_of_the_adapter_implements_no_resolver_and_hands_llms() {
    let mut files = Vec::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(
        files.iter().any(|file| file.ends_with("lib.rs")),
        "the scan read the crate root: {files:?}"
    );
    let mut own = Vec::new();
    let mut handed = 0;
    for file in &files {
        let code = code(&fs::read_to_string(file).expect("a source file"));
        let flat: String = code.split_whitespace().collect();
        if flat.contains("SecretResolverfor") {
            own.push(file.display().to_string());
        }
        handed += flat.matches("Arc::new(AnonymousResolver)").count();
    }
    assert!(own.is_empty(), "a resolver of the adapter's own: {own:?}");
    assert_eq!(
        handed, 1,
        "the one client is handed llm's AnonymousResolver"
    );
}
