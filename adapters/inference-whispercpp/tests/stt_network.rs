//! The recogniser runs local-only (`docs/design/inference.md:1120`): nothing it runs with can
//! open a connection, and nothing it builds with downloads. `cargo tree -p inference-whispercpp`
//! names no HTTP, TLS or socket crate over the normal edges, nor over every edge kind:
//! `whisper-rs-sys` compiles the whisper.cpp sources it ships with `cmake` and generates its
//! bindings with `bindgen`, so no download client is among its build dependencies. The adapter
//! declares that it runs in the calling process. Needs no weights.

use std::process::Command;

use inference_stt::inference_core::model::{ExecutionForm, Locality};
use inference_whispercpp::stt::{DEFAULT_MAX_CONCURRENCY, declare_adapter, declare_model};

/// Crates that speak HTTP, TLS or WebSocket, or open sockets.
const NETWORK_CRATES: [&str; 22] = [
    "reqwest",
    "hyper",
    "hyper-util",
    "h2",
    "h3",
    "ureq",
    "curl",
    "curl-sys",
    "isahc",
    "attohttpc",
    "rustls",
    "native-tls",
    "openssl",
    "openssl-sys",
    "webpki-roots",
    "tungstenite",
    "tokio-tungstenite",
    "async-tungstenite",
    "tokio",
    "async-std",
    "mio",
    "socket2",
];

/// The names of the crates `cargo tree` prints for the adapter over the edge kinds `edges`.
fn tree(edges: &str) -> Vec<String> {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "inference-whispercpp",
            "-e",
            edges,
            "--prefix",
            "none",
            "--locked",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "cargo tree failed: {stderr}");
    String::from_utf8(output.stdout)
        .expect("cargo tree prints UTF-8")
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_owned))
        .collect()
}

fn network(crates: &[String]) -> Vec<&str> {
    NETWORK_CRATES
        .into_iter()
        .filter(|name| crates.iter().any(|present| present == name))
        .collect()
}

#[test]
fn the_normal_dependency_tree_names_no_network_crate() {
    let normal = tree("normal");
    for present in ["inference-stt", "whisper-rs", "whisper-rs-sys", "sha2"] {
        assert!(
            normal.iter().any(|name| name == present),
            "the normal tree names {present}: {normal:?}"
        );
    }
    assert_eq!(
        network(&normal),
        Vec::<&str>::new(),
        "the normal tree names no network crate: {normal:?}"
    );
}

#[test]
fn the_build_downloads_nothing() {
    let built = tree("normal,build");
    for present in ["cmake", "bindgen"] {
        assert!(
            built.iter().any(|name| name == present),
            "the tree of normal and build edges names {present}, so whisper.cpp is compiled \
             here: {built:?}"
        );
    }
    assert_eq!(
        network(&built),
        Vec::<&str>::new(),
        "the tree of normal and build edges names no network crate: {built:?}"
    );
}

#[test]
fn the_adapter_declares_that_it_runs_in_the_calling_process() {
    let (model, _) = declare_model();
    let (adapter, _) = declare_adapter(model.model_id(), DEFAULT_MAX_CONCURRENCY)
        .expect("the default maximum concurrency is positive");
    assert_eq!(adapter.locality(), Locality::SameProcess);
    assert_eq!(adapter.execution_form(), ExecutionForm::NativeFfi);
}
