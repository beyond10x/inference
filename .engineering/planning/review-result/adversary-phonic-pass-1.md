---
format: aep.planning-md/3
id: review-result:adversary-phonic-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the Phonic duplex binding
relations:
- reviews: story:phonic-realtime-adapter
revision: 1
---
unit: ADP-phonic story:phonic-realtime-adapter, branch impl/phonic-realtime-adapter. The findings cover aa769a6. The test commit is a546645.
verdict: NEEDS-CHANGE
cases: executed 20→24, red 3
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 4 paths (part 6)
needs-coordinator: finding 1 needs a design decision: what the binding does with a tool call it did not declare when an agent is named

Cargo time was 21 s over 4 runs. Wall time was about 25 min. Peak `target/` was 380M, now discarded.

**1. Diff** (`git diff --stat aa769a6..HEAD`): `adapters/inference-phonic/tests/adversary_rf1_phonic.rs | 303 +` (test file only).

**2. Cases** in `adversary_rf1_phonic.rs`, each under the fixture's 20 s deadline. I ran them alone with `cargo-run.sh test -p inference-phonic --test adversary_rf1_phonic`: 1 passed, 3 failed, rc=101.
- **`a_websocket_tool_of_the_named_agent_does_not_fail_the_session`** (:144), RED: `a tool call of the named agent's pre-defined WebSocket tool ended the session: Some(ToolNotOffered { name: "get_temperature" })`.
- **`a_caller_running_its_tool_past_the_idle_limit_is_not_a_stalled_provider`** (:192), RED: `SpeechSessionFailed … kind: Stalled, detail: "the provider sent nothing readable for 1015 ms, the bound is 1000 ms"`.
- **`the_answer_to_the_next_turn_after_a_cut_is_not_dropped_as_late_audio`** (:238), RED: `the answer to the turn after the cut opened no response; late audio dropped: 2`, `left: 1`, `right: 2`.
- **`every_chunk_is_whole_samples_within_forty_milliseconds_at_every_format`**, green. It checks all 5 formats, including pcm_44100 at 882 samples ±1 and mulaw_8000, and lengths across chunk boundaries.

**3. Suite**, run after the cases: `cargo-run.sh test --no-fail-fast -p inference-phonic --lib --test duplex --test claims_record --test adversary_rf1_phonic` gave lib 4 ok, claims_record 1 ok, duplex 15 ok, adversary 1 ok / 3 FAILED, rc=101. That is red as intended. Clippy `--all-targets -D warnings` gave rc=0, and fmt is clean.

**4. Findings**
1. **Tools of a named agent** (`src/duplex.rs:1040`, NEEDS-CHANGE, warning). Phonic's WebSocket-tools guide documents pre-defined tools listed on an agent and named by `config.agent`, and the binding offers `PhonicConfig::with_agent`. That agent's first tool call fails the session. Reached by `with_agent` plus that documented workflow. Possible fixes: answer such a call with an error `tool_call_output`, or refuse `with_agent` in combination with the agent's tools.
2. **Stall during a tool call** (`src/duplex.rs:1045`, NEEDS-CHANGE, warning). The response is opened at `tool_call` and stays in progress while the caller runs a sync tool, during which Phonic is silent. The ledger therefore counts the caller's time as provider silence, against the contract's bound "from the answer to a call". Reached by a caller that keeps draining events while a tool runs longer than `idle_limit`; `compare` answers at once and is not hit. Possible fix: complete the tool-call response once the call is proposed, as the OpenAI binding's `response.done` does.
3. **Next turn dropped after a cut** (`src/duplex.rs:987`, INFEASIBLE, note). `dropping` is cleared only by `assistant_started_speaking` or `tool_call`. The binding also decided that audio with no response in progress opens one (open question 6), so a next turn without that marker is dropped whole. No documentation shows Phonic ever omits the marker; I constructed this state. Possible fix: also clear `dropping` at `input_text`.
4. **Judgement only, no case.**
   - `compare`'s demo tool schema has no `additionalProperties: false`, while the reference says "Inline tools require strict function calling". If Phonic enforces OpenAI's strict form, the phonic arm's config is refused (`examples/realtime-voice/src/lib.rs:101`, note, undecided whether Phonic enforces it).
   - The 16,000-character truncation and the 5 s `tool_call_output_timeout_ms` are stated only in `protocol/README.md`, not in the mapping docs of `duplex.rs`.

**5. Attacked, not broken**
- The 40 ms limit at every format (test above).
- Config before ready: the reference lists `ready_to_start_conversation` first among server messages, and the SDK example sends config at once. That is consistent.
- Little-endian PCM: `Int16Array`, consistent.
- Treating `error` as fatal: the documentation does not settle it (an over-40 ms chunk is "rejected with an error"). The binding never sends one.
- Cutting at `interrupted_response`: consistent with "finished not sent if interrupted".
- The key stays out of Debug and errors, including a blank key or one with a newline.
- Usage records every count as unknown with requests 1 as measured (seen in the red output).
- A dropped session.
- `compare` wiring and billing match the other providers.
- Odd-length input frames are passed through as the OpenAI binding does; there is no frame validation in the contract.
- The claims record matches what the fixture shows, and it claims no `native_duplex`.

**6. Outside the worktree:** four files in the wave's `ADP-phonic/` scratch directory: `adv1-red.log`, `adv1-suite.log`, `adv1-suite2.log` and `msg.76mJCY` (the commit message).

```findings
[{"file":"adapters/inference-phonic/src/duplex.rs","line":1040,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"A tool_call for a pre-defined WebSocket tool of the agent named via PhonicConfig::with_agent fails the session as ToolNotOffered; red: adversary_rf1_a_websocket_tool_of_the_named_agent_does_not_fail_the_session"},
{"file":"adapters/inference-phonic/src/duplex.rs","line":1045,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The tool-call response stays in progress while the caller runs a sync tool, so the session fails as Stalled past idle_limit although Phonic is waiting for the output; red: adversary_rf1_a_caller_running_its_tool_past_the_idle_limit_is_not_a_stalled_provider"},
{"file":"adapters/inference-phonic/src/duplex.rs","line":987,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"After an interruption, a next-turn answer without assistant_started_speaking is dropped whole as late audio, contradicting the binding's own rule that audio opens a response; red: adversary_rf1_the_answer_to_the_next_turn_after_a_cut_is_not_dropped_as_late_audio"}]
```
