//! Adversary pass 1 on `story:duplex-turn-ids`: `InputItemId` is documented "Never empty", and
//! `InputItemId::from_provider` turns an empty provider id into none. An event read back from its
//! serialized form is the other way an id is made, and it bypasses `InputItemId::new`.

use inference_duplex::{InputCommitted, InputItemId};

#[test]
fn an_empty_item_id_is_not_read_from_its_serialized_form() {
    let read = serde_json::from_str::<InputItemId>("\"\"");
    assert!(
        read.is_err(),
        "an input item id is never empty, yet \"\" reads as {read:?}"
    );
}

#[test]
fn an_input_committed_naming_an_empty_item_is_not_read() {
    let session_id = uuid_text();
    let read = serde_json::from_str::<InputCommitted>(&format!(
        "{{\"session_id\":\"{session_id}\",\"item\":\"\"}}"
    ));
    assert!(
        read.is_err(),
        "a commit names its user turn by a non-empty id, yet one naming \"\" reads as {read:?}"
    );
}

fn uuid_text() -> String {
    let id = inference_duplex::SpeechSessionId::generate();
    serde_json::to_value(id)
        .expect("a session id serializes")
        .as_str()
        .expect("as a string")
        .to_owned()
}
