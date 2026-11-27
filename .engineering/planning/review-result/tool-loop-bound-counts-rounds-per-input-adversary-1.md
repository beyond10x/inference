---
format: aep.planning-md/3
id: review-result:tool-loop-bound-counts-rounds-per-input-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on the tool loop's bound per input (2026-11-27)
relations:
- reviews: story:tool-loop-bound-counts-rounds-per-input
revision: 1
---
Adversary, pass 1, on story:tool-loop-bound-counts-rounds-per-input, 2026-11-27, as the adversary returned it; `<scratch>` is the pass's scratch directory outside the repository.

unit: story:tool-loop-bound-counts-rounds-per-input, branch `impl/sdk-one-call`. Findings cover head `bc508a146`; my test commit `04909ab1d` sits on top of it.
verdict: CONFIRMED (one warning, a doc that is now false). No defect found in what the code does.
cases: executed 790→794, red 0
origin: introduced 6 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory, `<scratch>/adversary-1/` (full list in part 6)
needs-coordinator: none

The code holds. No case went red against it. Two lines of the change were not covered by any existing test; I added cases for both, and each case fails on the matching mutant.

## 1. Diff
`git --no-pager diff --stat` of the worktree is empty now: everything is committed. The commit `04909ab1d` (made with `b10x-gates bot`) touches test files only:
```
 crates/inference-pipeline/tests/review_fix_tool_nodes.rs  |  75 ++++++
 crates/inference/tests/adversary2_w2u1_one_call.rs        | 263 ++++++++++
```
No implementation file was changed. Base `4c9c9ec04` to `bc508a146` leaves `crates/inference-runtime` unchanged. The `native_voice` tests are unchanged too; that diff only adds lines.

## 2. Cases added
All four are green on `bc508a146`. Each was run alone before any suite run:
- `adversary2_w2u1_one_call.rs::a_chat_answers_each_text_with_four_replies_of_calls_and_ends_at_a_fifth`: 3 texts, each answered with 4 replies of 2 calls, over 1 session (24 calls). A 4th text whose 5th reply has 3 calls ends with `Closed`; 36 calls run, none of the 5th reply. PASS 0.018 s.
- `…::a_reply_of_eight_calls_and_one_to_a_tool_not_given_runs_the_eight`: 10 conversations; all 8 calls run and `send` returns Ok. PASS 0.031 s.
- `…::a_reply_whose_one_call_names_a_tool_not_given_is_answered`: tests my guess that a reply calling only an unknown tool hangs. It does not hang. PASS 0.013 s.
- `review_fix_tool_nodes.rs::proposals_naming_no_triggering_event_count_per_generation_whatever_span_they_rest_on`: through the authority's node with a bound of 4. Unmarked proposals of one generation on 5 different spans: the 5th is `exhausted`. A marked proposal beside them is authorized. PASS 0.009 s.

Red output against mutant M03 (scratch clone, this case alone):
```
assertion `left == right` failed: the four unmarked proposals and the marked one are authorized, the fifth unmarked one is exhausted
  left: [Some(Ok(())), Some(Ok(())), Some(Ok(())), Some(Ok(())), Some(Ok(())), Some(Ok(()))]
 right: [Some(Ok(())), Some(Ok(())), Some(Ok(())), Some(Ok(())), Some(Err(Some(Exhausted))), Some(Ok(()))]
exit 100
```
Red output against mutant M08b:
```
conversation 0: Err(Layer { message: "the session's run failed: FailureReason { … detail: \"a reply ended with 9 tool calls, more than the 8 the edge leaving the model's port `tool_call` holds; none of them was sent\" }" …
```

## 3. Suite run, after all four cases existed (worktree)
```
cargo nextest run -p inference-pipeline --all-features   Summary [3.043s] 170 tests run: 170 passed   exit 0
cargo nextest run -p inference-runtime                   Summary [2.034s] 287 tests run: 287 passed   exit 0
cargo nextest run -p inference --features whispercpp,llama-server,openai,openai-live,aws,audio-host,audio-device,cost,testing
                                                         Summary [3.082s] 337 tests run: 337 passed   exit 0
```
- "Before" is 169 + 287 + 334 = 790. It comes from the mutant lanes, which ran with my cases excluded.
- No test took over 5 s.
- Under load (10 runs, alongside the mutant builds), the 12 bound cases all passed 10/10. Those runs include the 30-conversation 8-call and 9-call cases and the 24-call cases.

**Mutants** (scratch clone; existing suite run fail-fast with my cases excluded):

| # | Mutant | Existing suite | My cases |
|---|---|---|---|
| M01 | count per call (`tool_authority.rs:597`) | caught | caught |
| M02 | key without the span (`:596`) | caught | caught |
| M03 | unmarked proposal counted as marked (`:814`) | **survived** | caught (pipeline case) |
| M04 | round bound uses `>=` (`llm.rs:937`) | caught | caught |
| M05 | round bound +1 | caught | – |
| M06 | end of input before failing removed (`llm.rs:940-947`) | caught: 3 FAIL at 3 s, 1 TIMEOUT at 60 s | – |
| M07 | `self.inputs.clear()` removed (`llm.rs:939`) | **survived** | – |
| M08b | round bound counts calls to tools not given (`llm.rs:932`) | **survived** | caught |
| M09 | `outcome()` sets no cause (`:1129`) | caught | caught |
| M10 | `input()` sets no cause (`:1323`) | caught | caught |
| M11 | replies check uses `>` (`tool_authority.rs:600`) | caught | caught |
| M12 | unmarked check uses `<` (`:594`) | caught (pipeline lane) | – |
| M13 | no `with_round_bound` (`run.rs:398`) | caught | – |
| M14 | `!dropped` guard removed (`llm.rs:936`) | survived; equivalent | – |

