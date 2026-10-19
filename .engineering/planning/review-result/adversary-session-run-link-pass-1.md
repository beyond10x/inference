---
format: aep.planning-md/3
id: review-result:adversary-session-run-link-pass-1
kind: review-result
status: active
title: Adversary, story:session-run-link, pass 1
relations:
- reviews: story:session-run-link
revision: 1
---
```
unit: story:session-run-link, uncommitted working tree on base 78ab255 (inference-impl-session-run-link)
verdict: CONFIRMED (2 surviving mutants, now guarded) and NEEDS-CHANGE (1 spec gap with no UNMAPPED marker)
cases: executed 690→746, red 0 on the tree (red 2 on mutant M1, red 1 on mutant M2)
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 4 paths kept, 4 deleted (part 6)
needs-coordinator: whether a run may be current in two sessions at once (finding F3) is not decided in the story or the spec
```

**1. `git --no-pager diff --stat` (tracked files)**: the same 8 files as at handover, `8 files changed, 1875 insertions(+), 69 deletions(-)`. I changed none of them. The one path I added is untracked: `?? crates/inference-testkit/tests/adversary1_session_run_link.rs`. No implementation path was touched.

**2. Cases added** in `crates/inference-testkit/tests/adversary1_session_run_link.rs`. The file starts with `#![cfg(feature = "run")]`, so `Cargo.toml` needed no change. Each case drives `Target` through `ConformanceTarget`:

| Case | Asserts | Tree | Mutant |
|---|---|---|---|
| `each_session_compares_a_run_with_its_own_pipeline` (:176) | sessions over pipeline A and pipeline B; a run of B attaches to B's session; A's run on B's session and B's run on A's session both answer `pipeline-mismatch` | green | M2 red |
| `a_session_command_advances_the_target_token_by_its_events` (:208) | a refused `MarkSessionReady` leaves the token unchanged; each of AttachRun, MarkSessionReady, CloseSession and CompleteSessionClose adds 1 to `seq:` | green | M1 red |
| `session_events_interleave_with_graph_events_in_command_order` (:252) | the positions of SessionOpened, RunCreated, RunAttached, RunCreated, RunAttached rise by exactly 1 each, and the last position equals the last command's token | green | M1 red |

Run alone on the tree: `cargo test -p inference-testkit --all-features --test adversary1_session_run_link -- --exact <3 names>` gave `test result: ok. 3 passed; 0 failed; ... 53 filtered out`, EXIT=0.

Mutants were run in scratch copies without `target/`, each against the full suite (`cargo test -p inference-testkit --all-features --no-fail-fast`). The line numbers below are from before a 3-line rustfmt reflow at :131.
- **M1**: `tests/target/mod.rs:194` drops `+ self.session.events().len()` from `Stores::sequence`. All 744 other cases passed, conform included. Red:
  `panicked at .../adversary1_session_run_link.rs:233:5: assertion left == right failed: AttachRun  left: "seq:5"  right: "seq:6"`, and `...:276:5` for the interleave case.
- **M2**: `src/session.rs:482` compares the run's pipeline with the first session's pipeline instead of this session's. All 745 other cases passed. Red:
  `panicked at ...:183:5: a run of the session's own pipeline is attached: ... outcome: Some(... pipeline-mismatch) ... run_id: "00000003-0001-4000-8000-000000000002", session_id: "00000005-0000-4000-8000-000000000002"`

Both copies and their `target/` directories (1.1G each) were deleted after their runs.

**3. Package gate** (worktree, after the cases existed):
- `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p inference-testkit --all-features --no-fail-fast`: TEST_EXIT=0. Sum over the binaries: 746 passed, 0 failed. That is 690 plus 56: my 3 cases, and 53 target-module unit tests that `mod target;` compiles a second time into the new binary.
- `cargo clippy -p inference-testkit --all-targets --all-features -- -D warnings`: CLIPPY_EXIT=0
- `cargo fmt -p inference-testkit --check`: FMT_EXIT=0
- `ess specify validate --path ess`: `valid`, EXIT=0
- Synthesis: `324 scenario(s) (4 authored), 3 refusal(s)`, all three ESS-SYNTH-001 on stream.

**4. Findings** (all on the uncommitted tree above)

