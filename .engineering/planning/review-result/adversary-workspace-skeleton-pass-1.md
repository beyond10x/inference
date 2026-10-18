---
format: aep.planning-md/3
id: review-result:adversary-workspace-skeleton-pass-1
kind: review-result
status: active
title: Adversary, story:workspace-skeleton, pass 1
relations:
- reviews: story:workspace-skeleton
revision: 1
---
I found two red cases. They need a coordinator call rather than a fix that blocks the unit.

```
unit: story:workspace-skeleton, uncommitted working tree on base 533366a (worktree inference-impl-workspace-skeleton)
verdict: CONFIRMED
cases: executed 3→11, red 2
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: 4 paths (part 6)
needs-coordinator: decide whether DeclareAdapter's JSON path must accept ESS's whole-number floats (1.0, 0.0), now or in story:testkit-conformance-target
```

**1. `git --no-pager diff --stat`**
```
 AGENTS.md    | 5 ++++-
 README.md    | 8 ++++++--
 Taskfile.yml | 5 ++++-
 3 files changed, 14 insertions(+), 4 deletions(-)
```
All three lines are the implementor's changes. Git does not track my one file yet, so the stat leaves it out: `crates/inference-core/tests/adversary_model_wire.rs`. It is a test file. I changed no implementation file, manifest, Taskfile or doc. Lease taken and released (`session-start`, `session-end`).

**2. Cases added** (in `adversary_model_wire.rs`; this file alone was run first)

| case | asserts | now |
|---|---|---|
| `adversary_suite_declare_adapter_declared_input_is_accepted` :42 | the suite's literal input with `max_concurrency: 1.0` parses as `DeclareAdapter` and is declared | **red** |
| `adversary_suite_declare_adapter_refused_input_is_refused_not_unparseable` :55 | the suite's literal input with `max_concurrency: 0.0` parses and is refused with `submitted: 0` | **red** |
| `adversary_suite_declare_model_input_and_event_payload_match_ess` :69 | suite input parses; the event keys are exactly {model_id, family, checkpoint}; the Model keys are exactly the 8 ESS fields | green |
| `adversary_adapter_entity_and_event_carry_exactly_the_ess_fields` :119 | event and Adapter keys equal the ESS field sets; values use the ESS names | green |
| `adversary_adapter_id_is_fresh_per_declaration` :156 | 16 declarations give 16 different `adapter_id`s, each equal to its event's | green; red against the mutant below |
| `adversary_max_concurrency_boundaries` :171 | 0, -1 and i64::MIN are refused with `submitted` exact; 1, 2 and i64::MAX are declared; the error serializes as `{"submitted":-5}` | green |
| `adversary_identifiers_reject_non_uuid_values` :202 | all 7 identifier types reject `""`, `"not-a-uuid"`, a truncated uuid, `1` and `null` | green |
| `adversary_enums_reject_names_ess_does_not_list` :213 | wrong casing is rejected for all 4 enums | green |

Red output from the first run of the file alone (exit 101, 6 passed, 2 failed):
```
panicked at crates/inference-core/tests/adversary_model_wire.rs:45:25:
the literal input of inference.model.DeclareAdapter/outcome/declared, max_concurrency 1.0, is a DeclareAdapter: Error("invalid type: floating point `1.0`, expected i64", line: 0, column: 0)
panicked at crates/inference-core/tests/adversary_model_wire.rs:58:25:
the literal input of inference.model.DeclareAdapter/outcome/refused, max_concurrency 0.0, is a DeclareAdapter: Error("invalid type: floating point `0.0`, expected i64", line: 0, column: 0)
```

Mutant probe, run on a copy in scratch. I changed `model.rs:301` to `AdapterId::from(Uuid::from_u128(7))`, so every adapter gets the same id. The implementor's `--test model_vocabulary` still passed 3 of 3 (exit 0). My freshness case failed at `adversary_model_wire.rs:160`, the assertion whose `seen.insert(...)` is on :164.

**3. Suite run, after the cases existed**

`task check` exited 201. The ess, aep, fmt and clippy steps passed, and `cargo test --workspace` stopped on `adversary_model_wire` with "6 passed; 2 failed". The first `task check` attempt failed `cargo fmt --check` on my own file only. I ran rustfmt on that one file and reran.

`cargo test -p inference-core --no-fail-fast` exited 101:
- `adversary_model_wire`: 6 passed, 2 failed
- `model_vocabulary`: 3 passed
- lib and doc tests: 0

The 11 cases run equals 3 + 8. The "3" before is the implementor's `green-test.log`.

**4. Findings** (covering the uncommitted tree on 533366a)

| # | file:line | what was measured | what reaches it | verdict | origin |
|---|---|---|---|---|---|
| F1 | `crates/inference-core/src/model.rs:241` (`max_concurrency: i64`, serde derived) | the suite's literal `max_concurrency` 1.0 and 0.0 are rejected when parsed (`adversary_model_wire.rs:45`, `:58`, exit 101). All 105 numbers in `suite.json` are written as floats. ESS's `Number` serializer writes a whole number as `1.0` (`ess/crates/specify/ess-primitives/src/facts.rs:173-181`). | No Rust caller in the tree. The testkit story plans to translate ESS's request values into store calls (`testkit-conformance-target.md:76`) and does not say whether that goes through this JSON parsing; that is where it would hit. The story Context cites this exact scenario (`DeclareAdapter/outcome/refused`) as the reason for the wire names. | INFEASIBLE | introduced |
| F2 | `crates/inference-core/tests/model_vocabulary.rs:76-108` | check_2 never asserts that `adapter_id` is fresh per call; a fixed-id mutant passes it (exit 0) | `Adapter::declare`, the command the story names | CONFIRMED | introduced |

Fixes, not applied:
- **F1:** have `DeclareAdapter.max_concurrency` accept whole-number floats when parsing JSON, or have the testkit build `DeclareAdapter` field by field. That is the coordinator's call.
- **F2:** keep `adversary_adapter_id_is_fresh_per_declaration`, or add the same assertion to check_2.

**5. What I attacked and could not break**
- Every enum variant name against ESS, including the PascalCase Modality, and rejection of wrong casing.
- Entity and event field names, and that each event carries exactly the payload ESS declares.
- `model_id` is fresh per call.
- The concurrency guard at 0, -1, i64::MIN, 1 and i64::MAX, with `submitted` exact.
- No public way to build an Adapter or Model that skips the guard: fields are private, there is no Default or From, and neither derives Deserialize.
- Identifier round-trips, and rejection of non-uuid values.
- Every ESS entity field, enum, error, command and event is present.
- Manifests have no comments, `unsafe_code = "forbid"` is set, and there is no binary.
- All 5 `task check` steps run.
- The base 533366a has no `Cargo.toml` and no `crates/`, so nothing here pre-dates the unit.

**6. Paths written outside the worktree** (all under the unit's assigned scratch directory, `adv1/`)
- `adversary-alone.log`
- `task-check.log`
- `suite-no-fail-fast.log`
- `mut/`: the mutated copy of the workspace, including its own `mut/target/` build directory

```findings
- file: crates/inference-core/src/model.rs
  line: 241
  category: contract-drift
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: DeclareAdapter's derived Deserialize rejects the synthesized scenarios' literal max_concurrency 1.0 and 0.0 (ESS writes Integer as an integral float), so the suite input the story cites cannot be parsed through it; no Rust caller exists yet.
- file: crates/inference-core/tests/model_vocabulary.rs
  line: 76
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: check_2 never asserts adapter_id is fresh per declaration, and a fixed-id mutant of Adapter::declare passes the implementor's suite.
```
