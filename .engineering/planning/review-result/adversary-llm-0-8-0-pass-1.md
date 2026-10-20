---
format: aep.planning-md/3
id: review-result:adversary-llm-0-8-0-pass-1
kind: review-result
status: active
title: Adversary, story:llm-0-8-0, pass 1
relations:
- reviews: story:llm-0-8-0
revision: 1
---
unit: story:llm-0-8-0, uncommitted working tree on base 6e29e84 in `<trees>/inference-impl-llm-0-8-0`
verdict: NEEDS-CHANGE
cases: executed 1543→1552 (workspace; inference-llm 38→47), red 3
origin: introduced 7 / pre-existing 1 / undecided 0
wrote-outside-worktree: 1 directory (logs and one llm doc copy)
needs-coordinator: the story's Context says "retried by the llm clients" and its Planned scope maps ConcurrentInputOutput to `input_during_output`; findings 3 and 4 dispute both, so the story's owner decides

**1. `git --no-pager diff --stat`:** the same 13 tracked files as the implementor's change (194+/41−). I changed no tracked file. One untracked test file was added: `crates/inference-llm/tests/adversary1_llm_declarations.rs`. No non-test path was touched.

**2. Cases added** (first run, alone: `cargo test -p inference-llm --all-features --test adversary1_llm_declarations`, EXIT=101, 6 passed, 3 failed)

| line | case | now |
|---|---|---|
| :128 | a model declaring `output_streaming: false` that shows no delta, its text only in the outcome, gives one segment "Hello" at completion | RED |
| :146 | a streaming turn whose message had no delta still gives its text | RED |
| :164 | a model declaring more than the adapter claims gets a generator; input is still refused with CapabilityNotClaimed | green |
| :188 | a non-streaming model with an adapter claiming nothing gets a generator | green |
| :198 | a model that stops declaring streaming after `new` does not stream (kills the mutant in finding 5) | green |
| :218 | the refusal names the first undeclared claim | green |
| :232 | for Responses, Messages and Chat, llm's `validate_protocol` refuses `input_during_output: true`, and a declaration it accepts gets a ConcurrentInputOutput claim refused | green |
| :265 | an adapter claiming ToolCalling over a model with `tools: false` is refused | RED |
| :281 | `Rejected(Denied).output() == {"rejected":"denied"}` and `failed() == true` | green |

Red output as first captured:
```
:134 assertion `left == right` failed: llm: a message no delta was shown for carries the text the server sent
  left: []   right: [("Hello", Completed)]
:149 assertion `left == right` failed: llm: a message no delta was shown for carries the text the server sent
  left: []   right: ["Hello"]
:261 assertion `left == right` failed
  left: None   right: Some(CapabilityNotDeclared { capability: ToolCalling })
```

**3. Suite runs, all after the cases existed**

| command | exit | result |
|---|---|---|
| `cargo test -p inference-llm --all-features --no-fail-fast` | 101 | 44 passed, 3 failed (only mine); the other binaries: 1+10+1+5+7+10+4 = 38 green |
| `cargo clippy -p inference-llm --all-targets --all-features -- -D warnings` | 0 | |
| `cargo fmt -p inference-llm --check` | 0 | after I ran rustfmt on my own file |
| `cargo test -p inference-testkit --all-features --no-fail-fast` | 0 | 822 passed; conform: "test result: ok. 59 passed; 0 failed"; `ess/` untouched |
| `cargo test --workspace --all-features --no-fail-fast` | 101 | 1549 passed, 3 failed (only mine) |

The "before" count of 1543 is the after run minus my binary's 9 cases. I made no separate run with my file deselected.

**4. Findings** (they cover the working tree above)

1. **Text of a message no delta showed is lost.** `crates/inference-llm/src/lib.rs:1101`: `turn_ended` builds a segment only from delta text (`self.text`), so the text of an outcome message no delta was shown for never becomes a segment.
   - Measured: :134 and :146 are red. The same check against base 6e29e84 (scratch copy, llm 0.7.0) is red on both paths, so the origin is pre-existing.
   - What reaches it: llm 0.8.0 states "a message no delta was shown for carries the text the server sent" (`docs/design.md`, "Streamed events are final"). Its Responses client passes `contracts/responses/scenarios/kept-text-a-message-no-delta-showed-keeps-its-own-text.yaml`. Any non-streaming model, which is the story's "one segment per turn" path, can also hit it.
   - Fix to name: when a turn showed no delta, segment the outcome's assistant text.
