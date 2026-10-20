---
format: aep.planning-md/3
id: story:cascade-commit-gates
kind: story
status: implemented
title: 'Local voice cascade on mocks: commit gates between STT, LLM and TTS, and a sink that acknowledges playback'
relations:
- decomposes: epic:local-voice-cascade
- serves: vision:inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T08:01:30Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-20T08:01:30Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-20T13:52:15Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
## Context

Stage P2 of `docs/design/inference.md` (1160) and demonstrator step 4 (1174): connect STT, LLM
and TTS through typed pipeline nodes, explicitly committing transcripts and response segments, in
topology B (759-778). The default commit rules (624-628) say TTS consumes committed output segments
only, and only the sink confirms playback (388). The testkit cascade
(`crates/inference-testkit/src/cascade.rs`, `story:voice-cascade-on-mocks`) runs the mocks with no
commit gate and no sink node: its TTS node plays through the in-node `MockSink` double, and its
module documentation leaves the gate, the acknowledging sink and the real adapters to this epic.
The TTS contract acknowledges playback only with an `inference_tts::PlaybackReceipt` made by
consuming the `SegmentAudio` frames the sink was given (`crates/inference-tts/src/lib.rs:166-265`).
The device adapter has a `Speaker` with a played position, an immediate stop and a fake host the
test pulls (`adapters/inference-audio-device/src/playout.rs:434-550`, `src/fake.rs`), but its
`SpeakerSink` node acknowledges nothing (`src/node.rs`).

This story builds the cascade on the testkit mocks and the fake audio host, so that the gates, the
sink and the run semantics are proven before a real adapter exists. Interruption is
`story:cascade-barge-in`; the profile enforcement is `story:cascade-offline-profile`; the real
adapters, the microphone and the example run are `story:local-voice-cascade-example`.

Decisions taken on silence:

- The commit gate is one node kind of role `policy` in `inference-pipeline`
  (`src/nodes/commit_gate.rs`), as the VAD and turn-policy nodes are, reading its payload through a
  payload trait so that `inference-pipeline` gains no contract-crate dependency. It runs in one of
  two placements:
  - input gate, between STT and LLM: passes committed spans (`TranscriptCommitted`) and the turn
    events, holds back hypotheses, and refuses a revision naming a span it already passed as
    Committed: the revision is counted on the gate handle and never forwarded (the `wrong-state`
    outcome of `inference.stream.ReviseSpan`);
  - output gate, between LLM and TTS: passes a segment only once its `TextSegmentCommitted`
    arrived; a tentative `TextDelta` is held and never reaches the TTS node, so `QueueForSynthesis`
    is never asked of a Tentative segment; held segments of a generation that ends without
    committing them are dropped and counted.
  The LLM node keeps committing its segments as it does today; the gate enforces the commit and
  does not decide it. A gate that commits by a policy of its own (sentence boundaries) is later
  work.
- The acknowledging sink is a node of role `sink` in `inference-audio-device`, behind feature
  `pipeline`, which gains `inference-tts` as an optional dependency. It takes each segment frames
  as `SegmentAudio`, pushes them to a `Speaker`, and once the played position passes the last frame
  of a Synthesized segment makes the `PlaybackReceipt` from those frames and sends it on its
  `played` output. The TTS node takes receipts on a new `played` input over a declared feedback
  edge and acknowledges through its synthesis handle (`inference.stream.AcknowledgePlayback`). Its
  in-node `playing` mode stays for the tests that use it.
- `SegmentAudio` and `PlaybackReceipt` are not `Clone`, and the testkit cascade payload is. The
  speech path from TTS to sink and the receipt edge back carry no fan-out. How the payload of this
  cascade carries them is the implementor to prove first; a receipt is never made from copies.
- The cascade wiring lives in a new example package, `examples/local-voice-cascade` (the name the
  design layout gives, line 220), as `examples/realtime-voice` holds its own: a library generic
  over the audio host, tested with the testkit mocks and the fake host. The testkit cascade, which
  `story:session-topology-switch` uses, is not changed.
- The measure "committed input to first playable audio" (1022) is recorded per turn by the wiring:
  wall-clock time from the input gate passing the turn completion to the sink handing the first
  frame of that turn to the speaker, read through a handle. No metrics crate; a value recorded,
  not a target asserted.

## Planned scope

- `crates/inference-pipeline/src/nodes/commit_gate.rs` (new) with its `mod` line in
  `src/nodes/mod.rs` and its descriptor in `src/nodes/descriptors.rs`; unit tests beside it,
  including a revision of a passed span and a tentative delta injected directly.
- `adapters/inference-audio-device`: the acknowledging sink (new module under `src/`, behind
  `pipeline`), `Cargo.toml` optional `inference-tts`; tests against the fake host in `tests/`.
- `crates/inference-testkit/src/tts/node.rs`: the `played` input port and the acknowledgement from
  receipts.
- `examples/local-voice-cascade` (new package, `publish = false`, no binary target): `src/lib.rs`
  declares topology B with the typed builder (A-03, 1139) under `offline_strict` with egress
  `denied`: audio ingress, fan-out to VAD and STT, turn policy, STT, input gate, LLM, output gate,
  TTS, acknowledging sink and the receipt feedback edge, with egresses for the transcript and the
  assistant text; `tests/gates.rs` drives it on the mocks and the fake host.
- Section 23.3 test 3 (1126): each fan-out of the cascade delivers to every output in the order it
  received. Test 7 (1130): after `stop` the run reaches Completed, every node instance is Stopped,
  every queue is empty and dropped, and the speaker is closed (`FakeHost::output_open` is false).

## Domain relations

- Pipeline owns Node and Edge, many, and references DeploymentProfile, one:
  `ess/domains/graph.yaml`, `inference.graph.Pipeline`, relations `nodes`, `edges`, `profile`.
- FeedbackLoop references Pipeline and Edge, one each (the receipt edge from sink to TTS):
  `ess/domains/graph.yaml`, `inference.graph.FeedbackLoop`, relations `pipeline`, `edge`.
- Span references Turn and Run, one each: `ess/domains/stream.yaml`, `inference.stream.Span`,
  relations `turn`, `run`.
- Generation owns OutputSegment, many, and references its producing Node, one:
  `ess/domains/stream.yaml`, `inference.stream.Generation`, relations `segments`, `producer`.
- Run owns NodeInstance, many; NodeInstance references Node, one: `ess/domains/graph.yaml`,
  `inference.graph.Run` relation `instances`, `inference.graph.NodeInstance` relation `node`.

No noun is added and `ess/` does not change.

## Acceptance

`cargo test -p local-voice-cascade` runs the cascade on the testkit mocks and the fake audio host
with no network and shows the scripted answer segments reaching Played only through receipts of
the acknowledging sink
(`inference.stream.OutputSegment/transition/play/by/inference.stream.AcknowledgePlayback/played`),
a revision of a committed span stopped at the input gate
(`inference.stream.Span/state/Committed/refuses/inference.stream.ReviseSpan`), a tentative delta
never queued at the output gate
(`inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.QueueForSynthesis`),
fan-out order kept, the run Completed after `stop` with every node instance Stopped, every queue
dropped and the speaker closed, and the committed-input-to-first-playable-audio measure recorded
for the turn.

## Out of scope

Interruption and barge-in (`story:cascade-barge-in`); planning and run-time enforcement of the
profile (`story:cascade-offline-profile`); the real adapters, the microphone and the example run
(`story:local-voice-cascade-example`); speculative TTS (627); tools in this cascade
(`story:cascade-read-only-tool` covers them on the testkit cascade).
