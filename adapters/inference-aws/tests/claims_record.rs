//! The claims record of the Nova Sonic duplex binding, `records/claims-nova-sonic.json`, holds
//! exactly the claims the binding makes in code, in the same order. The documentation's adapter
//! reference is generated from this record by a crate that does not link this one, so a claim
//! added to or dropped from the binding fails here until the record says the same.
#![cfg(feature = "duplex")]

use std::fs;
use std::path::Path;

use inference_duplex::inference_core::model::Capability;
use serde::Deserialize;

/// A hosted binding's claims, as the documentation reads them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimsRecord {
    model: String,
    binding: String,
    features: Vec<String>,
    claims: Vec<Capability>,
    held_by: String,
    adapter_revision: String,
}

#[test]
fn the_nova_sonic_record_holds_the_claims_of_the_binding() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("records/claims-nova-sonic.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    let record: ClaimsRecord = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not a claims record: {error}", path.display()));
    assert_eq!(record.claims, inference_aws::duplex::CLAIMS);
    assert_eq!(record.model, inference_aws::nova_sonic::DEFAULT_MODEL_ID);
    assert_eq!(record.binding, "inference_aws::duplex::NovaSonicSpeech");
    assert_eq!(record.features, ["duplex"]);
    assert_eq!(
        record.held_by,
        "claims_record::the_nova_sonic_record_holds_the_claims_of_the_binding"
    );
    assert_eq!(record.adapter_revision, env!("CARGO_PKG_VERSION"));
}