2. **Docs quote half of llm's rule.** `lib.rs:29-31` and `docs/design/inference.md:371` say "a successful turn's text is the deltas it showed". llm's rule has a second half: a message no delta showed carries the server's text. Finding 1 is what hides behind the missing half. Origin: introduced.
3. **The retry guarantee does not exist.** `lib.rs:27-28` and `docs/design/inference.md:370` say "retrying a provider failure before any output is the llm client's, so the failure a turn returns is final". llm 0.8.0 says otherwise:
   - `crates/llm-core/src/port.rs:68`: `Model::turn` "Runs one attempt".
   - `error.rs:33-36`: `may_retry` "is the whole class decision and the caller owns visibility".
   - `docs/design.md`: "The client still makes one attempt; routing retries".
   - Retries are made only in `crates/llm-routing/src/fallback.rs:284,436`, which does not implement `Model`.

   So any `Arc<dyn Model>` given to `Generator` can return a failure that `may_retry()` allows, and the wrapper cancels on it. Origin: introduced (the story's Context says the same).
4. **ConcurrentInputOutput cannot be claimed over any llm-bound model.** `lib.rs:639` requires `input_during_output`, and llm's `validate_protocol` refuses that for every protocol (green case :232). So the claim is now only possible with models built outside llm.
   - Fixtures declare a value llm calls untrue for its clients: `crates/inference-testkit/tests/llm_model_node.rs:61`, `crates/inference-testkit/tests/adversary1_llm_node_turns.rs:71`, `crates/inference-testkit/src/llm.rs:106`, and `crates/inference-llm/tests/model_turns.rs` (the input test).
   - The wrapper still makes admitted input the next turn's text, which needs no model support. That is design §8.1 at `docs/design/inference.md:420`: "accepts additional input while emitting output".
   - The story asked for this mapping, so this is the coordinator's decision. Origin: introduced.
5. **Dead conjunct at `lib.rs:680`.** I replaced `self.model.capabilities().output_streaming &&` with `true &&` in a scratch copy. The unit's 38 cases stayed green; only my :198 failed. It is reached only by a model whose declaration changes after `Generator::new`, a case I built myself, so the verdict is INFEASIBLE. Either keep :198 or drop the conjunct.
6. **ToolCalling is not checked.** `lib.rs:640` (`_ => false`) accepts a ToolCalling claim over `tools: false`. The acceptance read literally ("an Adapter claim its model does not declare is refused") covers it; Planned scope names only the two streaming claims. Red at :265. Severity: note.
7. **The mock's "Neither" configuration is not a non-streaming model as llm defines one.** `crates/inference-testkit/src/llm.rs:105` declares `output_streaming: false`, but its turn still emits deltas at :397. So the suite never runs such a model, which is how finding 1 stayed hidden.
8. **Unrelated lockfile change.** `Cargo.lock:1171` (and 2594, 3004, 3472) moves `windows-sys` from 0.61.2 to 0.52.0 for four packages. The llm pin does not explain it.

**5. Attacked and could not break**
- The refusal in both directions: a model declaring more than claimed is accepted, and an adapter claiming nothing over a non-streaming model is accepted.
- The refusal names the first undeclared claim, as documented.
- The testkit mock declares exactly its configuration.
- The rejected-call encoding `{"rejected":"denied"}` with `failed: true` fits llm's "caller's own text in output".
- Callers of `Generator::new`: library code has one `expect`, at `crates/inference-testkit/src/llm.rs:263`, and it cannot fail because the declaration is derived from the same configuration; `proposing_round` keeps the two consistent.
- No llm gap note remains (grep).
- Conform still runs 59 cases, and the workspace has no regression outside my file.

**6. Paths written outside the worktree:** `<host path>` holds the run logs and `llm-design.md` (llm's design doc at 0.8.0). The `mutant/` and `base/` copies, including their `target/` directories, are deleted. The lease was released (session-end exit 0).

```findings
[
{"file": "crates/inference-llm/src/lib.rs", "line": 1101, "category": "acceptance", "severity": "warning", "verdict": "CONFIRMED", "origin": "pre-existing", "message": "A turn message with no shown delta (llm: carries the server's text) yields no segment on either path, so a non-streaming model can deliver nothing; red at adversary1_llm_declarations.rs:134 and :146."},
{"file": "crates/inference-llm/src/lib.rs", "line": 29, "category": "contract-drift", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "lib.rs:29-31 and design doc :371 quote only half of llm's rule; the half on messages no delta showed is missing."},
{"file": "crates/inference-llm/src/lib.rs", "line": 27, "category": "contract-drift", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "The doc says the llm client retries before output and a returned failure is final, but llm Model::turn runs one attempt and retries live only in llm-routing, which is not a Model."},
{"file": "crates/inference-llm/src/lib.rs", "line": 639, "category": "judgement", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "ConcurrentInputOutput now needs input_during_output, which llm refuses for every protocol, so no llm-bound model can back it; the fixtures pass by declaring that value."},
{"file": "crates/inference-llm/src/lib.rs", "line": 680, "category": "mutant", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "Dropping the declaration conjunct survives the unit's 38 cases; only a model whose declaration changes after new reaches it, killed by adversary1_llm_declarations.rs:198."},
{"file": "crates/inference-llm/src/lib.rs", "line": 640, "category": "acceptance", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "A ToolCalling claim over a model with tools false is accepted although the acceptance says any undeclared claim is refused; red at adversary1_llm_declarations.rs:265."},
{"file": "crates/inference-testkit/src/llm.rs", "line": 105, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "The mock's Neither configuration declares output_streaming false yet emits deltas (:397), so no test runs a non-streaming model as llm defines one."},
{"file": "Cargo.lock", "line": 1171, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "windows-sys moves from 0.61.2 to 0.52.0 for four packages, a lockfile change the llm pin does not explain."}
]
```