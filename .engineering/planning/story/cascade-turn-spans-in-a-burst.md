---
format: aep.planning-md/3
id: story:cascade-turn-spans-in-a-burst
kind: story
status: active
title: A cascade answers each of two turns that arrive in a burst with its own reply
tags:
- from-finding
relations:
- serves: vision:inference
- decomposes: epic:defects-2026-11-10
scope:
- confidence: cited
  path: crates/inference-pipeline/src/nodes/agent.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/commit_gate.rs
- confidence: inferred
  path: crates/inference-testkit/tests/agent_keeping_replies.rs
- confidence: inferred
  path: crates/inference-testkit/tests/agent_turn_order.rs
- confidence: inferred
  path: crates/inference/src/recipe/run.rs
- confidence: cited
  path: crates/inference/src/recipe/topology.rs
- confidence: inferred
  path: crates/inference/tests/cascade_burst.rs
- confidence: inferred
  path: crates/inference/tests/fixtures/reference_cascade.rs
- confidence: inferred
  path: examples/local-voice-cascade/src/lib.rs
- confidence: inferred
  path: examples/local-voice-cascade/tests/adversary1_barge_in_timing.rs
- confidence: inferred
  path: website/docs/examples/local-voice-cascade.md
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:16:57Z", actor: "human:operator", revision: 6}
- {from: "proposed", to: "active", at: "2026-11-10T00:16:57Z", actor: "human:operator", revision: 7}
---
## Context

Found while the fourth SDK wave diagnosed an intermittent cascade case of `story:sdk-agent`
(2026-11-08); it predates that story.

In `voice_cascade`, when two user turns arrive in a burst (audio fed from a file, or a test that
sends two utterances at once), the second turn's committed spans can be answered by the first
turn's generation, and the second turn then gets no answer at all:

- `AgentNode`'s `turn_completed` takes every committed span already on its input edge
  (`take_input`, `crates/inference-pipeline/src/nodes/agent.rs:465`).
- The turn events reach the node one hop later than the spans, through the cascade's turn
  fan-out, so the spans of turn 2 are often already on the edge when turn 1 completes.
- Turn 2's `TurnOpened` then supersedes that generation before any audio is routed (an
  `Interrupted`), and turn 2's `TurnCompleted` finds no span and starts no generation.

Measured with 16 agents at once over the two-utterance fixture: 44 of 960 rounds ended with no
reply audio, each stuck rather than slow (passing rounds took at most 34 ms). With one utterance
per round: 0 of 960.

A committed span (`TranscriptCommitted { span_id, text }`) names no turn, so the node cannot tell
which turn a span on its edge belongs to; the STT only promises that a turn's spans come before
that turn's `TurnCompleted`. Live microphone input leaves about 500 ms between turns, so the race
bites mainly on audio that arrives faster than real time.

## Acceptance

- Two turns of the cascade fixture fed at once over `voice_cascade` with the mocks yield two
  replies, each answering its own turn's text, in 960 of 960 rounds at 16 agents at once.
- The fix chooses one of: committed spans carry their turn, or text and turn events travel on one
  ordered edge; the choice is stated in the story before code.

## Decision

Option (b) was chosen before code: text and turn events travel on one ordered edge. It was not
enough on its own. Both turns' spans can be on the text lane before the gate reads any turn event,
so the implementor added a turn mark, and the coordinator accepted it on 2026-11-10:

- `SttNode` sets the `causation_id` of every transcript envelope to the event id of that turn's
  `TurnOpened`.
- At `TurnCompleted`, `AgentNode` answers only the spans naming that opening, or naming none
  (typed text). When a turn opens over one that never completed, the agent moves the held spans
  of the old opening to the new turn.
- The input gate copies every turn event it takes on another lane onto the text lane, also in its
  drain. It holds the text lane's end until every other lane has ended.
- The turn split node is removed. `AgentNode` loses its `turns` port and `GatePayload` requires
  `Clone`.

Option (a), changing `TranscriptCommitted`, stayed rejected for the reasons the scoper gave.

Two adversary passes reviewed the change: review-result:adversary-u2-cascade-burst-pass-1 and
review-result:adversary-u2-cascade-burst-pass-2.

## Scope

Rewritten 2026-11-10 from the merged change (`git diff ff0ba010 e056c441`, 23 files).

- **Source:**
  - `crates/inference-pipeline/src/nodes/agent.rs`: turn events on the input edge, spans matched
    to their opening, re-attribution at a reopening
  - `crates/inference-pipeline/src/nodes/commit_gate.rs`: turn copies onto the text lane, the
    text lane's end held until every other lane ended
  - `crates/inference-pipeline/src/nodes/stt.rs`: `causation_id` set from `TurnOpened`
- **Wiring:**
  - `crates/inference/src/recipe/topology.rs` and `crates/inference/src/recipe/run.rs`: the turn
    split is dropped, and so is the agent's `turns` port
  - `crates/inference/src/graph.rs`: the doc example
  - `examples/local-voice-cascade/src/lib.rs`: `CascadeIds` loses `turn_split` and `llm_turns`
- **Testkit:** `crates/inference-testkit/src/llm.rs` and `crates/inference-testkit/src/stt.rs`,
  for per-recognition scripts and recorded asks.
- **Tests:**
  - `crates/inference/tests/cascade_burst.rs`: 16 agents × 60 rounds. On the base 176 of 960
    rounds went wrong; it is green on the unit.
  - `crates/inference-testkit/tests/agent_turn_order.rs`
  - the adversary files `adversary_u2_*.rs` in `inference-pipeline`, `inference-testkit` and
    `inference`
  - updates to `agent_keeping_replies.rs`, `crates/inference/tests/graph.rs`, `recipes.rs`,
    `fixtures/reference_cascade.rs` and `examples/local-voice-cascade/tests/adversary1_barge_in_timing.rs`
- **Documents:** `website/docs/examples/local-voice-cascade.md`, and the CHANGELOG Breaking and
  Fixed lines.
- **Not touched:** `crates/inference-text`, `ess/`, `crates/inference-duplex` and
  `crates/inference-pipeline/src/nodes/duplex.rs`.
- **Left out:** a late `TurnCompleted` naming an old opening, which is pre-existing, filed as
  story:agent-turn-completed-names-its-turn.
