---
format: aep.planning-md/3
id: review-result:adversary-llm-contract-pass-2
kind: review-result
status: active
title: Adversary, story:llm-contract, pass 2
relations:
- reviews: story:llm-contract
revision: 1
---
```
unit: story:llm-contract, working tree at base 75df5fb plus the uncommitted correction round (worktree inference-impl-llm-contract)
verdict: NEEDS-CHANGE
cases: executed 118→127, red 4
origin: introduced 7 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 path (scratch/adversary-2/, 9 logs)
needs-coordinator: none
```

**Verdict:** the correction holds against Debug, Display, serde, Deref, pattern matching, a second `release` and a stale handle. One documented guarantee is broken: an implementation can override `offer_input` and take raw text. Its adapter does not have to claim `concurrent_input_output`.

### 1. `git --no-pager diff --stat 75df5fb`
```
 Cargo.lock                          |   9 ++
 crates/inference-testkit/Cargo.toml |   3 +-
 crates/inference-testkit/src/lib.rs |   2 +-
 crates/inference-testkit/src/llm.rs | 194 ++++++++++++++++++++++++++++++++++++
 4 files changed, 206 insertions(+), 2 deletions(-)
```
All of those lines are the unit's own changes. My three files are untracked tests: `crates/inference-llm/tests/adversary2_release.rs`, `crates/inference-llm/tests/adversary2_offer_override.rs` and `crates/inference-testkit/tests/adversary5_llm_stale.rs`. I touched no non-test path.

### 2. Cases added (each binary run alone before the suite)
| case | asserts | now |
|---|---|---|
| offer_override.rs:58 `an_adapter_not_claiming_…_through_offer_input` | a handle that overrides `offer_input` on a streaming-only adapter refuses with `CapabilityNotClaimed` | **red** |
| release.rs:146 `a_copy_of_the_output_taken_while_active_…` | a `GenerationOutput` cloned while Active refuses `release` after the original is superseded | **red** |
| release.rs:125 `the_text_is_not_observable_by_comparing_admissions` | `AdmittedInput` is not `PartialEq` (trait probe, checked against `String` and a non-`PartialEq` type) | **red** |
| stale.rs:64 `a_copy_of_a_superseded_handle_delivers_no_output…` | a cloned `MockGeneration` refuses input and delivers no `TextDelta` for the superseded id | **red** |
| release.rs:54 / stale.rs:34 | input admitted, then superseded or cancelled: `release` / `accept_input` returns `GenerationStateConflict{Superseded\|Cancelled}`, and nothing is delivered | green, guard |
| release.rs:76 | another generation's input, offered to a Completed one, gets `AdmittedByAnotherGeneration` (identity is checked before state) | green, guard |
| release.rs:94 | `{:?}`, `{:#?}` and `Ok(&input)` Debug never show the text and do name the generation id | green, guard |
| release.rs:169 | commit after supersede or cancel returns `Ok([])` and changes nothing | green, guard |

Red output from the solo runs:
```
offer_override.rs:81:5: the trait lets an implementation take input its adapter does not claim
  left: (Ok(()), ["input during output"])
 right: (Err(CapabilityNotClaimed(CapabilityNotClaimed { capability: ConcurrentInputOutput })), [])
release.rs:154:5: input admitted by a generation that was superseded is released by a copy of its output
  left: Ok("admitted text 7f3a")
 right: Err(GenerationStateConflict(GenerationStateConflict { current: Superseded }))
release.rs:133:5: AdmittedInput is PartialEq, so a holder of the handle confirms a guessed text without release: right guess equal = true, wrong guess equal = false
stale.rs:74:5: a stale generation admits no new output
  left: (Ok(()), ["a", "b", "late input"])
 right: (Err(GenerationStateConflict(GenerationStateConflict { current: Superseded })), [])
```
The Debug guard's first solo run was red because of a bug in my test: I compared against the `{:?}` id, and the pretty form splits it across lines. I switched to the id's Display form and the case went green before the suite ran.

Mutants, run on a scratch copy that is now deleted. The unit's suite (`generation_output` 7, `adversary1` 1, doctest 1) stayed green under each one, and only my guard went red:

| mutant | killed by |
|---|---|
| M1: `require_active` moved above the identity check in `release` | release.rs:76 |
| M2: `release` refuses only Completed | release.rs:54 |
| M3: Debug adds `.field("text", …)` | release.rs:94 |

### 3. Suite, after the cases existed
| command | exit |
|---|---|
| `cargo test -p inference-llm --all-features --no-fail-fast` | 101 (`adversary2_offer_override` 0/1, `adversary2_release` 4 passed, 2 failed; others ok) |
| `cargo test -p inference-testkit --all-features --no-fail-fast` | 101 (`adversary5_llm_stale` 1 passed, 1 failed; others ok) |
| `cargo clippy -p inference-llm --all-targets --all-features -- -D warnings` | 0 |
| `cargo clippy -p inference-testkit --all-targets --all-features -- -D warnings` | 0 |
| `cargo fmt -p inference-llm --check` | 0 (after `rustfmt` on my files; the first check failed only on them) |
| `cargo fmt -p inference-testkit --check` | 0 (same) |

