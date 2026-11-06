---
format: aep.planning-md/3
id: review-result:adversary-w23-refusal-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the facade session refusal event
relations:
- reviews: story:facade-session-refusal-event
revision: 1
---
unit: W23-refusal story:facade-session-refusal-event, tree inference-0-23-0-refusal, unit 112c449, adversary commit 32feac1
verdict: INFEASIBLE (one red case: real defect, but no pipeline in this repository was shown to reach it)
cases: executed 14→15, red 1
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 8 paths in the unit scratch directory (part 6)
needs-coordinator: `CHANGELOG.md` needs a `Breaking` line, because `SessionEvent` is not `#[non_exhaustive]` (events.rs has no such attribute).

**1. Diff stat** (`git --no-pager diff --stat 112c449 HEAD`):
`crates/inference-facade/tests/adversary_w23_refusal_after_cut.rs | 199 +++` (1 file). It is a test file only. No implementation file and no existing test was touched.

**2. Case added**
- File: `crates/inference-facade/tests/adversary_w23_refusal_after_cut.rs`, test `a_refusal_after_the_end_of_an_interrupted_generation_is_not_delivered`, 20 s deadline.
- What it does: a hand-built node starts generation G. The application receives `GenerationStarted`, interrupts and closes. On drain the node sends `GenerationCancelled(G)` and then `GenerationRefused(G)`.
- What it asserts: no refusal is delivered after the interrupt.
- Status: red now and red at first run. Command: `cargo-run.sh test -p inference-facade --test adversary_w23_refusal_after_cut`, rc=101, secs=1.
- Red output, verbatim: `panicked at …adversary_w23_refusal_after_cut.rs:193:9: the session delivered a refusal of the generation it interrupted, once that generation's end had released its cut: [SessionInterrupted, SessionDraining, GenerationRefused { generation_id: …, text: "I cannot help with that request." }, RunCompleted, SessionClosed]`. Event payloads are shortened here; the full output is in `adv-red3.log`.
- Cause: `Generations::admit` in `live.rs:232-249` calls `release(G)` on `GenerationCancelled`/`GenerationSuperseded`/`GenerationCompleted`. That removes G from `cut`, so a later refusal of G passes.
- Why this order can happen: `inference-llm` documents that a refusal held through a supersede or cancel is delivered after it (`lib.rs:1148-1151`, `cut()` at `:1410-1414`). The testkit node sends `GenerationSuperseded`/`GenerationCancelled` before that held refusal (`llm/node.rs:961-996`).
- What reaches it: nothing found in this repository. I also wrote a second case through the real `ModelNode`. It failed for an unrelated reason: the node fails without `ConcurrentInputOutput`, and every mock claiming that also streams. A streaming model's refusal goes out before any cancel the node takes. I deleted that case from my own file and did not report it as red. The defect is real for an adopter's node over a non-streaming adapter, which is not shown to exist, hence INFEASIBLE.
- Fix to name: keep a cut generation's `cut` mark past its end event. One way is to release a cut generation only through the `GENERATIONS_KEPT` bound. The other is for nodes to send kept refusals before the interruption event.

**3. Suite run** (after the case existed):
- Command: `cargo-run.sh test --no-fail-fast -p inference-facade --lib --test refusal --test interrupt_and_cancel --test adversary_rf1_generation_release --test adversary1_interrupt_model_node --test adversary_w23_refusal_after_cut`
- Results by target:
  - lib: 6 passed
  - adversary1_interrupt_model_node: 1 passed
  - adversary_rf1_generation_release: 2 passed
  - adversary_w23_refusal_after_cut: 0 passed, 1 failed
  - interrupt_and_cancel: 4 passed
  - refusal: 1 passed
- `error: 1 target failed`, rc=101, secs=1.
- The "before" count of 14 is the same run with my target taken out.
- Clippy `-p inference-facade --all-targets -D warnings`: rc=0, 6 s. fmt clean. Cargo time 14 s in total; wall time about 15 min.

**4. Judgement findings** (tree at 32feac1)
- `ess/domains/session.yaml:764`, CONFIRMED, introduced, note. Acceptance line 2 is met only vacuously.
  - The suite's coverage of `inference.stream.GenerationRefused` is `RefuseGeneration/outcome/refused`. It was already in `suite-before.json` (12 references to RefuseGeneration).
  - That the session delivers the refusal is stated only in a YAML comment, which neither validate nor synthesize reads.
  - The deviation itself holds: `session.yaml` declares no forwarded run event (`GenerationCancelled`, `TextDelta`, …), so a refusal declared there would be the odd one out.
- `crates/inference-facade/src/live.rs:1192-1207`, CONFIRMED, introduced, note. `deliver` records segments, transcripts and tool results in the session history, but not a delivered refusal. So the history shows a user turn with no assistant reply, while `llm`'s own conversation keeps the `AssistantRefusal` item. The story does not ask for it.

**5. Attacked and could not break**
- Refusal after partial text: the wrapper sends text, then the refusal, then usage. An interrupt before the end cuts the refusal (the unit's lib test, and its order).
- Cascade: `cascade/agent.rs:395` reaches the same `stream_session_event` (`cascade.rs:189`), so the refusal is forwarded.
- Duplex/native: the facade carries no duplex session, and the duplex contract has no refusal event. Out of this story's scope.
- Other exhaustive matches on the facade's `SessionEvent`: only the facade and the testkit, and both are updated. The examples match the duplex `SessionEvent`.
- Two generations with one refused: `ModelNode` runs one generation per run, so it is not reachable in one run.
- The unit's tests assert literal values. None of them passes on default return values.

**6. Paths written outside the worktree** (all under `<scratch>`):
- `adv-red.log`
- `adv-red2.log`
- `adv-red3.log`
- `adv-suite.log`
- `adv-clippy.log`
- `msg.i2N8oI`
- `<scratch>`, written by the harness

The tree's `target/` was emptied with `worktree discard-cache` (331 MiB).

```findings
[
{"file":"crates/inference-facade/src/live.rs","line":245,"category":"concurrency","severity":"warning","verdict":"INFEASIBLE","origin":"introduced","message":"An interrupted generation's end releases its cut, so a refusal that inference-llm delivers after a cancel or supersede reaches the application; red in adversary_w23_refusal_after_cut.rs, but no pipeline in this repository was shown to send a refusal after its generation's end."},
{"file":"ess/domains/session.yaml","line":764,"category":"acceptance","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The suite covers GenerationRefused only through the stream domain's existing RefuseGeneration scenarios, and the session's delivery of it is stated only in a YAML comment that neither validate nor synthesize reads."},
{"file":"crates/inference-facade/src/live.rs","line":1192,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A delivered refusal is not recorded in the session history, so the history shows a user turn with no reply while llm's conversation keeps the refusal item."}
]
```
