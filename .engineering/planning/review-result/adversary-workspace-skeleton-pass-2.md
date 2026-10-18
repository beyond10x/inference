---
format: aep.planning-md/3
id: review-result:adversary-workspace-skeleton-pass-2
kind: review-result
status: active
title: Adversary, story:workspace-skeleton, pass 2
relations:
- reviews: story:workspace-skeleton
revision: 1
---
```
unit: story:workspace-skeleton, uncommitted working tree on base 533366a (worktree inference-impl-workspace-skeleton)
verdict: INFEASIBLE
cases: executed 11→17, red 2
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 9 paths, all under scratch/adv2/ (part 6)
needs-coordinator: should the seven identifier newtypes refuse the Uuid spellings ESS refuses (F1)? Decide now, or leave it to whichever story first deserializes ids from a non-ESS producer.
```

I made the suite red with two new cases. Both show that all seven identifier types accept Uuid spellings that ESS explicitly refuses. Nothing in the tree or the synthesized suite sends those spellings today, so this is INFEASIBLE rather than a blocker. Three more cases are green now: each fails on a code mutation (M1-M3 below) that the earlier 11 cases let through.

**1. `git --no-pager diff --stat`**
```
 AGENTS.md    | 5 ++++-
 README.md    | 8 ++++++--
 Taskfile.yml | 5 ++++-
 3 files changed, 14 insertions(+), 4 deletions(-)
```
All three lines are the implementor's changes. Git does not track my two files yet, so the stat leaves them out: `crates/inference-core/tests/adversary2_identifier_spelling.rs` and `crates/inference-core/tests/adversary2_mutant_guards.rs`. Both are test files. I changed no implementation file, manifest, Taskfile or doc. I ran rustfmt on my own file only. The lease was taken and released.

**2. Cases added** (each file was run alone before the suite ran)

| case | asserts | now |
|---|---|---|
| `adversary2_identifier_spelling.rs:44` `..._accept_only_the_canonical_uuid_spelling` | all 7 identifier types refuse `urn:uuid:…`, `{…}` and the 32-hex form | **red** |
| `adversary2_identifier_spelling.rs:62` `..._declare_adapter_refuses_a_braced_model_id` | `DeclareAdapter` refuses a braced `model_id` | **red** |
| `adversary2_mutant_guards.rs:14` `..._into_uuid_returns_the_wrapped_uuid` | `Uuid::from(id)` gives back the wrapped Uuid | green; red on M1 |
| `adversary2_mutant_guards.rs:26` `..._display_is_the_canonical_uuid_text` | `Display` of each identifier is the canonical text | green; red on M4 |
| `adversary2_mutant_guards.rs:38` `..._all_lists_the_variants_in_the_order_ess_lists_them` | each `ALL` constant serializes to the ESS list, in order | green; red on M2 and M5 |
| `adversary2_mutant_guards.rs:78` `..._refusal_message_carries_the_submitted_value` | the refusal's Display ends with the submitted value | green; red on M3 |

Red output, `cargo test -p inference-core --test adversary2_identifier_spelling`, exit 101, 0 passed, 2 failed:
```
panicked at crates/inference-core/tests/adversary2_identifier_spelling.rs:76:5:
DeclareAdapter accepted model_id {00000000-0000-4000-8000-dfb547682652}, a spelling ESS refuses: Ok(DeclareAdapter { model_id: ModelId(00000000-0000-4000-8000-dfb547682652), ... max_concurrency: 1 })
panicked at crates/inference-core/tests/adversary2_identifier_spelling.rs:56:5:
21 non-canonical spellings accepted, ESS refuses each:
ModelId accepted "urn:uuid:00000000-0000-4000-8000-dfb547682652" and wrote "00000000-0000-4000-8000-dfb547682652"
ModelId accepted "{00000000-0000-4000-8000-dfb547682652}" and wrote "00000000-0000-4000-8000-dfb547682652"
ModelId accepted "00000000000040008000dfb547682652" and wrote "00000000-0000-4000-8000-dfb547682652"
... (same three for AdapterId, RunId, NodeId, StreamId, GenerationId, SessionId)
```

Mutation probes ran on a copy of the workspace in scratch (`mut/`). The "11 original" column is the implementor's 3 cases plus pass 1's 8.

| mutant | 11 original | adversary2_mutant_guards |
|---|---|---|
| M1 `id.rs:48`: `From<Id> for Uuid` returns nil | 11/11 pass, survives | :16 fails |
| M2 `model.rs:55-56`: first two entries of `Capability::ALL` swapped | 11/11 pass, survives | :39 fails |
| M3 `model.rs:264`: the refusal's Display drops `submitted` | 11/11 pass, survives | :81 fails |
| M4 `id.rs:54`: Display writes the unhyphenated form | killed (`adversary_model_wire.rs:100`, `:143`) | :28 fails |
| M5 `model.rs:86-87`: `Modality::ALL` drops two variants | killed (`model_vocabulary.rs:158`) | :53 fails |

**3. Suite run** (after the cases existed)

- `task check`, exit 201. The ess, aep, fmt and clippy steps passed. `cargo test --workspace` stopped at the first failing binary: `adversary2_identifier_spelling: FAILED. 0 passed; 2 failed`.
- `cargo test -p inference-core --no-fail-fast`, exit 101:

