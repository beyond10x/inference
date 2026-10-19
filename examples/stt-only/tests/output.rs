//! The example returns each span the mock recogniser commits, one line per span, in commit order.

#[test]
fn the_example_returns_the_committed_spans() {
    assert_eq!(
        stt_only::run(),
        ["committed: hello there", "committed: how are you"]
    );
}
