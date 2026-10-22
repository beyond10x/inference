//! The claims records of the two duplex bindings, `records/claims-realtime.json` and
//! `records/claims-gpt-live.json`, hold exactly the claims the bindings make in code, in the same
//! order. The documentation's adapter reference is generated from these records by a crate that
//! does not link this one, so a claim added to or dropped from a binding fails here until its
//! record says the same.
#![cfg(feature = "duplex")]

use std::fs;
use std::path::Path;

use inference_core::model::Capability;
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

fn load(record: &str) -> ClaimsRecord {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(record);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not a claims record: {error}", path.display()))
}

#[test]
fn the_realtime_record_holds_the_claims_of_the_binding() {
    let record = load("records/claims-realtime.json");
    assert_eq!(record.claims, inference_openai::duplex::claims());
    assert_eq!(record.model, inference_openai::DEFAULT_MODEL);
    assert_eq!(record.binding, "inference_openai::duplex::OpenAiRealtime");
    assert_eq!(record.features, ["duplex"]);
    assert_eq!(
        record.held_by,
        "claims_record::the_realtime_record_holds_the_claims_of_the_binding"
    );
    assert_eq!(record.adapter_revision, env!("CARGO_PKG_VERSION"));
}

#[cfg(feature = "gpt-live")]
#[test]
fn the_gpt_live_record_holds_the_claims_of_the_binding() {
    let record = load("records/claims-gpt-live.json");
    assert_eq!(record.claims, inference_openai::live_duplex::claims());
    assert_eq!(record.model, inference_openai::live::LIVE_MODEL);
    assert_eq!(record.binding, "inference_openai::live_duplex::OpenAiLive");
    assert_eq!(record.features, ["duplex", "gpt-live"]);
    assert_eq!(
        record.held_by,
        "claims_record::the_gpt_live_record_holds_the_claims_of_the_binding"
    );
    assert_eq!(record.adapter_revision, env!("CARGO_PKG_VERSION"));
}
