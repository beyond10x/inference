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
revision: 8
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
after every story was scoped by `aep:story-scoper` and its typed scope written (the last run, after
story:waiting-sends-land-in-the-order-they-started-waiting was scoped; the run before it listed that
story as unassessed):

```
wave 1
  story:audio-device-adapter
  story:cascade-turn-spans-in-a-burst (inferred)
  story:docs-build-without-warnings (inferred)
  story:duplex-turn-ids (inferred)
  story:ess-defect-fixes
  story:live-session-send-after-its-own-turn (inferred)
  story:tool-authority-schema-keywords (inferred)
  story:waiting-sends-land-in-the-order-they-started-waiting (inferred)
  story:withdrawn-ingress-send-gives-back-its-sequence
wave 2
  story:realtime-voice-demo (inferred)
  story:signed-url-token-copies (inferred)
collision: story:docs-build-without-warnings story:signed-url-token-copies adapters/inference-elevenlabs/src/lib.rs (inferred)
collision: story:duplex-turn-ids story:realtime-voice-demo examples/realtime-voice/tests/acceptance.rs
collision: story:duplex-turn-ids story:signed-url-token-copies adapters/inference-elevenlabs/src/agent.rs (inferred)
collision: story:duplex-turn-ids story:signed-url-token-copies adapters/inference-elevenlabs/src/agent_duplex.rs
2 wave(s), 4 collision(s), 0 unassessed
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
| U7 | story:waiting-sends-land-in-the-order-they-started-waiting | `crates/inference-runtime/src/queue.rs`, a new test file |

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
`/` 67 G free (floor 30 G); load 45. The shared compiler cache did not answer, so the wave's builds
use a cache of their own.

## Integration

Branch `wave/defects-2026-11-10`, managed tree `inference-wave-defects`.

## Unit state

| unit | branch | head | adversary passes | merge into the wave branch |
|---|---|---|---|---|
| U1 | impl/duplex-turn-ids | 38164e7f | 2, both fixed | e23b6130 |
| U2 | impl/cascade-turn-spans-in-a-burst | e056c441 | 2, both fixed | a6f40976 |
| U3 | impl/live-session-send-after-its-own-turn | 45748589 | 2: pass 1 no-op, pass 2 fixed | 966cdf29 |
| U4 | impl/tool-authority-schema-keywords | 9ea2127d | 2: pass 1 no-op, pass 2 fixed | 27c08621 |
| U5 | impl/docs-build-without-warnings | 7b99b079 | none (doc text and one gate line; the gate step is the check) | 50f6ff35 |
| U6 | impl/withdrawn-ingress-send-gives-back-its-sequence | fdfeacb1 | 1, fixed | bc841d7a |
| U7 | impl/waiting-sends-land-in-the-order-they-started-waiting | ae789d7a | 1, no-op (four notes) | d673c3be (first made as 436d7c96) |

Every unit tree was finished, archived and collected once its unit was merged.

## Delivered

Delivered on 2026-11-10. Seven stories moved active -> implemented:

- story:duplex-turn-ids
- story:cascade-turn-spans-in-a-burst
- story:live-session-send-after-its-own-turn
- story:tool-authority-schema-keywords
- story:docs-build-without-warnings
- story:withdrawn-ingress-send-gives-back-its-sequence
- story:waiting-sends-land-in-the-order-they-started-waiting

story:signed-url-token-copies stays active, for wave 2.

The full gate ran once, step by step, on `wave/defects-2026-11-10` at 436d7c96. Every step exited
0 except clippy:

- docs-generate (it changed no file)
- the sherpa-onnx archive
- `ess specify validate`
- `aep plan artifact validate`
- docs-check
- fmt
- the docs build with warnings denied
- `cargo test --workspace --all-features`: 3934 passed, 0 failed, 60 ignored
- the `stack` test without default features

Clippy exited 101: U4's test `tool_macro_schema_keywords.rs` never read two enum fields. 67bb0476
fixed that, and the workspace clippy then exited 0. The CHANGELOG lines are in 8f9a90d8.

Backlog drafts were filed from findings that already existed on the base. No later wave takes them
before every planned wave is delivered.

- story:tool-authority-more-schema-keywords, from review-result:adversary-u4-schema-keywords-pass-1
- story:agent-turn-completed-names-its-turn, from review-result:adversary-u2-cascade-burst-pass-2

The commits from the U7 store record on were made again. That record, as first committed, named a path of the coordinator's machine, which the PR's security check refused. The record now names the unit's scratch directory instead. The gate ran at 436d7c96. The tree of its replacement, d673c3be, differs from it only in that one line of the record. The clippy fix is now 99118a0f and the CHANGELOG commit bd2a63ca.
