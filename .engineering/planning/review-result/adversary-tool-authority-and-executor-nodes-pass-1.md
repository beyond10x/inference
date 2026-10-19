---
format: aep.planning-md/3
id: review-result:adversary-tool-authority-and-executor-nodes-pass-1
kind: review-result
status: active
title: Adversary, story:tool-authority-and-executor-nodes, pass 1
relations:
- reviews: story:tool-authority-and-executor-nodes
revision: 1
---
unit: story:tool-authority-and-executor-nodes, uncommitted working tree on base ddee2e7 (branch impl/tool-authority-and-executor-nodes)
verdict: NEEDS-CHANGE
cases: executed 77→86, red 6
origin: introduced 6 / pre-existing 0 / undecided 1
wrote-outside-worktree: 1 directory (part 6)
needs-coordinator: (a) F6 is a hang in the testkit source plus a run-loop gap, not in the two new nodes; decide whether it holds this unit. (b) The implementor edited `crates/inference-testkit/src/lib.rs`, which the brief marks "not yours".

**1. Diff stat.** `git diff --stat ddee2e7` matches the implementor's 7 files. I touched none of them. My only change is one new untracked test file, `crates/inference-runtime/tests/adversary1_tool_nodes.rs` (661 lines), so no non-test path is touched. The `lib.rs` line in the stat is the implementor's edit, not mine.

**2. Cases added**, each run alone first (logs in `alone-*.log` in scratch):

| line | case | now | red output when written |
|---|---|---|---|
| 350 | one call proposed twice → authorized once, executed once | red | `left: (2, [id, id]) right: (1, [id])` |
| 388 | confirmed and executed call cannot be confirmed again | red | `left: ([id], true, [id, id]) right: ([], false, [id])` |
| 433 | enum `[1,"one"]` admits `1.0` | red | `left: [Rejected{.. SchemaInvalid}] right: [Authorized{..}]` |
| 470 | `"required": "city"` fails `prepare` | red | `prepare admitted a schema whose 'required' is a string` |
| 490 | `prepare` refuses keywords outside the subset at root, under `additionalProperties` and under `items` | green guard | — |
| 526 | executor that ignores its deadline → `deadline_exceeded` | green guard | — |
| 594 | burst of 8 over the authority→executor or executor→sink edge (capacity 1) | green guard | — |
| 608 | burst of 8 over the source→authority edge (capacity 1) | red | `run None (not ended in 10s), 2 decisions, 2 of 8 results` |
| 636 | executor adapter panics → run must not complete | red, 3 of 3 runs | `the run ended Some(Completed); the sink received ["inference.tools.ToolCallStarted"]` |

**Mutants** (scratch copy, deleted after the run, log in `mutants.log`). Each one passes the unit's 6 cases in `tool_nodes.rs` and the 2 unit tests in the pipeline crate:

| mutant | killed by |
|---|---|
| `tool_authority.rs:370` `prepare` always returns `Ok(())` | :490 |
| `tool_authority.rs:439` the `additionalProperties` walk is dropped | :490 |
| `tool_executor.rs:183` the deadline-overrun branch reports `ExecutionError` | :526 |

**3. Package gate** (inference-runtime, run after the cases existed):
- `cargo test -p inference-runtime --all-features --no-fail-fast`: exit 101. My file: `3 passed; 6 failed`. All other binaries pass: 77 = 1+9+8+3+5+2+3+7+10+9+6+5+3+6.
- `cargo clippy -p inference-runtime --all-targets --all-features -- -D warnings`: exit 0, after I changed `.last()` to `.next_back()` in my own file.
- `cargo fmt -p inference-runtime --check`: exit 0.

**4. Findings** (they cover the working tree on ddee2e7)

- **F1 (`tool_authority.rs:280`, blocker, NEEDS-CHANGE, introduced). A call can be authorized and run twice.**
  - Measured (:350, :388): the authority reads the call's state from the incoming message and keeps no record of calls it already decided. A repeated proposal with the same `call_id` is authorized again, which ESS says must answer `AuthorizeToolCall/outcome/wrong-state`. Design 699 says decisions "must not yield conflicting irreversible decisions".
  - Measured (:388): for a `non_idempotent_write` tool that requires confirmation, a stale Proposed copy stays held after the call has run. `confirm` on it returns true and the transfer runs a second time.
  - What reaches it: any proposer that sends the same call twice. A second hook answer for the same `call_id` then executes the write again.
  - Fix: track the `call_id`s already decided or held, and refuse a repeat.
