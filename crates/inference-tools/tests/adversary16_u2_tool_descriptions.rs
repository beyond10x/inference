//! Adversary pass 16 on story `duplex-tool-descriptions`: `inference.tools.Tool.description` and
//! `inference.tools.DeclareTool.description` as `ess/domains/tools.yaml` declares them, kept by
//! both declaring paths and carried under their ESS name by serialization.

use inference_tools::{DeclareTool, SideEffectClass, Tool, ToolId};
use serde_json::json;

const DESCRIPTION: &str = "Books a \"table\"\nfor the day it names; caf\u{e9}.";

fn input(description: &str) -> DeclareTool {
    DeclareTool {
        name: "book".to_owned(),
        description: description.to_owned(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::IdempotentWrite,
        requires_confirmation: false,
    }
}

#[test]
fn declare_as_keeps_the_description_under_the_identity_it_is_given() {
    let (declared, event) = Tool::declare(input("any"));
    let tool_id: ToolId = event.tool_id;
    let (tool, _) = Tool::declare_as(tool_id, input(DESCRIPTION));
    assert_eq!(tool.tool_id(), tool_id);
    assert_eq!(tool.description(), DESCRIPTION);
    assert_eq!(declared.description(), "any");
}

#[test]
fn two_declarations_differing_only_in_description_are_different_tools() {
    let tool_id = Tool::declare(input("")).1.tool_id;
    let (described, _) = Tool::declare_as(tool_id, input(DESCRIPTION));
    let (bare, _) = Tool::declare_as(tool_id, input(""));
    assert_ne!(described, bare);
    assert_eq!(bare.description(), "");
}

#[test]
fn a_declare_tool_carries_its_description_under_the_ess_name() {
    let value = serde_json::to_value(input(DESCRIPTION)).expect("serializes");
    assert_eq!(value["description"], DESCRIPTION);
    let back: DeclareTool = serde_json::from_value(value).expect("deserializes");
    assert_eq!(back.description, DESCRIPTION);
    assert_eq!(back, input(DESCRIPTION));
}

#[test]
fn a_tool_serializes_its_description_under_the_ess_name() {
    let (tool, _) = Tool::declare(input(DESCRIPTION));
    let value = serde_json::to_value(&tool).expect("serializes");
    assert_eq!(value["description"], DESCRIPTION);
    assert_eq!(value["name"], "book");
    let (bare, _) = Tool::declare(input(""));
    assert_eq!(
        serde_json::to_value(&bare).expect("serializes")["description"],
        "",
        "an empty description is kept as declared, not dropped"
    );
}
