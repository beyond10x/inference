---
format: aep.planning-md/3
id: review-result:adversary-capability-planning-pass-1
kind: review-result
status: active
title: Adversary, story:capability-planning, pass 1
relations:
- reviews: story:capability-planning
revision: 1
---
unit: story:capability-planning, working tree at base 55ffd7f plus the unit's uncommitted change plus 2 adversary test files
verdict: CONFIRMED
cases: executed 384→422, red 3 (pipeline 50→55, testkit 334→367; the testkit count includes the 31 `target` module tests that `mod target;` compiles into my binary)
origin: introduced 6 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory, <unit scratch>/adversary-1/
needs-coordinator: F1 needs a call on which reading wins: the story's "no contribution means the full set" or graph.yaml's `mismatch` text. F6 is a file-ownership breach for the coordinator to rule on.

**1. Diff stat.** The tracked `git --no-pager diff --stat` is byte-identical to the one the unit handed over: 10 files, +709/−55, all of it the implementor's. My change is two untracked test files, nothing else:
```
crates/inference-pipeline/tests/adversary1_planning.rs  | 240 +++
crates/inference-testkit/tests/adversary1_planning.rs   | 140 +++
```

**2. Cases added.** Each was run alone before any suite run.

| case | asserts | now |
|---|---|---|
| pipeline `a_pipeline_whose_only_bound_adapter_lacks_its_requirement_is_not_planned` | a lone model node bound to `[output_streaming]`, pipeline requires `tool_calling`: `Mismatch{node_id: None, tool_calling}` | **red** |
| pipeline `a_node_mismatch_is_named_before_a_mismatch_of_the_pipeline` | node short of `tool_calling`, pipeline requires `native_duplex`: the node's mismatch is named | green, kills mutant A |
| pipeline `a_node_off_every_required_path_is_still_held_to_its_requirements` | an off-path node is refused as unbound, or as mismatched when bound | green |
| pipeline `of_two_refusing_nodes_the_first_declared_is_named` | the first declared node is named, for mismatch and for unbound | green, kills mutant D |
| pipeline `a_node_bound_twice_is_held_to_both_adapters` | effective = intersection of both bindings | green, kills mutant E |
| testkit `a_forced_unbound_carries_the_node_its_declared_error_requires` | a forced `unbound` error has `node_id` | **red** |
| testkit `a_forced_mismatch_carries_the_capability_its_declared_error_requires` | a forced `mismatch` error has `capability` | **red** |

Red output, verbatim:
```
panicked at crates/inference-pipeline/tests/adversary1_planning.rs:224:5:
assertion `left == right` failed: the pipeline requires tool_calling and the one adapter bound in it does not implement it
  left: Ok([OutputStreaming, IncrementalInput, ConcurrentInputOutput, NativeDuplex, RevisableInput, ToolCalling, Interruptible, ResumableModelState, AudioTextAlignment, OrchestratedDuplex])
 right: Err(Mismatch(CapabilityMismatch { node_id: None, capability: ToolCalling }))
panicked at crates/inference-testkit/tests/adversary1_planning.rs:122:5:
NodeUnbound.node_id is not Optional in ess/domains/graph.yaml: DeclaredErrorValue { error: ErrorRef(QualifiedName(inference.graph.NodeUnbound)), fields: {} }
panicked at crates/inference-testkit/tests/adversary1_planning.rs:133:5:
CapabilityMismatch.capability is not Optional in ess/domains/graph.yaml: DeclaredErrorValue { error: ErrorRef(QualifiedName(inference.graph.CapabilityMismatch)), fields: {} }
```
After rustfmt the three asserts sit at lines 230, 122 and 136. They are still red there.

**3. Suite runs** (after my cases existed, each with its own exit status, not piped):
- `cargo test -p inference-pipeline --all-features --no-fail-fast`: EXIT=101, `test result: FAILED. 4 passed; 1 failed` (adversary1_planning). Every other binary is ok, including `planning` 12 passed and doc-tests 2+3.
- `cargo test -p inference-testkit --all-features --no-fail-fast`: EXIT=101, `test result: FAILED. 31 passed; 2 failed` (adversary1_planning). `conform` reports `ok. 32 passed`, and every other binary is ok.
- `cargo clippy -p <crate> --all-targets --all-features -- -D warnings`: pipeline EXIT=0, testkit EXIT=0.
- `cargo fmt -p <crate> --check`: EXIT=1 on both at first, flagging only my two files. I ran rustfmt on those two files and both crates then gave EXIT=0.
- Mutants were run on a scratch copy of the tree. Exit status of each suite:

| mutant | pipeline suite | testkit suite |
|---|---|---|
| A: pipeline-level check moved before the node check | 0 | 0 |
| B: mismatch checked only on required-path nodes | 0 | 101 |
| C: unbound checked only on required-path nodes | 0 | 101 |
| D: nodes walked in reverse order | 0 | 0 |
| E: only the first binding of a node counts | 0 | 0 |

My cases kill all five (exit 101 on each).

**4. Findings** (all on the working tree above)

