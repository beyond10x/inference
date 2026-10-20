//! A-01 of `docs/design/inference.md` (line 1137): a user depends on `inference-stt` and one
//! adapter only, without the graph runtime or TTS. `cargo tree -p stt-sherpa-onnx -e normal` is
//! run without `--all-features`, so a workspace-wide feature flag does not unify features into
//! it, and its normal dependencies name the STT contract and the sherpa-onnx adapter, and none of
//! the TTS or LLM contracts, the runtime, the pipeline or the testkit.

use std::process::Command;

#[test]
fn stt_sherpa_onnx_depends_on_the_stt_contract_and_the_adapter_alone() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "stt-sherpa-onnx",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--locked",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "cargo tree failed: {stderr}");
    let tree = String::from_utf8(output.stdout).expect("cargo tree prints UTF-8");
    let crates: Vec<&str> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();

    for present in ["inference-stt", "inference-sherpa-onnx"] {
        assert!(
            crates.contains(&present),
            "the tree names {present}:\n{tree}"
        );
    }
    for absent in [
        "inference-tts",
        "inference-llm",
        "inference-runtime",
        "inference-pipeline",
        "inference-testkit",
    ] {
        assert!(
            !crates.contains(&absent),
            "the tree does not name {absent}:\n{tree}"
        );
    }
}
