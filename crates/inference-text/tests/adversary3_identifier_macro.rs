//! Adversary pass 2 against `inference_core::uuid_newtype!` now that `inference-text` declares
//! `SegmentId` with it: every identifier of `inference_core` and `SegmentId` must read, refuse and
//! write the same spellings the same way, with the same error, as the Uuid string they wrap. ESS
//! reads a Uuid in the canonical hyphenated form 8-4-4-4-12 only, in either case, and the
//! identifiers write it in lower case.

use inference_core::__private::uuid::Uuid;
use inference_core::id::{GenerationId, NodeId, RunId, SessionId, StreamId};
use inference_core::model::{AdapterId, ModelId};
use inference_text::SegmentId;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const CANONICAL: &str = "0f1e2d3c-4b5a-4987-a6b5-c4d3e2f1a0b9";

/// What one identifier type makes of each spelling: the value it writes back, or its error.
fn behaviour<T>() -> Vec<(Value, Result<Value, String>)>
where
    T: DeserializeOwned + Serialize + std::fmt::Display,
{
    let spellings = [
        json!(CANONICAL),
        json!(CANONICAL.to_uppercase()),
        json!("0F1E2D3C-4b5a-4987-A6B5-c4d3e2f1a0b9"),
        json!("00000000-0000-0000-0000-000000000000"),
        json!("ffffffff-ffff-ffff-ffff-ffffffffffff"),
        json!(format!("urn:uuid:{CANONICAL}")),
        json!(format!("{{{CANONICAL}}}")),
        json!(CANONICAL.replace('-', "")),
        json!(format!(" {CANONICAL}")),
        json!(format!("{CANONICAL} ")),
        json!(format!("{CANONICAL}-0")),
        json!("0f1e2d3c-4b5a-4987-a6b5-c4d3e2f1a0b"),
        json!("0f1e2d3c-4b5a-4987-a6b5-c4d3e2f1a0bg"),
        json!("0f1e2d3c-4b5a-4987-a6b5c-4d3e2f1a0b9"),
        json!("0f1e2d3c-4b5a-4987-a6b5-c4d3e2f1a0b\u{e9}"),
        json!(""),
        json!(42),
        json!(null),
        json!([CANONICAL]),
    ];
    spellings
        .into_iter()
        .map(|spelling| {
            let read = serde_json::from_value::<T>(spelling.clone())
                .map(|id| {
                    let written = serde_json::to_value(&id).expect("serializes");
                    assert_eq!(written, json!(id.to_string()), "Display is the wire form");
                    written
                })
                .map_err(|error| error.to_string());
            (spelling, read)
        })
        .collect()
}

#[test]
fn segment_id_and_every_core_identifier_read_refuse_and_write_alike() {
    let segment = behaviour::<SegmentId>();
    let accepted: Vec<_> = segment
        .iter()
        .filter_map(|(spelling, read)| read.as_ref().ok().map(|written| (spelling, written)))
        .collect();
    assert_eq!(
        accepted,
        vec![
            (&json!(CANONICAL), &json!(CANONICAL)),
            (&json!(CANONICAL.to_uppercase()), &json!(CANONICAL)),
            (
                &json!("0F1E2D3C-4b5a-4987-A6B5-c4d3e2f1a0b9"),
                &json!(CANONICAL)
            ),
            (
                &json!("00000000-0000-0000-0000-000000000000"),
                &json!("00000000-0000-0000-0000-000000000000")
            ),
            (
                &json!("ffffffff-ffff-ffff-ffff-ffffffffffff"),
                &json!("ffffffff-ffff-ffff-ffff-ffffffffffff")
            ),
        ],
        "SegmentId accepts exactly the canonical spellings and writes them in lower case"
    );
    let refusal = segment[5]
        .1
        .as_ref()
        .expect_err("the urn spelling is refused");
    assert!(
        refusal.contains("a Uuid in the canonical hyphenated form 8-4-4-4-12"),
        "{refusal}"
    );

    for (name, other) in [
        ("ModelId", behaviour::<ModelId>()),
        ("AdapterId", behaviour::<AdapterId>()),
        ("RunId", behaviour::<RunId>()),
        ("NodeId", behaviour::<NodeId>()),
        ("StreamId", behaviour::<StreamId>()),
        ("GenerationId", behaviour::<GenerationId>()),
        ("SessionId", behaviour::<SessionId>()),
    ] {
        assert_eq!(other, segment, "{name} and SegmentId differ");
    }
}

#[test]
fn segment_id_converts_orders_and_generates_as_the_uuid_it_wraps() {
    let uuids = [
        Uuid::from_u128(3),
        Uuid::from_u128(u128::MAX),
        Uuid::from_u128(1),
        Uuid::from_u128(1 << 64),
    ];
    let mut ids: Vec<SegmentId> = uuids.iter().copied().map(SegmentId::from).collect();
    for (id, uuid) in ids.iter().zip(uuids) {
        assert_eq!(id.as_uuid(), uuid);
        assert_eq!(Uuid::from(*id), uuid);
        assert_eq!(id.to_string(), uuid.hyphenated().to_string());
        assert_eq!(id.to_string(), RunId::from(uuid).to_string());
    }
    ids.sort();
    let mut sorted = uuids;
    sorted.sort();
    assert_eq!(
        ids.iter().map(SegmentId::as_uuid).collect::<Vec<_>>(),
        sorted
    );

    let generated = SegmentId::generate();
    assert_eq!(generated.as_uuid().get_version_num(), 4);
    assert_ne!(generated, SegmentId::generate());
}
