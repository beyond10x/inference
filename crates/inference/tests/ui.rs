//! Every `trybuild` case of the crate, in one `trybuild` run: one build of the crate and its
//! dependencies for all of them, instead of one per test.
//!
//! What compiles: a macro tool with and without arguments, its options, and the names an
//! application writes next to a tool (an argument named like its own tool or another one, a
//! local binding named like a tool), and both forms `Agent::tool` takes (`.tool(local_time)`,
//! `.tool(local_time())`, `docs/design/sdk.md` §1 and §6). What is refused, with the message its
//! `.stderr` snapshot holds: a tool argument that is not `Deserialize`, a `#[inference::main]`
//! without the stack, and a role method of the agent builder given a binding of another type.
//!
//! The snapshots are rustc's messages, so they hold for the rustc `rust-toolchain.toml` pins.
//! This binary is the slow lane (`task test-slow`, `.config/nextest.toml` profile `slow`); the
//! default profile leaves it out.

#[test]
fn ui() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/tool_pass_*.rs");
    cases.pass("tests/ui/adversary_w25_tool_macro_*.rs");
    cases.compile_fail("tests/ui/tool_argument_not_deserialize.rs");
    #[cfg(feature = "audio-device")]
    cases.compile_fail("tests/ui/main_without_stack.rs");
    cases.compile_fail("tests/compile_fail/*.rs");
}
