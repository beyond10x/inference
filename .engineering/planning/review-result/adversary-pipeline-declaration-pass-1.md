---
format: aep.planning-md/3
id: review-result:adversary-pipeline-declaration-pass-1
kind: review-result
status: active
title: Adversary, story:pipeline-declaration, pass 1
relations:
- reviews: story:pipeline-declaration
revision: 1
---
```
unit: story:pipeline-declaration, working tree of impl/pipeline-declaration (base 2ce95de plus the uncommitted change)
verdict: CONFIRMED
cases: executed 264→294, red 0
origin: introduced 6 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory, scratch/adversary-1/ (10 files, listed in part 6)
needs-coordinator: none
```

No case of mine fails on the tree. Each of the four mutant findings is a mutant that every one of the unit's own cases, the doc tests and `conform` let through. Each now has a guard that goes red on that mutant. The tree is correct where I could check it against graph.yaml and policy.yaml.

**1. Diff stat.** `git --no-pager diff --stat` reads `12 files changed, 1756 insertions(+), 136 deletions(-)`, the same as before I started, so I changed no tracked file. My four files are all new, untracked test files:
- `crates/inference-core/tests/adversary4_policy_table.rs`
- `crates/inference-pipeline/tests/adversary1_builder_direction.rs`
- `crates/inference-pipeline/tests/adversary1_builder_flags.rs`
- `crates/inference-testkit/tests/adversary8_graph_views.rs`

No non-test path. The `M crates/inference-testkit/tests/adversary1_target.rs` in the status is the implementor's edit (finding 6).

**2. Cases added.** All are green on the tree and red under the mutant shown; each mutant was run on a copy in scratch.

| File | What it asserts | Mutant it kills (implementor cases plus `conform` stay green) |
|---|---|---|
| `adversary1_builder_direction.rs` | `connect`'s source type is an output port and its sink an input port, read back by type inference | MA: `connect`'s `from` takes any direction. Doc tests 3/3 and `declaration.rs` 8/8 pass; mine does not compile: `error[E0283]: type annotations needed for &PortHandle<'_, _, …modality::Text>` |
| `adversary1_builder_flags.rs` | builder and commands give equal ports for 6 flag combinations, including each flag set alone | MB: the builder swaps `incremental` and `end_marker_required` (builder.rs:217-218). Mine: `assertion failed: (true, false, false) … left: incremental: false, end_marker_required: true … right: incremental: true, end_marker_required: false` |
| `adversary8_graph_views.rs` (5 cases) | every field of the Ports, Pipelines, Nodes, Edges and Bindings views, and of the NodeAdded, PortsConnected and NodeBound events, with each flag set alone and identities kept distinct | MC: the target reads `incremental` and `provisional_allowed` swapped (target/graph.rs:154/156). 235 pass, `conform` ok; mine: `left: [Bool(false), Bool(false), Bool(true)] right: [Bool(true), Bool(false), Bool(false)]`. MEFG, four field swaps at once: 233 pass, `conform` ok, 3 of mine fail |
| `adversary4_policy_table.rs` | all 512 rows (4 kinds × 4 egresses × 16 locality subsets in both orders) are decided as policy.yaml's `when` clauses decide them, in file order | MP: the cluster check reads `admitted.first()`. The other inference-core cases pass, mine fails. The testkit's target test also kills MP, so this only adds coverage inside inference-core |

**3. Suite run** (after my cases existed; each command run on its own, not piped):

| Command | Exit | Count |
|---|---|---|
| `cargo test -p inference-core --all-features --no-fail-fast` | 0 | passed 45 |
| `cargo test -p inference-pipeline --all-features --no-fail-fast` | 0 | passed 13 (my two files: `ok. 1 passed` each) |
| `cargo test -p inference-testkit --all-features --no-fail-fast` | 0 | passed 236 |
| `cargo clippy -p <each of the three> --all-targets --all-features -- -D warnings` | 0 | |
| `cargo fmt -p <each of the three> --check` | 0 | |

The 264 "before" is worked out, not measured: 294 minus the 30 cases my binaries add. That is 1 + 2 own cases, plus the adversary8 binary's 27, which are my 5 cases and 22 `target::` module tests that every test binary declaring `mod target` runs again. I don't have the implementor's own `cases:` line.

**4. Findings**

