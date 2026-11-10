---
format: aep.planning-md/3
id: review-result:adversary-u1-duplex-turn-ids-pass-2
kind: review-result
status: active
title: 'Adversary pass 2 on duplex turn ids: Deepslate maps assistant turn ids onto user commits, the per-turn maps are never pruned'
relations:
- reviews: story:duplex-turn-ids
revision: 1
---
```
unit: story:duplex-turn-ids, worktree inference-u1-duplex-turn-ids. Findings cover 051a71e0. Cases committed on top as 06057741.
verdict: NEEDS-CHANGE
cases: executed 156→160, red 2
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 14 paths (part 6)
needs-coordinator: how Deepslate's `turn_id` should map onto VAD commits (finding 1). The provider's docs number user and assistant turns on one counter, and the correction does not.
```

The 156 is not from a separate run. It is the 160 cases that ran after the cases existed, minus the 4 new cases, counted in the same runs: testkit 92−2, Deepslate 68−2.

1. **Files changed** (`git --no-pager diff --stat 051a71e0..HEAD`). Both are test files.
```
 .../tests/adversary_u1_pass2_turn_ids.rs           | 195 ++++++
 .../tests/adversary_u1_pass2_duplex_basis.rs       | 661 +++++++++++++++++++++
 2 files changed, 856 insertions(+)
```
Commit 06057741 went through the bot route with a body. `git status` is clean.

2. **Cases added.** Each file was run alone before any suite run.

`adapters/inference-deepslate/tests/adversary_u1_pass2_turn_ids.rs`
- `a_transcription_after_its_turns_reply_began_names_its_own_commit`: **red**. The reply to user turn 0 (assistant turn 1) begins before turn 0's transcription arrives. Each transcription should name its own commit.
  ```
  panicked at adapters/inference-deepslate/tests/adversary_u1_pass2_turn_ids.rs:159:9:
  assertion `left == right` failed: the transcription of user turn 0 names the first commit, though assistant turn 1 began before it came: [('c', "turn-1", ""), ('t', "turn-2", "What time is it?"), ('c', "turn-2", ""), ('t', "turn-3", "And the date?")]
    left: "turn-2"
   right: "turn-1"
  ```
- `transcriptions_after_two_commits_name_the_commits_in_order`: **red**. Two commits arrive before any message names a turn, then turns 0 and 2 are transcribed in order.
  ```
  panicked at adapters/inference-deepslate/tests/adversary_u1_pass2_turn_ids.rs:185:9:
  assertion `left == right` failed: the earlier turn's transcription names the earlier commit: [('c', "turn-1", ""), ('c', "turn-2", ""), ('t', "turn-2", "What time"), ('t', "turn-1", "is it?")]
    left: "turn-2"
   right: "turn-1"
  ```

`crates/inference-testkit/tests/adversary_u1_pass2_duplex_basis.rs` uses the pass-1 scripted harness, copied. Both cases are **green** and close a coverage gap: nothing else reaches the fallback.
- `a_call_with_no_commit_in_the_session_rests_on_the_transcript`: with no commit (the GPT-Live shape), the call's basis is the transcript's span. It would catch removing `.or(self.basis.as_ref())` (duplex.rs:808) or the `if last` basis update.
- `a_call_of_a_response_from_before_any_commit_rests_on_the_last_committed_turn`: a response created before any commit makes its call after turn A (transcribed) and turn B (not transcribed). The basis is not A's words. It would catch removing the basis update at commit (duplex.rs:860).
- The mock always commits before it creates a response (mock.rs:783, then `end_turn`), and every pass-1 script commits before `Create`. So in every other DuplexNode case the `answers` path answers and `basis` goes unread. This was shown by reading, not by running mutants; no file under attack was mutated.

3. **Suite runs**, after the cases existed, with `--no-fail-fast`.
- `cargo test -p inference-deepslate`: lib 26, adversary_rf1_tool_turn 2, adversary_rf1_wire 11, adversary_u1_turn_ids 3, claims_record 1, opal_duplex 23, all ok. adversary_u1_pass2_turn_ids: 0 passed, 2 failed. **exit 101**.
- `cargo test -p inference-testkit --features run,duplex --test duplex_node --test adversary_w24_duplex_basis --test adversary_u1_duplex_turn_ids --test adversary_u1_pass2_duplex_basis --test conform`: 20 + 1 + 3 + 2 + 66 passed, exit 0. Conform at 66/66 shows that `try_from = "String"` does not break the synthesized scenarios.
- Clippy with `-D warnings` on both new targets: clean. rustfmt check: clean.

4. **Findings** (tree 051a71e0)

