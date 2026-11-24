//! Adversary case, pass 2, for the test-budget story: the key check `talk` calls.
//!
//! `compare::hosted_tts` calls `elevenlabs::resolves`, and `resolves` is `resolves_within` with
//! `KEY_WAIT`. The case `a_keychain_that_never_answers_does_not_hold_the_key_check_forever` of
//! `adversary2_u3_talk.rs` now drives `resolves_within` with a wait of its own and holds `KEY_WAIT`
//! by an assertion on the constant, so no test runs `resolves` over a source that does not answer:
//! a `resolves` that hands on another wait, or none, holds `talk` as before and every test passes.
//! This case drives `resolves` itself. It waits the eight seconds of `KEY_WAIT`. Nothing here
//! reaches a real provider or a keychain.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use inference_elevenlabs::{Credential, CredentialError, CredentialFuture, CredentialSource};
use realtime_voice::elevenlabs;

/// A key source shaped as the keychain's: the read runs on Tokio's blocking pool, the future
/// awaits it, and the read does not answer in any time a test waits for.
struct UnansweredKeychain;

impl CredentialSource for UnansweredKeychain {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(async {
            tokio::task::spawn_blocking(|| {
                std::thread::sleep(Duration::from_secs(600));
                Ok::<_, CredentialError>(Credential::new(b"never".to_vec()))
            })
            .await
            .map_err(|_| CredentialError::Unavailable)?
        })
    }
}

/// `resolves`, the function `talk` checks its key with, gives up a keychain that does not answer
/// at `KEY_WAIT`: not before it, and within two seconds after it.
#[test]
fn adversary2_w1_the_key_check_talk_calls_gives_up_at_key_wait() {
    let (done, finished) = mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let _ = done.send(elevenlabs::resolves(&UnansweredKeychain));
    });
    let bound = elevenlabs::KEY_WAIT + Duration::from_secs(2);
    let outcome = finished.recv_timeout(bound);
    assert!(
        outcome.is_ok(),
        "elevenlabs::resolves had not returned {:?} after it was asked, over a keychain read that \
         does not answer: talk waits before anything opens, printing nothing",
        started.elapsed()
    );
    let refused = outcome
        .expect("it returned")
        .expect_err("a keychain that does not answer gives no key");
    assert!(refused.contains("no answer within 8 s"), "{refused}");
    assert!(
        started.elapsed() >= elevenlabs::KEY_WAIT,
        "the check gave up before its wait: {:?}",
        started.elapsed()
    );
}
