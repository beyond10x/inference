//! `Endpoint` keeps exactly its 0.16.0 public fields, so a downstream adopter that builds it by
//! struct literal or destructures it exhaustively still compiles. This file fails to compile when
//! a field is added, removed or renamed; the deadlines are on `Deadlines` and the connecting
//! types instead.

use std::time::Duration;

use inference_openai::{DEFAULT_READ_TIMEOUT, DEFAULT_URL, Endpoint};

#[test]
fn endpoint_is_built_and_destructured_with_its_0_16_0_fields() {
    let endpoint = Endpoint {
        url: DEFAULT_URL.to_owned(),
        model: "gpt-realtime-2.1".to_owned(),
        read_timeout: DEFAULT_READ_TIMEOUT,
    };
    let Endpoint {
        url,
        model,
        read_timeout,
    } = endpoint.clone();
    assert_eq!(url, DEFAULT_URL);
    assert_eq!(model, "gpt-realtime-2.1");
    assert_eq!(read_timeout, Duration::from_secs(30));
    assert_eq!(endpoint, Endpoint::new("gpt-realtime-2.1"));
}
