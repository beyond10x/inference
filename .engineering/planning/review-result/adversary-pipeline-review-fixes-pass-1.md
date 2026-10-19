---
format: aep.planning-md/3
id: review-result:adversary-pipeline-review-fixes-pass-1
kind: review-result
status: active
title: Adversary, story:pipeline-review-fixes, pass 1
relations:
- reviews: story:pipeline-review-fixes
revision: 1
---
```
unit: story:pipeline-review-fixes, working tree of inference-impl-pipeline-review-fixes (base 5f40904 + uncommitted change + my 2 untracked test files)
verdict: CONFIRMED (1 warning introduced, 2 notes pre-existing; no blocker)
cases: executed 486→489, red 3
origin: introduced 1 / pre-existing 2 / undecided 0
write-outside-worktree: 1 directory (.../scratch/adversary-1/, listed in part 6)
needs-coordinator: none
```

**1. Diff stat.** The tracked diff is the unit's own (27 files, +1408 −446). It is unchanged by me. I added only two untracked test files:
```
?? crates/inference-pipeline/tests/adversary1_review_fixes.rs
?? crates/inference-testkit/tests/adversary1_port_pipeline.rs
```
I changed no non-test path.

**2. Cases added.** All three are red now. Each was run alone before any suite run, logged in `red-pipeline.log` and `red-testkit.log`.

| case | asserts | now |
|---|---|---|
| `adversary1_review_fixes.rs::the_builder_writes_no_edge_capacity_connect_ports_cannot_carry` | the builder writes no edge capacity above `i64::MAX` (`ConnectPorts.capacity` is `i64`) | red |
| `adversary1_review_fixes.rs::an_essential_output_that_is_only_an_edge_sink_is_unresolved` | an essential output connected only as the sink of an output→output edge is a check-6 finding | red |
| `adversary1_port_pipeline.rs::the_pipeline_a_port_was_declared_under_is_published` | `Port.pipeline_id` appears in the `Ports` view or in `PortAdded` (`ess/domains/graph.yaml`) | red |

Red output, verbatim:
```
panicked at crates/inference-pipeline/tests/adversary1_review_fixes.rs:177:5:
assertion `left == right` failed: the essential output `to` feeds no input port
  left: []
 right: [Some(PortId(00000000-0000-4000-8000-000000000005))]
panicked at crates/inference-pipeline/tests/adversary1_review_fixes.rs:94:5:
the builder wrote an edge of capacity [18446744073709551615], which no `ConnectPorts` input (capacity: i64) can write
panicked at crates/inference-testkit/tests/adversary1_port_pipeline.rs:39:5:
`Port.pipeline_id` is set by `AddPort/added` and published by neither the `Ports` view (["port_id", "node_id", "name", "direction", "modality", "provisional_allowed", "essentiality", "delivery", "incremental", "end_marker_required"]) nor `PortAdded` (["port_id", "node_id", "name"]), so no scenario can tell the pipeline a port was declared under from its node's
```

**3. Suite run, after the cases existed.** All with `CARGO_PROFILE_*_DEBUG=0` and `--all-features --no-fail-fast`.

| command | exit | passed / failed |
|---|---|---|
| `cargo test -p inference-pipeline` | 101 | 61 / 2 (mine) |
| `cargo test -p inference-testkit` | 101 | 407 / 1 (mine) |
| `cargo test -p inference-runtime` | 0 | 18 / 0 |
| `cargo clippy -p {pipeline,testkit,runtime} --all-targets -- -D warnings` | 0 | |
| `cargo fmt -p {pipeline,testkit,runtime} --check` | 0 | |
| `ess specify validate --path ess` | 0 | |

- **Executed count:** the before number (486) is from the coordinator's `coord-verify.log`.
- **Conform** (`report.json`): `{'error': 0, 'failed': 0, 'passed': 54, 'skipped': 0, 'total': 231, 'unsupported': 177}`.
- **Synthesis:** 231 scenarios against 229 at base, still 3 refusals. The suite also passes in full against the interpreted target.

**4. Findings**

