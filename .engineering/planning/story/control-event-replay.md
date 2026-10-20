---
format: aep.planning-md/3
id: story:control-event-replay
kind: story
status: active
title: Record a run and replay its control decisions; best-effort telemetry
relations:
- decomposes: epic:session-facade
- serves: vision:inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T08:01:30Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-20T08:01:30Z", actor: "human:timo", revision: 3}
---
## Context

Stage P4 lists "replay/telemetry" among its deliverables (`docs/design/inference.md:1161`), and
§23.3 asks that "replay reproduces orchestrator control decisions from recorded events" (line
1130). §19.2 (lines 995-1001) guarantees control-event replay, not identical model output (line
1217). Telemetry is a best-effort channel that cannot block realtime inference (line 688). The
four P4 stories so far (session API, topology switch, subgraphs, declarative definitions) cover
the epic's exit; none records a run or replays it. Found on 2026-10-20 while checking
`epic:session-facade` against its exit.

## Planned scope

- A recorder on the run controller's event path: every envelope with the merge order the recorder
  assigns (line 320), written to an in-memory or file sink the caller supplies.
- A replay driver in `inference-testkit` or `inference-runtime` that feeds a recording's inputs
  and adapter outputs back through the same pipeline and policy version, and compares the control
  decisions (commit, supersede, cancel, discard, tool authorisation) with the recorded ones.
- A telemetry sink trait on a bounded, lossy channel: a slow or absent consumer drops telemetry
  and never delays a node; drops are counted.

## Acceptance

- A test records a voice-cascade run on the testkit mocks with one barge-in and one tool call,
  replays it, and gets the same control decisions in the same order.
- A test changes the policy version and the replay reports the first diverging decision.
- A test with a telemetry consumer that never reads shows the run completes within its deadline
  and the drop counter is above zero.
