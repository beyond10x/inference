---
format: aep.planning-md/3
id: review-result:adversary-u1-duplex-turn-ids-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on duplex turn ids: a late transcript is not its response''s basis, Deepslate''s oldest-commit claim, an empty id deserializes'
relations:
- reviews: story:duplex-turn-ids
revision: 1
---
```
unit: story:duplex-turn-ids, worktree inference-u1-duplex-turn-ids, findings cover 32e08ee8 (adversary cases committed on top as 752feaf1)
verdict: CONFIRMED
cases: executed 266→275, red 4
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 12 paths (part 6)
needs-coordinator: whether a call belongs to the turn its response answers (finding 1), and how Deepslate's turn_id should map onto VAD commits (finding 2)
```

The 266 is not from a separate run. It is the 275 cases that ran after my cases existed, minus my 9 cases, counted in the same runs. I had no `cases:` line from the implementor.

**1. Files I changed** (`git --no-pager diff --stat 32e08ee8..HEAD`). All four are test files, so the test-files-only rule held.
```
 .../inference-aws/tests/adversary_u1_nova_item.rs  | 104 ++++
 .../tests/adversary_u1_turn_ids.rs                 | 206 +++++++
 .../inference-duplex/tests/adversary_u1_item_id.rs |  35 ++
 .../tests/adversary_u1_duplex_turn_ids.rs          | 686 +++++++++++++++++++++
 4 files changed, 1031 insertions(+)
```
Commit 752feaf1 (`test(duplex): adversary cases on duplex turn ids`) went through `b10x-gates bot`. `git status` is clean.

**2. Cases added.** The red output below is from running each file alone, before any suite run.

`crates/inference-testkit/tests/adversary_u1_duplex_turn_ids.rs` uses a scripted provider over `SessionLedger`. Each step waits until the node has appended a given number of frames.
- `a_transcript_ahead_of_its_commit_stays_the_basis_of_the_call_after_the_commit`: **green**. No other test reaches this branch. It closes the mutant that drops the `ahead` check in the `InputCommitted` arm.
- `a_second_turns_transcript_ahead_of_its_commit_is_its_calls_basis_after_an_untranscribed_turn`: **green**. Same branch, after an untranscribed turn.
- `a_call_of_the_first_turns_response_rests_on_its_late_transcript_after_a_second_commit`: **red**.
  ```
  panicked at crates/inference-testkit/tests/adversary_u1_duplex_turn_ids.rs:678:5:
  assertion `left == right` failed: the call of the first turn's response rests on the first turn's transcript, not on the empty span committed at the second turn's commit
    left: SpanId(beb3d638-15dc-45f0-8483-c1f06384fc1b)
   right: SpanId(b0076cdc-8432-4cff-aa06-3b0a4a15e6d4)
  ```
  After rustfmt the assertion is at :680.

`adapters/inference-deepslate/tests/adversary_u1_turn_ids.rs`:
- `a_commit_never_transcribed_does_not_take_the_next_turns_transcription`: **red**.
  ```
  panicked at adapters/inference-deepslate/tests/adversary_u1_turn_ids.rs:122:9:
  assertion `left == right` failed: the transcription that follows the second commit names the second commit, not the first one, whose transcription never came: commits ["turn-1", "turn-2"]
    left: ["turn-1"]
   right: ["turn-2"]
  ```
- `a_second_transcription_of_one_turn_id_names_the_same_commit`: **green**. It closes the mutant that drops the `user_turns` lookup.
- `a_transcription_before_its_commit_names_the_turn_the_commit_carries`: **green**. No other test reaches the binding's `ahead` path.

`crates/inference-duplex/tests/adversary_u1_item_id.rs`:
- `an_empty_item_id_is_not_read_from_its_serialized_form`: **red**.
  ```
  panicked at crates/inference-duplex/tests/adversary_u1_item_id.rs:10:5:
  an input item id is never empty, yet "" reads as Ok(InputItemId(""))
  ```
- `an_input_committed_naming_an_empty_item_is_not_read`: **red**.
  ```
  panicked at crates/inference-duplex/tests/adversary_u1_item_id.rs:22:5:
  a commit names its user turn by a non-empty id, yet one naming "" reads as Ok(InputCommitted { session_id: SpeechSessionId(021563c1-…), item: InputItemId("") })
  ```

`adapters/inference-aws/tests/adversary_u1_nova_item.rs`:
- `a_user_turn_is_named_by_its_text_blocks_content_id`: **green**. It asserts that the item is the replay's `text-N` contentId. The unit's own test (`nova_sonic_duplex.rs:333-340`) only asserts that the commit and the transcript carry the same id. That test stays green if the commit uses `assign_item()` instead of the contentId.

