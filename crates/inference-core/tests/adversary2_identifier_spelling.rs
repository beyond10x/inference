//! Adversary cases for story:workspace-skeleton, pass 2: the identifier newtypes against the one
//! Uuid spelling ESS publishes. ESS accepts a Uuid only in the canonical hyphenated form and
//! refuses the `urn:uuid:`, brace-wrapped and unhyphenated spellings, so that one value has one
//! spelling (`ess-primitives`, `is_canonical_uuid`). Check 3 says each identifier round-trips "as
//! the Uuid string it wraps"; a spelling that is accepted and written back differently does not.

use inference_core::id::{GenerationId, NodeId, RunId, SessionId, StreamId};
use inference_core::model::{AdapterId, ModelId};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const CANONICAL: &str = "00000000-0000-4000-8000-dfb547682652";

fn non_canonical_spellings() -> [String; 3] {
    [
        format!("urn:uuid:{CANONICAL}"),
        format!("{{{CANONICAL}}}"),
        CANONICAL.replace('-', ""),
    ]
}

fn assert_only_canonical_spelling<T>(name: &str) -> Vec<String>
where
    T: DeserializeOwned + Serialize,
{
    let canonical: T = serde_json::from_value(json!(CANONICAL))
        .unwrap_or_else(|error| panic!("{name} refused the canonical spelling: {error}"));
    assert_eq!(
        serde_json::to_value(&canonical).expect("serializes"),
        json!(CANONICAL)
    );

    let mut accepted = Vec::new();
    for spelling in non_canonical_spellings() {
        if let Ok(id) = serde_json::from_value::<T>(Value::String(spelling.clone())) {
            let written = serde_json::to_value(&id).expect("serializes");
            accepted.push(format!("{name} accepted {spelling:?} and wrote {written}"));
        }
    }
    accepted
}

#[test]
fn adversary2_identifiers_accept_only_the_canonical_uuid_spelling() {
    let mut accepted = Vec::new();
    accepted.extend(assert_only_canonical_spelling::<ModelId>("ModelId"));
    accepted.extend(assert_only_canonical_spelling::<AdapterId>("AdapterId"));
    accepted.extend(assert_only_canonical_spelling::<RunId>("RunId"));
    accepted.extend(assert_only_canonical_spelling::<NodeId>("NodeId"));
    accepted.extend(assert_only_canonical_spelling::<StreamId>("StreamId"));
    accepted.extend(assert_only_canonical_spelling::<GenerationId>(
        "GenerationId",
    ));
    accepted.extend(assert_only_canonical_spelling::<SessionId>("SessionId"));
    assert!(
        accepted.is_empty(),
        "{} non-canonical spellings accepted, ESS refuses each:\n{}",
        accepted.len(),
        accepted.join("\n")
    );
}

#[test]
fn adversary2_declare_adapter_refuses_a_braced_model_id() {
    let input = json!({
        "backend": "backend",
        "capabilities": ["output_streaming"],
        "contract_revision": "contract_revision",
        "execution_form": "in_process",
        "locality": "same_process",
        "max_concurrency": 1,
        "model_id": format!("{{{CANONICAL}}}"),
    });
    let parsed: Result<inference_core::model::DeclareAdapter, _> = serde_json::from_value(input);
    assert!(
        parsed.is_err(),
        "DeclareAdapter accepted model_id {{{CANONICAL}}}, a spelling ESS refuses: {parsed:?}"
    );
}
