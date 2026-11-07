//! Adversarial `trybuild` cases for `#[inference::tool]`: names an application writes next to a
//! tool must compile.

#[test]
fn an_argument_named_like_its_own_tool_compiles() {
    trybuild::TestCases::new()
        .pass("tests/ui/adversary_w25_tool_macro_argument_named_like_tool.rs");
}

#[test]
fn an_argument_named_like_another_tool_compiles() {
    trybuild::TestCases::new()
        .pass("tests/ui/adversary_w25_tool_macro_argument_named_like_other_tool.rs");
}

#[test]
fn a_local_binding_named_like_a_tool_compiles() {
    trybuild::TestCases::new().pass("tests/ui/adversary_w25_tool_macro_local_named_like_tool.rs");
}
