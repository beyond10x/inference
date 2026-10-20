---
format: aep.planning-md/3
id: review-result:final-review-graph
kind: review-result
status: active
title: Final adversary review, graph area
relations:
- reviews: story:pipeline-exposed-ports
- reviews: story:declarative-graph-definitions
- reviews: story:subgraph-declaration
- reviews: story:subgraph-execution
revision: 1
---
unit: graph area (exposures, declarative definitions, subgraph declaration and execution); tree `inference-review-graph` at c733f12 plus 3 untracked test files
verdict: NEEDS-CHANGE
cases: executed 242→251 (runtime 134→139, pipeline 108→112), red 6
origin: introduced 11 / pre-existing 0 / undecided 0
wrote-outside-worktree: 2 paths (listed in part 6)
needs-coordinator: none. The known gap (a child failure while the parent sends nothing is only seen at drain) is documented at `subgraph.rs:33-34` and is not raised again here.

**1. `git --no-pager diff --stat`**: empty, so no tracked file changed. Untracked files, all tests:
`crates/inference-runtime/tests/adversary11_subgraph_bridges.rs` (778 lines), `crates/inference-pipeline/tests/adversary11_definition_builder.rs` (320), `crates/inference-pipeline/tests/adversary11_subgraph_interface.rs` (119).

**2. Cases added.** Each red output below was captured on the case's own first run, before any suite run.

| case | asserts | now |
|---|---|---|
| runtime `a_parent_cancelled_while_its_caller_does_not_read_the_egress_fed_by_the_subgraph_ends` | the parent's cancel returns within 10 s when its egress (capacity 1, `pause`) is held and not read | red |
| runtime `a_child_ingress_under_fail_does_not_stall_the_childs_egress_bridge` | a child ingress declared `fail` with capacity 1 still lets the parent complete with 6 texts | red |
| runtime `a_subgraph_node_whose_output_feeds_two_edges_starts` | a parent that validated and planned starts its subgraph node | red |
| runtime `a_subgraph_node_whose_start_is_refused_winds_its_child_run_down` | the child's node sees Cancel and Stop after its parent's start is refused | red |
| runtime `an_envelope_crossing_into_the_child_carries_the_child_run_and_names_its_parent_counterpart` | an ingress crossing carries the child run id, the cause, the producer and the port (guard) | green |
| pipeline `a_definition_edge_and_exposure_capacity_beyond_the_ess_integer_is_held_as_the_builder_holds_it` | a definition and the builder declare the same capacity | red |
| pipeline `a_definition_declares_its_exposure_and_its_feedback_loop` | a definition's exposure and feedback loop get declared (guard) | green |
| pipeline `a_definition_edge_from_a_descriptors_output_is_not_refused_when_an_input_shares_its_name` | an edge from an output port is accepted when an input port has the same name | red |
| pipeline `two_ports_of_one_name_cannot_both_take_the_childs_one_exposure_of_that_name` | planning refuses the second port with that name (guard) | green |

Red output, verbatim:
```
panicked at adversary11_subgraph_bridges.rs:388:9: the parent's cancel did not return within 10s: the subgraph node's egress bridge waits on the full parent egress and its stop waits on the bridge
panicked at adversary11_subgraph_bridges.rs:468:9: the parent did not complete within 10s: the ingress bridge retries the full `fail` ingress in a loop that never yields, ...
panicked at :548:5 ... left: Err(NodeFailed(FailureReason { ..., detail: "the child has no egress named `out` for the port 6fc4be5a-..." })) right: Ok(())
panicked at :572:5: the child run's node is cancelled and stopped once its parent's start was refused, as the parent's nodes are: [Describe, Prepare]
panicked at adversary11_definition_builder.rs:193:5: ... left: (Some(18446744073709551615), Some(18446744073709551615)) right: (Some(9223372036854775807), Some(9223372036854775807))
panicked at :318:5: ... Err(Rejected([DefinitionFinding { defect: EdgeFromInput, node: "relay", port: Some("text") }]))
```

