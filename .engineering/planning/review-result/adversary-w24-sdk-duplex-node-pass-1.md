---
format: aep.planning-md/3
id: review-result:adversary-w24-sdk-duplex-node-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the duplex node
relations:
- reviews: story:sdk-duplex-node
revision: 1
---
unit: W24-sdk-duplex-node story:sdk-duplex-node, unit head ee5f42f, findings cover e54284f (tree inference-0-24-0-sdk-duplex-node)
verdict: CONFIRMED
cases: executed 18→19, red 1
origin: introduced 1 / pre-existing 0 / undecided 0
wrote-outside-worktree: 7 paths in the unit scratch dir (part 6)
needs-coordinator: whether the late-turn ordering below is reached by a live provider (inferred, not observed)

**1. `git --no-pager diff --stat ee5f42f HEAD`**
```
 .../tests/adversary_w24_duplex_basis.rs | 550 +++++
 1 file changed, 550 insertions(+)
```
I changed one file, a test file. Commit e54284f went through the bot route: author and committer are both `b10x-bot[bot]`. Nothing was pushed.

**2. The case I added**
- File: `crates/inference-testkit/tests/adversary_w24_duplex_basis.rs`.
- Test: `a_call_on_a_later_turn_does_not_rest_on_the_previous_turns_transcript`. It is red now.
- Setup: a test-local wrapper runs the mock with turn 1 `Say("hello")` and turn 2 `Call(weather)`. It holds turn 2's `InputTranscribed` back until that turn's `SpeechToolCallProposed` has been yielded.
- Assertion: the proposed call's `basis_span_id` is not the span of turn 1's "hello" transcript.
- Red output, from the case run alone (`cargo-run.sh test -p inference-testkit --features run,duplex --test adversary_w24_duplex_basis`):
```
assertion `left != right` failed: the second turn's call rests on the first turn's transcript "hello": ...
  left: SpanId(20264520-35a6-44aa-9038-6f23ce3b2d40)
 right: SpanId(20264520-35a6-44aa-9038-6f23ce3b2d40)
test result: FAILED. 0 passed; 1 failed
cargo-run: rc=101 secs=23 target=761M free=68G
```

**3. The suite run, after the case existed**
- Command: `cargo-run.sh test --no-fail-fast -p inference-testkit --features run,duplex --test duplex_node --test adversary_w24_duplex_basis`
- `adversary_w24_duplex_basis`: 0 passed, 1 failed (the panic at :523).
- `duplex_node`: 18 passed, 0 failed.
- Exit status 101.
- An earlier run of the same command without `--no-fail-fast` stopped after my target, so it only counted mine.
- `fmt --check` and `clippy -D warnings` on the new target are both clean.
- Cargo time was about 1 minute; wall time about 25 minutes. Peak `target/` was 848M, and `worktree discard-cache` has run.

**4. Findings**
- **Basis carried over from the previous turn** (`crates/inference-pipeline/src/nodes/duplex.rs:794-801`; CONFIRMED, introduced).
  - The `transcribed` flag is reset only at `InputCommitted`. The mock yields `InputCommitted` before `InputTranscribed` (`mock.rs:709-711`, then `end_turn`), so the flag is always set when the next turn commits.
  - As a result, the story's empty span is only ever committed on the first turn. A later turn whose transcript has not arrived yet proposes its call on the previous turn's words.
  - What reaches it: any provider whose transcript arrives after its commit. The OpenAI adapter feeds `input_committed` (`adapters/inference-openai/src/duplex.rs:938`) and `input_transcribed` (`:880`) from separate server events. That provider ordering is inferred from the code, not observed at runtime.
  - Fix direction: track "transcribed" per committed turn, i.e. clear it at the commit and set it only for a transcript of the current turn. Alternatively, always commit the empty span at `InputCommitted` and let a later transcript replace the basis.
- **Late transcript lands on the next turn** (`duplex.rs:665-671` and `:582-584`; INFEASIBLE, introduced, note).
  - With a continuous microphone, a frame taken between `InputCommitted` and a late `InputTranscribed` opens a new `Turn`. The late transcript's span is then emitted on that new turn.
  - It can't be observed from outside: `TranscriptCommitted` carries no turn id, so I wrote no case.
- **Cancel-once guard has no effect** (`duplex.rs:610`; INFEASIBLE, introduced, note).
  - The guard `cancelled == Some(response_id)` never changes an outcome. `SessionLedger::cancel` clears `in_progress` synchronously (`ledger.rs:771-776`), so a second `Cancel` already sees no response in progress.
  - So "at most once per response" holds without the guard, and the suite can't detect a mutant that removes it.

**5. What I attacked and could not break**
- **Facade's new variants:** no exhaustive match over facade `SessionEvent` in `crates/`, `examples/` or `crates/inference/src`; every match has a wildcard.
- **Manual and automatic models:** I read the code but wrote no case. `respond()` checks `manual_response`, and the mock's automatic path starts its own response.
- **Tool results after close or drain:** dropped through `admits()`.
- **Double result and unknown id:** both dropped.
- **Denied tool:** answered without leaving the node.
- **Response failure:** `SpeechResponseFailed` is sent before `GenerationCancelled`.
- **Session failure:** the node clears its inputs, so the next frame is refused.
- **`stop` and `cancel`:** the exchange thread exits through `abandon`.

Not attacked: backpressure on the single egress, and a consumer leaving mid-stream. I did not write either case.

**6. Paths written outside the worktree**, all under `<scratch>`:
- `adv-basis-red.log`
- `adv-clippy.log`
- `adv-clippy-2.log`
- `adv-suite.log`
- `adv-suite-2.log`
- `msg.PLek7H`

The background-task output files the harness wrote are under `<scratch>`.

```findings
[
 {"file":"crates/inference-pipeline/src/nodes/duplex.rs","line":795,"category":"acceptance","severity":"blocker","verdict":"CONFIRMED","origin":"introduced","message":"the transcribed flag outlives its turn when transcripts follow the commit, so a later untranscribed turn's call rests on the previous turn's transcript instead of an empty committed span"},
 {"file":"crates/inference-pipeline/src/nodes/duplex.rs","line":583,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"a transcript arriving after the next turn's first frame is emitted as a span of that next turn; unobservable because TranscriptCommitted carries no turn id"},
 {"file":"crates/inference-pipeline/src/nodes/duplex.rs","line":610,"category":"mutant","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"the once-per-response cancel guard is dead because the ledger clears in_progress synchronously on cancel, so no case can kill its removal"}
]
```