| # | file:line | what | verdict | origin |
|---|---|---|---|---|
| 1 | adapters/inference-deepslate/src/opal.rs:1136 | Assistant-turn messages (`ResponseBegin`, text, audio, `InferenceComplete`) map their assistant `turn_id` onto the newest unmapped user commit. When a reply begins before its turn's transcription, the transcription takes a fresh id, and the next commit inherits it. From then on every turn's transcript names the following turn, and each call rests on the previous turn's words. | NEEDS-CHANGE | introduced |
| 2 | adapters/inference-deepslate/src/opal.rs:1110 | `pop_back`: after two commits that no message has named, the first transcription takes the newer commit, so the two turns' transcripts are swapped. Going by commit order alone cannot satisfy both this and pass 1's case. | INFEASIBLE | introduced |
| 3 | crates/inference-pipeline/src/nodes/duplex.rs:808 | No existing case reached the fallback for a response that answers no committed turn, which is the path GPT-Live takes for every call. Two green cases close it. | CONFIRMED | introduced |
| 4 | crates/inference-pipeline/src/nodes/duplex.rs:441 | `spans` and `answers` (and `turns`) gain one entry per turn and per response and are never pruned, except the spans of ids evicted from `ahead`. They grow, transcript text included, for the life of the session. | CONFIRMED | introduced |
| 5 | adapters/inference-openai/src/duplex.rs:821 | An `AudioCommitted` with an empty `item_id` resolves to `last_item`, so it becomes a second commit of the previous turn. DuplexNode then replaces that turn's span with an empty one. | INFEASIBLE | introduced |

What reaches each one:
1. **The provider's documented order reaches it.** Deepslate's reference uses one counter for the whole conversation: its `ChatHistory` example has the user turn as `turnId: 3` and the assistant turn as `turnId: 4`. `ResponseBegin` and the other response messages carry the assistant turn id. `UserTranscriptionResult` is "Sent after the transcription worker finishes processing", so it is asynchronous. The crate's own fixture in `opal_duplex.rs:210-235` numbers turns the same way (user 0, reply 1, user 2, reply 3). At 752feaf1 the transcription arm alone claimed commits, so this case passed. Suggested fix: map only `UserTranscription` ids onto commits; for pass 1's never-transcribed commit, use the counter, so that an assistant turn M answers user turn M−1.
2. Reaching it needs two VAD commits with no response begun between them, for example a user who resumes speaking before the model answers. Not shown to happen. Severity note.
3. GPT-Live sends no `InputCommitted`, so every call takes this path in production.
4. One node holds one session, so the growth stops when the session closes. Long sessions only. Severity note.
5. OpenAI always names the item it committed. Only a malformed event triggers this.

5. **Attacked and could not break**
- A barge-in or cancelled response: `answers` records the turn committed last at its `ResponseCreated`, and the next response answers the new turn.
- A transcript that arrives after its response ended: its span is replaced and nothing else changes.
- Two commits before any response: the response answers the later turn, which follows the documented rule.
- A response that starts before its commit: no binding does this. OpenAI commits before `response.created`, Nova Sonic at the user text block, ElevenLabs, Deepgram and Phonic at the user transcript. Deepslate's order is unknown.
- A wrong fallback: every committed id has a span (the empty one at commit or one from `ahead`), so the fallback runs only when `latest` is None.
- `InputItemId` serde: nothing deserializes `SessionEvent` from records, there is no untagged enum and no `#[serde(default)]`, and conform passes.
- The other six bindings: each uses `from_provider` or `assign_item`, so none builds an empty id. The one remaining gap is finding 5.
- A Deepslate reconnect: the binding never reconnects. A new session gets fresh maps.

6. **Paths written outside the worktree**: 14, all in the unit's `adv2/` scratch directory (the directory, `docs/` with two fetched provider pages, six logs, three harness fragments, the commit message).

7. Findings block:
```findings
[
  {"file": "adapters/inference-deepslate/src/opal.rs", "line": 1136, "category": "acceptance", "severity": "blocker", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "Assistant-turn ids are mapped onto user commits, so with Deepslate's documented single turn counter and asynchronous transcription, a reply that begins before its transcription shifts every later transcript onto the following turn (red: adversary_u1_pass2_turn_ids.rs:159)."},
  {"file": "adapters/inference-deepslate/src/opal.rs", "line": 1110, "category": "boundary", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "Claiming the newest unmapped commit swaps the transcripts of two commits that no message named yet; not shown to be reached (red: adversary_u1_pass2_turn_ids.rs:185)."},
  {"file": "crates/inference-pipeline/src/nodes/duplex.rs", "line": 808, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "The fallback basis for a response that answers no committed turn, GPT-Live's only path, was reached by no case; closed by two green cases in adversary_u1_pass2_duplex_basis.rs."},
  {"file": "crates/inference-pipeline/src/nodes/duplex.rs", "line": 441, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "spans, answers and turns are never pruned, so they grow by one entry per turn and per response for the life of the session."},
  {"file": "adapters/inference-openai/src/duplex.rs", "line": 821, "category": "boundary", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "An AudioCommitted with an empty item_id becomes a second commit of the last turn, which DuplexNode answers by replacing that turn's span with an empty one."}
]
```