| target | passed | failed |
|---|---|---|
| lib | 0 | 0 |
| `adversary2_identifier_spelling` | 0 | 2 |
| `adversary2_mutant_guards` | 4 | 0 |
| `adversary_model_wire` | 8 | 0 |
| `model_vocabulary` | 3 | 0 |
| doc tests | 0 | 0 |

- "Before" is 11: the same run with only `--test model_vocabulary --test adversary_model_wire` selected gave 8 + 3. That excludes my two files.

**4. Findings** (uncommitted tree on 533366a)

| # | file:line | what was measured | what reaches it | verdict | origin |
|---|---|---|---|---|---|
| F1 | `crates/inference-core/src/id.rs:19-24` (derived `Deserialize` through `uuid`) | ESS accepts a Uuid only in canonical hyphenated form: "one value has one spelling, or two systems agree on the schema and disagree on equality" (ess `ess-primitives/src/facts.rs:601-610`). The 7 types accept 3 refused spellings each (21 total) and write them back canonical, so the round-trip story check 3 names does not hold for them. `adversary2_identifier_spelling.rs:56`, `:76`; exit 101. | Nothing found. No Rust code deserializes ids yet. All 79 uuid values in `suite.json` are canonical, and none is braced or urn. | INFEASIBLE | introduced |
| F2 | `crates/inference-core/src/id.rs:46-50` | M1 survives the 11 original cases | no caller of `Uuid::from(id)` in the tree | CONFIRMED | introduced |
| F3 | `crates/inference-core/src/model.rs:53` | the doc promises ESS order for `ALL`; M2 survives the 11 | no caller of `ALL` outside the tests | CONFIRMED | introduced |
| F4 | `crates/inference-core/src/model.rs:259-267` | M3 survives the 11 | no caller | CONFIRMED | introduced |
| F5 | `AGENTS.md:16-25` (Layout table) | it lists no `Cargo.toml` and no `crates/`, both of which now exist. That part of AGENTS.md is outside the unit's file assignment. | every agent that reads AGENTS.md | CONFIRMED | introduced |

Fixes, not applied:
- **F1:** a hand-written `Deserialize` in `uuid_newtype!` that refuses anything but the 8-4-4-4-12 form. Or decide that leniency is fine and record that.
- **F2-F4:** keep `adversary2_mutant_guards.rs`.
- **F5:** add one Layout row for `Cargo.toml` and `crates/` (coordinator's surface).

Pass-1 correction: the original float-acceptance assertions became float-refused plus integer-accepted. The downstream checks are kept: `max_concurrency() == 1`, `capabilities`, and `Err(NonPositiveConcurrency { submitted: 0 })`. Nothing was dropped or relaxed, and it matches the "Wire integers" section of `story:testkit-conformance-target`.

**5. Attacked and could not break**
- `rust-version` 1.88: on a scratch copy, `cargo +1.88.0 test --locked` built and passed everything except my two red cases. Every locked dependency declares a rust-version of 1.85 or lower (uuid 1.26.1 and getrandom 0.4.3 at 1.85), so the README claim "1.88 or later" holds.
- `Cargo.lock` is lockfile v4 with 30 packages, is not git-ignored, and is consistent under `--locked`. Resolver is 3.
- Hash, Eq and Ord: the 7 identifiers are distinct types, so two different id kinds cannot be compared at all, and ordering follows the Uuid.
- `task check` runs in the order the story names and stops at the first failing step: run 1 stopped at `cargo test --workspace` with exit 201.
- ESS declares no field validation on `DeclareModel`, so there is nothing to refuse there. The refusal serializes as `{"submitted":0}`; the suite expects `submitted` under the error name `inference.model.NonPositiveConcurrency`.
- Doc comments match the ESS summaries for the error, Capability and Adapter.
- `FromStr` is absent; ESS does not require it.

**6. Paths written outside the worktree** (all under the unit's assigned scratch directory, `adv2/`)
- `spelling-alone.log`
- `guards-alone.log`
- `mutate.sh`
- `mutate.log`
- `msrv-1.88.log`
- `task-check.log`
- `suite-no-fail-fast.log`
- `mut/`: a copy of the workspace (restored to the original source after the probes) holding `id.rs.orig` and `model.rs.orig`
- `mut/target/`: its build directory, 274M

```findings
- file: crates/inference-core/src/id.rs
  line: 19
  category: contract-drift
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: The identifier newtypes deserialize the urn, braced and unhyphenated Uuid spellings that ESS refuses and write them back canonical, so a non-canonical input is accepted where an ESS-generated consumer refuses it; no producer of such input exists in the tree or the suite.
- file: crates/inference-core/src/id.rs
  line: 46
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: From<Id> for Uuid returning nil survives the 11 prior cases; adversary2_identifier_into_uuid_returns_the_wrapped_uuid kills it.
- file: crates/inference-core/src/model.rs
  line: 53
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: The documented ESS order of Capability::ALL is unobserved; a swap survives the 11 prior cases and adversary2_all_lists_the_variants_in_the_order_ess_lists_them kills it.
- file: crates/inference-core/src/model.rs
  line: 259
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: The NonPositiveConcurrency Display dropping the submitted value survives the 11 prior cases; adversary2_refusal_message_carries_the_submitted_value kills it.
- file: AGENTS.md
  line: 16
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: The Layout table omits the new Cargo.toml and crates/ that this unit adds, and that part of AGENTS.md is outside the unit's file assignment.
```
