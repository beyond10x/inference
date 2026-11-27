---
format: aep.planning-md/3
id: review-result:time-bound-defaults-and-close-bounds-held-adversary-1
kind: review-result
status: active
title: Adversary pass on the tests that fix the value of the time-bound defaults
relations:
- reviews: story:time-bound-defaults-and-close-bounds-held
revision: 1
---
unit: story:time-bound-defaults-and-close-bounds-held (narrowed to the 34 defaults), commits 6d83c041f + 1b4697831 on base 25a5610b1
verdict: red (NEEDS-CHANGE): 7 findings, no error; 4 warnings, 3 notes
head: 1b46978319191e46115fdbd15ec879eb684ca06e
wrote-outside-worktree: 12 entries, all under <scratch>/ (listed in part 6)
uncommitted-in-worktree: none
cases: executed 1102→1102, red 0 (no case added)
origin: introduced 3 / pre-existing 0 / undecided 4
needs-coordinator: whether F1, F2 and F4 belong to this unit or to a new story (the story's table lists constants, not the constructors that hand them out)

All 34 defaults go red when the constant or literal is changed, and the four "fixed before" tests do too. The red verdict comes from three places where a caller's default can change with every test of the crate green, and from one page sentence that claims more than that.

## 1. Proof of what I touched

```
$ git rev-parse HEAD
1b46978319191e46115fdbd15ec879eb684ca06e
$ git diff HEAD -- ':(glob)adapters/*/src/**'
(empty)
$ git status --short
(empty)
$ git --no-pager diff --stat
(empty)
```

- **Lease:** `worktree hook session-end ... --session quality4-adversary` exited 0.
- **Departure from the role file:** it says to mutate a copy. The brief ordered planted and restored mutants in the worktree, and I followed the brief. Each of the 43 mutant outputs ends with `restored: adapters/*/src equals HEAD`.
- **Sync:** all 63 cargo runs print `slot: synced to the build node ... at 1b4697831 + N changed file(s)`; none synced a folder outside the repository.
- **Interruption at about 10:38 CET:** no run was lost. `out/s1.finished`, `s2.finished` and `s3.finished` exist and every mutant output is complete.

## 2. Cases added

None. A case can only fail on the committed tree if a committed default is wrong, and none is. The gaps below show only under a mutant.

## 3. Suite on the committed tree

Run by `final.sh` after the last mutant was restored; outputs are in `out/final-*.txt`.

| Run | Result |
|---|---|
| 14 new or changed tests alone, nine crates | all pass, 0.004 s to 0.011 s each, exit 0 |
| `cargo nextest run -p inference-docs` | `Summary [ 1.493s] 74 tests run: 74 passed, 0 skipped`, exit 0 |
| inference-audio-device, whole | `128 passed, 1 skipped`, exit 0 |
| inference-aws, whole | `135 passed, 3 skipped`, exit 0 |
| inference-cartesia, whole | `39 passed, 1 skipped`, exit 0 |
| inference-deepgram, whole | `Summary [ 13.838s] 129 tests run: 129 passed, 3 skipped`, exit 0 |
| inference-deepslate, whole | `71 passed, 1 skipped`, exit 0 |
| inference-elevenlabs, whole | `167 tests run: 165 passed, 2 failed, 5 skipped`, exit 100 |
| inference-llama-server, whole | `48 passed, 4 skipped`, exit 0 |
| inference-openai, whole | `Summary [ 12.575s] 352 tests run: 352 passed, 7 skipped`, exit 0 |
| inference-phonic, whole | `33 passed, 2 skipped`, exit 0 |
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy` on the nine crates, `--all-targets --all-features -- -D warnings` | exit 0 |

The two elevenlabs failures are the known ticker tests, not counted: `async_handles twenty_recognitions_wait_for_the_server_without_blocking_the_runtime` and `twenty_syntheses_...` (`the ticker's largest gap 151.672383ms`).

## 4. Findings

They cover the tree at 1b4697831. I did not build or run the base, so F1 to F4 carry origin `undecided`. By reading: `git diff 25a5610b1..HEAD -- ':(glob)adapters/*/src/**'` is empty and the unit only added or tightened assertions, so the same mutants would survive at the base.

**F1. warning, CONFIRMED, undecided (gap there before; the unit's test does not close it). The patience a sink starts with is not held.**
- **Mutant g1:** `adapters/inference-audio-device/src/acknowledging.rs:1029`, `patience: PLAYBACK_PATIENCE,` changed to `patience: Duration::from_secs(3),`.
- **Run:** `cargo nextest run -p inference-audio-device --all-features` gave `Summary [ 1.684s] 128 tests run: 128 passed, 1 skipped`, `exit 0`.
- **Why it survives:** the new test at `adapters/inference-audio-device/tests/acknowledging.rs:1033-1040` reads only the constant. Every test sink is built with `.with_playback_patience(..)` (`tests/acknowledging.rs:489`, `tests/adversary_rf1_acknowledging.rs:407`).
- **What reaches it:** `crates/inference/src/recipe/speaker.rs:145` and `examples/local-voice-cascade/src/lib.rs:645` build a sink without setting a patience.
- **Disclosed:** the implementor says so in the test's comment and in the report. A killing test has to stall a drain and read `PlaybackFault::Stalled { patience, .. }`, which waits about 2 s.

**F2. warning, CONFIRMED, undecided (there before; the constructors are not rows of the story's table). The OpenAI bindings' default deadlines are not held.**
- **Mutant g2 (five plants):**
  - `src/duplex.rs:294`: `Deadlines::default().with_connect_timeout(5 s)` (`OpenAiRealtime`)
  - `src/live_duplex.rs:361`: `.with_write_timeout(5 s)` (`OpenAiLive`)
  - `src/client.rs:229`: `.with_connect_timeout(5 s)` (`RealtimeClient::connect`)
  - `src/live.rs:800`: `.with_write_timeout(5 s)` (`LiveClient::connect`)
  - `src/live.rs:114`: `read_timeout: Duration::from_secs(31)` (see F3)
- **Run g2:** `cargo nextest run -p inference-openai --all-features` gave `Summary [ 7.500s] 352 tests run: 352 passed, 7 skipped`, `exit 0`.
- **Mutant g5:** `live_duplex.rs:361` connect 5 s, `client.rs:229` write 5 s, `live.rs:800` connect 5 s gave `Summary [ 5.871s] 352 tests run: 352 passed, 7 skipped`, `exit 0`.
- **The one held case, g6:** `duplex.rs:294` `.with_write_timeout(5 s)` alone is red, `exit 100`, at `tests/transport_deadlines.rs:480` (`left: 5s right: 10s`), a test older than the unit.
- **Why the rest survives:** `adapters/inference-openai/tests/transport_deadlines.rs:147-184` reads the two constants and `Deadlines::default()` only.
- **Readers exist that need no connection:** `OpenAiRealtime::connect_timeout()` (`src/duplex.rs:321`), and `OpenAiLive::connect_timeout()` / `write_timeout()` (`src/live_duplex.rs:394`, `:399`). I did not check whether the two clients have readers.
- **What reaches it:** `crates/inference/src/registry/openai.rs:27,50`, and the examples `compare.rs:335,347`, `talk.rs:105,115`, `podcast.rs:169,177`.

**F3. note, CONFIRMED, undecided (there before; belongs to a default counted "fixed before"). `Endpoint::live(..)` hands out a read timeout no test reads.**
- **Place:** `adapters/inference-openai/src/live.rs:114`, `read_timeout: DEFAULT_READ_TIMEOUT`.
- **Run:** a literal of 31 s there was part of g2, which stayed green.
- **Contrast:** the constant itself is held; `DEFAULT_READ_TIMEOUT` 31 s in b5 was killed at `tests/endpoint_literal.rs:24`.

**F4. warning, CONFIRMED, undecided (a miss of the story's inventory, there before the unit). The llama-server connect bound is in no list.**
- **The default:** `adapters/inference-llama-server/src/lib.rs:402` calls `HttpClient::new(config.limits)`. At the pinned tag that is `with_connect_timeout(limits, CONNECT_TIMEOUT)` with `CONNECT_TIMEOUT` 15 s (`crates/llm-http/src/transport.rs:33`, `:116-117`, commit 541a831, `Cargo.lock:2803`).
- **A caller cannot set it:** `Config` has no field for it (`src/lib.rs:141-159`).
- **Mutant g3:** `HttpClient::with_connect_timeout(config.limits, Duration::from_secs(1))?` gave `48 tests run: 48 passed, 4 skipped`, `exit 0`.
- **Not listed anywhere:** it is in neither the story's Scope table nor its Out of scope, and neither `quality/not-measured.md` nor `quality/adapters.md` names it.
- **Consequence:** a move of the pin that changes it fails nothing, which is the case the story's Planned scope gives as the reason for the three limits.

**F5. warning, NEEDS-CHANGE, introduced. One page sentence claims more than the tests hold.**
- **Place:** `quality/not-measured.md:295-296`, "A default changed in the source fails its test."
- **Measured:** true for the 34 constants and literals (part 5). Not true for the default a sink starts with (g1) or for the deadlines the OpenAI bindings start with (g2, g5).
- **Second sentence, `:294-295`:** "The test reads the public constant, and the field of the default endpoint or configuration a caller gets where there is one." For OpenAI, readers exist (F2) and are not read.
- **Fix, either one:** the tests of F1 and F2, or the sentence narrowed to what is read.

**F6. note, CONFIRMED, introduced (expected by the rule that entries change only from a `check` run). Three lines of `quality/adapters.md` are now older than the tests.**
- `:366-369`: ElevenLabs text to speech close, "no test of the gate fires it or asserts it".
- `:464-465`: llama-server, "No test holds the default limits".
- `:610-614`: OpenAI, "It does not fix the numbers".
- The page says so in general at `quality/not-measured.md:345-347`. `quality/README.md` has no contradiction.

**F7. note, CONFIRMED, introduced. The page contradicts itself about the ElevenLabs `CLOSE_BOUND`.**
- **Place:** `quality/not-measured.md:90`, "ElevenLabs text to speech: no test holds `CLOSE_BOUND`." The line is unchanged.
- **Against it:** `:84-86`, added by the unit, says a test now asserts the value of each of the five constants.
- **Vocabulary:** `quality/adapters.md:52-59` counts "asserts the value" as one of the two ways a test holds a bound. The line means "no test fires it" and does not say so.
- **Measured:** mutant a12 (`CLOSE_BOUND` 2 s) is red at `adapters/inference-elevenlabs/tests/closed_stream.rs:338`.

## 5. Mutant table and what I could not break

Outputs are `out/<id>.txt`, planted diffs `out/<id>.plant.diff`. "Killed" means `exit 100` with the failing assertion naming the default. The killing test is `the_time_bound_defaults_are_the_stated_numbers` of the named file unless another is named.

**Value changed, planted alone (20 of the 34):**

| Id | Crate | Mutant | Killed at |
|---|---|---|---|
| a01 | audio-device | `PLAYBACK_PATIENCE` 2.5 s | `tests/acknowledging.rs:1036` (`left: 2.5s right: 2s`) |
| a02 | aws | `SessionConfig::new` read 31 s | `tests/nova_sonic_stall_and_bounds.rs:305` |
| a03 | aws | duplex idle limit 29 s | same |
| a04 | cartesia | `DEFAULT_CONNECT_TIMEOUT` 10.5 s | `tests/errors.rs:296` |
| a05 | cartesia | `CLOSE_BOUND` 2 s | same |
| a06 | deepgram | `DEFAULT_KEEPALIVE_INTERVAL` 5 s | `tests/errors.rs:270` |
| a07 | deepgram | `DEFAULT_AGENT_READ_TIMEOUT` 31 s | `tests/agent_duplex.rs:521` |
| a08 | deepgram | `DEFAULT_CONNECT_TIMEOUT` 10.001 s | `tests/errors.rs:270` |
| a09 | deepslate | `DEFAULT_PING_INTERVAL` 9 s | `tests/opal_duplex.rs:1129` |
| a10 | deepslate | `CLOSE_BOUND` 1.5 s | same |
| a11 | elevenlabs | agent `DEFAULT_METADATA_TIMEOUT` 11 s | `tests/agent_duplex.rs:532` |
| a12 | elevenlabs | `CLOSE_BOUND` 2 s | `tests/closed_stream.rs:338` |
| a13 | elevenlabs | `DEFAULT_READ_TIMEOUT` 9 s | same |
| a14 | llama-server | `limits.idle` 181 s | `tests/offline.rs:279` (`[("Config::new(..).limits.idle", 181s, 180s)]`) |
| a15 | openai | `DEFAULT_WRITE_TIMEOUT` 11 s | `the_deadlines_are_stated_and_settable`, `tests/transport_deadlines.rs:175` |
| a16 | openai | GPT-Live idle limit 31 s | `tests/gpt_live_duplex.rs:1472` |
| a17 | openai | `DEFAULT_CONNECT_TIMEOUT` 10.5 s | `tests/transport_deadlines.rs:175` and `tests/speech_request.rs:231` |
| a18 | phonic | `DEFAULT_READY_TIMEOUT` 11 s | `tests/duplex.rs:577` |
| a19 | phonic | `CLOSE_BOUND` 2 s | same |
| a20 | phonic | `DEFAULT_PING_INTERVAL` 9 s | same |

**The other 14 of the 34, batched per crate (each assertion names every changed default):**

| Id | Mutant | Killed at |
|---|---|---|
| r1 | cartesia `DEFAULT_READ_TIMEOUT` 11 s | `tests/errors.rs:296` |
| r2 | deepgram `DEFAULT_READ_TIMEOUT` 11 s, `CLOSE_BOUND` 2 s, `DEFAULT_AGENT_OPEN_TIMEOUT` 14 s | `tests/errors.rs:270`, `tests/agent_duplex.rs:521` |
| r3 | deepslate `DEFAULT_OPEN_TIMEOUT` 16 s, `DEFAULT_READ_TIMEOUT` 31 s | `tests/opal_duplex.rs:1129` |
| r4 | elevenlabs `DEFAULT_CONNECT_TIMEOUT` 11 s, agent connect 9 s, agent read 31 s | `tests/closed_stream.rs:338`, `tests/agent_duplex.rs:532` |
| r5 | openai Realtime idle limit 31 s | `tests/duplex.rs:948` |
| r6 | phonic `DEFAULT_CONNECT_TIMEOUT` 11 s, `DEFAULT_READ_TIMEOUT` 29 s | `tests/duplex.rs:577` |
| b8 | llama-server response headers 61 s, total 599 s | `tests/offline.rs:279` |

**Constant unchanged, constructor no longer using it; and the four "fixed before":**

| Id | Mutant | Result |
|---|---|---|
| b1 | cartesia `Endpoint::new` literals read 11 s, connect 9 s | killed, both fields named |
| b2 | deepgram endpoint, agent endpoint and `ListenConfig` literals | killed, each field named |
| b2 | deepgram `DEFAULT_AGENT_KEEPALIVE_INTERVAL` 9 s (fixed before) | killed by `config::tests::the_agent_endpoint_takes_a_plain_scheme_and_a_checked_url`, `src/config.rs:457` |
| b3 | deepslate literals 16 / 31 / 9 s | killed |
| b4 | elevenlabs endpoint literals; agent read taking the other endpoint's constant | killed: `("AgentEndpoint::new().read_timeout", 10s, 30s)` |
| b5 | openai `Deadlines::default()` literals 9 / 11 s, `SpeechEndpoint::new` literals 9 / 31 s | killed at `tests/transport_deadlines.rs:175`, `tests/speech_request.rs:231` |
| b5 | openai `CLOSE_TIMEOUT` 16 s (fixed before) | killed at `tests/gpt_live.rs:560`, `tests/gpt_live_duplex.rs:1459` |
| b5 | openai `DEFAULT_READ_TIMEOUT` 31 s (fixed before) | killed at `tests/endpoint_literal.rs:24` |
| b6 | phonic `PhonicEndpoint::new` literals | killed, four fields named |
| b7 | aws `OPEN_TIMEOUT` 11 s (fixed before) | killed by `the_open_deadline_is_stated`, `tests/nova_sonic_stall_and_bounds.rs:279` |
| g1 | audio-device sink constructor literal 3 s, whole crate | **green** (F1) |
| g2 | openai binding and client deadlines, `Endpoint::live` read, whole crate | **green** (F2, F3) |
| g5 | openai, the other deadline of each binding and client, whole crate | **green** (F2) |
| g6 | openai `OpenAiRealtime` write 5 s alone, whole crate | killed at `tests/transport_deadlines.rs:480` |
| g3 | llama-server connect bound 1 s, whole crate | **green** (F4) |

**Two defaults of the same value swapped:**

| Id | Mutant | Result |
|---|---|---|
| c1 | cartesia `Endpoint::new` read and connect constants swapped | green; equivalent, both are 10 s and no caller can tell |
| c2 | openai `SpeechEndpoint::new` taking `DEFAULT_WRITE_TIMEOUT` for connect | green; equivalent, both are 10 s |
| c3 | elevenlabs agent connect and metadata swapped, plus `DEFAULT_METADATA_TIMEOUT` 11 s | killed, naming `AgentEndpoint::new().connect_timeout` 11 s: the test tells them apart once the values differ |

**Out of the unit, one sample:** g4, phonic `CLOSE_BOUND` use sites (`src/duplex.rs:540`, `:900`) taking a 5 s literal, whole crate: green (`33 passed, 2 skipped`). This is firing a close bound, which is story:close-bounds-fired-by-a-test, and `quality/not-measured.md:82-86` says so. Not a finding.

**Attacked and not broken:**
- **Self-comparison:** no new assertion compares a default with its constant, directly or through a helper, alias or second read. The a mutants (constant changed, field follows) and the b mutants (field changed, constant not) are both red.
- **Precision:** comparisons are full `Duration` equality; 10.001 s, 10.5 s, 1.5 s and 2.5 s are caught.
- **Connections and waits:** no new test opens a connection, starts a server or waits; the slowest ran 0.011 s.
- **Literals:** each equals the number at the place its comment names: `CHANGELOG.md:748`, `adapters/inference-deepslate/protocol/README.md:13`, `website/docs/guides/compare-voice-providers.md:95`, and the entries of `quality/adapters.md`.
- **OpenAI's two comparisons:** both are exact, 10 s; the comparisons with the constants and with the wide upper limit are gone.
- **llama-server numbers:** 60 / 180 / 600 s match `crates/llm-http/src/transport.rs:46-48` at commit 541a831, which the tag 0.20.0 resolves to.
- **The page:** every named test exists in the named file (the docs check is green, and I read each). Every `CLOSE_BOUND` is still listed as unfired; the Cartesia cancel and ElevenLabs text to speech are still listed. No link to the renamed or removed headings exists outside `.engineering/`.
- **Nova Sonic sentence:** it matches `adapters/inference-aws/src/duplex.rs:671-674`, `:1357-1363` and `nova_sonic/session.rs:165-172`: no timeout on the close.
- **Docs check not loosened:** `git diff 25a5610b1..HEAD -- crates/inference-docs/` is empty.

**Acceptance, line by line:**
1. "Each of the 34, changed in the source, fails a test of its crate": held for the constant or literal of all 34 (a01-a20, r1-r6, b8). Not held for the same default changed where the constructor hands it out, for the audio device (F1).
2. "No assertion compares a default with its constant": held.
3. "No new test over 5 s, none opens a connection, the check of `quality/` green": held.

**Not attacked:**
- `task test-run` and the whole workspace were not run; only the nine crates and `inference-docs`.
- The base was read, not built.
- No live test, example or benchmark was run.
- A binding that stops calling `default_session_config()` (aws, openai) was not planted.
- The `CLOSE_BOUND` use sites of the four crates other than phonic were not planted.
- The field alone was checked for `DEFAULT_AGENT_KEEPALIVE_INTERVAL`, not a compound mutant.

## 6. Paths written outside the worktree

All under `<scratch>/`:
- `mutant.sh`, `drive.sh`, `define.sh`, `final.sh` (POSIX sh, temporary)
- `story.txt`, `unit-tests.diff`, `unit-page.diff`
- `const-grep.txt`, `literal-grep.txt`, `duration-grep.txt`
- `mutants/` (86 files: `.plant` and `.args` for 43 mutants)
- `out/` (112 files: 43 outputs, 43 planted diffs, `s1.finished`, `s2.finished`, `s3.finished`, 23 `final*` files)

Nothing else. The build outputs are the pool's own on the build node; I pointed no compiler at a directory.

## 7. Findings block

```findings
- file: adapters/inference-audio-device/tests/acknowledging.rs
  line: 1034
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: undecided
  message: the test reads only PLAYBACK_PATIENCE, so the sink constructor at src/acknowledging.rs:1029 taking a 3 s literal leaves all 128 tests of the crate green
- file: adapters/inference-openai/tests/transport_deadlines.rs
  line: 147
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: undecided
  message: the test reads the constants and Deadlines::default() only, so OpenAiRealtime, OpenAiLive, RealtimeClient::connect and LiveClient::connect starting with other deadlines leave all 352 tests green, except the write deadline of OpenAiRealtime
- file: adapters/inference-openai/src/live.rs
  line: 114
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: undecided
  message: the read timeout Endpoint::live hands out is read by no test, a 31 s literal there leaves the crate green
- file: adapters/inference-llama-server/src/lib.rs
  line: 402
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: undecided
  message: the 15 s connect bound the adapter takes from the pinned llm-http is in neither the story's table nor its out of scope nor any quality page, and a 1 s bound there leaves all 48 tests green
- file: quality/not-measured.md
  line: 295
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the sentence "A default changed in the source fails its test" is not true for the patience a sink starts with nor for the deadlines the OpenAI bindings start with
- file: quality/adapters.md
  line: 366
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: lines 366-369, 464-465 and 610-614 say no test asserts or fixes a default that a test of the unit now fixes, expected under the rule that entries change only from a check run
- file: quality/not-measured.md
  line: 90
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the line "no test holds CLOSE_BOUND" for ElevenLabs text to speech stands six lines under the new sentence that a test asserts the value of each of the five constants
```
