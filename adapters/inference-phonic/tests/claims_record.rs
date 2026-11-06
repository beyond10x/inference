//! The claims record of the binding, `records/claims-duplex.json`, holds exactly the claims the
//! binding makes in code, in the same order. The documentation's adapter reference is generated
//! from the record by a crate that does not link this one, so a claim added to or dropped from
//! the binding fails here until its record says the same.

use std::fs;
use std::path::Path;

use inference_phonic::inference_duplex::inference_core::model::Capability;
use serde_json::Value;

#[test]
fn the_duplex_record_holds_the_claims_of_the_binding() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("records/claims-duplex.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    let record: Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()));
    let mut fields: Vec<_> = record
        .as_object()
        .expect("the record is an object")
        .keys()
        .cloned()
        .collect();
    fields.sort();
    assert_eq!(
        fields,
        [
            "adapter_revision",
            "binding",
            "claims",
            "features",
            "held_by",
            "model"
        ]
    );
    let claims: Vec<Capability> =
        serde_json::from_value(record["claims"].clone()).expect("the claims are capabilities");
    assert_eq!(claims, inference_phonic::claims());
    assert_eq!(record["model"], inference_phonic::DEFAULT_MODEL);
    assert_eq!(record["binding"], "inference_phonic::PhonicSpeech");
    assert_eq!(record["features"], serde_json::json!([]));
    assert_eq!(
        record["held_by"],
        "claims_record::the_duplex_record_holds_the_claims_of_the_binding"
    );
    assert_eq!(record["adapter_revision"], env!("CARGO_PKG_VERSION"));
}
