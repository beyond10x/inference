//! The adapter serves a `llama-server` on loopback only (`story:llama-server-loopback-only`): a
//! base URL whose host is not loopback is refused before a binding or a client is made, so a
//! prompt never leaves the machine under the adapter's local record, and the refusal names the
//! URL's scheme, host and port, never its userinfo. No server is started and nothing is sent.

use inference_llama_server::{Config, LlamaServer, NotLoopback, binding, loopback};
use inference_llm::llm_core::ErrorCode;

const SERVED_MODEL: &str = "qwen2.5-1.5b-instruct";

/// Refused hosts. A `\` ends the authority of an `http` URL as `/` does (the WHATWG parser `llm`
/// reads the base URL with), so the host of the last one is `example.com`, not loopback.
const REMOTE: [&str; 11] = [
    "http://10.0.0.5:8080/v1",
    "http://example.com\\@127.0.0.1:8080/v1",
    "http://127.0.0.1@example.com:8080/v1",
    "http://example.com/@127.0.0.1:8080/v1",
    "http://192.0.2.1:8080/v1",
    "http://example.com:8080/v1",
    "https://10.0.0.1/v1",
    "http://[2001:db8::1]:8080/v1",
    "http://127.0.0.1.example.com/v1",
    "http://user:pass@10.0.0.5:8080/v1",
    "127.0.0.1:8080",
];

const LOCAL: [&str; 6] = [
    "http://127.0.0.1:8080/v1",
    "http://[::1]:8080/v1",
    "http://127.8.9.10:8080/v1",
    "http://localhost:8080/v1",
    "HTTP://LOCALHOST/v1",
    "http://[::ffff:127.0.0.1]:8080/v1",
];

#[test]
fn a_base_url_whose_host_is_not_loopback_is_refused() {
    for url in REMOTE {
        let config = Config::new(url, SERVED_MODEL);
        let refused = binding(&config).expect_err("the binding is refused");
        assert_eq!(
            refused.code,
            ErrorCode::InvalidRequest,
            "{url}: {refused:?}"
        );
        assert!(
            refused.message.contains("loopback"),
            "{url}: the refusal says why: {}",
            refused.message
        );
        let refused = LlamaServer::new(&config).expect_err("the model is refused");
        assert_eq!(
            refused.code,
            ErrorCode::InvalidRequest,
            "{url}: {refused:?}"
        );
        assert!(loopback(url).is_err(), "{url} is refused");
    }
}

#[test]
fn a_base_url_on_loopback_is_accepted() {
    for url in LOCAL {
        assert_eq!(loopback(url), Ok(()), "{url} is loopback");
        let config = Config::new(url, SERVED_MODEL);
        binding(&config).unwrap_or_else(|refused| panic!("{url}: {refused:?}"));
        LlamaServer::new(&config).unwrap_or_else(|refused| panic!("{url}: {refused:?}"));
    }
}

#[test]
fn the_refusal_names_scheme_host_and_port_and_never_userinfo() {
    let url = "http://user:pass@10.0.0.5:8080/v1?key=secret#frag";
    let refused: NotLoopback = loopback(url).expect_err("refused");
    assert_eq!(refused.origin(), "http://10.0.0.5:8080");
    let config = Config::new(url, SERVED_MODEL);
    let texts = [
        refused.to_string(),
        format!("{refused:?}"),
        binding(&config).expect_err("refused").message,
        LlamaServer::new(&config).expect_err("refused").to_string(),
    ];
    for text in texts {
        for secret in ["user", "pass", "secret", "frag", "/v1"] {
            assert!(!text.contains(secret), "{secret:?} is not echoed: {text}");
        }
        assert!(
            text.contains("10.0.0.5:8080"),
            "the host and port are named: {text}"
        );
    }
}

#[test]
fn userinfo_whose_password_holds_a_slash_is_not_echoed() {
    let refused = loopback("http://user:pa/ss@10.0.0.5:8080/v1").expect_err("refused");
    let text = refused.to_string();
    for secret in ["user", "pa/", "ss@"] {
        assert!(!text.contains(secret), "{secret:?} is not echoed: {text}");
    }
    assert_eq!(refused.origin(), "http://(withheld)");
}

#[test]
fn a_refusal_never_names_a_loopback_host_it_did_not_read_as_the_host() {
    let refused = loopback("http://example.com\\@127.0.0.1:8080/v1").expect_err("refused");
    assert_eq!(refused.origin(), "http://(withheld)", "{refused}");
    let refused = loopback("http://127.0.0.1@example.com:8080/v1").expect_err("refused");
    assert_eq!(refused.origin(), "http://example.com:8080", "{refused}");
}