- **F2 (`tool_authority.rs:478`, note, CONFIRMED, introduced). Enum check compares numbers by type.** `allowed.contains(value)` uses serde_json equality, so `1.0` is not equal to `1`. The validator's own `integer` rule accepts `3.0`, so the two rules disagree. Reached by a model that writes `1.0`.
- **F3 (`tool_authority.rs:486-489`, note, CONFIRMED, introduced). Malformed keyword values check nothing.** When `required` is not an array, or `properties` is not an object, the keyword is silently ignored. That contradicts the module doc at :17 ("fails preparation rather than being validated in part"). Reached by a typo in a tool declaration.
- **F4 (`mod.rs:247` with `tool_executor.rs:251`, blocker, NEEDS-CHANGE, introduced). A panicking executor loses the call silently.**
  - Measured (:636): when the executor adapter panics, `run()` never records a failure and `let _ = thread.join()` discards the panic. `drain` returns Ok, the run ends Completed and the call stays Started with no result or failure (design 687 says no silent loss).
  - What reaches it: any adapter bug. By reading only (not run), a deadline of `Duration::MAX` reaches the same path, because `Instant::now() + deadline` at `tool_executor.rs:178` panics on overflow.
  - Fix: turn a join `Err` into a node failure.
- **F5 (`tool_authority.rs:20-22`, warning, CONFIRMED, introduced, judgement).** The confirmation hook can only say yes. A host that declines has no way to reject (`denied`), so the held call stays Proposed and `drain` blocks until the run is cancelled. Design §21.2 asks for confirmation hooks. Not made into a case, because no spec gives drain a deadline.
- **F6 (testkit `graph/nodes.rs:617-619` and `drive.rs:10-11`, warning, CONFIRMED, undecided). Overflowing the source edge hangs the run.**
  - Measured (:608): with the source→authority edge at capacity 1, the source records its send failure and goes on. Its `stop` is lost, and the run loop never drains (a gap `drive.rs` documents). Result: 2 of 8 calls answered and the run never ends.
  - The same code is at base (`git show ddee2e7:…/nodes.rs:494-500`), but I did not run it there, so the origin stays undecided.
- **F7 (`crates/inference-testkit/src/lib.rs`, note, CONFIRMED, introduced, judgement).** The brief puts this file under "not yours" and asks for `<scratch>/testkit-lib-doc.patch` instead. The tree edits the file directly (doc lines only).
- **F8 (`tool_executor.rs:13-15`, note, CONFIRMED, introduced, judgement).** The doc says a cancelled node "sends nothing more". But `within` polls the future before checking the cancel flag, and an executor that follows the contract answers `cancelled`. The node then runs `FailToolCall(cancelled)` and sends it. Read only, not run, and timing-dependent.

**5. Attacked and not broken:**
- The six outcome ids and the event names match `ess/domains/tools.yaml` (outcomes at :445-558, events at :586-627).
- A tool that requires confirmation is never authorized without the hook. The flag is read at :249 and checked before state, in ESS order.
- A confirm for a call that is not held is refused (unit case).
- Every step at the sink carries the `call_id` and `generation_id`.
- The executor's own deadline holds even when the executor ignores it (:526).
- Overload `fail` on the two tool edges either delivers everything or fails the run explicitly (:594).
- `additionalProperties: false`, nested `items`/`properties`, and an integer of `3.0` behave as JSON Schema does. A `required` entry naming an undeclared property under `additionalProperties: false` is unsatisfiable, which is correct.

**6. Paths written outside the worktree:** `<unit scratch>/adversary-1/` (`alone-*.log`, `alone3-*.log`, `mut-*.log`, `mutants.sh`, `mutants.log`, `suite.log`, `clippy.log`, `fmt.log`). The mutant copy `…/adversary-1/mut/` (307M, with its target) is already deleted.

```findings
- file: crates/inference-pipeline/src/nodes/tool_authority.rs
  line: 280
  category: concurrency
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the authority keeps no record of decided call_ids, so one call proposed twice is authorized and executed twice and a held stale copy lets the hook confirm a non-idempotent write a second time
- file: crates/inference-pipeline/src/nodes/tool_authority.rs
  line: 478
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: enum membership uses serde_json equality, so 1.0 is refused against enum [1] while the subset's integer type admits 3.0
- file: crates/inference-pipeline/src/nodes/tool_authority.rs
  line: 486
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a malformed required or properties value is ignored and prepare admits the schema, contrary to the module doc's no-partial-validation promise
- file: crates/inference-pipeline/src/nodes/mod.rs
  line: 247
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a panicking executor adapter leaves the call Started with no outcome and the run Completed because the join error is discarded and no failure is recorded
- file: crates/inference-pipeline/src/nodes/tool_authority.rs
  line: 20
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the confirmation hook has no deny path, so a declined call stays Proposed and drain blocks until cancel
- file: crates/inference-testkit/src/graph/nodes.rs
  line: 617
  category: concurrency
  severity: warning
  verdict: CONFIRMED
  origin: undecided
  message: a burst over the source-to-authority edge at overload fail loses the source's stop and the run never ends, with 2 of 8 calls answered
- file: crates/inference-testkit/src/lib.rs
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the implementor edited a file the brief marks not yours instead of leaving a patch in scratch
- file: crates/inference-pipeline/src/nodes/tool_executor.rs
  line: 13
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the doc says a cancelled node sends nothing more, but an executor that answers cancelled is polled first and its FailToolCall(cancelled) is sent
```