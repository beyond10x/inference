//! Adversarial `trybuild` case for the correction of `#[inference::tool]`: the shape
//! `Agent::tool` is documented to take.

/// `docs/design/sdk.md` §1 and §6 write `.tool(local_time)`, and story:sdk-agent plans `tool` as
/// taking `impl Into<(Tool, Box<dyn ToolExecutor>)>`. A tool that is a function converts into
/// nothing.
#[test]
fn a_tool_converts_into_the_pair_agent_tool_takes() {
    trybuild::TestCases::new().pass("tests/ui/adversary_w25_tool_macro_pass2_into_pair.rs");
}