- **M07:** I found no observable effect. The inputs are dropped anyway when the node's `run` returns.
- **M14:** equivalent. No reply reaches the round-bound check once the generation is exhausted or no longer active.

## 4. Judgement findings (cover `bc508a146`)

| file:line | Finding | Verdict | Origin | Severity |
|---|---|---|---|---|
| `crates/inference-pipeline/src/lib.rs:1200` | The public doc of `FeedbackLoop` still says "a generation proposing several calls at once uses one iteration per call, not one per round of calls: the tool authority counts it so". That is false now. | CONFIRMED | introduced | warning |
| `crates/inference-pipeline/src/nodes/llm.rs:932` | Mutant M08b: no existing test covered the rule that only calls to given tools count against the round bound. Closed by `04909ab1d`. | CONFIRMED | introduced | note |
| `crates/inference-pipeline/src/nodes/tool_authority.rs:814` | Mutant M03: no existing test covered unmarked proposals of one generation on several spans. Reachable when the speech node's calls in one response fall back to a basis that changes mid-response (`duplex.rs:811-815`). Closed by `04909ab1d`. | CONFIRMED | introduced | note |
| `crates/inference/tests/oneshot.rs:821`, `:847`, `:890` | The names say `…_tool_loop_fails_an_edge_…`, and so do their doc comments. No edge fails any more: the model node fails before it sends any call. The names now state something false. | CONFIRMED | introduced | note |
| `crates/inference-runtime/src/subgraph.rs:889` | A proposal crossing a subgraph bridge gets a new `causation_id`. A chat reply of n calls would then count as n replies, and an unmarked proposal would become marked. No recipe nests a tool loop across a bridge. | INFEASIBLE | introduced | note |
| `crates/inference/tests/adversary2_w2u1_one_call.rs:599` | The new `native_voice` test never reaches the bound. It passes with no bound and on the base. No recipe-level test shows a `native_voice` response's 5th call rejected, and the duplex mock can script only one call per response. | INFEASIBLE | introduced | note |

Fix for the warning: reword `lib.rs:1199-1203` to the per-input reply count (the sentence is in `ess/domains/graph.yaml:430-450`). The fix goes back to the implementor; I did not apply it.

## 5. Attacked, could not break
- The count against the spec text: two replies on one span, a new span counted anew, a new generation counted anew, marked and unmarked counted apart (unit tests plus my cases).
- The 5th reply in a later turn ends with `Closed`, and none of its 3 calls runs.
- Replies of 9 and 24 calls: no hang, no call sent, the error names the 8. Stable over 300 conversations of each and under load.
- 8 known calls plus 1 unknown: all 8 run. A reply calling only an unknown tool is answered and does not hang.
- The error text does not race: authority and executor end cancelled, not failed.
- The approval's conditions: `crates/inference-runtime` has no diff, and the `native_voice` tests are unchanged.

## 6. Paths written outside the worktree
All under `<scratch>/adversary-1/`:
- `mut/`: scratch clone. Its `target/` (3.8 GB) is deleted; the sources stay.
- `mutants/`, `mutants-run1/`: mutant logs and the summary.
- Scripts: `suite.sh`, `mutants.sh`, `repeat.sh`, `wait-summary.sh`, `m03-check.sh`.
- Logs: `suite-worktree.log`, `suite-worktree-2.log`, `repeat.log`, `run-new-cases.log`, `run-pipeline-case.log`.
- `commit-msg.txt`.

My lease `inference-bound-adv` is ended; the worktree is left in place.

## 7.
```findings
- file: crates/inference-pipeline/src/lib.rs
  line: 1200
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the public doc of FeedbackLoop still says the authority uses one iteration per call, not one per round, which the change made false
- file: crates/inference-pipeline/src/nodes/llm.rs
  line: 932
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: counting calls to tools not given against the round bound survived the existing suite; the case added in 04909ab1d catches it
- file: crates/inference-pipeline/src/nodes/tool_authority.rs
  line: 814
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: treating an unmarked proposal as its own marked reply survived the existing suite; the pipeline case added in 04909ab1d catches it
- file: crates/inference/tests/oneshot.rs
  line: 821
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: three test names and docs (821, 847, 890) say the tool loop fails an edge, while the model node now fails before sending and no edge fills
- file: crates/inference-runtime/src/subgraph.rs
  line: 889
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: a bridge gives every crossing proposal a new causation_id, so the per-reply count breaks across a subgraph boundary; no recipe does this
- file: crates/inference/tests/adversary2_w2u1_one_call.rs
  line: 599
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: the new native_voice test never reaches the bound and passes with none; the duplex mock cannot script several calls in one response
```
