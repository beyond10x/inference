---
format: aep.planning-md/3
id: review-result:adversary-core-model-sync-pass-1
kind: review-result
status: active
title: Adversary, story:core-model-sync, pass 1
relations:
- reviews: story:core-model-sync
revision: 1
---
I found no defect in the implementation. The two cases I added pass on the tree. Each fails on a one-line change to `model.rs` that the implementor's 21 cases do not catch.

```
unit: story:core-model-sync, working tree of inference-impl-core-model-sync (base b35a05b + uncommitted diff)
verdict: CONFIRMED (two gaps where the implementor's suite misses a change, both closed by green guard cases; no red on the tree)
cases: executed 21→23, red 0
origin: introduced 2 / pre-existing 1 / undecided 0
wrote-outside-worktree: 1 tree, the unit's scratch adversary-1 directory (listed in part 6)
needs-coordinator: whether to keep crates/inference-core/tests/adversary3_core_model_sync.rs in the unit commit
```

**1. What I touched.** `git --no-pager diff --stat` shows only the implementor's 4 tracked files (model.rs 42 lines, three test files). My only file is the untracked test file `crates/inference-core/tests/adversary3_core_model_sync.rs`, so I touched no non-test path. The 21 before my additions is the count the implementor reported, not a run of mine.

**2. Cases added**, all in `crates/inference-core/tests/adversary3_core_model_sync.rs`. The inputs are the literal values of the ESS `DeclareAdapter/outcome/unknown-model` scenario I synthesized.

| Case | Asserts | On the tree | Mutant it catches | Red output on that mutant |
|---|---|---|---|---|
| `adversary3_declare_model_requires_provenance_and_source_revision_mutant_serde_default` | the scenario's input declares a model carrying both values; the input without `provenance`, or without `source_revision`, is refused | green | m1: `#[serde(default)]` on both fields (model.rs:147-148) | `:43:9: DeclareModel without provenance parsed, but model.yaml declares it a String input with no default: Ok(DeclareModel { ... provenance: "", ... })` |
| `adversary3_model_not_found_reads_the_suite_expectation_and_names_the_identity_mutant_display` | the scenario's `expect_error` fields `{"model_id":"00000000-0000-4000-8000-50f408d7384c"}` read in and write back unchanged; as `Box<dyn Error>` the message contains the id; `{}` is refused | green | m2: Display without the id (model.rs:303) | `:63:5: the refusal message does not name the identity that was sent: "no model carries the identity"` |

Run of this file alone on the tree: `cargo test -p inference-core --test adversary3_core_model_sync`, 2 passed, exit 0. On each mutant copy the full suite exits 101. Only my case fails there; the implementor's 21 cases all pass on both mutants.

**3. Gate on the tree, run after my file existed:**
- `cargo test -p inference-core --no-fail-fast` exits 0 with 23 ok: lib 1, adversary2_identifier_spelling 2, adversary2_mutant_guards 4, adversary3 2, adversary_model_wire 8, core_model_sync 3, model_vocabulary 3.
- `cargo clippy -p inference-core --all-targets -- -D warnings` exits 0.
- `cargo fmt -p inference-core --check` first exited 1 on my own file. I ran rustfmt on that file and the check exited 0; test and clippy then exited 0 again.

**4. Findings:**

| # | Where | What was measured | What reaches it | Verdict | Origin |
|---|---|---|---|---|---|
| F1 | model.rs:147-148 | m1 leaves all 21 implementor cases green; adversary3 line 43 fails | the brief names "optionality" as a check; model.yaml:196-200 declares `type: String` with no default; the conformance suite's `DeclareModel` inputs | CONFIRMED | introduced |
| F2 | model.rs:303 | m2 leaves all 21 implementor cases green; adversary3 line 63 fails | the implementor's own claim "Display+Error"; the store that will raise this error | CONFIRMED | introduced |
| F3 | model.rs:325-330 | judgement only. `Adapter::declare` says "Runs `inference.model.DeclareAdapter`" and its `# Errors` lists only `NonPositiveConcurrency`. model.yaml:247-253 gives the command an `unknown-model` outcome, and the function does not run that outcome. | only the `ModelNotFound` comment (:292-295) says so; the base already had both the YAML outcome and this comment | CONFIRMED | pre-existing |

F1 and F2 need no change to `src/`. Keeping my file closes both gaps. The fix for F3 is one comment line in `Adapter::declare` saying it does not decide `unknown-model`.

**5. Attacked and not broken:**
- Ten capability names in YAML order, and `orchestrated_duplex` round-trips.
- `Model` writes out both new fields, and they are not swapped (the existing check uses different values for each).
- `ModelDeclared` carries only `model_id`, `family` and `checkpoint`, as model.yaml:221-225 and 284-291 say.
- `ModelNotFound` is written as an object, not a bare string, and it matches the scenario's expected fields exactly.
- `inference-core` is the only crate in the workspace, so nothing outside it uses the changed types.

**6. Paths written outside the worktree**, all under the unit's scratch `adversary-1/` directory:
- the synthesized suite: `suite.json` (5.2 MB), `synth.log`
- my scripts: `model_scen.py`, `mutate.sh`, `apply.py`
- the two mutant copies, each with its own `target/` build directory: `mutants/m1/`, `mutants/m2/`
- the mutations and their runs: `m1.diff`, `m2.diff`, `m1-suite.log`, `m2-suite.log`
- gate logs: `gate-test.log`, `gate-clippy.log`, `gate-fmt.log`, `gate-test2.log`, `gate-clippy2.log`

My lease was taken, renewed and released; `session-end` exited 0.

```findings
- file: crates/inference-core/src/model.rs
  line: 147
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: making DeclareModel.provenance and source_revision optional with serde(default) leaves all 21 implementor cases green, though model.yaml:196-200 declares them required String inputs; adversary3_declare_model_requires_provenance_and_source_revision_mutant_serde_default catches it
- file: crates/inference-core/src/model.rs
  line: 303
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a ModelNotFound Display that drops the model_id leaves all 21 implementor cases green; adversary3_model_not_found_reads_the_suite_expectation_and_names_the_identity_mutant_display catches it
- file: crates/inference-core/src/model.rs
  line: 325
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: Adapter::declare says it runs inference.model.DeclareAdapter and lists only NonPositiveConcurrency under Errors, yet it does not decide the unknown-model outcome of model.yaml:247-253
```
