---
format: aep.planning-md/3
id: story:tool-authority-schema-keywords
kind: story
status: draft
title: The tool authority admits or the tool attribute refuses every argument type schemars describes
tags:
- from-finding
relations:
- informed_by: review-result:adversary-w25-sdk-tool-macro-pass-1
revision: 1
---
## Context

The tool authority validates a tool's argument schema against a subset of JSON Schema
(`crates/inference-pipeline/src/nodes/tool_authority.rs`, `supported` and `admits`), and refuses a
tool whose schema uses any other keyword. The `#[inference::tool]` attribute generates schemas with
schemars; after the second SDK wave the subset admits what schemars emits for strings, booleans,
integers, floats, `Option` of a scalar, `Vec` and structs (subschemas inlined, `format` an
annotation, `minimum`/`maximum` validated). Argument types whose schemars output uses other keywords
are still refused when the authority prepares the tool, with the keyword named: `char`
(`minLength`/`maxLength`), tuples and fixed arrays (`prefixItems`, `minItems`, `maxItems`), enums
with data (`oneOf`), and `Option` of a struct (`anyOf`). Found by the correction round of
story:sdk-tool-macro.

## Planned scope

- `crates/inference-pipeline/src/nodes/tool_authority.rs`: validate `minLength`, `maxLength`,
  `minItems`, `maxItems`, `prefixItems`, `oneOf` and `anyOf`, or decide per keyword that the macro
  refuses the argument type at compile time instead.

## Acceptance

- A macro tool with an argument of each listed type prepares under the authority, or fails to
  compile with a message naming the type; a test per type.
