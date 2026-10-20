//! Adversary pass 1: what a failed stream start puts into its error message.
//!
//! No network: the credential provider fails while it reads an in-memory profile file, before any
//! request is signed, and the endpoint is a closed local port. The profile and sso-session names
//! below stand for an operator's private names, which the brief forbids printing; the live test
//! prints a [`NovaSonicError::Connect`] message when the stream does not open.

#![allow(deprecated)]

use aws_config::profile::ProfileFileCredentialsProvider;
use aws_config::profile::profile_file::{ProfileFileKind, ProfileFiles};
use aws_sdk_bedrockruntime::config::{Config, Region};
use inference_aws::nova_sonic::{NovaSonic, NovaSonicError, SessionConfig};

const PROFILE: &str = "adv1-private-profile";
const SSO_SESSION: &str = "adv1-private-sso-session";

/// A profile that names an sso-session the file does not define: the shape of a typo in an
/// operator's own configuration, which the SDK reports as an invalid configuration.
fn misconfigured_profile() -> ProfileFileCredentialsProvider {
    let contents = format!(
        "[profile {PROFILE}]\nsso_session = {SSO_SESSION}\nsso_account_id = 000000000000\nsso_role_name = adv1-role\nregion = us-east-1\n"
    );
    ProfileFileCredentialsProvider::builder()
        .profile_files(
            ProfileFiles::builder()
                .include_default_config_file(false)
                .include_default_credentials_file(false)
                .with_contents(ProfileFileKind::Config, contents)
                .build(),
        )
        .profile_name(PROFILE)
        .build()
}

#[tokio::test]
async fn a_connect_error_names_no_profile_or_sso_session() {
    let config = Config::builder()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .endpoint_url("http://127.0.0.1:9")
        .credentials_provider(misconfigured_profile())
        .build();
    let nova = NovaSonic::new(
        aws_sdk_bedrockruntime::Client::from_conf(config),
        "amazon.nova-2-sonic-v1:0",
    );
    let result = nova.start(&SessionConfig::new("x")).await;
    let Err(NovaSonicError::Connect(message)) = result else {
        panic!("expected a connect error, got {result:?}");
    };
    assert!(
        !message.contains(PROFILE) && !message.contains(SSO_SESSION),
        "the connect error names the operator's profile or sso-session: {message}"
    );
}
