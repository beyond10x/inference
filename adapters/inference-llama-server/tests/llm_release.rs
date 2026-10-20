//! The `llm` release the adapter is built on (`story:llm-0-8-3`): every `b10x-llm-*` package in
//! its tree is at 0.8.3, and the resolver its client is handed is `llm`'s `AnonymousResolver`, not
//! one of the adapter's own. `cargo tree -p inference-llama-server -e normal --all-features` is
//! read so that the packages the `tools` feature pulls count too.

use std::fs;
use std::path::Path;
use std::process::Command;

/// The release of `llm` every `b10x-llm-*` package of the tree is at.
const RELEASE: &str = "0.8.3";

#[test]
fn every_llm_package_of_the_adapter_is_at_the_pinned_release() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "inference-llama-server",
            "-e",
            "normal",
            "--all-features",
            "--prefix",
            "none",
            "--locked",
            "--color",
            "never",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "cargo tree failed: {stderr}");
    let tree = String::from_utf8(output.stdout).expect("cargo tree prints UTF-8");
    let llm: Vec<(&str, &str)> = tree
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?;
            let version = words.next()?;
            name.starts_with("b10x-llm-").then_some((name, version))
        })
        .collect();
    for name in [
        "b10x-llm-blocking",
        "b10x-llm-chat",
        "b10x-llm-core",
        "b10x-llm-credentials",
        "b10x-llm-http",
    ] {
        assert!(
            llm.iter().any(|(listed, _)| *listed == name),
            "the tree names {name}:\n{tree}"
        );
    }
    let expected = format!("v{RELEASE}");
    let elsewhere: Vec<_> = llm
        .iter()
        .filter(|(_, version)| *version != expected)
        .collect();
    assert!(
        elsewhere.is_empty(),
        "every b10x-llm package is at {RELEASE}, these are not: {elsewhere:?}\n{tree}"
    );
}

#[test]
fn the_adapter_source_holds_no_resolver_of_its_own() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut own = Vec::new();
    let mut handed = false;
    for entry in fs::read_dir(&src).expect("the source directory") {
        let path = entry.expect("an entry").path();
        let source = fs::read_to_string(&path).expect("a source file");
        if source.contains("impl SecretResolver for") {
            own.push(path.display().to_string());
        }
        handed |= source.contains("Arc::new(AnonymousResolver)");
    }
    assert!(own.is_empty(), "a resolver of the adapter's own: {own:?}");
    assert!(handed, "the client is handed llm's AnonymousResolver");
}
