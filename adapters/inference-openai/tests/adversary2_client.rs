//! Adversary cases, pass 2 (final review): server event decoding read against the published
//! Realtime reference (`RealtimeResponse` in the OpenAI OpenAPI document), offline.

use inference_openai::{ResponseStatus, ServerEvent, Usage};
use serde_json::json;

/// `RealtimeResponse.usage` declares `total_tokens`, `input_tokens` and `output_tokens` with no
/// `required` list. `RealtimeError::Protocol` is documented for "a typed event missing a field the
/// reference requires"; a usage object without its totals is not that, and refusing it fails a
/// whole duplex session as a provider error.
#[test]
fn a_response_done_whose_usage_lacks_the_totals_is_not_a_protocol_violation() {
    let parsed = ServerEvent::parse(
        &json!({
            "type": "response.done",
            "event_id": "event_rd",
            "response": {
                "object": "realtime.response",
                "id": "resp_1",
                "status": "cancelled",
                "output": [],
                "usage": {"output_token_details": {"text_tokens": 0, "audio_tokens": 0}}
            }
        })
        .to_string(),
    );
    assert!(
        matches!(
            &parsed,
            Ok(ServerEvent::ResponseDone { response_id: Some(response_id), status: Some(ResponseStatus::Cancelled), usage, unreadable_usage: None })
                if response_id == "resp_1"
                    && usage.is_none_or(|usage: Usage| usage.output_audio_tokens == Some(0))
        ),
        "{parsed:?}"
    );
}
