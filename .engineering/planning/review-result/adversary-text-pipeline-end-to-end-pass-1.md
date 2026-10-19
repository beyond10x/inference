---
format: aep.planning-md/3
id: review-result:adversary-text-pipeline-end-to-end-pass-1
kind: review-result
status: active
title: Adversary, story:text-pipeline-end-to-end, pass 1
relations:
- reviews: story:text-pipeline-end-to-end
revision: 1
---
```
unit: story:text-pipeline-end-to-end, working tree of impl/text-pipeline-end-to-end (base a66156b plus the uncommitted change)
verdict: NEEDS-CHANGE
cases: executed 59→70, red 5
origin: introduced 6 / pre-existing 0 / undecided 1
wrote-outside-worktree: 6 paths (part 6)
needs-coordinator: decide whether the story section "Envelopes on emitted events" binds this unit (F5); scope breach in testkit src/lib.rs (F8)
```

**1. Diff stat.** `git --no-pager diff --stat` is the same as at start: 9 tracked files, +317/−18, all the implementor's. I changed no tracked file. I added two untracked test files:
- `crates/inference-runtime/tests/adversary1_drive.rs` (507 lines)
- `crates/inference-runtime/tests/adversary1_text_pipeline.rs` (361 lines)

**2. Cases added.** Each file was run alone before any suite run. Logs: `red-drive.log` and `red-text.log`. The panic line numbers below are from before rustfmt.

| case | asserts | now |
|---|---|---|
| drive: `a_cancel_ends_a_run_whose_node_never_finishes_draining` | a cancel asked for while a node drains ends the run Cancelled | **red** |
| drive: `a_drain_asked_for_before_the_run_is_active_is_dropped` | a drain asked for at Created is dropped, as the `RunRequests` doc says | **red** |
| drive: `the_queues_of_a_run_cancelled_before_it_started_are_dropped` | a run cancelled from Validating releases its queue | **red** |
| text: `a_run_of_the_pipeline_declared_downstream_first_drains_to_completed` | Check 3 with the nodes declared sink, model, source | **red** |
| text: `a_repeated_stream_sequence_a_node_emits_on_a_run_is_refused_before_the_sink` | a node's repeated (stream_id, sequence) never reaches the sink | **red** |
| drive: 6 guards (two edges, two sources, content and "Stop" and "end", zero sources, non-source node, cancel while drain pending) | mutant killers | green |

Red output, verbatim:
```
a_drain_asked_for_before_the_run_is_active_is_dropped ... panicked at adversary1_drive.rs:474:5:
DrainRun is not admitted from Created, so the request made there was dropped (RunRequests: ...)
the_queues_of_a_run_cancelled_before_it_started_are_dropped ... panicked at adversary1_drive.rs:495:5:
a run that ended releases its edge queues (§23.3 test 7), started or not
a_cancel_ends_a_run_whose_node_never_finishes_draining ... panicked at adversary1_drive.rs:450:5:
  left: Err(Timeout)
 right: Ok((Cancelled, Ok(())))
test result: FAILED. 6 passed; 3 failed; ... finished in 5.01s
a_repeated_stream_sequence_... panicked at adversary1_text_pipeline.rs:355:5:
  left: [1, 1]
 right: [1]
a_run_of_the_pipeline_declared_downstream_first_... panicked at adversary1_text_pipeline.rs:204:9:
Check 3 on the same pipeline declared sink, model, source: the drained run never ended within 10s
(a cancel asked for afterwards ended it: false); the sink held ["s1", "s2", "s3"]
test result: FAILED. 0 passed; 2 failed; ... finished in 12.01s
```

**Mutants.** Run in a scratch copy, which I have deleted. Log: `mutants.log`. Six of seven mutants pass `run_loop` and `text_pipeline`; each one fails a guard of mine:

| mutant | unit suite | killed by |
|---|---|---|
| M1 drive.rs:50 drop `has_sources &&` | green | `a_run_without_a_source…` |
| M2 drive.rs:77 `*open = 0` | green | `a_source_feeding_two_edges…` |
| M3 drive.rs:50 `all` → `any` | green | `one_of_two_sources…` |
| M4 drive.rs:155 any envelope ends input | **caught** by text_pipeline | `content_from_a_source…` |
| M5 controller.rs:576 Draining ignores cancel | green | `a_cancel_asked_for_while_the_run_drains…` |
| M6 controller.rs create: every node counts as a source | green | `only_a_source_ends_the_input…` |
| M7 drive.rs:158 `swap` → `load` | green | `a_source_feeding_two_edges…` |

**3. Suite run** (after the cases existed): `cargo test -p inference-runtime --all-features --no-fail-fast` exit 101. 70 tests ran, 5 failed (exactly the 5 above), every other binary passed. Before = 70 − 11 of mine = 59. `cargo clippy -p inference-runtime --all-targets --all-features -- -D warnings` exit 0. `cargo fmt -p inference-runtime --check` exit 0.

**4. Findings** (all on the working tree above)

- **F1, blocker, NEEDS-CHANGE, introduced.** Check 3 deadlocks when the same pipeline is declared downstream first.
  - Measured: `adversary1_text_pipeline.rs:147`. The run never ends, and a cancel afterwards does not end it either.
  - Mechanism: `controller.rs:447` `drain()` drains nodes one at a time in declaration order. The `ModelNode` drain (`llm/node.rs:213`, `:307`) and the `TextSink` drain (`graph/nodes.rs:747`) wait until their input edge closes, which only happens when the upstream node has drained. The trait says drain "admits no new input" (`inference-pipeline/src/node.rs`); these mocks keep taking input until the edge closes.
  - What reaches it: any caller that declares nodes in a non-topological order with the typed builder. Validation accepts it. The unit's test passes only because it declares source, model, sink.
  - Fix, named not applied: drain in topological order or concurrently, and have `drive` select the cancel signal against `drain()`.