- **F1, mutant, CONFIRMED, introduced.** Measured: M1 at `tests/target/mod.rs:194` survives all 690 existing cases; the new cases at :236 and :279 catch it. What reaches it: the runner reads every view at the token of the last command, and no scenario reads a session command's token.
- **F2, mutant, CONFIRMED, introduced.** Measured: M2 at `src/session.rs:482` survives. The suite's `pipeline-mismatch` scenario only forces the outcome (`target/session.rs:132`). Every real attach in the suite sends a run id no run carries (`...c3741068ce62`, from my scratch `suite.json`). The unit's own cases attach only to the first session opened. What reaches it: any second session that compares a real run.
- **F3, judgement, NEEDS-CHANGE, introduced.** One run can be the current run of several sessions at once, and the spec neither decides this nor marks it UNMAPPED (`ess/domains/session.yaml:62`, `:214`).
  - Measured: the suite's `Session/transition/attach` scenario attaches `00000000-0000-4000-8000-c3741068ce62` to `session` and `session-2`, which are opened over two different pipelines, and expects `attached` both times. `attach_run` (`src/session.rs:476-497`) has no check for this.
  - What reaches it: any host that attaches one run to two sessions. The design gives an event envelope one optional `session_id` (`docs/design/inference.md:310`), so such a run has no single session to stamp on its events.
  - Fix to name: an `UNMAPPED:` marker beside the topology-switch marker, or a decision recorded in the story.
- **F4, judgement, note, CONFIRMED, introduced.** A run id that no run carries is attached, and `MarkSessionReady` then makes the session Ready, still pointing at that missing run (`src/session.rs:482`; marker at `session.yaml:203`). The marker is accurate on both counts I checked:
  - A scratch copy of the spec with a `when_related {via: input.run_id, exists: false}` outcome was refused with `error[ESS-COMMAND-004] ... selects on a related row (when_related) and on an unknown_instance branch`.
  - The suite's `attached` scenario sends a run id no run carries, so a store that refused unknown runs would fail conform.

  What reaches it: a caller passing a stale or foreign run id. `00000003-0001-…`-style ids from other domains are accepted too.
- **F5, judgement, note, CONFIRMED, introduced.** Attaching the run a Ready session already holds moves it back to Opened and emits a second `RunAttached`. The spec declares this (attach goes from Opened or Ready to Opened, `session.yaml:84`). What reaches it: a host that retries an attach after a timeout loses readiness.

**5. Attacked and could not break**
- AttachRun on Draining or Closed, on an unknown session, twice, and while a run is current: the store follows the spec, and the unit's tests or conform cover each.
- MarkSessionReady with no run, after attach, after re-attach, and from Draining (whose suite arrangement holds no run): it checks the state first, then the run, which matches the spec's predicate.
- CloseSession and CompleteSessionClose: from-state sets match `session.yaml`.
- Cardinality: ess 0.60.0 accepts only `one` or `many` (`unknown variant zero_or_one`), so `Optional` plus `one` is the closest declaration that validates.
- Identity spaces: session 5 does not collide with model 1, policy 2, graph 3, runs `00000003-0001`, or tools 4.
- Contract drift: Session, the view, RunAttached, PipelineMismatch and NoCurrentRun have the same field names, optionality and order in Rust and in ESS; the state enum order matches.
- `SKIPPED.md` lists no session id.

**6. Paths written outside the worktree**
- Kept: `<unit scratch>/adversary-1/suite.json`, `.../m1.log`, `.../m2.log`, `.../gate-test.log`
- Deleted: `.../adversary-1/m1/`, `.../adversary-1/m2/` (with their `target/`), `.../adversary-1/ess-v1/`, `.../adversary-1/ess-v2/`
- The lease was taken, renewed and ended with `session-end`, all for session `f8bfe626-wave16-adv1-session-run-link`.

```findings
- file: crates/inference-testkit/tests/target/mod.rs
  line: 194
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: dropping session events from Stores::sequence survives all 690 existing cases including conform; adversary1_session_run_link.rs:236 and :279 now catch it
- file: crates/inference-testkit/src/session.rs
  line: 482
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: comparing the run's pipeline with another session's pipeline survives every existing case because the suite only forces pipeline-mismatch and the unit's cases attach to the first session only; adversary1_session_run_link.rs:186 now catches it
- file: ess/domains/session.yaml
  line: 214
  category: judgement
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: one run may be current in several sessions at once (the suite's transition/attach scenario attaches one run id to two sessions over two pipelines), which the one optional envelope session_id cannot represent, and no UNMAPPED marker states it
- file: ess/domains/session.yaml
  line: 203
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a run id no run carries is attached and the session can then be marked Ready while pointing at a run that does not exist; the UNMAPPED marker is accurate (ESS-COMMAND-004 reproduced, and the suite's attached scenario sends a run id no run carries)
- file: ess/domains/session.yaml
  line: 84
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: re-attaching the run a Ready session already holds moves it back to Opened and emits a second RunAttached, so a retried attach loses readiness
```