How the causes were checked, all in a scratch copy of the tree:
- **The two hangs:** I patched `send` in the copy so a waiting send also checks the cancel flag, and a full `fail` queue yields instead of sleeping. Both hang cases then went green, which ties both hangs to `send`.
- **Mutants the suite misses:** each of these left the shipped suite green (pipeline and testkit, including conform, for the planning one) and was caught by one of my guards:
  - `definition.rs:397`, exposures never inserted: caught at `:259`.
  - `definition.rs:412`, feedback loops never inserted: caught at `:279`.
  - `planning.rs:369`, `!matched.insert` dropped: caught by the interface guard at `:108`.
  - `subgraph.rs:325`, ingress crossing keeps the parent run id: caught by the crossing guard ("stamped with the child run's identity").

**3. Suite runs**, after the cases existed:
- `cargo test -p inference-runtime --all-features --no-fail-fast`: exit 101, 135 passed, 4 failed (my 4 red cases).
- `cargo test -p inference-pipeline --all-features --no-fail-fast`: exit 101, 110 passed, 2 failed (my 2 red cases).
- `cargo clippy -p inference-runtime -p inference-pipeline --all-targets --all-features -- -D warnings`: exit 0.
- `cargo fmt ... --check`: exit 0, after I ran rustfmt on my own 3 files.
- The "before" counts are these totals with my cases (5 runtime, 4 pipeline) taken out.

**4. Findings**, all against c733f12.

