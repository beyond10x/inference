//! `tools::tool_spec`: a declared tool as the `llm` model is offered it. Its name slot carries the
//! tool's identity, so the description carries the tool's name and its declared description.
#![cfg(feature = "tools")]

use inference_llm::tools::{tool_name, tool_spec};
use inference_tools::{DeclareTool, SideEffectClass, Tool};
use serde_json::json;

fn declared(description: &str) -> Tool {
    Tool::declare(DeclareTool {
        name: "weather".to_owned(),
        description: description.to_owned(),
        schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

#[test]
fn llm_tool_spec_sends_the_tool_description() {
    let described = declared("The current weather in the city named.");
    let spec = tool_spec(&described);
    assert_eq!(spec.name, tool_name(described.tool_id()));
    assert_eq!(
        spec.description,
        "weather: The current weather in the city named."
    );
    assert_eq!(spec.input_schema, *described.schema());

    assert_eq!(
        tool_spec(&declared("")).description,
        "weather",
        "a tool declared without a description is described by its name"
    );
}
