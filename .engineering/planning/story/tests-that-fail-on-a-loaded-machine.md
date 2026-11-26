---
format: aep.planning-md/3
id: story:tests-that-fail-on-a-loaded-machine
kind: story
status: active
title: Three tests of two adapters hold on a loaded machine or state the condition they need
relations:
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T04:28:11Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T04:28:12Z", actor: "human:operator", revision: 3}
---
## Context

On 2026-11-25 `task test-run` ran twice on a workstation whose 1-minute load average was between
95 and 122 on 20 CPUs, at a tree that changes no file under `adapters/`. Both runs ended at the
step's bound of 120 s before a third of the tests had run (1146 and 765 of 3175), and in both the
same kinds of test failed before the bound:

- `inference-cartesia::adversary_w2_drop_backed_up
  a_drop_whose_close_cannot_be_written_holds_the_runtime_for_the_grace_and_no_longer`, both runs.
  First run: "inside a runtime the drop waits at most `CLOSE_GRACE`, 5ms, and held its thread
  20.421046ms" (`adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs:269`).
- `inference-aws::nova_sonic_duplex`, a different test in each run:
  `speech_during_a_response_cancels_it_and_the_next_response_has_a_new_id` (line 430: left `""`,
  right `"Sure"`) and `a_count_nova_sonic_leaves_out_is_recorded_unknown_never_zero`.

The same tests pass in the check run on the forge and on an idle machine. A test that fails
when other work runs beside it makes the gate say something about the machine, not about the
change.

## Acceptance

- Each of the three tests either holds on a machine under load (its assertion bounds what the
  code guarantees, with the scheduler's delay named and allowed for, or it waits for an event
  where it now waits for a time), or the measure it holds is stated in `quality/` with the
  condition it needs.
- The cause of the two `nova_sonic_duplex` failures is known: a wait for a time where an event
  exists, or a defect of the adapter that load makes visible.
- No bound of a test is raised without a sentence in the test that says what the larger bound
  still proves.

## Out of scope

- The bound of 120 s of `task test-run` and the number of tests a loaded machine runs in it.
- The load of the machine.

## Scope as built

Built on the branch `wave/u3-tests-under-load`, merged into `wave/2026-11-26-quality`:

- `adapters/inference-aws/tests/nova_sonic_duplex.rs`: the speech test and the count test wait with
  `until` for the event they need; `until` fails after 3 s with no event in a row and after
  `UNTIL_LIMIT` (20 s) overall, each with its own message (`until_within` takes the limit).
- `adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs` and the ElevenLabs file of the
  same name: the hold of a drop inside a runtime is measured less the time the thread was ready to
  run without a processor (`ReadyWait`, now `pub` in
  `adapters/inference-elevenlabs/tests/support/ticker.rs`), against `CLOSE_GRACE` and `HOLD_SLACK`
  (15 ms). A drop that yields or spins instead of parking is counted as ready; the doc of
  `HOLD_SLACK` and `quality/not-measured.md` say so.

Left open: 11 more call sites of `drain` in the Nova Sonic duplex tests (the draft
story:nova-sonic-tests-wait-for-their-event); the ticker tests under contention (the draft
story:ticker-test-holds-under-contention).
