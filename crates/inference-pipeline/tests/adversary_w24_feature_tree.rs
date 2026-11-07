//! Each model-contract dependency of `inference-pipeline` is optional and is pulled by its own
//! feature alone: without features the normal tree names none of `inference-stt`,
//! `inference-llm`, `inference-tts` and `inference-duplex`, and each of the features `stt`,
//! `llm`, `tts` and `duplex` adds its own contract and none of the other three. The example
//! `stt-only` already notices `inference-tts` or `inference-llm` turning unconditional; nothing
//! else notices `inference-stt` or `inference-duplex` doing so, since that example and the
//! testkit depend on both anyway.

use std::process::Command;

const CONTRACTS: [&str; 4] = [
    "inference-stt",
    "inference-llm",
    "inference-tts",
    "inference-duplex",
];

fn normal_tree(features: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "tree",
        "-p",
        "inference-pipeline",
        "-e",
        "normal",
        "--locked",
        "--color",
        "never",
        "--no-default-features",
    ];
    if let Some(features) = features {
        args.push("--features");
        args.push(features);
    }
    let output = Command::new(env!("CARGO"))
        .args(&args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "cargo tree failed: {stderr}");
    String::from_utf8(output.stdout)
        .expect("cargo tree prints UTF-8")
        .lines()
        .filter_map(|line| {
            line.trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
                .split_whitespace()
                .next()
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn without_features_the_pipeline_names_no_model_contract() {
    let crates = normal_tree(None);
    assert!(crates.iter().any(|name| name == "inference-core"));
    for absent in CONTRACTS {
        assert!(
            !crates.iter().any(|name| name == absent),
            "the featureless tree does not name {absent}: {crates:?}"
        );
    }
}

#[test]
fn each_feature_pulls_its_own_contract_and_no_other() {
    for (feature, contract) in [
        ("stt", "inference-stt"),
        ("llm", "inference-llm"),
        ("tts", "inference-tts"),
        ("duplex", "inference-duplex"),
    ] {
        let crates = normal_tree(Some(feature));
        assert!(
            crates.iter().any(|name| name == contract),
            "feature {feature} names {contract}: {crates:?}"
        );
        for absent in CONTRACTS.iter().filter(|name| **name != contract) {
            assert!(
                !crates.iter().any(|name| name == absent),
                "feature {feature} does not name {absent}: {crates:?}"
            );
        }
    }
}
