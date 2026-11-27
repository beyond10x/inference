---
format: aep.planning-md/3
id: review-result:connect-timeouts-fired-by-a-test-adversary-1
kind: review-result
status: active
title: Adversary pass on the tests that fire the connect timeout of five bindings
relations:
- reviews: story:connect-timeouts-fired-by-a-test
revision: 1
---
unit: story:connect-timeouts-fired-by-a-test, commits d99652d48 + 473046a3b on base 8f8735fde
verdict: red (6 findings: 3 warnings, 3 notes; no error; the acceptance holds)
head: 473046a3bf2c29f070ea8a030d174d4bb26085b1, branch `quality/connect-timeouts`, tree clean, not pushed
wrote-outside-worktree: only `<scratch>/` (listed in part 7)
uncommitted-in-worktree: none
cases: executed 362 → 362 over the four crates, red 0 (I added no case)
origin: introduced 5 / pre-existing 0 / undecided 1
needs-coordinator: whether findings 1 to 3 go back to the implementor or into a follow-up story
lease: taken as `quality3-adversary`, released (`session-end` exit 0)

Removing a bound turns its test red, and a refusal does not pass for a timeout. What the five tests do not hold is the size of the wait or the number of attempts: a doubled bound, a tenfold bound and a second connect attempt all leave them green.

One departure from the role's procedure: it says never to mutate a file under attack, and your brief orders planted and restored source mutants. I followed the brief, because the build runs on a mirror synced from the worktree. There were 12 plants under my lease, each restored with `git apply -R`.

## 1. What I touched

```
$ git status --short
exit 0
$ git --no-pager diff --stat
exit 0
$ git diff HEAD --stat -- ':(glob)adapters/*/src/**'
exit 0
```
All three print nothing. The pathspec your brief names is in finding 5.

## 2. Cases added

None. Each finding is a mutant the tests miss, or a matter of the page or of the proof command. A case that catches such a mutant is green on the committed tree, which your brief excludes.

## 3. Findings

Test lines, used below: `adapters/inference-cartesia/tests/errors.rs:200`, `adapters/inference-phonic/tests/duplex.rs:503`, `adapters/inference-elevenlabs/tests/agent_duplex.rs:377`, `adapters/inference-elevenlabs/tests/closed_stream.rs:246`, `adapters/inference-deepgram/tests/errors.rs:158`.

**1. warning, introduced, CONFIRMED: a second connect attempt after the timeout is not noticed.**
- Mutant `retry` (after the timeout, the same connect once more) in all five opens: all five new tests pass, in about twice the bound.
  ```
  PASS [   0.609s] inference-cartesia::errors a_handshake_never_answered_fails_the_synthesis_at_the_connect_timeout
  PASS [   0.611s] inference-phonic::duplex a_handshake_never_answered_fails_the_open_at_the_connect_timeout
  PASS [   0.607s] inference-elevenlabs::agent_duplex a_handshake_never_answered_fails_the_open_at_the_connect_timeout
  PASS [   0.609s] inference-elevenlabs::closed_stream a_handshake_never_answered_fails_the_synthesis_at_the_connect_timeout
  PASS [   0.610s] inference-deepgram::errors a_handshake_never_answered_fails_the_recognition_at_the_connect_timeout
  Summary [   8.727s] 362 tests run: 360 passed, 2 failed, 11 skipped     (the 2: the ticker tests)
  ```
- Mutant `elreopen`, one token: `ended` at `adapters/inference-elevenlabs/src/synthesiser.rs:212` also matches `ElevenLabsError::Connect(_)`. The story's scope cites this function as the reason a connect failure is not reopened. The text-to-speech test passes at 0.634 s; nothing else in the crate fails but the ticker tests.
- What reaches it: an edit of that kind. Nothing in the tree does it today.
- Fix, not applied: after the open ends, count the connections the listener holds (non-blocking `accept` until it would block) and assert one; for the agent, the fixture's count. That needs no timing.