- **F1** `crates/inference-pipeline/src/planning.rs:253-259` (warning, CONFIRMED, introduced)
  - Measured: the red case above. When no source-to-sink essential path exists, effective is `Capability::ALL`, so a pipeline requiring `tool_calling` plans and reports `tool_calling` as effective. Its only bound adapter lacks it.
  - What reaches it: any pipeline without source and sink roles, including the testkit's own fixtures (`one_model` in `tests/target/graph.rs`, a lone model node). It also contradicts graph.yaml:994: "a mandatory capability … of the pipeline is not implemented by its bound adapter".
  - It matches the story's literal reading, which is why it needs your call. The fix is either to treat bound nodes as contributing when no required path exists, or to amend the story.
- **F2** `crates/inference-testkit/tests/target/graph.rs:421-440` (note, CONFIRMED, introduced)
  - Measured: two red cases. A forced `unbound` or `mismatch` that the planner does not itself decide answers the declared error with no fields. graph.yaml:475 and :466 declare `node_id` and `capability` non-Optional, and the outcome payload makes them `{generated: true}`.
  - What reaches it: every conform run, through the synthesized `PlanPipeline/outcome/unbound` and `/mismatch`. Those scenarios check the error name only, so the gap stays green. `locality-denied` has the same shape (4 fields missing).
- **F3** `planning.rs:243` against `:259` (warning, CONFIRMED, introduced)
  - Measured: mutant A survives both unit suites (exit 0), so the order "node mismatch before pipeline mismatch" is unguarded.
  - What reaches it: any pipeline where both refusals hold. My green case now guards it.
- **F4** `planning.rs:243` (note, CONFIRMED, introduced)
  - Measured: mutant D survives both suites (exit 0). The documented "first in declaration order" is unguarded for two refusing nodes.
- **F5** `planning.rs:255` (note, CONFIRMED, introduced)
  - Measured: mutant E survives both suites (exit 0). The documented "a node bound more than once contributes each binding" is unguarded.
  - What reaches it: BindNode, which has no guard against rebinding (graph.yaml:834-897).
- **F6** `crates/inference-testkit/src/lib.rs:19` and `crates/inference-testkit/tests/adversary1_target.rs:334` (note, CONFIRMED, introduced)
  - Measured: the unit brief names `src/lib.rs` as "always" not the unit's. The implementor edited both files in the tree and also left `testkit-lib-doc.patch` and `adversary1-target-other-domain.patch` in scratch.
  - The test edit rewrites an earlier adversary case (PlanPipeline changed to CreateRun) that the unit's own gate log showed red.

**5. Attacked and not broken**
- The intersection rule: the property test's oracle enumerates paths independently of `reach()`. Mutants B and C are caught, but only by the testkit suite.
- The refusal order unbound before node mismatch is guarded by `an_unbound_node_is_refused_before_a_mismatch_of_another`.
- State and existence before forced outcomes: the ess 0.60 interpreter answers a forced `mismatch` on a Declared pipeline and on a Planned one. My two authored scenarios passed 230/230. A control expecting `planned` on a Declared pipeline failed 229/230. So answering the force before checking state matches ess, and there is no finding here.
- The `ValidatedPipeline` gate: private field, no Clone, Default or Deserialize, and only `Declaration::validate` builds one. The compile_fail doctest is a one-line twin of a passing doctest.
- Field shapes: `PipelinePlanned{pipeline_id}` = graph.yaml:1356; `CapabilityMismatch` and `NodeUnbound` match :466 and :475; `Capability::ALL` has the 10 variants in model.yaml order.
- `SKIPPED.md`: the 9 removed lines equal the acceptance list and `GRAPH_PLANNING_SCENARIOS`. No `capability-planning` line remains, and conform passes while asserting unsupported == SKIPPED.

**6. Paths written outside the worktree.** All under <unit scratch>/adversary-1/ (38 entries, 5.6M):
- logs: `suite-*.log`, `clippy-*`, `fmt-*`, `red-testkit.log`, `mutant-*-{unit,adv,testkit}.log`, `interp*.log`
- ess output: `suite.json`, `interp*.json`
- scenario directories: `scen-declared/`, `scen-planned/`, `scen-control/`
- scripts: `mutate.py`, `run-mutants.sh`, `run-mutants-testkit.sh`, `show.py`

I deleted the mutant copy, its `target/` and `held/`. I released my lease (session-end).

```findings
- file: crates/inference-pipeline/src/planning.rs
  line: 253
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: with no required path the effective set is every capability, so a pipeline requiring tool_calling plans although its only bound adapter lacks it, against graph.yaml mismatch text
- file: crates/inference-testkit/tests/target/graph.rs
  line: 421
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: forced unbound, mismatch and locality-denied answer their declared error without the non-Optional fields graph.yaml gives a generated value
- file: crates/inference-pipeline/src/planning.rs
  line: 243
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: moving the pipeline-level mismatch before the node mismatch survives both unit suites; the stated refusal order is unguarded
- file: crates/inference-pipeline/src/planning.rs
  line: 244
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: walking nodes in reverse survives both unit suites; first-in-declaration-order naming of a refusing node is unguarded
- file: crates/inference-pipeline/src/planning.rs
  line: 255
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: counting only a node's first binding survives both unit suites; the documented each-binding-contributes rule is unguarded
- file: crates/inference-testkit/src/lib.rs
  line: 19
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the unit edited testkit src/lib.rs, which its brief marks always not its own, and rewrote an existing adversary case in tests/adversary1_target.rs:334
```