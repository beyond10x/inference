---
format: aep.planning-md/3
id: review-result:adversary-duplex-tool-descriptions-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: duplex bindings send each tool''s description'
relations:
- reviews: story:duplex-tool-descriptions
revision: 1
---
## Adversary pass 1 on story:duplex-tool-descriptions (U2, head cfca591)

The adversary's report header, as returned:

```
unit: story:duplex-tool-descriptions, head cfca591 (findings cover cfca591; my test commit 5ab6d91 sits on top)
verdict: nothing found
cases: executed 233→241, red 0
origin: introduced 0, pre-existing 0, undecided 0
wrote-outside-worktree: 3 (logs in the wave's scratch directory)
needs-coordinator: no
```

Eight cases, all passing, committed as 5ab6d91: three tools keep their descriptions in order
(Realtime); a description with quotes, newline, backslash, accents and an emoji reaches
`session.update` unchanged; GPT-Live backend tools first, then session tools, `strict: false`
kept; building the GPT-Live configuration twice lists no tool twice; a tool without a description
is sent with no other text than its name; `declare_as` keeps it; two tools differing only in
description are unequal; `DeclareTool` and `Tool` round-trip it under `description`. Suites:
`cargo test -p inference-tools`, `cargo test -p inference-openai --features gpt-live,duplex`,
exit 0, 241 passed, 0 failed.

```findings
[
  {"file": "ess/domains/tools.yaml", "line": 93, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "The description's read-from lines cite design lines 847 and 859, which name no tool description; neighbouring fields mark such a gap UNMAPPED."},
  {"file": "ess/domains/tools.yaml", "line": 90, "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "The spec comment says an adapter sends the description as the provider's tool description, while inference-llm tool_spec (crates/inference-llm/src/tools.rs:56) sends 'name: description'."},
  {"file": "CHANGELOG.md", "line": 7, "category": "contract-drift", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "The breaking-change entry names struct literals only; saved DeclareTool JSON without 'description' from earlier versions no longer deserializes, and no in-repo caller deserializes it."}
]
```
