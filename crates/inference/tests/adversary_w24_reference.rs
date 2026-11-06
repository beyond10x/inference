//! Adversary pass 1 on story:sdk-crate: the generated crate reference describes the SDK's macro
//! crate as what it is.
//!
//! The unit regenerated `website/docs/reference/crates.md`. Its generator classifies a package
//! with no `lib` and no `bin` target as "Examples only.", and `inference-macros` has one target of
//! kind `proc-macro`, so the published reference tells a reader that the crate behind
//! `#[inference::tool]` and `#[inference::main]` holds only examples.

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn within<T: Send + 'static>(case: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(case());
    });
    finished
        .recv_timeout(Duration::from_secs(20))
        .expect("the case finishes within 20 s without panicking")
}

#[test]
fn the_reference_does_not_call_the_macro_crate_examples_only() {
    within(|| {
        let page =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../website/docs/reference/crates.md");
        let text = std::fs::read_to_string(&page).expect("the generated crate reference exists");
        let row = text
            .lines()
            .find(|line| line.starts_with("| [`inference-macros`]"))
            .expect("the crate reference has a row for inference-macros");
        assert!(
            !row.contains("Examples only."),
            "the reference row of inference-macros, a proc-macro library every SDK user compiles, \
             says it holds examples only: {row}"
        );
    });
}
