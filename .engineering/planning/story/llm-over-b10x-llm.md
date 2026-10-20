---
format: aep.planning-md/3
id: story:llm-over-b10x-llm
kind: story
status: implemented
title: inference-llm wraps the beyond10x llm Model contract
summary: inference-llm builds on llm_core::Model from b10x-llm at a release tag and adds only what a graph node needs; design §7.2 and AGENTS.md follow.
relations:
- decomposes: epic:contracts-and-testkit
- serves: vision:inference
scope:
- confidence: cited
  path: AGENTS.md
- confidence: cited
  path: crates/inference-llm
- confidence: inferred
  path: crates/inference-testkit/src/llm
- confidence: inferred
  path: crates/inference-testkit/src/llm.rs
- confidence: cited
  path: docs/design/inference.md
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T01:07:59Z", actor: "human:timo", revision: 6}
- {from: "proposed", to: "active", at: "2026-10-20T01:07:59Z", actor: "human:timo", revision: 7}
- {from: "active", to: "implemented", at: "2026-10-20T01:08:00Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

Owner decision, 2026-10-20, relayed by another session of the owner's: "I want infernence-llm use
../llm !". `inference-llm` stops defining its own text-model contract and wraps
`llm_core::Model` (`Model::turn`) and its wires (Responses, Messages, Chat Completions) from the
beyond10x `llm` repository at a release tag (0.7.0 is current). It adds only what a graph node
needs: ports, streaming events, commit points. No other change to this repository's scope.

This reverses part of the owner decision of 2026-10-18 recorded in `AGENTS.md` ("Other beyond10x
repositories (`llm`, `llm-gateway`, `loom`) are not dependencies"); the `secrets` exception of
2026-10-19 stays. `AGENTS.md` changes with this story.

## Planned scope

- `docs/design/inference.md` §7.2 (line 364, the `inference-llm` contract) rewritten to say the
  contract is `llm_core::Model` from `b10x-llm`, and what `inference-llm` adds for the graph.
- `crates/inference-llm`: depends on `b10x-llm-core` (and the wire crates only behind features) at
  the release tag; the current generation request and event types become an adaptation over
  `Model::turn`, keeping the stream semantics the spec declares (`inference.stream` generations,
  segments, commit points) and the `tools` feature's proposal and outcome types.
- The testkit mock LLM implements `llm_core::Model` (or the adapter over it), so every existing
  test, the conform run and the tool feedback loop stay green unchanged in behaviour.
- `AGENTS.md`: the dependency bullet names `llm` as allowed for `inference-llm`, with this decision
  and its date; the rust-version consequence (llm 0.7.0 needs 1.98) stated and handled.

## Acceptance

`inference-llm` builds on `b10x-llm` at a release tag and defines no model trait of its own; the
workspace gate passes with the same conform counts; the design's §7.2 and `AGENTS.md` say so.

## Out of scope

Any provider route of `llm` being qualified, live calls, the gateway; changes to STT, TTS or the
duplex contract.

## Owner decision

Confirmed 2026-10-20 (owner, relayed by another session of the owner's), option B:

- `inference-llm` is a thin wrapper over `llm_core::Model` (b10x-llm at a release tag, 0.7.0 now)
  and adds only graph concerns: ports, streaming node events, `TextSegmentCommitted` (design §7.2,
  lines 335 and 390). It defines no second text-model vocabulary.
- Rejected: A (delete `inference-llm` and let graphs call `llm` directly; the commit point would
  have to move into `llm`) and C (an own contract plus an adapter crate; two text contracts).
- The 2026-10-18 rule in `AGENTS.md` reverses for `llm` only; `llm-gateway` and `loom` stay
  non-dependencies. The workspace moves to Rust 1.98, as `llm` 0.7.0 requires.
- If `llm` lacks something the wrapper needs, it is reported as an `llm` gap, not re-created here.

## llm gaps

Gaps in `llm` 0.7.0 that the wrapper works around; each workaround goes when `llm` closes the gap.
Reported to the owner's sessions on 2026-10-20.

- llm's `Capabilities` cannot declare output streaming or input during output (llm 0.7.0
  `crates/llm-core/src/vocabulary.rs:45-58`). The `inference.model.Adapter` record's claims decide
  delivery; nothing checks them against the model.
- llm's `Model::turn` takes a fixed `TurnRequest`, so no input reaches a running turn (llm 0.7.0
  `crates/llm-core/src/port.rs:42-52`). Input admitted during output becomes user text of the next
  turn.
- llm's stream does not state that a `TextDelta` is never revised (llm 0.7.0
  `crates/llm-core/src/port.rs:10-35`). The wrapper commits each delta's segment at once.
- llm's `ToolResult` cannot tell a call rejected before running from one that failed while running
  (llm 0.7.0 `crates/llm-core/src/item.rs:25-29`). `ToolOutcome::output` encodes `{"failure": …}`
  or `{"rejected": …}` inside the output, with `failed: true` for both.
- Every llm crate is `publish = false`, so inference depends on llm by git tag.
