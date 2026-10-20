---
format: aep.planning-md/3
id: story:cascade-barge-in
kind: story
status: implemented
title: 'Barge-in in the local voice cascade: supersede the generation, discard unplayed audio, refuse stale frames at the sink'
relations:
- decomposes: epic:local-voice-cascade
- serves: vision:inference
- depends_on: story:cascade-commit-gates
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T08:43:26Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-20T08:43:26Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-20T13:52:19Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
## Context

Interruption and barge-in, section 11.4 (`docs/design/inference.md:632-641`); A-10 (1146):
interruption stops unplayed speech and suppresses stale generations; section 23.3 test 5 (1128):
cancelled generations cannot produce newly played stale audio. The cascade of
`story:cascade-commit-gates` plays one answer per turn and nothing stops it when the user speaks
again. The testkit model node runs one generation per run and is superseded only through its
handle (`crates/inference-testkit/src/llm/node.rs`, module documentation); the TTS node discards
nothing on a supersede; the acknowledging sink stops nothing.

`inference.stream.SupersedeGeneration` (`ess/domains/stream.yaml`) is steps 1, 2 and 4 in one
outcome: the generation moves Active to Superseded and every segment of it not yet Played moves
to Discarded. A Generation leaves Active only once, so a generation that completed while its audio
still plays is no longer superseded; its unplayed segments are discarded one by one
(`inference.stream.DiscardSegment`, from Tentative, Committed, Queued or Synthesized). The authored
scenario `supersede-discards-unplayed-segments` (`ess/scenarios/supersede-discards-unplayed.yaml`)
already covers the cross-row discard that synthesis cannot arrange and stays green.

Decisions taken on silence:

- The trigger is the `TurnOpened` of a new turn from the turn policy while output of an earlier
  turn is not all Played. The turn events reach the LLM node, as today, and also the output gate,
  the TTS node and the sink.
- The LLM node, at that `TurnOpened`, runs `SupersedeGeneration` on its Active generation, sends
  `GenerationSuperseded` on, drops its generation handle so the backend stops computing, and
  reports the cancellation of computation apart from the cancellation of delivery (641). It starts
  a new generation at the `TurnCompleted` of the new turn. With a turns port wired it runs one
  generation per turn; without one (the text pipeline, the tool-loop tests) it keeps one
  generation per run.
- The output gate drops what it holds of an older generation. The TTS node discards every segment
  of an older generation not yet Played and offers none of them again. The sink stops the speaker
  at once (queue cleared, played position read) and from then on drops and counts every frame of
  an older generation that reaches it, and makes no receipt for it (step 4).
- Step 3, "signal cancellation to affected node instances", is the `GenerationSuperseded` event
  delivered to every node downstream of the producer plus the dropped generation handle. It is not
  the `cancel` transition of `inference.graph.NodeInstance`, which would end the instance for the
  rest of the run. Step 5 has no subject here (no tools in this cascade); step 6 is the existing
  turn policy admitting the new turn.
- The measure "interruption to playback stop" (1023) is recorded per interruption: wall-clock time
  from the sink taking the `TurnOpened` to the speaker stop returning, read through the handle that
  carries the first-audio measure of `story:cascade-commit-gates`. Recorded, not a target.

## Planned scope

- `crates/inference-testkit/src/llm/node.rs`: the supersede at a new turn and one generation per
  turn when turns are wired; its module documentation.
- `crates/inference-testkit/src/tts/node.rs`: discard on a supersede or on the trigger.
- `crates/inference-pipeline/src/nodes/commit_gate.rs`: drop held segments of an older generation.
- `adapters/inference-audio-device`: the acknowledging sink stops at the trigger and refuses stale
  frames.
- `examples/local-voice-cascade`: the turn events routed to the gate, TTS and sink;
  `tests/barge_in.rs` (new).
- Would collide with any unit editing the testkit model node or TTS node; the tool-loop tests
  (`crates/inference-runtime/tests/tool_feedback_loop.rs`, `tool_call_interruption.rs`,
  `cascade_read_only_tool.rs`) and the testkit cascade tests must pass unchanged.

## Domain relations

- Generation owns OutputSegment, many, and references its producing Node, one:
  `ess/domains/stream.yaml`, `inference.stream.Generation`, relations `segments`, `producer`; the
  discard reaches the segments through `segments`, and the downstream nodes are found from
  `producer`.
- Span references Turn and Run, one each: `ess/domains/stream.yaml`, `inference.stream.Span`,
  relations `turn`, `run`; the new generation answers the spans of the new turn.
- Run owns NodeInstance, many; NodeInstance references Node, one: `ess/domains/graph.yaml`,
  `inference.graph.Run` relation `instances`, `inference.graph.NodeInstance` relation `node`; no
  instance changes state on an interruption.

No noun is added and `ess/` does not change.

## Acceptance

`cargo test -p local-voice-cascade --test barge_in`, on the testkit mocks and the fake audio host,
feeds a second utterance while the first answer plays and shows the speaker stopped, the first
generation Superseded (`inference.stream.SupersedeGeneration/outcome/superseded`) with its
unplayed segments Discarded (`inference.stream.DiscardSegment/outcome/discarded`) and none Played
after the stop, frames the mock synthesiser is scripted to emit late for it dropped at the sink
with the played position unchanged, the second answer played and acknowledged, and the
interruption-to-playback-stop measure recorded, while `cargo test --workspace --all-features`
still passes.

## Out of scope

Barge-in with the real adapters (`story:local-voice-cascade-example`); tool audit and side-effect
reconciliation on interruption (`story:tool-call-interruption`); echo cancellation; the session
interrupt (`story:session-interrupt-and-cancel`).