| # | file:line | verdict | what was measured | what reaches it |
|---|---|---|---|---|
| 1 | runtime/src/subgraph.rs:556 | NEEDS-CHANGE | The egress bridge's `output.send(..).await` under `pause` never checks the cancel flag. `stop_child` waits for the bridge (`:420`), so the parent's cancel hangs. | A parent egress exposure fed by a subgraph node whose caller cancels without reading it, or any downstream that stops reading without dropping its receiver. |
| 2 | runtime/src/subgraph.rs:571 | NEEDS-CHANGE | A `Full` answer is retried with `thread::sleep` in a loop that never yields. The child's drive loop and its other bridge share that thread and starve, so the run deadlocks. | Any `fail` policy on a queue the bridge sends into. The unit's own fixtures declare `fail` (tests/subgraph.rs:147 and :175). |
| 3 | runtime/src/subgraph.rs:333 (and :314) | NEEDS-CHANGE | Each endpoint takes the child's end by port name, once. A second edge on the same port finds the end gone and the start fails. | Validation allows several edges per port (`validation.rs` has no such check), and an output feeding two consumers is ordinary. |
| 4 | runtime/src/subgraph.rs:294 | CONFIRMED | On the refusals at :314 and :333 the Prepared child controller is dropped with no cancel or stop. The child run stays Preparing and `at_stop` stays None. | Only through #3. A failed parent instance is not stopped (`controller.rs:19`), so nothing winds the child down later. |
| 5 | pipeline/src/definition.rs:391 (and :402) | INFEASIBLE | `edge.capacity` and `exposure.capacity` are inserted without the builder's `carried()` clamp. A definition stores `usize::MAX` where the builder stores `i64::MAX`. | Only a JSON capacity above 9.2e18. Nothing found writes one. |
| 6 | pipeline/src/definition.rs:535 | INFEASIBLE | A port is resolved by name, first match only. An input and an output with the same name get `EdgeFromInput` for an edge the builder accepts. | No shipped descriptor repeats a port name (7 checked). `DeclareNodeDescriptor` accepts duplicates, and graph.yaml:901 leaves name uniqueness UNMAPPED. |
| 7 | pipeline/src/definition.rs:397 | CONFIRMED | Mutant "exposures not inserted" survives the suite: no test declares a definition with an accepted exposure. | Guard added. |
| 8 | pipeline/src/definition.rs:412 | CONFIRMED | Mutant "feedback loops not inserted" survives the suite. | Guard added. |
| 9 | pipeline/src/planning.rs:369 | CONFIRMED | Mutant dropping `!matched.insert` survives the pipeline and testkit suites, conform included. Two ports with one name then plan and fail at start (#3). | Guard added. |
| 10 | runtime/src/subgraph.rs:325 | CONFIRMED | Mutant stamping the parent run id on ingress crossings survives. No shipped test reads what crosses into the child. | Guard added. |
| 11 | runtime/tests/subgraph.rs:380 | CONFIRMED | The cancel test waits until both replies are committed, then cancels. "While the child streams" is never exercised, which is why #1 escaped it. | Judgement. |

How to fix, not applied:
- #1 and #2: inside `send`, race the pending send against `bridges.cancel`, and yield (return Pending) on `Full` instead of sleeping. The scratch probe of exactly this turned both cases green.
- #3 and #4: allow several endpoints per exposure name (fan out to clones of the end), or refuse fan-out at planning; and cancel the child before returning an error from `start_child`.

**5. Attacked and not broken:**
- ExposePort refusal order. Duplicate exposure names, including an ingress and an egress with the same name.
- Check 6 with exposures.
- Interface matching on direction, modality, a missing port and an extra port.
- No cycle through Planned children: state only moves forward, so a Planned pipeline cannot gain a node.
- A refused definition writes nothing and takes no identity.
- Egress crossings: run id, `causation_id` and producer.
- `NestRun` parent read back.
- Exposure ends the caller never took are dropped at the run's end.
- The interface-mismatch conform scenario is answered by the target as a forced, stand-in outcome rather than by the store. That is by design (story text); the store logic is guarded by the pipeline tests instead.

**6. Paths written outside the worktree:**
- `<host path>`
- `<host path>`
- The mutant copy `<host path>`, about 2 GiB of build output: `cargo clean`ed and removed, together with its patch files.

**7. Findings block**
```findings
- {"file": "crates/inference-runtime/src/subgraph.rs", "line": 556, "category": "concurrency", "severity": "blocker", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "the egress bridge's pending send never observes cancel, so cancelling a parent whose egress fed by the subgraph is not read hangs forever"}
- {"file": "crates/inference-runtime/src/subgraph.rs", "line": 571, "category": "concurrency", "severity": "blocker", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "a Full answer under the fail policy is retried with thread::sleep in a non-yielding loop that starves the child's drive and other bridge on the same thread, deadlocking the run"}
- {"file": "crates/inference-runtime/src/subgraph.rs", "line": 333, "category": "acceptance", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "a subgraph port with two parent edges validates and plans but its start is refused because each child end is taken by name only once"}
- {"file": "crates/inference-runtime/src/subgraph.rs", "line": 294, "category": "boundary", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "a refused start drops the prepared child run without cancelling or stopping its nodes, leaving it Preparing with no at_stop"}
- {"file": "crates/inference-pipeline/src/definition.rs", "line": 391, "category": "contract-drift", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "a definition stores edge and exposure capacities above i64::MAX unclamped where the builder holds them at i64::MAX"}
- {"file": "crates/inference-pipeline/src/definition.rs", "line": 535, "category": "boundary", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "port references resolve to the first descriptor port of the name, so an output sharing its name with an input is refused as EdgeFromInput"}
- {"file": "crates/inference-pipeline/src/definition.rs", "line": 397, "category": "mutant", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "dropping the exposure insertion of a declared definition leaves the shipped suite green"}
- {"file": "crates/inference-pipeline/src/definition.rs", "line": 412, "category": "mutant", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "dropping the feedback-loop insertion of a declared definition leaves the shipped suite green"}
- {"file": "crates/inference-pipeline/src/planning.rs", "line": 369, "category": "mutant", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "letting two ports of one name match one exposure leaves the pipeline and testkit suites, conform included, green"}
- {"file": "crates/inference-runtime/src/subgraph.rs", "line": 325, "category": "mutant", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "stamping the parent run id on envelopes crossing into the child leaves the shipped suite green"}
- {"file": "crates/inference-runtime/tests/subgraph.rs", "line": 380, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "the cancel test cancels only after both replies are committed, so cancelling while the child streams is never exercised"}
```