---
format: aep.planning-md/3
id: review-result:adversary-llm-wrapper-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the llm wrapper outcomes and Debug
relations:
- reviews: story:llm-wrapper-outcomes-and-debug
revision: 1
---
unit: RF-llm, story:llm-wrapper-outcomes-and-debug, unit head 146303d; my cases are in commit e0b2288 on impl/llm-wrapper-outcomes-and-debug
verdict: NEEDS-CHANGE
cases: executed 66→71, red 2
origin: introduced 1 / pre-existing 1 / undecided 0
wrote-outside-worktree: 7 paths in the unit scratch directory (part 6)
needs-coordinator: none

Cargo time 0.3 min (10 s, 3 s, 3 s). Wall time about 25 min. Peak `target/` was 314M; `worktree discard-cache` has run.

1. `git --no-pager diff --stat 146303d HEAD`: `crates/inference-llm/tests/adversary_rf1_outcomes.rs | 403 +` (test file only). The work tree is clean after commit e0b2288 (`b10x-bot[bot]` is author and committer).

2. Cases in `crates/inference-llm/tests/adversary_rf1_outcomes.rs`, each with a 20 s deadline. Run alone with `cargo-run.sh test -p inference-llm --test adversary_rf1_outcomes`: 3 passed, 2 failed, rc=101 secs=10.
   - `a_tool_round_then_a_truncated_turn_completes_truncated`: green. A tool-calls turn then a max-tokens turn completes with `MaxOutputTokens`.
   - `a_truncated_turn_then_an_input_turn_completes_with_the_later_reason`: green. Max-tokens then end-turn completes with `EndTurn`.
   - `text_and_a_refusal_in_one_turn_keep_their_order_and_stay_apart`: green, streamed or not and streaming or not. One segment, order delta < commit < Refused < Usage, and `RefusalDelta` text never becomes a segment. It catches a mutant the unit's suite misses: Refused queued before the turn's text, since the unit's refusal test has no text.
   - `the_refusal_of_an_ended_turn_survives_a_later_failed_turn`: **red** at :362 for `[ConcurrentInputOutput]` only. The non-streaming handle returned `[Usage, Usage, Cancelled]` with no Refused. The streaming run of the same case passes.
   - `a_failed_turn_keeps_the_final_mark_the_provider_gave_its_counts`: **red** at :396. The record has counts 40/9 `ProviderReported` but `final: false`.

3. Suite: `cargo-run.sh test -p inference-llm --features tools --lib --test` with all 11 targets named, `--no-fail-fast`, rc=101 secs=3.
   - Every existing target is green: lib 0, adversary1_commit_after_complete 1, adversary1_cut_turn 9, adversary1_llm_declarations 9, adversary1_turn_sequencing 10, adversary2_offer_override 1, adversary2_release 5, generation_output 7, model_turns 17, tool_spec 1, wrapper_outcomes 6.
   - adversary_rf1_outcomes: 3 passed, 2 failed.
   - 66 executed without my target, 71 with it.
   - Clippy `-D warnings` is clean (rc=0) and fmt is clean.

4. Findings (all on tree e0b2288):
   - **N25 fix is incomplete.** `crates/inference-llm/src/lib.rs:1348` passes a hard-coded `false`. `ess/domains/usage.yaml:24,201` says `final` follows the provider's mark ("False when the provider marked its counts partial"). `cut_turn_ended` at `:1414` keeps the mark on an `Err`, so the two paths N25 named still disagree. The unit's own test (`wrapper_outcomes.rs`, "never final, whatever the observation marked") asserts the contradiction.
     - What reaches it: `llm` attaches final observations to errors in llm-responses `stream.rs:637` (`observation(.., true)` then a deferred refusal) and llm-routing `fallback.rs:508` (a validated outcome refused).
     - Fix: pass `observation.final_usage && observation.usage.is_some()`, as the cut path does.
     - Verdict NEEDS-CHANGE, origin pre-existing: at base the counts were dropped as well, so the case fails there too.
   - **Refusal lost on a later failure, non-streaming only.** `keep_held_usage` at `lib.rs:1360` keeps only Usage from `held`, so a Refused of a turn that ended is dropped without trace when a later turn fails. A text segment dropped the same way still shows up as `SegmentDiscarded`; a refusal has no such record. A streaming handle delivers the refusal in the same situation.
     - What reaches it: an adapter claiming `ConcurrentInputOutput` without `OutputStreaming`, for example through the testkit agent node (`cascade/agent.rs:562`). No adapter record in this repository makes that claim.
     - Verdict INFEASIBLE, introduced.
   - **GenerationRefused is never checked by the conform suite.** `ess/domains/stream.yaml:1018` declares `GenerationRefused` and no command emits it. So the suite never observes it, and the target mapping at `crates/inference-testkit/tests/target/stream.rs:656` is never exercised. Note, CONFIRMED, introduced judgement; it rides with the second finding and is not a separate block row.

5. Could not break:
   - The stop reason comes from the right turn: the last turn, several turns, no turn (None), and a cut turn (None, as documented).
   - Refused always comes before Completed; stream.yaml has no invariant that orders them.
   - The handle's Debug shows only counts and ids; the failure shows only its code.
   - `ess specify validate` passes (0.88.0). The synthesized suite drives `CompleteGeneration` with `stop_reason: end_turn` and checks the payload, so the testkit conform target does check `stop_reason`, though only the `end_turn` value.
   - No caller in the repository is left unmigrated: both testkit forwarders handle Refused and use a wildcard for the `#[non_exhaustive]` enum.

6. Written outside the worktree, all under `.cache/scratch/9f5a4edc-f114-47a7-863c-55ffc5ec02c6/wave-0.17.0/` in the home directory:
   - `RF-llm/scratch/adv-suite.json`
   - `RF-llm/scratch/adv-rf1-case.log`
   - `RF-llm/scratch/adv-rf1-suite.log`
   - `RF-llm/scratch/adv-rf1-clippy.log`
   - `RF-llm/scratch/msg.K1Zy1q`
   - `.session-cargo.lock` (the shared wrapper lock)
   - The harness also wrote background-task output files under `.cache/claude-tmp/`.

   I did not take a worktree session lease of my own.

```findings
[{"file":"crates/inference-llm/src/lib.rs","line":1348,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"pre-existing","message":"turn_failed hard-codes final=false where usage.yaml:201 and cut_turn_ended keep the provider's final mark, so N25's two paths still disagree; red: adversary_rf1_outcomes::adversary_rf1_a_failed_turn_keeps_the_final_mark_the_provider_gave_its_counts"},
 {"file":"crates/inference-llm/src/lib.rs","line":1360,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"keep_held_usage drops the Refused of an ended turn for a non-streaming adapter when a later turn fails, while a streaming handle delivers it; red: adversary_rf1_outcomes::adversary_rf1_the_refusal_of_an_ended_turn_survives_a_later_failed_turn"}]
```