**3. Suite runs.** These ran after the cases existed, all with `--no-fail-fast`. Every run exits 101, and only my red cases fail. All existing cases pass.
- `cargo test -p inference-testkit --features run,duplex --test duplex_node --test adversary_w24_duplex_basis --test adversary_u1_duplex_turn_ids --test conform`: 89 passed, 1 failed (conform 66/66, duplex_node 20/20), exit 101.
- `cargo test -p inference-duplex`: 92 passed, 2 failed (both in `adversary_u1_item_id`), exit 101.
- `cargo test -p inference-deepslate`: 65 passed, 1 failed (`opal_duplex` 23/23), exit 101.
- `cargo test -p inference-aws --features duplex --test nova_sonic_duplex --test adversary_u1_nova_item`: 25 passed, exit 0.
- `cargo clippy -D warnings` on each new test target is clean. `ess specify validate --path ess` reports "valid", with no warning on the changed nouns.

**4. Findings** (tree at 32e08ee8)

| # | file:line | what | verdict | origin |
|---|---|---|---|---|
| 1 | crates/inference-pipeline/src/nodes/duplex.rs:873 | A transcript sets the basis only if its turn is the last committed one (`if last`). So a call from turn 1's response, proposed after turn 2 was committed and after turn 1's late transcript arrived, rests on turn 2's empty span. | NEEDS-CHANGE | introduced |
| 2 | adapters/inference-deepslate/src/opal.rs:1172 | A transcription with a new `turn_id` takes the oldest unclaimed commit. This is the queue the story's Context calls the defect. One commit that is never transcribed shifts every later transcript onto the turn before its own, and those transcripts are then never the basis. | CONFIRMED | introduced |
| 3 | crates/inference-duplex/src/lib.rs:105 | `#[serde(transparent)]` Deserialize bypasses `new`, so `""` reads as an id, and `new` only debug-asserts. The doc says "Never empty", but the ESS type has no such constraint. | CONFIRMED | introduced |

What reaches each one:
1. The OpenAI binding's default config has `auto_interrupt: false` (`adapters/inference-openai/src/duplex.rs:157-174`), and ESS has a scenario for this case: `input-during-output-continues-response`. With that setting, turn 2 is committed while response 1 is still running. That OpenAI's transcription arrives late is the story's own premise, inferred from code and not observed. Origin is from reading the base (`git show ff0ba010`, :842-852): there the late transcript became the basis, so this case's span-id assertion would have held, though on the wrong turn. The case cannot compile on the base. Whether "a call's own turn" means the turn its response answers is for you to decide. Fix: record the basis per response when it is created, and update it when that turn's transcript arrives.
2. I could not show at runtime that Deepslate drops a transcription, for example on a noise-triggered VAD turn. The story's Outcome requires correctness "when a provider ... never sends one". The scoper had flagged the mapping as an open question, and you need to decide it. Claiming the newest unclaimed commit fixes this case but breaks transcripts that arrive late. Mapping through `turn_id` needs to know how the counter is numbered.
3. Bindings use `from_provider`, which filters out empty ids, so no binding produces one. Only something reading serialized events would, such as the conform target via `named`. Severity is note. Fix: a custom `Deserialize` that rejects empty strings, a checked constructor, or a non-empty constraint in ESS.

**5. What I attacked and could not break**
- Acceptance as the unit wrote it (out of order plus one missing, calls after each turn): holds. Its `!=` assertions are weak because empty spans are never emitted, but its `bases[2] != first` assertion catches the `if last` mutant.
- OpenAI manual-commit aliasing and refused-commit pop: consistent. A server VAD commit racing a caller commit gets the wrong alias, but the old counter had the same race.
- Ledger ids across sessions: one counter per ledger, one session per node, no shared state.
- ESS validate and conform 66/66: pass.
- GPT-Live: each fragment goes through `ahead`, capped at 64. Same basis behaviour as before.
- Mutants were checked by reading the code against my green cases, not by running them. Your instructions allowed mutating files in the worktree, but my procedure forbids mutating a file under attack even briefly, so I mutated nothing.

**6. Paths written outside the worktree**, all in the unit's scratch directory `adv1/`: `testkit-red.log`, `duplex-red.log`, `deepslate-red.log`, `nova-red.log`, `suite-testkit.log`, `suite-duplex.log`, `suite-deepslate.log`, `suite-aws.log`, `clippy-a.log`, `clippy-b.log`, `commit-msg.txt`, `deepslate-part1.rs`. The directory itself was also created by me. Lease `inference-u1-adv1` is released.

**7. Findings block**
```findings
[
  {"file": "crates/inference-pipeline/src/nodes/duplex.rs", "line": 873, "category": "acceptance", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "A late transcript of a turn that is no longer the last committed one never becomes the basis, so a call of that turn's still-running response rests on the next turn's empty span (red: adversary_u1_duplex_turn_ids.rs:680)."},
  {"file": "adapters/inference-deepslate/src/opal.rs", "line": 1172, "category": "acceptance", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "Claiming the oldest unclaimed commit is the queue the story names as the defect: one untranscribed VAD commit shifts every later transcript onto the previous turn (red: adversary_u1_turn_ids.rs:122)."},
  {"file": "crates/inference-duplex/src/lib.rs", "line": 105, "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "InputItemId is documented never empty, but serde transparent deserialization accepts an empty string and new only debug-asserts (red: adversary_u1_item_id.rs:10 and :22)."}
]
```
