---
format: aep.planning-md/3
id: story:tool-authority-more-schema-keywords
kind: story
status: draft
title: The tool authority admits or the tool attribute refuses sets, non-zero integers, flattened structs, durations and patterned strings
tags:
- from-finding
relations:
- informed_by: review-result:adversary-u4-schema-keywords-pass-1
revision: 1
---
## Context

Found by review-result:adversary-u4-schema-keywords-pass-1 (2026-11-10), pre-existing: after
story:tool-authority-schema-keywords the tool authority validates the keywords schemars emits for
`char`, tuples, fixed arrays and enums with data, but other common argument types are still
refused when the authority prepares the tool, with no compile error from `#[inference::tool]`. By
the adversary's reading of schemars 1.2.3 (not run): `HashSet`/`BTreeSet` (`uniqueItems`),
`NonZero*` (`not`), `#[serde(flatten)]` (`allOf`, `unevaluatedProperties`), chrono durations
(`exclusiveMaximum`), `Decimal` and semver (`pattern`). The refusal is in `supported`
(`crates/inference-pipeline/src/nodes/tool_authority.rs`, the `other =>` arm).

## Acceptance

- A macro tool with an argument of each listed type prepares under the authority and has its
  calls checked, or fails to compile with a message naming the type; a test per type.