**2. warning, introduced, CONFIRMED: a multiplied bound leaves all five green.**
- Mutant `x2` (`connect_timeout * 2`): five pass at 0.606 to 0.610 s. Mutant `x10`: five pass at about 3 s.
  ```
  PASS [   3.042s] inference-cartesia::errors a_handshake_never_answered_fails_the_synthesis_at_the_connect_timeout
  PASS [   3.027s] inference-phonic::duplex a_handshake_never_answered_fails_the_open_at_the_connect_timeout
  PASS [   3.014s] inference-elevenlabs::agent_duplex a_handshake_never_answered_fails_the_open_at_the_connect_timeout
  PASS [   3.019s] inference-elevenlabs::closed_stream a_handshake_never_answered_fails_the_synthesis_at_the_connect_timeout
  PASS [   3.034s] inference-deepgram::errors a_handshake_never_answered_fails_the_recognition_at_the_connect_timeout
  Summary [  59.020s] 362 tests run: 360 passed, 2 failed, 11 skipped     (the 2: the ticker tests)
  ```
- Cause: `took < CEILING` with `CEILING` 4 s against a bound of 300 ms (`errors.rs:27` and `:232`, same shape in the other four). By arithmetic, not run: every factor up to 13 passes.
- The tests are named "at the connect timeout"; what they hold is "between 300 ms and 4 s". The lower side is held: mutant `half` fails all five ("the synthesis failed after 152.455379ms, before its connect timeout").
- Fix, not applied: a ceiling tied to the bound. The most a new test ran over its bound on the loaded build node was 51 ms (0.351 s in the 20 runs).

**3. warning, introduced, CONFIRMED: the five bullets stand under a heading that says the opposite.**
- `quality/not-measured.md:72` sits under `## No command produces it` (`:11`). The five bullets now say a test fires the bound, and `task test-run` runs that test.
- The page has the fitting heading at `:272`, `## The command exists, the value is not kept`.
- The changed `###` heading and intro do cover the list. No check reads this.

**4. note, introduced, CONFIRMED: the credential lookup moved inside the bound is not noticed.**
- Mutant `credinside` (one deadline started before `source.resolve`) in Cartesia, Phonic, Deepgram and ElevenLabs text to speech: all four pass at 0.306 to 0.308 s. The agent has no lookup.
- The fixtures' key source answers at once. The story names "the lookup is outside" as a fact of the scope, not as acceptance.

**5. note, introduced (the implementor's proof, and your brief's end check), CONFIRMED: `git diff HEAD -- 'adapters/*/src'` matches no file.**
- With five source files modified (`out/pathspec-vacuous.out`):
  ```
  $ git status --short
   M adapters/inference-cartesia/src/connection.rs
   ... (5 files)
  $ git diff HEAD --stat -- 'adapters/*/src'
  exit 0
  $ git diff HEAD --stat -- ':(glob)adapters/*/src/**'
   5 files changed, 5 insertions(+), 5 deletions(-)
  ```
- So both "empty" lines of `impl/red/src-diff-after.out` and the report's `git diff 8f8735fde HEAD -- 'adapters/*/src'` prove nothing.
- The claim itself is true: the `git status --short` in the same file lists no source, and `git diff --stat 8f8735fde HEAD -- ':(glob)adapters/*/src/**' ':(glob)**/Cargo.toml' crates/inference-docs quality/adapters.md quality/README.md quality/records quality/adapters.ratchet.yaml` prints nothing.

**6. note, undecided, CONFIRMED: an existing test failed once.**
- `inference-elevenlabs::auth a_handshake_refused_with_401_is_a_typed_refusal_without_the_key`, `adapters/inference-elevenlabs/tests/auth.rs:64`: `left: None`, `right: Some("fixture-key-d41d8cd98f00b204")`.
- Once in 13 whole-suite runs, in the run with mutant `default` planted, so it says nothing certain about the committed tree. The unit touches neither the test nor its fixture. Not run against the base.

## 4. Mutant table

One cargo call per round over the four crates, whole suites: `cargo nextest run -p inference-cartesia -p inference-phonic -p inference-elevenlabs -p inference-deepgram --all-features --no-fail-fast`. The two ticker tests failed in almost every round and are left out of "other tests".

