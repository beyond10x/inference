---
format: aep.planning-md/3
id: review-result:adversary-deepslate-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the Deepslate Opal binding
relations:
- reviews: story:deepslate-realtime-adapter
revision: 1
---
unit: ADP-deepslate story:deepslate-realtime-adapter, unit commit c7e0ed0; adversary cases committed as 4b07507 on impl/deepslate-realtime-adapter
verdict: NEEDS-CHANGE
cases: executed 44→57, red 2
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: 5 paths in the unit's assigned scratch directory (part 6)
needs-coordinator: none

**1. Diff stat (c7e0ed0..HEAD)** — test files only:
```
 .../tests/adversary_rf1_tool_turn.rs | 212 ++++
 .../tests/adversary_rf1_wire.rs      | 249 ++++
 2 files changed, 461 insertions(+)
```

**2. Cases added.** Each test is wrapped in the fixture's 25 s `within_deadline`. Each file was first run alone:
- `adversary_rf1_tool_turn.rs` (`cargo-run.sh test -p inference-deepslate --test adversary_rf1_tool_turn`, 1 passed, 1 failed):
  - `an_answer_continued_on_its_own_turn_leaves_nothing_owed` is **RED**. The binding's own tool test has the answer continue on the call's own turn and then end. After that the server stays silent but keeps answering pings. The session fails anyway:
    `left: Some("Stalled: the provider sent nothing readable for 1019 ms, the bound is 1000 ms")`
  - `an_answer_in_a_new_turn_leaves_nothing_owed` is the control case and is green.
- `adversary_rf1_wire.rs` (`--test adversary_rf1_wire`, 10 passed, 1 failed):
  - `tool_parameters_nested_within_struct_depth_are_read` is **RED**. A tool call whose parameters nest 20 objects deep (`STRUCT_DEPTH` is 32) is refused: `left: Err(Depth)`.
  - The other 10 are green: a 10-byte max u64 packet id, a negative enum as 10 bytes, an overlong varint, a varint of 11 bytes or past 64 bits, unknown fields of every wire type inside and outside a member, a packed member passed over, a length past the end or near 2^64, empty members, 30 nested lists, and 20,000 fixed-seed arbitrary frames with no panic.

**3. Suite run** (after the cases existed): `cargo-run.sh test -p inference-deepslate --lib --tests --no-fail-fast` gave rc=101.

| target | passed | failed |
|---|---|---|
| lib | 20 | 0 |
| claims_record | 1 | 0 |
| opal_duplex | 23 | 0 |
| adversary_rf1_tool_turn | 1 | 1 |
| adversary_rf1_wire | 10 | 1 |

The before count of 44 is the three targets without my files, from this same run. Clippy `-p inference-deepslate --all-targets -D warnings` is clean (rc=0), and `fmt --check` is clean. Cargo time: 14+1+8+7 s, under 1 minute; about 5 minutes wall. Peak `target/` was 397M, and it has been discarded.

**4. Findings**
- **The tool answer stalls the session** (`adapters/inference-deepslate/src/opal.rs:1233`, CONFIRMED, introduced):
  - `submit_tool_result` calls `ledger.answer`, which marks the session as awaiting a new response.
  - In the shape the binding's own test models, the answer arrives on the same turn's response, which then completes. No response is ever created, so the session is still waiting.
  - Pongs never reach the ledger. A healthy session therefore fails as Stalled one `idle_limit` after the turn (30 s in compare).
  - What reaches it: the binding's own test (`a_tool_call_is_proposed_and_its_result_goes_back_by_id`), and the definition's `ChatMessageContent`, which carries `tool_call` and `tool_result` inside one turn's message. The docs leave this open (README open question).
  - Fix: after an answer, begin a new response on the next output of the call's turn.
- **`STRUCT_DEPTH` counts two levels per object** (`adapters/inference-deepslate/src/protocol.rs:948`, INFEASIBLE, introduced):
  - Each object counts twice (once for the Struct, once for the Value holding it), so the real limit is 16 objects. Lists count once per level.
  - A refused `ToolCallRequest` becomes an unreadable frame, so the call is never answered and the model waits.
  - What reaches it: nothing found. That needs tool arguments nested 17 or more objects deep.
  - Fix: count one level per nesting, or document 16.
- **Note, not tested:** a read timeout below the ping interval (default 10 s) is accepted by `Endpoint::url` (`config.rs:134`). A silent, healthy server then fails as Stalled. This is documented, and the defaults (30 s / 10 s) are safe.
- **Note, not tested:** an unoffered tool fails the whole session (`opal.rs:1126`). Answering with an error result would keep the call alive. This is a design choice and is documented.

**5. Attacked, not broken**
- Every field number and wire type against `realtime.proto`.
- Varint and length edge cases, and frames with no panic or huge allocation.
- ResponseBegin/ResponseEnd ordering, late audio after PlaybackClearBuffer or ResponseEnd, and audio without a turn.
- The VAD clamp (a `u64::MAX` packet id clamps to the input sent).
- The key is never in Debug or errors (`Credential` is redacted; `credential` in the Debug output is only the secret's name).
- Compare wiring (gated by `DEEPSLATE_VOICE_ID`, inherits the demo tool, reports no usage).
- The claims record matches the code. Usage is recorded as unknown.

**6. Paths written outside the worktree** (the unit's assigned scratch directory, `wave-0.17.0/ADP-deepslate/`):
- `adv1-tool-turn.log`
- `adv1-wire.log`
- `adv1-suite.log`
- `adv1-clippy.log`
- `msg.0LxhIY` (the commit message file, already deleted)

```findings
[
  {"file": "adapters/inference-deepslate/src/opal.rs", "line": 1233, "category": "acceptance", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "A tool answer continued on the call's own turn leaves the ledger awaiting a response that never begins, so a healthy silent session fails as Stalled one idle limit later (red: adversary_rf1_tool_turn::an_answer_continued_on_its_own_turn_leaves_nothing_owed)."},
  {"file": "adapters/inference-deepslate/src/protocol.rs", "line": 948, "category": "boundary", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "STRUCT_DEPTH 32 admits only 16 nested objects because a Struct and its Value each count a level, and a refused ToolCallRequest is never answered (red: adversary_rf1_wire::tool_parameters_nested_within_struct_depth_are_read)."}
]
```
