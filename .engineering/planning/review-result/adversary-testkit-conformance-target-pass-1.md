---
format: aep.planning-md/3
id: review-result:adversary-testkit-conformance-target-pass-1
kind: review-result
status: active
title: Adversary, story:testkit-conformance-target, pass 1
relations:
- reviews: story:testkit-conformance-target
revision: 1
---
unit: story:testkit-conformance-target, uncommitted working tree on base edf8ca7 (worktree inference-impl-testkit-conformance-target)
verdict: CONFIRMED (9 mutants the unit's suite misses; no red case against the implementation)
cases: executed 9→26, red 0
origin: introduced 10 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory: the unit scratch's adversary-1/ (part 6)
needs-coordinator: none

**1. Diff stat**

`git --no-pager diff --stat` lists only the unit's tracked files: AGENTS.md, Cargo.lock, Taskfile.yml and crates/inference-core/src/model.rs. Mine are two new, untracked test files:
- `crates/inference-testkit/tests/adversary1_target.rs`
- `crates/inference-testkit/tests/adversary1_store.rs`

I changed no implementation file. `diff -r` of the crates against my pre-format scratch copy differs only in those two files.

**2. Cases added (written before anything ran)**

| File:line | Asserts | Now |
|---|---|---|
| adversary1_target.rs:140 | DeclareModel with an undeclared `colour` field returns `Unavailable` and stores nothing | green |
| :155 | a missing `checksum` returns `Unavailable` | green |
| :166 | an undeclared enum member (`execution_form: teleported`) returns `Unavailable` | green |
| :178 | `max_concurrency` of 1.5, 0.5, -0.5, 2^63, 1e20 or 1e-40 returns `Unavailable`, and the Adapters view stays empty | green |
| :204 | -2^63 is read, refused, and echoed as `submitted = i64::MIN` | green |
| :233 | a refusal of -7 echoes `submitted = -7` and emits no event | green |
| :257 | Adapters row: concurrency 7, two capabilities in order, ids `…0001` and `…0002` | green |
| :291 | a read demanding `seq:5` of a store at `seq:1` is an error; the command's own token reads | green |
| :319 | undeclared model names and other-domain names come back `unsupported` | green |
| :343 | `observe_events`: one event each with sequence 1 and 2; the refusal records no event | green |
| :385 | `end_scenario` then `begin_scenario` empties the stores and restarts at id `…0001` | green |
| :463 | through `Runner`, an edited synthesized suite (1.5, 9.3e18, 1e20, extra field) gives `Status::Error`; the unedited control gives `Passed` | green |
| adversary1_store.rs:41 | an `unknown-model` refusal takes no id; the next adapter gets `…0002` | green |
| :60 | both views list rows in declaration order (3 models, 2 adapters) | green |
| :100 | two runs that include both refusals issue the same 3 ids | green |

The first run of these cases alone (`--test adversary1_store --test adversary1_target`) exited 101. All 11 failures were my own typo, not a finding:
```
panicked at crates/inference-testkit/tests/adversary1_target.rs:111:39:
a scenario id: Identifier { kind: "scenario id", value: "adversary1/scenario", reason: "is not a scenario name; ...
```
After I replaced the ScenarioId with a well-formed one, `--test adversary1_target` gave `test result: ok. 14 passed; 0 failed` with EXIT=0. The implementation holds on every edge I tried. These cases are guards: on the current tree nothing goes red, but each one kills a mutant the unit's own suite lets through (part 4).

**3. Package gate (run after the cases existed)**

| Command | Exit | Summary |
|---|---|---|
| `cargo test -p inference-testkit --all-features --no-fail-fast` | 0 | lib 6, adversary1_store 3, adversary1_target 14, conform 3, doc 0 |
| `cargo clippy -p inference-testkit --all-targets --all-features -- -D warnings` | 0 | |
| `cargo fmt -p inference-testkit --check` | 1, then 0 | the first failure was only my two files; after `rustfmt` on those two, it exits 0. Test and clippy re-ran after formatting, both exit 0 |
| `… --lib --test conform` (my files left out) | 0 | 6 + 3 = 9, which is the "before" count |

Two of the 14 adversary1_target cases are `target::wire_integers` again. `tests/target/mod.rs:290` has no `#[cfg(test)]`, so every binary that declares `mod target;` runs those two tests again.

**4. Findings**

Mutants were applied one at a time to a scratch copy. Each row shows the unit's own suite (`--lib --test conform`) staying green and my guard going red.

| # | Mutant (file:line) | Unit suite | Guard | Severity |
|---|---|---|---|---|
| M1 | `Input::finish` accepts leftover fields (target/mod.rs:232) | exit 0 | target.rs:140 red | warning |
| M8 | `Input::integer` falls back to `Some(1)` on 1.5 (target/mod.rs:205) | exit 0 | target.rs:178 and :463 red | warning |
| M2 | refusal echoes `integer_node(0)` (target/model.rs:106) | exit 0 | target.rs:233 red | note |
| M3 | Adapters row writes `max_concurrency` as 1 (target/model.rs:223) | exit 0 | target.rs:257 red | note |
| M4 | consistency-token guard removed (target/model.rs:120) | exit 0 | target.rs:291 red | note |
| M5 | `observe_events` returns nothing (target/model.rs:157) | exit 0 | target.rs:343 red | note |
| M9 | stores never reset between scenarios (target/mod.rs:72, :142) | exit 0 | target.rs:385 red | note |
| M6 | `unknown-model` refusal consumes an id (src/model.rs:189) | exit 0 | store.rs:41 red | note |
| M7 | models stored newest-first (src/model.rs:171) | exit 0 | store.rs:60 red | note |

What reaches each mutant:
- **M1, M8:** the target's documented contract (module doc in mod.rs:9-11) and the story's binding `## Wire integers` section. Today's suite only ever sends 1.0 and 0.0 and never an extra field, so the conform test cannot tell. The unit tested the free `integer()` function, not the path a request actually takes.
- **M2–M5, M9:** the suite only sends a refusal of 0, a concurrency of 1 and one capability. It never sends a read token ahead of the store or an observe step. The 4 model scenarios use only "contains" checks, so they cannot detect state leaking from one scenario into the next.
- **M6, M7:** these break the claims "a refused command consumes no id" and "in declaration order". Neither the conform test nor the unit tests check them for an unknown-model refusal or for more than one row.

Control mutant M10 (a refused adapter gets stored anyway) is caught by the unit's `a_non_positive_concurrency_is_refused_and_nothing_is_stored`.

The fix is to commit the two guard files. No implementation change is needed.

Judgement finding J1 (note, CONFIRMED, introduced): the docs at `crates/inference-testkit/src/model.rs:166` and `:176` say the store keeps what `Model::declare` / `Adapter::declare` return. The code calls `declare_as` under counter ids, so the doc names the wrong constructor.

**5. Attacked and could not break**
- **Outcomes decided by bookkeeping:** none found. `unknown-model` is decided by the store; `refused` comes from core called through the store; the token is `seq:<store events>`; event sequence is the position in the store.
- **Wrong store passing conform:** the M10 store mutant is red in the unit suite. The store mutants that survive conform (M6, M7) are now guarded.
- **SKIPPED.md routing against the story's step 3:** computed from the 5 stories' `## Scenarios` sections and their routing sections. All 225 lines match, 0 mismatches, no id listed by two stories (routing.py).
- **Wire integers:** 1.5, 0.5, -0.5, 2^63, 1e20 and 1e-40 all give `error`; -2^63 is accepted. `ess_primitives::Number::as_i64` is exact.
- **Contract drift against model.yaml:** event, error, view and input field names all match.
- **Id determinism:** two runs give the same ids, a new scenario restarts at `…0001`, and refusals take no id.

**6. Paths written outside the worktree**

All under the unit scratch's `adversary-1/`:
- `suite.json`
- `story-{inbound-text-spans-and-envelope,outbound-text-generations-and-segments,pipeline-declaration,pipeline-validation,run-lifecycle}.txt`
- `routing.py`, `mutants.py`, `mutants.log`, `mutants-m7.log`
- `cases-alone.log`, `cases-alone-2.log`
- `suite-test.log`, `suite-test-2.log`, `suite-clippy.log`, `suite-clippy-2.log`, `suite-fmt.log`, `suite-fmt-2.log`, `suite-before.log`
- `mut/`: a 1.3G copy of the worktree, including its own `target/`. I left it in place because the rules forbid deleting a `target/` by hand.

Inside the worktree, my e2e case also leaves `target/tmp/adversary1-<millis>-<pid>/suite.json` on each run.

**7. Findings block**
```findings
- file: crates/inference-testkit/tests/target/mod.rs
  line: 232
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: Input::finish accepting undeclared fields survives the unit suite; guarded by adversary1_target.rs:140
- file: crates/inference-testkit/tests/target/mod.rs
  line: 205
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: Input::integer turning a fractional wire value into an integer survives the unit suite; guarded by adversary1_target.rs:178 and :463
- file: crates/inference-testkit/tests/target/model.rs
  line: 106
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a refusal echoing a constant submitted value survives because the suite only refuses 0; guarded by adversary1_target.rs:233
- file: crates/inference-testkit/tests/target/model.rs
  line: 223
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the Adapters row publishing a constant max_concurrency survives because the suite only declares 1; guarded by adversary1_target.rs:257
- file: crates/inference-testkit/tests/target/model.rs
  line: 120
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: removing the read-your-writes token guard survives the unit suite; guarded by adversary1_target.rs:291
- file: crates/inference-testkit/tests/target/model.rs
  line: 157
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: observe_events returning nothing survives because no model scenario observes events; guarded by adversary1_target.rs:343
- file: crates/inference-testkit/tests/target/mod.rs
  line: 72
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: never resetting the stores between scenarios survives the unit suite; guarded by adversary1_target.rs:385
- file: crates/inference-testkit/src/model.rs
  line: 189
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: an unknown-model refusal consuming a counter id survives the unit suite; guarded by adversary1_store.rs:41
- file: crates/inference-testkit/src/model.rs
  line: 171
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: storing models newest-first breaks the declaration-order claim and survives the unit suite; guarded by adversary1_store.rs:60
- file: crates/inference-testkit/src/model.rs
  line: 166
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the declare_model and declare_adapter docs name Model::declare and Adapter::declare while the code calls declare_as under counter ids
```
