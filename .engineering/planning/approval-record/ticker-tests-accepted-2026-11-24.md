---
format: aep.planning-md/3
id: approval-record:ticker-tests-accepted-2026-11-24
kind: approval-record
status: draft
title: The ticker-test finding accepted into the dub-voice wave (2026-11-24)
relations:
- decides: story:ticker-tests-hold-on-a-loaded-host
revision: 1
---
## Decision

On 2026-11-24 at 14:33 CET the coordinator put the question which wave fixes the two ticker tests
of story:ticker-tests-hold-on-a-loaded-host to the operator, with three options: A, accept the
story into the wave of approval-record:wave-2026-11-24-dub-voice, built by the unit that already
owns `adapters/inference-elevenlabs/tests/async_handles.rs`; B, leave it a draft; C, retry the two
tests in the runner configuration. The operator answered at 14:37 CET: "D5 = A. Accept
story:ticker-tests-hold-on-a-loaded-host into wave 2 with the unit that owns async_handles.rs.
Establish the cause before changing the bound (a larger bound without a cause is option C in
disguise)."

The coordinator ran the moves draft to proposed to active on these words.

## What it changes in the wave

- Unit U2 (`impl/synthesiser-drop-inside-runtime`) builds this story first and
  story:synthesiser-drop-inside-runtime after it, on one branch, as two commits: both edit
  `adapters/inference-elevenlabs/tests/async_handles.rs` and use the same ticker.
- The cause is established before any bound changes. A larger bound without a cause is refused.
- The pull request of approval-record:wave-2026-11-24-test-budget is judged on the runner's checks:
  its local test step stays red on these two tests until this wave has merged.

## Commits this approval authorises

The unit's commit for this story on `impl/synthesiser-drop-inside-runtime`, its merge into
`wave/2026-11-24-dub-voice`, and the store commits (this record, the move, scope, evidence).
