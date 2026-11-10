---
format: aep.planning-md/3
id: approval-record:wave-defects-2026-11-10
kind: approval-record
status: draft
title: 'Wave approved with no operator present: eight defect stories, one pull request (2026-11-10)'
tags:
- non-interactive
relations:
- decides: story:cascade-turn-spans-in-a-burst
- decides: story:docs-build-without-warnings
- decides: story:duplex-turn-ids
- decides: story:live-session-send-after-its-own-turn
- decides: story:signed-url-token-copies
- decides: story:tool-authority-schema-keywords
- decides: story:waiting-sends-land-in-the-order-they-started-waiting
- decides: story:withdrawn-ingress-send-gives-back-its-sequence
revision: 2
---
## Decision

Non-interactive. The operator gave the instruction below on 2026-11-10 and is not present during
the run; a coordinator session runs this wave from it. The instruction is the approval of this
wave and the acceptance of the defect stories it holds:

> "do work on llm, inference, connectors - start 3 opus 5.5 /aep:implementing agents - each
> processing a whole wave of up to 10 issues (the more the better) - run no expensive gates in the
> middle, only at the end of the wave"

The run's goal brief adds, for this repository, one wave of up to 10 stories under one epic
(epic:defects-2026-11-10), defect stories first, then the stories the store shows as accepted, as
many as the collision check allows; one pull request through the bot route at the end, merged; no
release and no tag.

## Stop bypassed

The stage-1 wave proposal (aep:implementing 0.26.0, wave mode). What the operator would have been
asked: approve this wave, its units, the split below and the commits it authorises. What was
decided in their absence: the wave runs as written here. The eight defect stories moved draft,
proposed, active on the operator's word; three of them first gained `serves vision:inference`,
which the move to proposed requires.

## Wave plan

Derived from `aep plan artifact waves --kind story --status active` (aep 0.79.0) on 2026-11-10,
after every story was scoped by `aep:story-scoper` and its typed scope written:

```
wave 1
  story:audio-device-adapter
  story:cascade-turn-spans-in-a-burst (inferred)
  story:docs-build-without-warnings (inferred)
  story:duplex-turn-ids (inferred)
  story:ess-defect-fixes
  story:live-session-send-after-its-own-turn (inferred)
  story:tool-authority-schema-keywords (inferred)
  story:withdrawn-ingress-send-gives-back-its-sequence
wave 2
  story:realtime-voice-demo (inferred)
  story:signed-url-token-copies (inferred)
collision: story:docs-build-without-warnings story:signed-url-token-copies adapters/inference-elevenlabs/src/lib.rs (inferred)
collision: story:duplex-turn-ids story:realtime-voice-demo examples/realtime-voice/tests/acceptance.rs
collision: story:duplex-turn-ids story:signed-url-token-copies adapters/inference-elevenlabs/src/agent.rs (inferred)
collision: story:duplex-turn-ids story:signed-url-token-copies adapters/inference-elevenlabs/src/agent_duplex.rs
unassessed: story:waiting-sends-land-in-the-order-they-started-waiting
```

This wave is wave 1 of that plan, less the stories that cannot run now. story:signed-url-token-copies
collides with two stories of wave 1 and waits for wave 2 with story:realtime-voice-demo.

## Units

| unit | story | surface (typed scope) |
|---|---|---|
| U1 | story:duplex-turn-ids | `ess/domains/duplex.yaml`, `crates/inference-duplex`, `crates/inference-pipeline/src/nodes/duplex.rs`, the seven duplex bindings, the testkit duplex mock and target |
| U2 | story:cascade-turn-spans-in-a-burst | `crates/inference-pipeline/src/nodes/{agent,commit_gate}.rs`, `crates/inference/src/recipe/{run,topology}.rs`, the local-voice-cascade example |
| U3 | story:live-session-send-after-its-own-turn | `crates/inference-facade/src/live.rs`, a new facade test |
| U4 | story:tool-authority-schema-keywords | `crates/inference-pipeline/src/nodes/tool_authority.rs`, a new test in `crates/inference/tests` |
| U5 | story:docs-build-without-warnings | `Taskfile.yml`, crate-root docs of `inference` and four adapters |
| U6 | story:withdrawn-ingress-send-gives-back-its-sequence | `crates/inference-runtime/src/drive.rs`, two adversary cases from `f54cb70` |
| U7 | story:waiting-sends-land-in-the-order-they-started-waiting | `crates/inference-runtime/src/queue.rs` (joins once scoped and disjoint) |

All scopes rest on `story-scoper` readings; which lines are cited and which inferred is in each
story's `## Scope`. `CHANGELOG.md` is the coordinator's: no unit edits it; each reports its line
and the coordinator writes them once on the integration branch. Generated reference pages are
regenerated once there too.

## Left out of the wave

- story:audio-device-adapter, story:realtime-voice-demo: property 1, implementable in this tree;
  what remains of each is the operator's run on a laptop with a microphone and a speaker.
- story:ess-defect-fixes: blocked by dependency-blocker:ess-affects-witness (open).
- story:signed-url-token-copies: collides (above); wave 2.
- story:sdk-usage-cost: not a defect, not accepted, and `depends_on` story:sdk-one-call, a draft
  whose handles are stubs on `main`.

## What each story needs to close

`aep plan artifact lifecycle story`: active -> implemented declares no evidence requirement in this
store's lifecycle; each story still gets a `test_result` record for its red and green runs and a
`verification` record before its move, against the merge commit.

## Dispatch

`aep:story-scoper` (opus) per story, done. `aep:implementor` (sonnet) per unit in its own managed
worktree, at most four sub-agents at once and at most three cargo builds at once while the load is
over 60. `aep:adversary` (opus) per green unit, two passes at most. No sub-agent on any other model.

## Gates

Package-scoped per unit (its red and green cases and its package's tests). The full repository gate
(`task check`, step by step) runs once on `wave/defects-2026-11-10` after every unit is merged, and
CI runs once on the one pull request.

## Commits authorised

One or more commits per unit on `impl/<story-slug>`, their merges into `wave/defects-2026-11-10`,
the coordinator's store, `CHANGELOG.md` and regenerated-docs commits on that branch, one pull request
to `main` through the bot route, and its merge once CI is green. No release, no tag, no version bump.

## Pre-flight

Main checkout clean on `main` at `af93222a`; no managed tree of this repository before this wave;
`/` 67 G free (floor 30 G); load 45. The host's default sccache server (port 4226) does not answer,
so every build of this wave uses its own server (port 4321, `~/.cache/sccache/inference`).

## Integration

Branch `wave/defects-2026-11-10`, managed tree `inference-wave-defects`.

## Unit state

| unit | tree id | branch | head | stage |
|---|---|---|---|---|
| U1 | inference-u1-duplex-turn-ids | impl/duplex-turn-ids | | planned |
| U2 | inference-u2-cascade-turn-spans | impl/cascade-turn-spans-in-a-burst | | planned |
| U3 | inference-u3-live-session-send | impl/live-session-send-after-its-own-turn | | planned |
| U4 | inference-u4-tool-authority | impl/tool-authority-schema-keywords | | planned |
| U5 | inference-u5-docs-build | impl/docs-build-without-warnings | | planned |
| U6 | inference-u6-withdrawn-ingress | impl/withdrawn-ingress-send-gives-back-its-sequence | | planned |
| U7 | inference-u7-waiting-sends | impl/waiting-sends-land-in-the-order-they-started-waiting | | planned |

Each tree builds into its own `target/`; each unit's scratch is a directory of the coordinator's
session scratch named after the unit.
