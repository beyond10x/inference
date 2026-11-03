//! `inference_core::usage::QuantityKind` follows `inference.usage.QuantityKind` of
//! `ess/domains/usage.yaml`: the test reads the variants from the specification file itself, so
//! a kind added to one and not the other, or listed in another order, fails here.

use inference_core::usage::QuantityKind;

/// The specification document, as committed beside the crate.
const USAGE_DOMAIN: &str = include_str!("../../../ess/domains/usage.yaml");

/// The variants of the enum type `name` in a domain document, in the order it lists them: the
/// `- name:` line of the type, then each `- <variant>` line under its `variants:` key.
fn variants(document: &str, name: &str) -> Vec<String> {
    let mut lines = document
        .lines()
        .skip_while(|line| line.trim() != format!("- name: {name}"))
        .skip(1);
    for line in lines.by_ref() {
        match line.trim() {
            "variants:" => break,
            other if other.starts_with("- name:") => {
                panic!("{name} has no block `variants:` list in the document")
            }
            _ => {}
        }
    }
    lines
        .map(str::trim)
        .take_while(|line| line.starts_with("- ") && !line.starts_with("- name:"))
        .map(|line| line.trim_start_matches("- ").to_owned())
        .collect()
}

#[test]
fn quantity_kind_all_matches_the_specification() {
    let specified = variants(USAGE_DOMAIN, "inference.usage.QuantityKind");
    assert!(
        specified
            .iter()
            .any(|kind| kind == "cached_input_audio_tokens"),
        "the specification declares cached_input_audio_tokens: {specified:?}"
    );
    let implemented: Vec<String> = QuantityKind::ALL
        .iter()
        .map(|kind| {
            serde_json::to_value(kind)
                .expect("serializes")
                .as_str()
                .expect("a kind serializes as a string")
                .to_owned()
        })
        .collect();
    assert_eq!(
        implemented, specified,
        "QuantityKind::ALL serializes to the variants of inference.usage.QuantityKind, in order"
    );
    for name in &specified {
        let read: QuantityKind = serde_json::from_value(serde_json::Value::String(name.clone()))
            .unwrap_or_else(|error| panic!("{name} is not read: {error}"));
        assert_eq!(
            serde_json::to_value(read).expect("serializes"),
            name.as_str()
        );
    }
}