Totals: 123 passed, 4 failed. Before = 127 minus my 9 cases (offer_override 1, release 6, stale 2) = 118.

### 4. Findings
| # | file:line | finding | verdict / origin | reached by |
|---|---|---|---|---|
| F1 | crates/inference-llm/src/lib.rs:447 (claim at :24) | `offer_input` is a provided trait method, so an override takes a raw `String` and admission never runs. "No implementation accepts input its adapter does not claim" does not hold. Fix: make the admit-then-accept path non-overridable (free function or sealed extension trait), or reword the claim | NEEDS-CHANGE / introduced | Every future `GenerationHandle` implementation; the trait is the extension point. In the repo only `MockGeneration` implements it, and it does not override |
| F2 | crates/inference-llm/src/lib.rs:211, crates/inference-testkit/src/llm.rs:114 | `GenerationOutput` and `MockGeneration` derive `Clone`. A copy taken while Active still releases input after the generation is superseded, and keeps emitting deltas under the superseded id. Fix: drop `Clone` from both, or document that the Active check is per value | INFEASIBLE / introduced | Nothing found. Outside tests nobody clones an output or a handle; the unit's own `.clone()` calls are before/after comparisons only |
| F3 | crates/inference-llm/src/lib.rs:185 | `#[derive(PartialEq, Eq)]` on `AdmittedInput` lets anyone holding the handle confirm a guessed text without `release`. That contradicts "reachable only through release", which the hidden Debug implies. Fix: drop the derive | INFEASIBLE / introduced | Nothing found holds an `AdmittedInput` without knowing its text |
| F4 | crates/inference-llm/src/lib.rs:347 | Suite gap, M1 survived the unit's cases; now covered by release.rs:76 | CONFIRMED / introduced | n/a (coverage) |
| F5 | crates/inference-llm/src/lib.rs:354 | Suite gap, M2 survived (release was tested after Completed only); now covered by release.rs:54 | CONFIRMED / introduced | n/a |
| F6 | crates/inference-llm/src/lib.rs:195 | Suite gap, M3 survived (no Debug case); now covered by release.rs:94 | CONFIRMED / introduced | n/a |

Coordinator patch to `adversary4_llm_admission.rs`: no assertion was lost. The "deltas exclude INPUT" check and `reflected == 1` are both kept. A typed `AdmittedByAnotherGeneration` assert and an `Ok(())` assert were added. The removed second take no longer compiles, and the passing `compile_fail` doctest pins that.

### 5. Attacked and not broken
- **Debug:** text hidden in `{:?}`, `{:#?}`, and nested in a `Result`.
- **Other routes to the text:** no Display, serde or Deref, and the fields are private, so no outside pattern match.
- **Taking an admission twice:** not possible. `release` consumes the input, and there is no `Clone`.
- **Superseded or cancelled generation:** releases nothing, through `GenerationOutput` or through the mock handle.
- **Identity before state:** checked in that order, as documented.
- **Commit after Completed:** commits the Tentative segment, matching ESS `CommitSegment` (no generation guard, wrong-state only).
- **Commit after Superseded or Cancelled:** `Ok([])`, nothing changes, every segment stays Discarded.

### 6. Paths written outside the worktree
`<unit scratch>/adversary-2/` holds `red-llm.txt`, `red-llm-release.txt`, `red-testkit.txt`, `mutant-m1-order.txt`, `mutant-m2-completed-only.txt`, `mutant-m3-debug-text.txt` and `gate-{test,clippy,fmt}-{llm,testkit}.txt`. The mutant copy (`adversary-2/mutant/`, including its `target/`) and `mutate.py` are deleted. The lease for session f8bfe626-wave5-adv2-llm is released.

```findings
- file: crates/inference-llm/src/lib.rs
  line: 447
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: offer_input is an overridable provided method, so an implementation can take raw input text without admission, contradicting the crate doc's guarantee that no implementation accepts input its adapter does not claim.
- file: crates/inference-llm/src/lib.rs
  line: 211
  category: property
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: GenerationOutput and MockGeneration derive Clone, so a copy taken while Active releases input and emits deltas for a generation the original has superseded.
- file: crates/inference-llm/src/lib.rs
  line: 185
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: AdmittedInput derives PartialEq, an equality oracle that confirms a guessed text without release although Debug hides it.
- file: crates/inference-llm/src/lib.rs
  line: 347
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: Moving the Active check before the admitting-generation check survived the unit's suite; adversary2_release.rs:76 now catches it.
- file: crates/inference-llm/src/lib.rs
  line: 354
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: A release that refuses only Completed survived the unit's suite; adversary2_release.rs:54 now catches it.
- file: crates/inference-llm/src/lib.rs
  line: 195
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: A Debug that prints the admitted text survived the unit's suite; adversary2_release.rs:94 now catches it.
```
