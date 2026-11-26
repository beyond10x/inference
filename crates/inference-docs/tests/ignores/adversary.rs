//! Adversary cases for the ignore check: forms it does not read and kinds it admits too widely.

use std::fs;
use std::path::Path;

use super::{planted, scan};
use crate::root;

/// The refusals of a planted tree whose test file is `source`, with extra files beside it.
fn refusals_with(source: &str, extra: &[(&str, &str)]) -> (usize, Vec<String>) {
    let tree = planted(source);
    for (path, text) in extra {
        let path = tree.join("crates/planted").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let found = scan(&tree);
    let _ = fs::remove_dir_all(&tree);
    (found.read, found.refusals)
}

#[test]
fn a_long_cfg_attr_ignore_as_rustfmt_writes_it_is_read_and_refused() {
    let source = "#[test]\n#[cfg_attr(\n    all(not(feature = \"some_long_feature_name\"), not(target_os = \"macos\")),\n    ignore = \"flaky on a busy runner\"\n)]\nfn planted_case() {}\n";
    let (read, refusals) = refusals_with(source, &[]);
    assert_eq!(
        (read, refusals.len()),
        (1, 1),
        "read {read}, refusals {refusals:?}"
    );
}

#[test]
fn an_ignore_in_a_test_module_directory_named_target_is_read_and_refused() {
    let (read, refusals) = refusals_with(
        "mod target;\n",
        &[(
            "tests/target/mod.rs",
            "#[test]\n#[ignore]\nfn hidden() {}\n",
        )],
    );
    assert_eq!(
        (read, refusals.len()),
        (1, 1),
        "read {read}, refusals {refusals:?}"
    );
}

#[test]
fn a_hosted_live_test_that_names_its_clip_variable_and_drops_billed_is_refused() {
    let package = root().join("adapters/inference-elevenlabs");
    let original = fs::read_to_string(package.join("tests/stt_live.rs")).unwrap();
    let old = "#[ignore = \"billed: needs ELEVENLABS_API_KEY with speech-to-text access and INFERENCE_ELEVENLABS_STT_CLIP\"]";
    assert!(
        original.contains(old),
        "stt_live.rs no longer holds the reason this case rewrites"
    );
    let rewritten = original.replace(
        old,
        "#[ignore = \"calls ElevenLabs speech-to-text on the clip in INFERENCE_ELEVENLABS_STT_CLIP\"]",
    );
    let tree = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("ignores-adversary-hosted-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tree);
    let copy = tree.join("adapters/inference-elevenlabs");
    fs::create_dir_all(copy.join("tests")).unwrap();
    fs::copy(package.join("Cargo.toml"), copy.join("Cargo.toml")).unwrap();
    fs::write(copy.join("tests/stt_live.rs"), rewritten).unwrap();
    let found = scan(&tree);
    let _ = fs::remove_dir_all(&tree);
    assert_eq!(
        found.refusals.len(),
        1,
        "read {}, refusals {:?}",
        found.read,
        found.refusals
    );
}

#[test]
fn a_child_whose_name_stands_only_in_a_comment_is_refused() {
    let (_, refusals) = refusals_with(
        "// planted_case is run by nobody\n#[test]\n#[ignore = \"a child of the parent cases of this file\"]\nfn planted_case() {}\n",
        &[],
    );
    assert_eq!(refusals.len(), 1, "{refusals:?}");
}

#[test]
fn a_child_whose_name_is_only_a_prefix_of_another_function_is_refused() {
    let (_, refusals) = refusals_with(
        "#[test]\n#[ignore = \"a child of the parent cases of this file\"]\nfn case() {}\n\n#[test]\nfn case_two() {}\n",
        &[],
    );
    assert_eq!(refusals.len(), 1, "{refusals:?}");
}

#[test]
fn a_local_variable_the_package_names_only_in_a_comment_is_refused() {
    let (_, refusals) = refusals_with(
        "// INFERENCE_PLANTED_MODEL is not read yet\n#[test]\n#[ignore = \"needs the checkpoint in INFERENCE_PLANTED_MODEL\"]\nfn planted_case() {}\n",
        &[],
    );
    assert_eq!(refusals.len(), 1, "{refusals:?}");
}

#[test]
fn a_local_variable_that_is_only_a_prefix_of_one_the_package_reads_is_refused() {
    let (_, refusals) = refusals_with(
        "#[test]\n#[ignore = \"needs the checkpoint in INFERENCE_PLANTED\"]\nfn planted_case() {}\n",
        &[],
    );
    assert_eq!(refusals.len(), 1, "{refusals:?}");
}

#[test]
fn limit_and_by_hand_do_not_admit_a_flaky_reason() {
    for reason in [
        "limit: flaky on a busy runner",
        "flaky on a busy runner; rerun it by hand",
    ] {
        let (_, refusals) = refusals_with(
            &format!("#[test]\n#[ignore = \"{reason}\"]\nfn planted_case() {{}}\n"),
            &[],
        );
        assert_eq!(refusals.len(), 1, "{reason}: {refusals:?}");
    }
}

#[test]
fn not_billed_does_not_admit_a_hosted_reason() {
    let (_, refusals) = refusals_with(
        "#[test]\n#[ignore = \"calls the hosted API, not billed on the free tier; needs a key\"]\nfn planted_case() {}\n",
        &[],
    );
    assert_eq!(refusals.len(), 1, "{refusals:?}");
}

#[test]
fn forms_the_check_refuses_or_reads_as_it_should() {
    for source in [
        "#[test]\n#[ignore = r\"needs INFERENCE_PLANTED_DIR\"]\nfn planted_case() {}\n",
        "#[test]\n#[ignore =\n    \"needs INFERENCE_PLANTED_DIR\"]\nfn planted_case() {}\n",
        "#[test]\n#[ignore = \"needs credentials and INFERENCE_PLANTED_DIR\"]\nfn planted_case() {}\n",
        "#[test]\n#[ignore = \"costs money; needs INFERENCE_PLANTED_DIR\"]\nfn planted_case() {}\n",
        "#[test]\n#[ignore = \"needs two keys and INFERENCE_PLANTED_DIR\"]\nfn planted_case() {}\n",
    ] {
        let (read, refusals) = refusals_with(source, &[]);
        assert_eq!((read, refusals.len()), (1, 1), "{source}: {refusals:?}");
    }
    let (read, refusals) = refusals_with(
        "",
        &[
            (
                "src/unit.rs",
                "#[cfg(test)]\nmod t {\n    #[test]\n    #[ignore]\n    fn unit() {}\n}\n",
            ),
            ("examples/demo.rs", "#[test]\n#[ignore]\nfn example() {}\n"),
        ],
    );
    assert_eq!((read, refusals.len()), (2, 2), "{refusals:?}");
}
