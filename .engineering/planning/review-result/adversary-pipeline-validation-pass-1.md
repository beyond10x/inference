---
format: aep.planning-md/3
id: review-result:adversary-pipeline-validation-pass-1
kind: review-result
status: active
title: Adversary, story:pipeline-validation, pass 1
relations:
- reviews: story:pipeline-validation
revision: 1
---
```
unit: story:pipeline-validation, uncommitted working tree on base 4a96264 (impl/pipeline-validation)
verdict: CONFIRMED (suite gaps only: 7 mutants survive the unit's suite; no red case against the code)
cases: executed 292→324, red 0
origin: introduced 8 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory (part 6)
needs-coordinator: none
```

The code held up. Every case I wrote passes against the unit's code. The findings are holes in the unit's own tests: 7 mutants survive them, and 1 step of the story is only half delivered.

**1. Diff stat.** `git --no-pager diff --stat` matches the implementor's 10 files exactly (610 insertions, 69 deletions). My additions are 2 new, untracked test files and nothing else:
- `crates/inference-pipeline/tests/adversary2_validation.rs`
- `crates/inference-testkit/tests/adversary10_validation_force.rs`

No non-test path is mine. The edit to `crates/inference-testkit/src/lib.rs`, which the brief does not give the implementor, is identical to `scratch/testkit-lib-doc.patch`.

**2. Cases added.** All are green now and were green on their first run, alone. Each one fails against a mutant copy kept in scratch, and the unit's own suite stays green against that same mutant (`mutants.log`, `mutant-testkit.log`).