| Mutant | New tests | Other tests that go red |
|---|---|---|
| `removed` (the implementor's five patches) | 5 FAIL at 4.005 to 4.007 s, "did not end within 4s" | none |
| `tcponly` (bound on the TCP connect, handshake unbounded) | 5 FAIL at 4.004 to 4.012 s | none |
| `default` (waits `DEFAULT_CONNECT_TIMEOUT`, detail keeps the configured value) | 5 FAIL at 4.005 to 4.007 s | none of the bound (finding 6 fell in this run) |
| `half` (`/ 2`) | 5 FAIL at 0.156 to 0.163 s, lower bound | none |
| `remap` (`Stalled(connect_timeout)` in place of `Connect`) | 5 FAIL, `left: Some(Stalled(300ms))` | `inference-elevenlabs::stt a_handshake_never_answered_fails_within_the_connect_timeout`, `inference-elevenlabs::security_u1_stt a_handshake_still_pending_when_the_read_timeout_after_input_passes_fails_the_recognition` |
| `nodur` (detail "no handshake") | 5 FAIL, `detail: "no handshake"` | none |
| `x2` | **5 PASS** at 0.606 to 0.610 s | none |
| `x10` | **5 PASS** at 3.014 to 3.042 s | none |
| `retry` | **5 PASS** at 0.607 to 0.611 s | none |
| `elreopen` (ElevenLabs text to speech only) | **PASS** at 0.634 s | none |
| `credinside` (four bindings) | **4 PASS** at 0.306 to 0.308 s | none |
| `shared` (bound out of ElevenLabs and Deepgram `src/connection.rs`, outer bounds kept) | the 2 of those bindings FAIL at 4.008 and 4.023 s; the other 3 pass | none |

In `shared`, the neighbours stay green as the story says: the speech-to-text test `a_handshake_never_answered_fails_within_the_connect_timeout` passes at 3.020 s, and so does every Deepgram Voice Agent test.

## 5. Attacked and not broken

- **Acceptance 1** (removal fails within the own deadline): held by `removed`, 5 of 5 fail at about 4 s, none killed at 60 s.
- **Acceptance 2** (a refusal does not pass): not run by me, because it needs an edit of the five tests. I read the five `impl/red/refuse-*.patch` (`drop(listener)`) and their outputs: `left: ... "IO error: Connection refused (os error 111)"` against `"no handshake within 300ms"`, exit 100 each.
- **Acceptance 3**: no new test over 5 s (0.307 to 0.351 s passing, 4.0 s failing). Every listener binds `127.0.0.1:0`; the agent fixture binds at `tests/support/agent.rs:138`. The quality check is green:
  ```
  PASS [   0.099s] inference-docs::workflows quality::the_quality_documents_agree_with_each_other_and_with_the_taskfile
  Summary [   1.781s] 74 tests run: 74 passed, 0 skipped        cargo exit 0
  ```
- **Wrong-reason pass**: `tcponly` shows the kernel completes the TCP connection here, so the timeout fires in the handshake. A failed bind panics at `unwrap`. Only the detail string and the time tell a timeout from a refusal, and both are asserted.
- **Lower bound**: the clock starts before the open in all five; `half` fails all five.
- **Leaks**: a passing test's thread ends with its send, and the listener lives to the end of the function. No `LEAK` line in any run.
- **Beside the whole suite**, committed tree:
  ```
  PASS [   0.307s] inference-cartesia::errors ...   PASS [   0.309s] inference-deepgram::errors ...
  PASS [   0.307s] inference-elevenlabs::agent_duplex ...   PASS [   0.307s] inference-elevenlabs::closed_stream ...
  PASS [   0.307s] inference-phonic::duplex ...
  Summary [   9.125s] 362 tests run: 360 passed, 2 failed, 11 skipped        cargo exit 100
  ```
  The two failures are the ticker tests ("the ticker's largest gap 204.103065ms", "95.399686ms").
- **The page**: the five named functions exist in the named files; `named_tests` in `crates/inference-docs/tests/quality/adapters.rs:303` reads all five. The three `DEFAULT_CONNECT_TIMEOUT` files hold the constant (`config.rs:28`, `protocol.rs:100`, `agent.rs:91`). No file under `quality/` other than the page names a new test, so "No record of a run holds its result yet" is true. The workflow `check` exists. Nothing in `quality/adapters.md` or `quality/README.md` says a connect bound has no test. `git grep` finds the old heading nowhere.
- **Unchanged**: `crates/inference-docs/`, manifests, binary counts (9, 5, 20, 32). `cargo fmt --all --check` exit 0.

## 6. Twenty runs in a row

Twenty cargo calls on the clean tree, the five tests each: 100 PASS, 0 other, every call exit 0.

| Test | Fastest | Slowest |
|---|---|---|
| `inference-cartesia::errors` | 0.307 s | 0.346 s |
| `inference-deepgram::errors` | 0.308 s | 0.343 s |
| `inference-elevenlabs::agent_duplex` | 0.307 s | 0.348 s |
| `inference-elevenlabs::closed_stream` | 0.307 s | 0.351 s |
| `inference-phonic::duplex` | 0.307 s | 0.332 s |

```
round 1 exit 0  Summary [   0.322s] 5 tests run: 5 passed, 368 skipped
round 5 exit 0  Summary [   1.610s] 5 tests run: 5 passed, 368 skipped
round 20 exit 0  Summary [   0.323s] 5 tests run: 5 passed, 368 skipped
```
All twenty lines are in `out/twenty.out`.

## 7. Not attacked

- The refusal case by a run of my own (see acceptance 2).
- A connect attempt that outlives its timeout (a spawned handshake): not planted.
- The factor at which the ceiling starts to fail (13 or 14): arithmetic only.
- `cargo clippy`, `task check`, the whole workspace, the examples, any live test.
- macOS or Windows behaviour of a listener that never accepts.
- The unchanged sentences of the page (defaults no test asserts, close bounds).
- The auth failure of finding 6 on the base commit.

## 8. Paths outside the worktree

All under `<scratch>/`:
- eight helper scripts: `run-new.sh`, `plant-expr.sh`, `plant-copy.sh`, `mutant-run.sh`, `chain.sh`, `summ.sh`, `twenty.sh`, `clean.sh`
- `mutants/`: 12 patches, one per row of the table
- `mut/`: 33 mutated copies of source files
- `out/`: 37 files (one output per mutant, `baseline-new.out`, `pathspec-vacuous.out`, `twenty.out` with 20 raw files, `clean-suites.out`, `clean-docs.out`)

No `target/` directory exists in the worktree.

```findings
- file: adapters/inference-elevenlabs/tests/closed_stream.rs
  line: 246
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a second connect attempt after the timeout leaves all five new tests green, also as the one-token edit of `ended` in the ElevenLabs synthesiser, because no test counts the connections its listener received
- file: adapters/inference-cartesia/tests/errors.rs
  line: 232
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a connect bound multiplied by 2 or by 10 leaves all five new tests green, because the only upper assertion is the 4 s ceiling against a bound of 300 ms
- file: quality/not-measured.md
  line: 72
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the five bullets that say a test fires the connect timeout stand under the heading "No command produces it", while the page's heading for a command whose value is not kept is at line 272
- file: adapters/inference-cartesia/tests/errors.rs
  line: 200
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: moving the credential lookup inside the connect bound leaves the four tests of the bindings with a lookup green, because the fixture key source answers at once
- file: adapters/inference-cartesia/src/connection.rs
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the proof command `git diff HEAD -- 'adapters/*/src'` matches no file and printed nothing with five source files modified, so the implementor's two empty diffs and the brief's end check prove nothing
- file: adapters/inference-elevenlabs/tests/auth.rs
  line: 64
  category: concurrency
  severity: note
  verdict: CONFIRMED
  origin: undecided
  message: the existing test a_handshake_refused_with_401_is_a_typed_refusal_without_the_key failed once in 13 whole-suite runs with the fixture having recorded no key, in a run with a mutant planted
```
