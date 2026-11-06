//! Adversary pass 1 on `story:llama-server-loopback-only` (finding W7): base URLs on which the
//! check and the WHATWG parser `llm` reads the base URL with could disagree about the host. Each
//! URL's host, as that parser reads it, is not loopback, so `loopback` refuses it. The `?` and `#`
//! cases kill the mutant that drops them from the characters ending the authority, which the
//! unit's own suite does not.

use inference_llama_server::loopback;

const NOT_LOOPBACK_HOST: [&str; 14] = [
    "http://example.com?@127.0.0.1:8080/v1",
    "http://example.com#@127.0.0.1:8080/v1",
    "http://example.com?@localhost/v1",
    "http://example.com#@[::1]:8080/v1",
    "http://localhost.example.com/v1",
    "http://localhost@example.com/v1",
    "http://[::1]@example.com/v1",
    "http://[::ffff:10.0.0.5]:8080/v1",
    "http://[::127.0.0.1]:8080/v1",
    "http://[::1]example.com/v1",
    "http://local\thost:8080/v1",
    "http://%6cocalhost.example.com/v1",
    "http://0.0.0.0:8080/v1",
    "http:///example.com/v1",
];

#[test]
fn a_host_the_url_parser_reads_as_not_loopback_is_refused() {
    for url in NOT_LOOPBACK_HOST {
        assert!(loopback(url).is_err(), "{url:?} is refused");
    }
}

#[test]
fn the_refusal_withholds_userinfo_cut_by_a_query_or_fragment_character() {
    for url in [
        "http://user:pa?ss@10.0.0.5:8080/v1",
        "http://user:pa#ss@10.0.0.5:8080/v1",
        "http:/user:pass@10.0.0.5:8080/v1",
    ] {
        let refused = loopback(url).expect_err("refused");
        let text = format!("{refused} {refused:?}");
        for secret in ["user", "pa", "ss@"] {
            assert!(!text.contains(secret), "{secret:?} is not echoed: {text}");
        }
    }
}
