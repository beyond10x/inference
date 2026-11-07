//! Adversarial `trybuild` case for the correction of `#[inference::tool]`: the shape
//! `Agent::tool` is documented to take.

/// `docs/design/sdk.md` §1 and §6 write `.tool(local_time)`, and `Agent::tool` takes
/// `impl IntoTool`: both the function the macro emits and the pair it returns convert.
#[test]
fn a_tool_converts_into_the_pair_agent_tool_takes() {
    trybuild::TestCases::new().pass("tests/ui/adversary_w25_tool_macro_pass2_into_pair.rs");
}
