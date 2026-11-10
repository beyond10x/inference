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
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:16:57Z", actor: "human:timo", revision: 6}
- {from: "proposed", to: "active", at: "2026-11-10T00:16:57Z", actor: "human:timo", revision: 7}
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

Option (b), text and turn events on one ordered edge (decided by the coordinator on 2026-11-10 from
the scoper's reading, before code). Option (a) changes the `TranscriptCommitted` contract
(`crates/inference-text/src/inbound.rs:121`), its wire form, `ess/domains/stream.yaml:1022` and 46
files that build it, and couples to story:duplex-turn-ids through `duplex.rs:709-719`. Under (b) the
input gate also passes each turn event (not `stop`) on its `output`, the agent's `user_input` edge;
`AgentNode` takes `TurnOpened` and `TurnCompleted` from that edge and loses its `turns` port, and
`voice_cascade` drops the turn split's `llm` output. The gate already drains the other lane's spans
before it passes a turn event (`commit_gate.rs:583-596`), and the STT sends a turn's spans before
its `TurnCompleted` (`stt.rs:565-574`). Removing only the turn-split hop does not fix it: the agent
polls turns before input (`agent.rs:320-325`). Keeping an unread `turns` port would be a shim
(`AGENTS.md`, backwards compatibility not kept).

## Scope

Derived 2026-11-10 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-pipeline/src/nodes/agent.rs` — cited, the story names it
- **Files:** `crates/inference-pipeline/src/nodes/agent.rs:465` (`take_input`), `:534` (`turn_completed`), `:320` (`step`), `:153` (`AgentNode::new`) — cited
- **Files:** `crates/inference/src/recipe/topology.rs:130`, `:143`, `:157` (agent `turns` port, turn split `llm` output, their edge) — cited
- **Files:** `crates/inference-pipeline/src/nodes/commit_gate.rs:570`, `:583` (`take_held`, `exchange`: pass turn content, not `stop`, also on the text lane's output) — inferred
- **Files:** `crates/inference/src/recipe/run.rs:273`, `:344` (the `AgentNode::new` call; the input gate's construction) — inferred
- **Files:** `examples/local-voice-cascade/src/lib.rs:146`, `:348`, `:361`, `:376`, `:421`, `:653` (the reference cascade's `declare` and its `AgentNode`) — inferred
- **Files:** `crates/inference/tests/fixtures/reference_cascade.rs:77`, `:90`, `:105` (compared with `Recipe::voice_cascade()` at `crates/inference/tests/recipes.rs:215`) — inferred
- **Files:** `crates/inference-testkit/tests/agent_keeping_replies.rs:197` (turn events move to the input edge) — inferred
- **Symbols:** `AgentNode::new`, `Exchanging::take_input`, `Exchanging::turn_completed`, `CommitGate::new` — cited
- **Test, red and green:** new `crates/inference/tests/cascade_burst.rs` behind `#![cfg(feature = "testing")]`: 16 agents at once × 60 rounds over `voice_cascade` with the mocks, two generations each over its own turn's text; `cargo test -p inference --features testing --test cascade_burst`; red on main only by rate (story: 44 of 960) — inferred
- **Test, deterministic:** new `crates/inference-testkit/tests/agent_turn_order.rs` on the `agent_keeping_replies.rs` harness, with `TurnOpened`, span, `TurnCompleted`, `TurnOpened`, span, `TurnCompleted` queued on the input edge before start; `cargo test -p inference-testkit --features run,stt,llm,tts --test agent_turn_order` — inferred
- **Also likely:** `examples/local-voice-cascade/tests/adversary1_barge_in_timing.rs:451` — inferred
- **Not touched:** `crates/inference-text`, `ess/`, `crates/inference-duplex`, `crates/inference-pipeline/src/nodes/duplex.rs` — inferred
- **Documents:** `website/docs/examples/local-voice-cascade.md:21-22`; a `CHANGELOG.md` Breaking line (written by the coordinator) — inferred
- **Confidence:** medium — the defect site and the wiring are read from the tree; the gate-copy mechanism and the file set beyond `agent.rs` and `topology.rs` are the scoper's design
- **Would collide with:** any unit touching `inference-pipeline`'s agent or commit-gate nodes, `crates/inference/src/recipe/{run,topology}.rs`, the `local-voice-cascade` example's build, the recipes fixture
- **Safety fact:** a turn event copied onto the gate's `output` lands right behind its own turn's spans (`commit_gate.rs:583-596`, `stt.rs:565-574`) — unproven

Not established by the scoper: whether `MockRecogniser` can be scripted per recognition (the named `mock` STT runs the same script every turn, `crates/inference-testkit/src/stt.rs:157-178`); that "two replies" needs `auto_interrupt` off or a count of generations (`run.rs:282-287`); that the gate must not rotate `passed`/`previous` twice when it copies `TurnCompleted` (`commit_gate.rs:301-304`); whether the turn split, left with one output, is removed; whether the doc example in `crates/inference/src/graph.rs:84-135` mirrors the topology.

Defect confirmation on `main` at `af93222a` (by reading):
1. `agent.rs:534-541` `turn_completed` calls `take_input` (`:465-473`), which takes everything on the input edge without a boundary; `received` (`:483-485`) keeps every span's text and no turn, because `TranscriptCommitted` is only `{ span_id, text }` (`inbound.rs:121-124`).
2. Turn events reach the agent through `gate_turns_out → turn split → llm_turns` (`topology.rs:156-157`), spans straight from `gate_out` to `user_input` (`topology.rs:155`), so turn 2's spans can already be held when turn 1 completes.
3. `TurnOpened` of turn 2 then supersedes that generation (`agent.rs:522-530`); `TurnCompleted` of turn 2 finds `held` empty and starts nothing (`agent.rs:539-541`).
