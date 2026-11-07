//! Conformance check of the credential contract: a blank key is refused as
//! `CredentialError::Invalid` before the realtime connection is opened.

use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, RealtimeClient,
    RealtimeError, SessionConfig,
};

struct Blank(&'static str);

impl CredentialSource for Blank {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::ready(Ok(Credential::new(
            self.0.as_bytes().to_vec(),
        ))))
    }
}

#[tokio::test]
async fn a_blank_key_is_refused_as_invalid_before_connecting() {
    for key in ["", "   ", "\t"] {
        let endpoint = Endpoint::new("gpt-realtime-2.1").with_url("ws://127.0.0.1:9/v1/realtime");
        let result = RealtimeClient::connect(
            &endpoint,
            &Blank(key),
            "openai-api-key",
            &SessionConfig::default(),
        )
        .await;
        assert!(
            matches!(
                result,
                Err(RealtimeError::Credential(CredentialError::Invalid))
            ),
            "key {key:?}: {:?}",
            result.err()
        );
    }
}
