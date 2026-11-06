---
format: aep.planning-md/3
id: review-result:adversary-elevenlabs-multi-context-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the ElevenLabs multi-context connection
relations:
- reviews: story:elevenlabs-multi-context-tts-connection
revision: 1
---
unit: ADP-elevenlabs story:elevenlabs-multi-context-tts-connection, commit 9a0f46f plus adversary commit cb8e017 (branch impl/elevenlabs-multi-context-tts-connection)
verdict: NEEDS-CHANGE
cases: executed 81→88, red 2
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: 5 paths in the unit scratch directory (part 6)
needs-coordinator: whether the live endpoint sends `is_final` and whether it closes an idle multi-stream connection; this needs a billed key, which I did not use

**1. Diff against 9a0f46f:** one file, `adapters/inference-elevenlabs/tests/adversary_rf1_multi_context.rs`, +501 lines. It is a test file only, committed as cb8e017 by `b10x-bot[bot]` as author and committer.

**2. Cases**, run alone with `cargo-run.sh test -p inference-elevenlabs --test adversary_rf1_multi_context`: 5 passed, 2 failed (rc=101, 5 s).
- RED `adversary_rf1_a_final_message_spelled_as_the_guide_reads_it_ends_the_segment`: `left: (Some(Protocol("an object with neither `audio` nor `isFinal`")), Some(Queued)) right: (None, Some(Synthesized))`
- RED `adversary_rf1_a_connection_closed_between_segments_does_not_fail_the_next_segment`: `left: (Some(Transport("IO error: Broken pipe (os error 32)")), 0, Some(Queued)) right: (None, 1, Some(Synthesized))`
- Green:
  - late audio of a cancelled context, interleaved with the next context's audio, never leaks into its frames, and the clock continues at 80 and 160;
  - a dropped handle sends `close_socket`, both between segments and right after a cancel while the cancelled context streams, within 2 s;
  - a cancelled context that keeps streaming does not hold a silent next context past the read timeout (stalls in under 3 s). This case also catches a mutant that would move the deadline on cancelled messages, which the existing adversary1 streaming case does not;
  - six barge-ins in a row, no cancelled context ever finishing: the seventh segment is spoken clean on the same connection.

**3. Suite**, run after the cases existed: `cargo-run.sh test -p inference-elevenlabs --features duplex --no-fail-fast --lib` plus all 13 test targets by name. rc=101, 8 s.
- Counts: lib 12, adversary1_attack 13, adversary2_agent_attack 3, agent_duplex 22, auth 4, cancel 3, claims_record 2, closed_stream 11, conformance 6, first_audio 3, fixtures 2, one_connection 2, output_formats 5, all passing.
- adversary_rf1_multi_context: 5 passed, 2 failed.
- "Before" is 81: the same run with my 7 cases deselected. The duplex agent binding still passes (22 + 3).
- Clippy on the new target with `-D warnings` and `cargo fmt --check`: clean.

**4. Findings**
- **`is_final` spelling** (src/protocol.rs:302). NEEDS-CHANGE, introduced, warning.
  - **Measured:** tests/adversary_rf1_multi_context.rs:335. A final message `{"is_final":true,"contextId":"s0"}` fails the whole synthesis as a protocol error.
  - **What reaches it:** the vendor's two pages contradict each other. The AsyncAPI page says `isFinal`; the multi-context guide reads `is_final` in both its Python and its JavaScript example. Which one the server actually sends is not verified. Note 118 records the choice, but a wrong guess fails every segment.
  - **Fix:** accept either spelling as the final flag.
- **Connection closed between segments** (src/synthesiser.rs:269, `speak` reuses the connection unconditionally). NEEDS-CHANGE, introduced, warning.
  - **Measured:** tests/adversary_rf1_multi_context.rs:374. A connection the server closes between segments fails the next segment as `Transport`, and the segment is never spoken.
  - **What reaches it:** the testkit's cascade speech node (`crates/inference-testkit/src/cascade/speech.rs:5`) keeps one synthesis "until the end of input", across turns. The per-segment binding could not fail this way. The documentation does not say whether the server closes an idle connection; contexts time out after 20 s.
  - **Fix:** reopen the connection and resend the context when the failure comes before any message of the new context.
- **Judgement (no test):** cancel stops nothing on the server. `flush` and `close_context` are already sent with the text, and the documentation says a flushed context "will continue flushing". So a barge-in still pays for generating the whole cancelled segment. The story's planned scope, "Map cancel to `close_context`", is met only on the client side. Nothing in the documentation offers a fix.

**5. Attacked, not broken:**
- Protocol against the current docs: re-fetched on 2026-11-06, byte-identical to the implementor's saved copies. Nothing the binding sends contradicts the AsyncAPI page.
- A message without `contextId` fails the synthesis. This is the documented decision and was already tested by the implementor.
- The five-context cap: the server's answer is undocumented, so it cannot be reached in a fixture. The six-barge-in case passes against a server with no cap.
- Usage on the new path: the record arrives once, last and final, after a cancel.
- First audio: clock positions continue across a cancel.

**6. Paths outside the worktree** (all in the unit scratch directory):
- `ADP-elevenlabs/adv1-red.log`
- `ADP-elevenlabs/adv1-suite.log`
- `ADP-elevenlabs/adv1-clippy.log`
- `ADP-elevenlabs/msg.j2mp8l`
- `ADP-elevenlabs/adv1-docs/` (api.md, guide.md)

Cargo time was 18 s and wall time about 15 min. The tree's cache is discarded and my session lease is released.

```findings
[
  {"file": "adapters/inference-elevenlabs/src/protocol.rs", "line": 302, "category": "contract-drift", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "A final message spelled is_final, as the vendor's multi-context guide reads it, fails the whole synthesis as a protocol error; red test adversary_rf1_a_final_message_spelled_as_the_guide_reads_it_ends_the_segment."},
  {"file": "adapters/inference-elevenlabs/src/synthesiser.rs", "line": 269, "category": "acceptance", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "A shared connection the server closes between segments fails the next segment instead of reconnecting, a failure the per-segment binding could not have, reachable through the cascade speech node's synthesis that lasts across turns; red test adversary_rf1_a_connection_closed_between_segments_does_not_fail_the_next_segment."}
]
```