- **F2, blocker, NEEDS-CHANGE, introduced.** `drive()` (`controller.rs:573`) awaits `drain()` without watching the cancel signal.
  - Measured: `adversary1_drive.rs:439`, Timeout.
  - The doc says "unless a cancel was asked for meanwhile, which a Draining run still takes", but a cancel is only seen after every node's drain returns. Run state Draining admits CancelRun.
  - What reaches it: any node whose drain hangs; F1 is one concrete case built from the unit's own mocks.
- **F3, warning, NEEDS-CHANGE, introduced.** A drain asked for before Active is kept, then applied as soon as the run is Active.
  - Measured: `adversary1_drive.rs:467`.
  - Contradicts `drive.rs:119` ("dropped otherwise") and `:132`.
  - What reaches it: a caller that takes `requests()` after create and asks for a drain during prepare. No caller in the repo does this today.
- **F4, warning, CONFIRMED, undecided.** A run cancelled from Created or Validating keeps every edge queue alive while the controller lives.
  - Measured: `adversary1_drive.rs:491`.
  - Cause: `wind_down` skips nodes with no instance, so their `Driven.endpoints` are never cleared (only `stop()` clears them). This goes against §23.3 test 7 as the unit's `QueueWatch` doc frames it.
  - Origin is undecided because `QueueWatch` does not exist at a66156b, so I could not run the case there.
- **F5, warning, NEEDS-CHANGE, introduced.** The story section "Envelopes on emitted events" says this story's runtime puts every emitted event through the event log, so a repeated (stream_id, sequence) from a node is refused. The runtime does no such check.
  - Measured: `adversary1_text_pipeline.rs:291`. A node's duplicate sequence reaches the sink: `[1, 1]`.
  - Only the testkit mocks log, by choice of their own.
  - The unit's case `text_pipeline.rs:535` calls `run.log.record(repeated)` itself in the test body. That exercises the existing EventLog, not the run, so it cannot fail for this defect.
  - What reaches it: any `NodeImplementation` that is not a testkit mock.
- **F6, warning, CONFIRMED, introduced.** The unit's tests cannot catch six of the seven mutants (table above). `run_loop.rs` never has two edges, two sources, zero sources, a non-source node or a cancel during drain.
- **F7, note, CONFIRMED, introduced.** The `drive.rs:4` module doc says the end of input is a `stop` envelope only. A source that drops its senders, or whose `stop` its edge discards under an overload policy (`endpoint.rs:33`), is treated inconsistently: dropping never auto-drains, while a discarded `stop` still counts as sent. Control envelopes are also not logged (`graph/nodes.rs:12`). Both are stated gaps; I wrote no case for them.
- **F8, warning, CONFIRMED, introduced.** `crates/inference-testkit/src/lib.rs` is edited in the tree (+8/−4). The brief lists it as "not yours", and the implementor also wrote `scratch/testkit-lib-doc.patch`. This is a scope breach.

**5. Attacked and not broken**
- Two stops on one edge.
- A cancel taken before a drain while Active.
- A double drain or cancel request, and requests after a terminal state (`run_loop` covers these).
- `ControlKind` round-trip and case: "Stop", "end" and "" are not control kinds.
- Reusing the generator after a cancelled run: run 3 starts.
- A cancel while the drain future is pending, once it returns (guard passes).

**6. Paths written outside the worktree**, all under `<unit scratch>/adversary-1/`:
- `mutants.sh`
- `mutants.log`
- `red-drive.log`
- `red-text.log`
- `suite.log`
- `clippy.log`, `fmt.log` (counted as one path in the header)

`mutant/` (188M, with its own target/) has been deleted.

**7. Findings block**
```findings
- file: crates/inference-runtime/src/controller.rs
  line: 447
  category: concurrency
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: drain() drains nodes in declaration order while the model and sink mocks wait for upstream close, so Check 3 on the pipeline declared sink-model-source never ends and no cancel ends it
- file: crates/inference-runtime/src/controller.rs
  line: 573
  category: concurrency
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: drive awaits drain() without watching the cancel signal, so a cancel asked for while a node drains is never taken although CancelRun is admitted from Draining
- file: crates/inference-runtime/src/drive.rs
  line: 119
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a drain requested while the run is Created is kept and applied once Active, contrary to the doc saying a request the state does not admit is dropped
- file: crates/inference-runtime/src/controller.rs
  line: 484
  category: boundary
  severity: warning
  verdict: CONFIRMED
  origin: undecided
  message: a run cancelled from Created or Validating keeps every edge queue alive because wind_down never clears endpoints of nodes without an instance
- file: crates/inference-runtime/tests/text_pipeline.rs
  line: 535
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the run delivers a node's repeated (stream_id, sequence) to the sink unrefused, and the unit's case asserts the refusal by calling the log directly
- file: crates/inference-runtime/tests/run_loop.rs
  line: 1
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: six of seven mutants of the end-of-input and Draining-cancel logic survive the unit's suite and each dies on an adversary1_drive guard
- file: crates/inference-runtime/src/drive.rs
  line: 4
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: end of input is a stop envelope only, so a source dropping its senders never auto-drains while a stop discarded by an overload policy still counts as sent
- file: crates/inference-testkit/src/lib.rs
  line: 5
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the unit edited a file its brief marks not-yours instead of leaving only the scratch patch
```