---
format: aep.planning-md/3
id: story:duplex-turn-ids
kind: story
status: implemented
title: Duplex input events name the turn they belong to
summary: InputCommitted and InputTranscribed carry an item or turn id, so DuplexNode pairs each transcript with its own turn
relations:
- serves: vision:inference
- informed_by: review-result:adversary-w24-sdk-duplex-node-pass-1
- decomposes: epic:defects-2026-11-10
scope:
- confidence: cited
  path: adapters/inference-aws/src/duplex.rs
- confidence: cited
  path: adapters/inference-deepgram/src/agent_duplex.rs
- confidence: cited
  path: adapters/inference-deepslate/src/opal.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/src/agent.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/agent_duplex.rs
- confidence: cited
  path: adapters/inference-openai/src/duplex.rs
- confidence: cited
  path: adapters/inference-openai/src/live_duplex.rs
- confidence: cited
  path: adapters/inference-phonic/src/duplex.rs
- confidence: cited
  path: crates/inference-duplex/src/ledger.rs
- confidence: cited
  path: crates/inference-duplex/src/lib.rs
- confidence: cited
  path: crates/inference-duplex/tests/adversary1_ledger.rs
- confidence: cited
  path: crates/inference-duplex/tests/adversary2_probe.rs
- confidence: cited
  path: crates/inference-duplex/tests/ledger.rs
- confidence: cited
  path: crates/inference-pipeline/src/nodes/duplex.rs
- confidence: cited
  path: crates/inference-testkit/src/duplex.rs
- confidence: cited
  path: crates/inference-testkit/src/duplex/mock.rs
- confidence: cited
  path: crates/inference-testkit/tests/adversary_w24_duplex_basis.rs
- confidence: inferred
  path: crates/inference-testkit/tests/duplex_node.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/duplex.rs
- confidence: cited
  path: ess/domains/duplex.yaml
- confidence: inferred
  path: ess/scenarios/input-during-output-continues-response.yaml
- confidence: cited
  path: examples/realtime-voice/tests/acceptance.rs
- confidence: cited
  path: examples/realtime-voice/tests/adversary1_gpt_live_transcript.rs
- confidence: cited
  path: examples/realtime-voice/tests/adversary1_talk_over_cut_answer.rs
- confidence: cited
  path: examples/realtime-voice/tests/compare.rs
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:16:58Z", actor: "human:operator", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-10T00:16:58Z", actor: "human:operator", revision: 6}
- {from: "active", to: "implemented", at: "2026-11-10T02:46:21Z", actor: "human:operator", revision: 8, decided_on: {"recorded":{"test_result":1,"review_outcome":2,"verification":1}}}
---
## Outcome

A host running a native speech model through `DuplexNode` gets each tool call based on the
transcript of the turn the user spoke, also when a provider sends a transcript after a later turn
was committed or never sends one.

## Context

review-result:adversary-w24-sdk-duplex-node-pass-1 and its correction round (story:sdk-duplex-node):
`InputCommitted` and `InputTranscribed` in `inference-duplex` carry only the session id, so
`DuplexNode` (`crates/inference-pipeline/src/nodes/duplex.rs`) pairs a transcript with the last
committed turn still waiting for one. A transcript that arrives only after a later turn was
committed is taken as that later turn's; a queue of committed turns would misplace every transcript
after one failed transcription. The OpenAI adapter feeds the two events from separate server
events (`adapters/inference-openai/src/duplex.rs`), so the ordering is possible; it was inferred
from the code, not observed at runtime.

## Acceptance

- `ess/domains/duplex.yaml` first: `InputCommitted` and `InputTranscribed` carry the provider's
  item id (or a turn id the binding assigns), validated by `ess specify validate --path ess`.
- Every duplex binding fills it (openai, openai live, aws, elevenlabs agent, deepgram agent, phonic,
  deepslate, the testkit mock); a binding whose provider names no item states how it assigns one.
- `DuplexNode` pairs a transcript with the committed turn of the same id; a test with transcripts
  out of order and one missing transcript shows each call's basis is its own turn's span.

## Out of scope

Observing the ordering against a live provider.

## Scope

Rewritten 2026-11-10 from the merged change (`git diff ff0ba010 38164e7f`, 32 files).

- **Contract:** `crates/inference-duplex/src/lib.rs` (`InputItemId`, never empty, also when read back; the `item` field of `InputCommitted` and `InputTranscribed`) and `crates/inference-duplex/src/ledger.rs` (`assign_item`, the id taken by `commit_input`, `input_committed`, `input_transcribed`); `ess/domains/duplex.yaml`, `ess/scenarios/input-during-output-continues-response.yaml`
- **Node:** `crates/inference-pipeline/src/nodes/duplex.rs`: a span per turn id, a call resting on the turn its response answers, the latest-span basis kept only for a response that answers no committed turn, at most 64 turns kept and a response's entry dropped at its end
- **Bindings:** `adapters/inference-openai/src/duplex.rs` (an audio commit naming no item is a turn of its own), `adapters/inference-openai/src/live_duplex.rs`, `adapters/inference-aws/src/duplex.rs`, `adapters/inference-deepgram/src/agent_duplex.rs`, `adapters/inference-elevenlabs/src/agent_duplex.rs` (one line in `commit_input`, accepted although outside the scoped arm), `adapters/inference-phonic/src/duplex.rs`, `adapters/inference-deepslate/src/opal.rs` (turn ids mapped onto commits on the provider's single counter, at most 64 commits kept)
- **Testkit:** `crates/inference-testkit/src/duplex.rs`, `crates/inference-testkit/src/duplex/mock.rs`
- **Tests:** `crates/inference-testkit/tests/duplex_node.rs` (red on the base at `:1622`), the adversary files `adversary_u1_*.rs` in `inference-testkit`, `inference-duplex`, `inference-aws` and `inference-deepslate`, and the updated callers in `inference-duplex`, `inference-openai`, `inference-aws`, `inference-deepslate`, `inference-testkit/tests/target` and `examples/realtime-voice/tests`
- **Residual (documented in `opal.rs`):** the Deepslate rule assumes strictly alternating user and assistant turn ids; a skipped untranscribed turn whose neighbour shares its room can be mapped wrong
- **Reviews:** review-result:adversary-u1-duplex-turn-ids-pass-1, review-result:adversary-u1-duplex-turn-ids-pass-2
