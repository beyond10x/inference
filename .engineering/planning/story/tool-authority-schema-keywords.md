---
format: aep.planning-md/3
id: story:tool-authority-schema-keywords
kind: story
status: active
title: The tool authority admits or the tool attribute refuses every argument type schemars describes
tags:
- from-finding
relations:
- informed_by: review-result:adversary-w25-sdk-tool-macro-pass-1
- decomposes: epic:defects-2026-11-10
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-pipeline/src/nodes/tool_authority.rs
- confidence: inferred
  path: crates/inference/tests/tool_macro_schema_keywords.rs
- confidence: inferred
  path: docs/design/sdk.md
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:17:28Z", actor: "human:timo", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-10T00:17:28Z", actor: "human:timo", revision: 6}
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

## Scope

Derived 2026-11-10 by `story-scoper` against `main` at `af93222a`. Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-pipeline/src/nodes/tool_authority.rs`, functions `supported` (942-985) and `admits` (1043-1101) — cited
- **Files:** `crates/inference-pipeline/src/nodes/tool_authority.rs:942`, `:1043`, and its `mod tests` (`:1103`; the assertion at `:1262-1265` expects `oneOf` to be refused and has to change) — cited
- **Symbols:** `supported`, `admits`, `TYPES`, `equal` (reused for `const`) — cited
- **Tests (new):** `crates/inference/tests/tool_macro_schema_keywords.rs`, one macro tool per listed type checked through `ToolAuthority::prepare`, written like `prepared` at `crates/inference/tests/adversary_w25_tool_macro.rs:115` — inferred
- **Also likely:** `CHANGELOG.md` `## [Unreleased]` and `docs/design/sdk.md:395` (the schema sentence) — inferred
- **Not touched:** `crates/inference-macros/src/lib.rs`; it refuses nothing at compile time because every keyword is validated in the authority — inferred
- **Documents:** `CHANGELOG.md`, maybe `docs/design/sdk.md`; no website page describes the subset — cited
- **Red:** `cargo test -p inference --test tool_macro_schema_keywords` (no `required-features`) — cited
- **Green:** that command, `cargo test -p inference-pipeline --lib nodes::tool_authority::tests`, and `cargo test -p inference --test adversary_w25_tool_macro_pass2` (the recursive `$ref` stays refused) — cited
- **Confidence:** high
- **Would collide with:** any unit touching `supported`/`admits` or the `tool_authority.rs` test module, or `CHANGELOG.md` Unreleased — inferred
- **Safety fact:** `supported` and `admits` are private; their only caller is `prepare` (`:841-850`) and the execution path in the same file; `$ref`/`$defs` stay refused — unproven

### Decision per keyword (schemars 1.2.3, from `Cargo.lock`)

| Type | Keywords schemars emits | Decision |
|---|---|---|
| `char` | `minLength: 1`, `maxLength: 1` (`primitives.rs:131-135`) | validate in the authority, counting code points (`str::chars().count()`) |
| tuples | `prefixItems`, `minItems`, `maxItems` (`tuple.rs:27-34`) | validate in the authority |
| fixed arrays `[T; N]` | `items`, `minItems`, `maxItems` (`array.rs:39-47`); `[T; 0]` only `maxItems: 0` | validate in the authority |
| enums with data | `oneOf` of object subschemas; tagged enums also emit `const` | validate `oneOf` (exactly one subschema admits) and add `const` |
| `Option` of a struct | by reading, `type: ["object","null"]`, not `anyOf`; `anyOf` comes from `Option` of a data enum and from untagged enums | validate `anyOf` (any subschema admits); the red case is `Option` of a data enum |

Defect confirmation on `main` at `af93222a` (by reading):
1. `supported` lists only `type`, `properties`, `required`, `additionalProperties`, `items`, `enum`, `minimum`, `maximum`, annotations and `default` (`tool_authority.rs:949-977`); every other keyword reaches `other => return Err(...)` at `:978`.
2. `prepare` turns that into the `NodeError` "uses the keyword ..., which the tool authority does not validate" (`:841-850`), refusing `char`, tuples, fixed arrays, data enums and `Option` of a data enum.
3. `admits` checks none of these keywords (`:1043-1101`), so `supported` must not admit one until `admits` validates it.

Could not establish: whether `Option` of a plain struct is already admitted (likely, so the story's `anyOf` example is probably wrong); whether `oneOf: []` should be refused as malformed.
