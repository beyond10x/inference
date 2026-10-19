---
format: aep.planning-md/3
id: review-result:adversary-tool-call-lifecycle-pass-1
kind: review-result
status: active
title: Adversary, story:tool-call-lifecycle, pass 1
relations:
- reviews: story:tool-call-lifecycle
revision: 1
---
unit: story:tool-call-lifecycle, uncommitted working tree on base 010843f (impl/tool-call-lifecycle)
verdict: CONFIRMED (3 surviving mutants, 1 contract finding; no red case)
cases: executed 573→625, red 0
origin: introduced 6 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory (11 files)
needs-coordinator: an owner decision on the confirmation hook (finding 4)

I found nothing broken. Every new case is green on the tree and red on a mutant that the unit's own suite (conform included) lets through.

**1. What I touched.** `git diff --stat 010843f` is unchanged (10 tracked files, all the implementor's). I added two untracked test files and nothing else:
- `crates/inference-tools/tests/adversary1_confirmation.rs` (109 lines)
- `crates/inference-testkit/tests/adversary1_tools_target.rs` (185 lines, `#![cfg(feature = "run")]`)

**2. The cases (green now; each run alone first).**

| case | asserts | mutant that turns it red |
|---|---|---|
| `a_declaration_keeps_requires_confirmation_as_declared` | `declare(false).requires_confirmation() == false` | M1 |
| `the_hook_answers_before_the_state_for_a_call_that_is_not_proposed` | an Authorized or Rejected call given (true,false) returns `Unconfirmed`; given (true,true) returns `Conflict` | M3 |
| `a_side_effecting_proposal_on_a_hypothesis_is_barred_with_its_class_through_the_target` | the suite's `barred` scenario, edited to an idempotent or non-idempotent write tool and expecting the `side_effect` field, passes the runner | M2 |
| `a_side_effecting_proposal_on_a_committed_span_is_proposed_with_its_class_through_the_target` | the suite's `proposed` scenario, edited to its idempotent-write tool, passes with view `side_effect: idempotent_write` | none run; it guards the conform route |
| `a_tool_declared_without_confirmation_reads_back_false_through_the_target` | the suite's `declared` scenario with `requires_confirmation: false` passes | M1 |

The first run of the target file failed in my own harness (two cases synthesized into one directory). I fixed that; it was not a finding. Logs: `case-tools.log`, `case-target.log`.

**3. Gate and mutant runs.** Exit codes: test-tools 0, test-testkit 0, clippy-tools 0, clippy-testkit 0, fmt-tools 0, fmt-testkit 0 (after running rustfmt on my file only).
- **Executed count:** 625 after. 573 is that same run with my binaries deselected: 2 cases in `adversary1_confirmation` and 50 in `adversary1_tools_target`, which also re-runs the 47 `target::*` unit tests because of `mod target;`.
- **Conform report:** passed 250, total 271, failed 0, error 0, unsupported 21. All 21 are `inference.session` and equal the 21 lines in `ess/SKIPPED.md`; all 53 `inference.tools` scenarios passed.
- **stt-only:** `cargo tree` finds 0 `inference-runtime` lines.
- **Mutants**, one scratch copy at a time, copy and its `target/` deleted afterwards:
  - M1: `lib.rs:425` set to `requires_confirmation: true`. Only my 2 cases fail (`m1.log`).
  - M2: `target/tools.rs:140` reports `ReadOnly` as the barrier's class. Conform and all target tests pass; only my barred case fails (`m2.log`).
  - M3: `call.rs:252` moves the hook after `moved()`. Only my hook case fails (`m3.log`).

**4. Findings**

| # | file:line | verdict | measured | what reaches it |
|---|---|---|---|---|
| 1 | crates/inference-tools/src/lib.rs:425 | CONFIRMED | M1 survives every unit case | The authority is meant to read `requires_confirmation` (tools.yaml:101); nobody calls it yet. Every suite tool that is read back is declared `true`. |
| 2 | crates/inference-testkit/tests/target/tools.rs:140 | CONFIRMED | M2 passes conform | The suite's `barred` uses a read-only tool and checks no error field. Conform therefore never witnesses §23.1 test 5 with a side-effecting tool; only the Rust tests do. |
| 3 | crates/inference-tools/src/call.rs:252 | CONFIRMED | M3 survives | Only direct Rust callers reach it, and there are none: the testkit store checks the hook before looking up the call. |
| 4 | ess/domains/tools.yaml:446 | CONFIRMED | Suite `AuthorizeToolCall/outcome/authorized`, steps 205–212: a tool with `requires_confirmation: true` is authorized with (false,false). The suite requires this, so a target that enforced the tool's flag would fail conform. In Rust, `ToolCall::authorize` (call.rs:248) never sees the `Tool`. | Only the conform suite today; no executor or node exists. The story recorded the decision and left it as UNMAPPED, an open question for the owner. One possible fix: `authorize` takes `&Tool` and refuses when `tool.requires_confirmation() && !confirmed`, which needs the spec to stop requiring this scenario. |
| 5 | crates/inference-tools/Cargo.toml:12 | CONFIRMED | `cargo tree`: core plus a mandatory `inference-text` | The story says "inference-core only"; the design (line 240) says core, optionally text. The span types live in text, so the mandatory dependency is a deviation to record or put behind a feature. |
| 6 | crates/inference-tools/tests/lifecycle.rs:307 | CONFIRMED | `assert_eq!(call.state(), state)` runs after a `&self` method, so it cannot fail | Conform's unchanged-subject checks cover the store; this line covers nothing. |

**5. Attacked and not broken**
- State and enum names, order, optionality and terminal states agree between tools.yaml and the Rust types.
- All 24 state×command refusal cells exist; each moves nothing and emits nothing (conform checks no-event and unchanged subject).
- `unconfirmed` is answered before the call is looked up.
- Tools ids are in space 4 and a refusal takes none.
- Mutants that drop tools from the publication offset or from the token are caught by `tools_events_take_their_place_in_the_one_publication_order`.
- The 16 acceptance ids are all in the passed set.

**6. Written outside the worktree:** `<unit scratch>/adversary-1/` with `suite.json` (7 MB), `synth.log`, `case-tools.log`, `case-target.log`, `m1.log`, `m2.log`, `m3.log`, `gate-*.log`, `fmt-*.log`, and the scripts `show.py`, `steps.py`, `count.py`, `report.py`. Inside the worktree, the target cases write 7 `target/tmp/adversary1-tools-*` directories. Lease released with `session-end`, exit 0.

```findings
- file: crates/inference-tools/src/lib.rs
  line: 425
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a declaration that hard-codes requires_confirmation true passes every unit case and conform, because every tool the suite reads back is declared true
- file: crates/inference-testkit/tests/target/tools.rs
  line: 140
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the target can report a wrong side_effect in ProvisionalInputBarrier and conform still passes, since the suite's barred witness is a read-only tool and checks no error field
- file: crates/inference-tools/src/call.rs
  line: 252
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: swapping the confirmation hook after the state check in ToolCall::authorize survives the unit's cases, which exercise the hook on Proposed calls only
- file: ess/domains/tools.yaml
  line: 446
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the synthesized authorized scenario requires authorizing a requires_confirmation tool with no confirmation, and ToolCall::authorize never sees the tool, so the per-tool flag binds nothing until the owner decides
- file: crates/inference-tools/Cargo.toml
  line: 12
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: inference-tools depends on inference-text unconditionally where the story says core only and the design says text is optional
- file: crates/inference-tools/tests/lifecycle.rs
  line: 307
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the refused-move assertion reads a call that a by-reference method cannot change, so it cannot fail
```