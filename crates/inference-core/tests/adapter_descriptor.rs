//! `inference.model.AdapterDescriptor` (`story:contract-usage-and-naming-drift`): the descriptors
//! of design §8.2 and §7.3 the capability record does not carry, every part optional, on the wire
//! under the names `ess/domains/model.yaml` declares.

use inference_core::model::{AdapterDescriptor, RateLimit};
use serde_json::json;

const MODEL_DOMAIN: &str = include_str!("../../../ess/domains/model.yaml");

#[test]
fn a_descriptor_with_voices_a_context_size_and_a_rate_limit_round_trips_through_its_wire_form() {
    let descriptor = AdapterDescriptor {
        voices: vec!["alloy".to_owned(), "verse".to_owned()],
        context_tokens: Some(128_000),
        rate_limits: vec![RateLimit {
            unit: "requests".to_owned(),
            limit: 60,
            window_ms: 60_000,
        }],
        ..AdapterDescriptor::default()
    };
    let wire = serde_json::to_value(&descriptor).expect("serializes");
    assert_eq!(
        wire,
        json!({
            "voices": ["alloy", "verse"],
            "codecs": [],
            "context_tokens": 128_000,
            "max_input_duration_ms": null,
            "max_output_duration_ms": null,
            "token_budget": null,
            "state_scope": null,
            "rate_limits": [{"unit": "requests", "limit": 60, "window_ms": 60_000}],
        })
    );
    let back: AdapterDescriptor = serde_json::from_value(wire).expect("deserializes");
    assert_eq!(back, descriptor);
}

#[test]
fn a_part_left_out_is_undeclared_and_a_field_ess_does_not_declare_is_refused() {
    let empty: AdapterDescriptor = serde_json::from_value(json!({})).expect("every part optional");
    assert_eq!(empty, AdapterDescriptor::default());
    assert!(
        serde_json::from_value::<AdapterDescriptor>(json!({"voice": ["alloy"]})).is_err(),
        "a misspelt field is not read as a descriptor without it"
    );
    assert!(
        serde_json::from_value::<RateLimit>(json!({"unit": "requests", "limit": 1})).is_err(),
        "a rate limit names its window"
    );
}

#[test]
fn the_descriptor_and_the_rate_limit_are_declared_in_model_yaml() {
    for name in [
        "inference.model.AdapterDescriptor",
        "inference.model.RateLimit",
    ] {
        assert!(
            MODEL_DOMAIN.contains(&format!("  - name: {name}\n    kind: struct\n")),
            "{name} is a struct type of model.yaml"
        );
    }
    let descriptor = serde_json::to_value(AdapterDescriptor::default()).expect("serializes");
    let fields = descriptor.as_object().expect("an object");
    let declared = MODEL_DOMAIN
        .split("  - name: inference.model.AdapterDescriptor\n")
        .nth(1)
        .expect("declared")
        .split("\n\n")
        .next()
        .expect("its declaration");
    for field in fields.keys() {
        assert!(
            declared.contains(&format!("      - name: {field}\n")),
            "{field} is declared on the descriptor in model.yaml"
        );
    }
    assert_eq!(
        declared.matches("      - name: ").count(),
        fields.len(),
        "every declared field is on the wire"
    );
}