| # | file:line | verdict / origin | measured | what reaches it |
|---|---|---|---|---|
| F1 | `ess/domains/graph.yaml:1487` (Ports view), `:1358` (PortAdded) | CONFIRMED / introduced, warning | `AddPort/added` sets `pipeline_id`, but no view or event publishes it. Mutant (`mutant-port.diff`): `insert_port` records the node's pipeline instead of the input's. Conform on that copy: passed 54, failed 0, error 0, same as unmutated. The synthesized `AddPort/added` sends `pipeline-360361` with `node-138601` of another pipeline, so the mutant was exercised and went unseen. | Every `AddPort` through the target. The Rust test `a_port_named_under_another_pipeline…` catches this mutant; the spec and conform do not. Fix: add `pipeline_id` to the `inference.graph.Ports` view and to `ports_row` in `tests/target/graph.rs`, as `Nodes` does. |
| F2 | `crates/inference-pipeline/src/validation.rs:234-237` | CONFIRMED / pre-existing, note | `connected` counts an edge's `to_port`. So an essential output that is only the sink of an output→output edge counts as resolved. `ConnectPorts` refuses only a non-output source. | Reached through the public `ConnectPorts` and the target. The pipeline is still rejected by check 2, so only the findings list is incomplete. The base condition matches (read via `git show`). Fix: count `from_port` only for outputs. |
| F3 | `crates/inference-pipeline/src/builder.rs:263-277` | INFEASIBLE / pre-existing, note | `connect` takes `NonZeroUsize`, which goes past `i64::MAX`. No `ConnectPorts` input can write that edge, which contradicts the builder doc's "equals one written by the commands". The testkit `edges_row` would publish `i64::MAX`. | No caller found. The target never uses the builder, and `EdgeQueue` does not allocate by capacity. |

**5. Attacked and not broken**
- **D1 measurement holds.** With existence checked first, the synthesized `ValidatePipeline/outcome/rejected` (`configure_external_outcome`, then an unknown id `…98cb65e48f79`) observes `wrong-state` (`measure-d1.log`). The three forced `PlanPipeline` scenarios send the same unknown id. The target answers the forced outcome first for both commands, as ESS does.
- **D1 probes.** The two `probe-*` scenarios differ only in the expected outcome, and both pass on the interpreter. They show the interpreter answers whatever outcome the scenario expects on a Validated pipeline. They do not discriminate on their own; the synthesized scenario above is the evidence. The D1 case was never red: the base forced branch is identical, so story Decision 1 stays unimplemented.
- **D2.** Profile and adapter checks run before any insert. The type-level guards (direction, modality, handle brand) leave no other path past what the commands refuse. Mutants that move either check after the insert are caught by the unit's equality tests.
- **D3.** Wrong-typed ids are rejected at every cross-domain and same-domain lookup. No view takes an id argument. Swapping any space constant with another domain's is caught by `no_two_domains_issue_the_same_identity` or by the src pins. Overflow: `issued` touches the `clock_seq` bits at 2^48, collides inside its space at 2^63 and crosses into the next space at 2^96. No run reaches that.
- **D4.** Rust and ESS agree for `AddPort`: field names, order and outcome order (unknown-pipeline, unknown-node, not-declared). These mutants are all caught: refusal order swapped, the not-declared guard dropped, the stray-port loop removed, the pipeline filter removed.
- **D5.** The input-port mutant is caught by `check_6_an_unconnected_essential_input_is_not_a_finding`.
- **Coordinator patches.** The re-pinned adversary1/adversary8 tests are mechanical; no assertion was weakened.

**6. Paths written outside the worktree.** All are under `<unit scratch>/adversary-1/`:
- Logs: `red-pipeline.log`, `red-testkit.log`, `suite-{pipeline,testkit,runtime}.log`, `clippy.log`, `fmt.log`, `synth.log`, `base-synth.log`, `interp.log`, `mutant-port-conform.log`.
- Suites and reports: `suite.json`, `base-suite.json`, `interp.json`.
- Mutant diff: `mutant-port.diff`.
- Deleted after logging: the mutant copy `mutant-port/` with its `target/` (504M), `base/` and `show.py`.
- Lease `f8bfe626-wave10-adv1-pipeline-review-fixes` is released.

**7.**
```findings
- file: ess/domains/graph.yaml
  line: 1487
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: Port.pipeline_id is set by AddPort/added but published by no view or event, so a target recording the node's pipeline passes conform unchanged (mutant proved 54 passed, 0 failed).
- file: crates/inference-pipeline/src/validation.rs
  line: 234
  category: acceptance
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: check 6 treats an essential output that is only the sink of an output-to-output edge as connected, so that unresolved output is never named.
- file: crates/inference-pipeline/src/builder.rs
  line: 263
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: pre-existing
  message: the builder's NonZeroUsize capacity writes edges above i64::MAX that no ConnectPorts input can write, contrary to the builder's equivalence doc; no caller found.
```