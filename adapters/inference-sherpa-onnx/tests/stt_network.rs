//! The recogniser runs local-only (`docs/design/inference.md:1120`): nothing it runs with can
//! open a connection. `cargo tree -p inference-sherpa-onnx -e normal --features stt` names no
//! HTTP, TLS or socket crate, and the adapter declares that it runs in the calling process. The
//! native archive is fetched by the build dependencies of `sherpa-onnx-sys`, whose HTTP and TLS
//! crates are in the tree of every edge kind and never in the normal one. Needs no weights.
#![cfg(feature = "stt")]

use std::process::Command;

use inference_sherpa_onnx::stt::{DEFAULT_MAX_CONCURRENCY, declare_adapter, declare_model};
use inference_stt::inference_core::model::{ExecutionForm, Locality};

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

/// The names of the crates `cargo tree` prints for the adapter with feature `stt` over the edge
/// kinds `edges`.
fn tree(edges: &str) -> Vec<String> {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "inference-sherpa-onnx",
            "--features",
            "stt",
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
    for present in ["inference-stt", "sherpa-onnx", "sherpa-onnx-sys"] {
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
fn the_archive_download_is_reached_only_through_build_dependencies() {
    let every_edge = tree("all");
    let normal = tree("normal");
    let downloaders = network(&every_edge);
    assert!(
        downloaders.contains(&"ureq"),
        "the tree of every edge kind names sherpa-onnx-sys's download client, so the list of \
         network crates finds what is there: {every_edge:?}"
    );
    for name in downloaders {
        assert!(
            !normal.iter().any(|present| present == name),
            "{name} is not in the normal tree"
        );
    }
}

#[test]
fn the_adapter_declares_that_it_runs_in_the_calling_process() {
    let (model, _) = declare_model();
    let (adapter, _) = declare_adapter(model.model_id(), DEFAULT_MAX_CONCURRENCY)
        .expect("the default maximum concurrency is positive");
    assert_eq!(adapter.locality(), Locality::SameProcess);
    assert_eq!(adapter.execution_form(), ExecutionForm::NativeFfi);
}