| # | file:line | Verdict / origin | What was measured | What reaches it |
|---|---|---|---|---|
| 1 | crates/inference-pipeline/src/builder.rs:232 | CONFIRMED / introduced | MA passes the whole package. The single `compile_fail` example (:305) wires an Audio output into a Text input, so it fails on the modality and says nothing about direction. lib.rs:14-16 promises that "an edge from an input port … cannot be written through it". | Any Rust consumer of the builder. Fix: one `compile_fail` example per mistake (input→input with the same modality, output→output). My inference case guards this now |
| 2 | crates/inference-pipeline/src/builder.rs:217 | CONFIRMED / introduced | MB passes the acceptance case `the_builder_and_the_commands_yield_the_same_declaration`: it sends `true, true, false`, so swapping the first two flags changes nothing | Builder consumers; the swap compiles because the three flags share one type |
| 3 | crates/inference-testkit/tests/target/graph.rs:154 | CONFIRMED / introduced | MC passes `conform` (the AddPort/added scenario sends all three flags `true`) and the unit test (`true, false, true`) | `conform` today |
| 4 | crates/inference-testkit/tests/target/graph.rs:313,380,429,442 | CONFIRMED / introduced | Swapping Nodes.pipeline_id, NodeAdded.pipeline_id, Edges.from_port, or Bindings node_id/adapter_id passes everything. None of the 14 passing scenarios reads Nodes, Edges or Bindings. The unit test checks Nodes by row count only, never reads Edges.from_port, and asserts Bindings where node and adapter are both `…0002`, because the two domains' counters line up | Nothing until `story:pipeline-validation` un-skips `connected`, `bound` and `added`; until then my guards are the only ones |
| 5 | crates/inference-testkit/tests/target/graph.rs:433 | INFEASIBLE / introduced | The builder accepts a `NonZeroUsize` capacity above `i64::MAX`; no `ConnectPorts` input can say that, and the Edges view would silently report `i64::MAX` | Nothing found: the testkit never uses the builder |
| 6 | crates/inference-testkit/tests/adversary1_target.rs:335 | CONFIRMED / introduced | The implementor replaced `inference.graph.DeclarePipeline` with `inference.graph.RetirePipeline`, a name graph.yaml does not declare. The case now checks an undeclared name, not a declared name the target does not answer yet | Reviewers of the earlier adversary pass. Fix: use a declared, unanswered name such as `inference.graph.ValidatePipeline` |

All six rows cover the working tree described in the header.

**5. What I attacked and could not break**
- **Refusal order** for ConnectPorts (unknown-pipeline, unknown-source, unbounded, not-declared, source-not-output) and BindNode, against graph.yaml as written: matches.
- **DeclareProfile**: the three refusals in order, and that each error carries the row as sent, against policy.yaml and the profile table (design 1044-1049): matches, including the 512-row sweep.
- **View fields and enum members**: all five graph views, Profiles, and every enum member and its order match the ESS.
- **Profile/pipeline id mix-up**: making Pipelines.profile_id the pipeline id (MD) is caught by `conform`, because its scenario declares two profiles.
- **Builder brand**: I found no way to use one builder's handle in another, by nesting builds or by stashing a handle outside the closure.
- **Not-declared refusals**: present but unreachable until validation lands, as the implementor said.
- **Commands vs ESS**: the sink port's existence and pipeline membership are not refused by a command, which matches the ESS.

**6. Paths written outside the worktree**, all in `<unit scratch>/adversary-1/`:
- `suite.json` (synthesized suite, 5 MB)
- `mutate.py`
- `MA.log`, `MA-guard.log`, `MB.log`, `MC.log`, `MD.log`, `MEF.log`, `MEFG.log`, `MP.log`

The mutant copy `mutant/` and its `target/` (766 MB) are deleted. The lease `f8bfe626-wave7-adv` is released.

```findings
- file: crates/inference-pipeline/src/builder.rs
  line: 232
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: connect accepting a source port of any direction passes every case and doc test, because the one compile_fail example fails on modality, not direction
- file: crates/inference-pipeline/src/builder.rs
  line: 217
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: swapping incremental and end_marker_required in the builder passes the builder-equals-commands acceptance case, which sends true,true,false
- file: crates/inference-testkit/tests/target/graph.rs
  line: 154
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the target reading incremental and provisional_allowed swapped passes conform (all flags true) and the unit test (true,false,true)
- file: crates/inference-testkit/tests/target/graph.rs
  line: 313
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: Nodes.pipeline_id, NodeAdded.pipeline_id, Edges.from_port and Bindings node/adapter swaps pass conform and every unit case, since no passing scenario reads them and node and adapter ids coincide in the unit arrangement
- file: crates/inference-testkit/tests/target/graph.rs
  line: 433
  category: contract-drift
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: a builder edge capacity above i64::MAX cannot be written by ConnectPorts and the Edges view would clamp it silently, but the testkit never uses the builder
- file: crates/inference-testkit/tests/adversary1_target.rs
  line: 335
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: an earlier adversary case was rewritten to probe an undeclared name instead of a declared name the target does not answer yet, so it no longer checks what it was written to check
```