| Case | What it asserts | Mutant it kills (unit's suite stays green) |
|---|---|---|
| `adversary2_validation.rs:177` | committed output into an input that admits provisional data validates | `validation.rs:148` `&& !` changed to `!=` |
| `:202` | whole frames into an incremental input validate | `validation.rs:152` `&& !` changed to `!=` |
| `:227` | a Text-to-Audio edge, provisional and incremental, into a Tool gives exactly 1 `incompatible_edge` | `validation.rs:134` `continue` removed |
| `:257` | another pipeline's own edge and binding add no finding to this pipeline; both pipelines validate | `validation.rs:62` edge filter dropped; `:111` binding filter dropped |
| `:291` | findings come in check order 1, 2, 5, 6 even when declared in reverse | `validation.rs:30-31` cycle and reachability checks swapped |
| `adversary10_validation_force.rs:105` | a forced `rejected` survives a `DeclarePipeline` in between and then lapses | `tests/target/mod.rs:117` `take_if(..)` changed to `take()` |

Red output from the mutant copies:
```
MUTANT no-continue-after-check-2 TEST validation EXIT=0
  test result: ok. 18 passed; 0 failed; ...
MUTANT no-continue-after-check-2 TEST adversary2_validation EXIT=101
  test an_edge_check_2_refuses_is_not_read_by_checks_3_and_9 ... FAILED
(forced take() mutant)
  left: "inference.graph.ValidatePipeline/validated"
 right: "inference.graph.ValidatePipeline/rejected"
test result: FAILED. 26 passed; 1 failed
```
The other 5 pipeline mutants show the same pattern: `validation` exits 0 with 18 passed, and `adversary2_validation` exits 101 on the case named in the table.

**3. Gate**, run after the cases existed. Each command was run on its own with debug info off.

| Command | Exit |
|---|---|
| `cargo test -p inference-pipeline --all-features --no-fail-fast` | 0 |
| `cargo test -p inference-testkit --all-features --no-fail-fast` | 0 |
| clippy, inference-pipeline, `-D warnings` | 0 |
| clippy, inference-testkit, `-D warnings` | 0 |
| `cargo fmt -p inference-pipeline --check` | 1 on the first run, from my file only; after `rustfmt`, rerun: 0 (tests and clippy rerun: 0) |
| `cargo fmt -p inference-testkit --check` | 0 |

- **inference-pipeline:** 36 tests ran, 0 failed (`adversary2_validation` 5 passed, `validation` 18 passed).
- **inference-testkit:** 288 tests ran, 0 failed (`adversary10_validation_force` 27 passed, `conform` 27 passed).
- **How "before" was counted:** I did not run the suite before writing my cases. 292 is the after-run with my two test binaries' counts taken out: 5 for the pipeline file, and 27 for the testkit file, which includes 26 copies of the target's own unit tests.

**4. Findings**
- **The 7 mutants (`mutant`, `CONFIRMED`, `introduced`).**
  - **Measured:** the rows in part 2.
  - **What reaches them:** the code paths are live: `ConnectPorts` accepts every edge shape used, and the conformance `rejected` scenario forces an outcome and then validates. Today's code is correct on each of these lines. The gap was only in the tests, and my cases close it.
- **Story step 3 is half delivered (`acceptance`, `CONFIRMED`, `introduced`, note).** Step 3 says "the Validated form of the typed builder is the return value". `builder.rs:357` `build` is unchanged and still returns the pipeline identity; there is no Validated typed form. The story's Acceptance section does not name it.
  - **What reaches it:** no caller needs it yet.
  - **Fix:** make `build` return a Validated handle, or move that half of the step out of this story.

**5. Attacked and not broken**
- Self-loops and two-node cycles; the cycle finding names the first node in declaration order.
- Optional and control-only ports may stay unconnected.
- A sink with no source reaching it is found.
- Check 9 reports `disallowed_data_flow` only, and a tool input that explicitly admits provisional data validates.
- Field names, `Optional`s and enum order match `graph.yaml`.
- The synthesized `wrong-state` and `rejected` scenarios use identities no record carries, so "wrong-state with no current" and a forced rejection with empty findings are what ESS itself asks for.
- `begin_scenario` resets the forced outcome between scenarios.
- `SKIPPED.md`: the 14 ids moved and the 9 now naming `story:capability-planning` are consistent with `conform.rs`.

**6. Paths written outside the worktree:** `<unit scratch>/adversary-1/`
- Contents: `suite.json`, `show.py`, `mutate.py`, `mutants.log`, `mutant-testkit.log`, `case-testkit.log`, and `gate-{test,clippy,fmt}-{pipeline,testkit}.log`.
- The mutant workspace copy `adversary-1/mutant/`, including its `target/`, is deleted.
- The lease `f8bfe626-wave8-adv-pv` is released.

**7.**
```findings
- file: crates/inference-pipeline/src/validation.rs
  line: 148
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: making the provisional check symmetric survives the unit's suite; killed by adversary2_validation.rs:177
- file: crates/inference-pipeline/src/validation.rs
  line: 152
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: making the incremental check symmetric survives the unit's suite; killed by adversary2_validation.rs:202
- file: crates/inference-pipeline/src/validation.rs
  line: 134
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: dropping the continue that keeps checks 3 and 9 off a check-2-failed edge survives the unit's suite; killed by adversary2_validation.rs:227
- file: crates/inference-pipeline/src/validation.rs
  line: 62
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: reading every pipeline's edges survives the unit's suite; killed by adversary2_validation.rs:257
- file: crates/inference-pipeline/src/validation.rs
  line: 111
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: reading every pipeline's bindings survives the unit's suite; killed by adversary2_validation.rs:257
- file: crates/inference-pipeline/src/validation.rs
  line: 30
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: swapping check 5 and check 6 breaks the documented finding order and survives the unit's suite; killed by adversary2_validation.rs:291
- file: crates/inference-testkit/tests/target/mod.rs
  line: 117
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a forced outcome consumed by any graph command survives the unit's suite; killed by adversary10_validation_force.rs:105
- file: crates/inference-pipeline/src/builder.rs
  line: 357
  category: acceptance
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: story step 3 asks for a Validated form of the typed builder as the return value and build still returns the bare pipeline identity
```
