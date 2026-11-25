---
format: aep.planning-md/3
id: story:connect-timeouts-fired-by-a-test
kind: story
status: active
title: A test fires the connect timeout of Cartesia, Phonic and the ElevenLabs agent
tags:
- from-finding
relations:
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-cartesia/tests/errors.rs
- confidence: cited
  path: adapters/inference-deepgram/tests/errors.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/agent_duplex.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/closed_stream.rs
- confidence: cited
  path: adapters/inference-phonic/tests/duplex.rs
- confidence: cited
  path: adapters/inference-phonic/tests/support/mod.rs
- confidence: cited
  path: quality/not-measured.md
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T07:49:25Z", actor: "human:operator", revision: 8}
- {from: "proposed", to: "active", at: "2026-11-25T07:49:25Z", actor: "human:operator", revision: 9}
---
## Context

Found while the time bounds of each adapter were written down
(story:quality-per-adapter-and-latency-defined), and read again when this story was scoped. No
test of the gate fires the connect timeout of five bindings: Cartesia text to speech, Phonic, the
ElevenLabs agent, Deepgram transcription and ElevenLabs text to speech. The draft named the first
three and left the last two undetermined; the scope read found no such test for them either.

Each bound is the `connect_timeout` of the binding's endpoint, 10 s by default, and is set through
a public builder. A connect that never ends is the case the bound exists for, and a change that
drops the bound leaves the gate green.

Two things make the existing tests insufficient:

- A refused connection and a handshake that timed out give the same error variant with no status.
  The tests that open against a closed port therefore do not hold the bound.
- ElevenLabs speech to text and the Deepgram Voice Agent call the same connect as the
  text-to-speech and the transcription path, under an outer bound of their own. Their tests stay
  green with the inner bound removed.

## Planned scope

- One test per binding, five in all, against a local listener that accepts the connection and
  never answers the handshake. The test sets the bound through the endpoint's builder to under a
  second, asserts the binding's connect error with the words of the timeout, and asserts that the
  open ended no earlier than the bound and within a ceiling the test names.
- Each test ends its own wait: with the bound gone it fails within a deadline of its own and does
  not hang.
- No file under `adapters/*/src` changes, and no test binary is added.
- `quality/not-measured.md` says for each of the five bindings that a test fires the bound and
  that no record of a run holds its result yet.
- `quality/adapters.md`, its plan and the four time-bound records follow in a later change, from
  the first `check` run on `main` that ran the new tests: a record is of a run that happened.

## Acceptance

- With the connect timeout taken out of a binding's open, that binding's new test fails within its
  own deadline; each of the five planted removals is shown red once (state transition: the bound
  is removed).
- With the listener refusing the connection instead of holding it, each new test fails: a refusal
  does not pass for a timeout (state transition: the fixture refuses).
- No new test runs over 5 s, none opens a connection beyond loopback, and the check of `quality/`
  stays green with the changed page (state transition: the tests are added).

## Out of scope

- The default values themselves and the close bounds
  (story:time-bound-defaults-and-close-bounds-held).
- The entries of `quality/adapters.md`, the lists of `quality/adapters.ratchet.yaml` and the
  records under `quality/records/`: they change when a `check` run on `main` has run the tests.
- Any change to a bound, a default or an error.

## Scope

Coordinator's ruling, 2026-11-25: of the files under "Documents" below, this unit changes `quality/not-measured.md` alone. The entries of `quality/adapters.md`, the plan and the records follow after a `check` run on `main` that ran the new tests.

Derived 2026-11-25 by `story-scoper` at `d99471c4a`. Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** the test directories of four adapter crates (`inference-cartesia`, `inference-phonic`, `inference-elevenlabs`, `inference-deepgram`) and four files under `quality/` — cited
- **Source under `adapters/*/src`:** no change. Each of the five bounds is a public field with a public builder, and no constructor checks its value (`git grep connect_timeout` over the four crates finds only the definition, the builder and the use) — cited

### The five bounds

- **Cartesia text to speech:** `tokio::time::timeout(connect_timeout, handshake)` at `adapters/inference-cartesia/src/connection.rs:62` covers TCP, TLS and the WebSocket handshake (`:60-61`). The credential lookup at `:40-43` is outside it. Field `Endpoint::connect_timeout` (`src/config.rs:40`), builder `Endpoint::with_connect_timeout` (`:72`), default `DEFAULT_CONNECT_TIMEOUT` 10 s (`:28`), passed at `src/synthesiser.rs:282`. Error: `CartesiaError::Connect(ConnectError { status: None, detail: "no handshake within <d>" })` (`src/connection.rs:64-69`, `src/error.rs:58`, `:125-127`) — cited
- **Phonic:** `adapters/inference-phonic/src/duplex.rs:462` in `handshake` (`:428`). The key lookup at `:434-437` is outside it. Field `PhonicEndpoint::connect_timeout` (`src/protocol.rs:118`), builder `with_connect_timeout` (`:152`), default 10 s (`:100`). Error: `PhonicError::Connect { status: None, detail }` (`src/duplex.rs:463-466`, `src/protocol.rs:583`), read through `PhonicSpeech::open_error` (`src/duplex.rs:267`); the open returns `OpenError::Failed` with `SessionFailureKind::TransportClosed` (`:282-296`) — cited
- **ElevenLabs agent:** `adapters/inference-elevenlabs/src/agent_duplex.rs:582` in `handshake` (`:559`), raced against the cancel at `:536-540`. Field `AgentEndpoint::connect_timeout` (`src/agent.rs:111`), builder `with_connect_timeout` (`:147`), default 10 s (`:91`). Error: `AgentError::Connect { status: None, detail }` (`src/agent_duplex.rs:583-586`, `src/agent.rs:528`), read through `open_error` (`src/agent_duplex.rs:372`); kind `TransportClosed` (`:377-395`) — cited
- **Deepgram transcription:** `adapters/inference-deepgram/src/connection.rs:73` in `open_with_scheme` (`:38`), reached through `open` (`:28-35`) from `src/recogniser.rs:771-776`. The key lookup at `src/connection.rs:45-48` is outside it. Field `Endpoint::connect_timeout` (`src/config.rs:64`), builder `with_connect_timeout` (`:106`), default 10 s (`:22`). Error: `DeepgramError::Connect(ConnectError { status: None, detail })` (`src/connection.rs:75-80`, `src/error.rs:96`, `:177-179`), sent to the handle at `src/recogniser.rs:783-786`. The Voice Agent calls the same function with its open timeout and a second bound around it (`src/agent_duplex.rs:517-529`) — cited
- **ElevenLabs text to speech:** `adapters/inference-elevenlabs/src/connection.rs:74` in `Connection::open` (`:38`), called at `src/synthesiser.rs:712-719`. Field `Endpoint::connect_timeout` (`src/config.rs:98`), builder `with_connect_timeout` (`:163`), default 10 s (`:26`). Error: `ElevenLabsError::Connect(ConnectError { status: None, detail })` (`src/connection.rs:76-81`, `src/error.rs:96`, `:224-226`). A connect failure is not reopened: `ended` matches only `Closed` and `Transport` (`src/synthesiser.rs:212-217`). Speech to text calls the same `Connection::open` under two more bounds of its own (`src/recogniser.rs:873-905`, `:509-525`) — cited

### Tests that exist today

- **Cartesia:** none found. Searched every file of `adapters/inference-cartesia/tests/` and `src/`. The only `Connect` assertion is a refusal (`tests/errors.rs:160-165`); the connect timeout is set to 5 s (`tests/support/mod.rs:193`, `tests/adversary_rf1_protocol.rs:117`) — cited
- **Phonic:** none found. Searched `tests/duplex.rs`, `tests/adversary_rf1_phonic.rs`, `tests/support/mod.rs`, `src/`. The only `Connect` assertion is a refusal with 401 (`tests/duplex.rs:491-510`); the connect timeout is 3 s (`tests/duplex.rs:88`) — cited
- **ElevenLabs agent:** none found. `a_cancel_during_the_handshake_closes_the_connection_within_the_close_bound` (`tests/agent_duplex.rs:340-362`) opens against a stalled handshake, cancels after 200 ms with a connect timeout of 3 s (`:68`) and asserts `AgentError::Cancelled`. No test of `tests/` asserts `AgentError::Connect` — cited
- **Deepgram transcription:** none found (determined). Searched all 18 test files of `adapters/inference-deepgram/tests/` and `src/`. `no_reachable_failure_renders_the_key` connects to a port whose listener was dropped, a refused connection (`tests/security1_boundaries.rs:133-137`, `:153-157`). `a_cancel_during_the_opening_handshake_closes_the_connection_within_the_close_bound` holds the handshake open and cancels with a connect timeout of 8 s (`:290-310`). The only `Connect` assertion is a refusal (`tests/auth.rs:34`) — cited
- **ElevenLabs text to speech:** none found (determined). `no_server_at_all_is_a_connect_failure` connects to a closed port and asserts `status == None` (`tests/closed_stream.rs:209-227`); a refused connection gives the same variant with the same status, so it does not fire the timeout. `a_handshake_never_answered_fails_within_the_connect_timeout` (`tests/stt.rs:491-521`) is the speech-to-text path and stays green with `src/connection.rs:74` removed, because `src/recogniser.rs:874` bounds the same call again — cited, the second sentence walked by reading, not run
- **Examples:** `examples/realtime-voice/tests/` sets connect timeouts of 2 s and 5 s for ElevenLabs (`adversary_u3_talk.rs:480`, `compare_elevenlabs.rs:117`, `adversary_compare_cascade_1.rs:106`, `adversary_compare_cascade_2.rs:108`) against fixtures that serve or refuse — inferred, only `adversary_u3_talk.rs:465-481` was read in full

### Files the unit would touch

- **`adapters/inference-elevenlabs/tests/agent_duplex.rs`:** one new test beside `:340`, with `script.handshake = Handshake::Stall` and `endpoint(&server).with_connect_timeout(<under 1 s>)`, asserting `AgentError::Connect { status: None, .. }` and the elapsed time. The fixture needs no change: `Handshake::Stall` exists (`tests/support/agent.rs:56-57`, served at `:225-236`) — inferred (choice of file)
- **`adapters/inference-phonic/tests/duplex.rs`:** one new test beside `:460`, inside `bounded` (`tests/support/mod.rs:36`, deadline 20 s at `:32`) — inferred
- **`adapters/inference-phonic/tests/support/mod.rs`:** a `Stall` variant on `Handshake` (`:72-77`) and its branch at the head of `serve` (`:258-262`), as the ElevenLabs agent fixture has it; or no change, if the test binds its own `std::net::TcpListener` as `adapters/inference-elevenlabs/tests/stt.rs:492-498` does — inferred
- **`adapters/inference-cartesia/tests/errors.rs`:** one new test beside `:160`, inside `within` (`tests/support/mod.rs:564`, deadline 25 s at `:43`), with a listener of its own; `FixtureServer` has no stalled handshake, its `Script` answers generation requests after the upgrade (`tests/support/mod.rs:47-63`) and `launch` knows only `refuse` (`:117`) — inferred
- **`adapters/inference-deepgram/tests/errors.rs`** (or `tests/security1_boundaries.rs`, which already binds raw listeners at `:291`): one new test with a listener of its own; the listen fixture has `Script::Steps` and `Script::Refuse` only (`tests/support/mod.rs:74-78`). The stalled handshake of `tests/support/voice_agent.rs:60-61` belongs to the Voice Agent fixture and does not speak `/v1/listen` — inferred
- **`adapters/inference-elevenlabs/tests/closed_stream.rs`:** one new test beside `:209`, with a listener of its own; the text-to-speech `Script` has no stalled handshake (`tests/support/mod.rs:60-76`) — inferred
- **`crates/inference-testkit`:** gives no listener (no `TcpListener` in `crates/inference-testkit`); not touched — cited
- **New test files:** none. Each crate builds one binary per file under `tests/`; a function in an existing file adds no binary and no `[[test]]` entry (`adapters/inference-elevenlabs/Cargo.toml:39-65`) — inferred

### Documents

- **`quality/adapters.ratchet.yaml`:** one line added to each `inputs.tests` list: `cartesia-time-bounds` (`:155-157`), `deepgram-time-bounds` (`:170-177`), `elevenlabs-time-bounds` (`:207-213`, two lines: agent and text to speech), `phonic-time-bounds` (`:264-267`) — cited
- **`quality/adapters.md`:** a `Connect` bullet in each entry, and the count in its `Baseline` line. Cartesia: entry `:161`, baseline `:168` ("2 of 2"), bullets `:169-176`. Deepgram: entry `:221`, baseline `:228` ("7 of 7"), bullets `:229-252`. ElevenLabs: entry `:354`, baseline `:361` ("6 of 6"), the existing `Connect, speech to text` bullet at `:370-373` is the model, two bullets with names of their own are added (a second line of one name in an entry is a problem, `crates/inference-docs/tests/quality/mod.rs:371-380`). Phonic: entry `:729`, baseline `:735` ("3 of 3"), bullets `:736-745`. The wording "holds the mechanism, at <value>" is defined at `:51-60` — cited
- **`quality/not-measured.md`:** under the heading at `:72`, the first sentence leaves each of three bullets: Cartesia `:76-77`, Phonic `:79-80`, ElevenLabs agent `:82-83`. The second sentence of each ("No test asserts the value of ...", `:77-78`, `:80-81`, `:83-84`) stays; it belongs to story:time-bound-defaults-and-close-bounds-held. The clause "and one that starts a session against a listener that never answers the handshake" at `:116-117` leaves. The bullets for ElevenLabs text to speech (`:85-86`) and Deepgram transcription (`:87-88`) name only defaults and do not change — cited
- **`quality/records/{cartesia,deepgram,elevenlabs,phonic}-time-bounds.evidence.json`:** one entry in `inputs.tests.value`, one result, and the coverage counts, per added test. Cartesia: tests `:19-22`, coverage `:27-35`, results `:36-53`, source `:54-57`. Deepgram: `:19-28`, `:32-40`, `:41-98`, `:99-102`. ElevenLabs: `:19-27`, `:31-39`, `:40-89`, `:90-93`. Phonic: `:19-24`, `:28-36`, `:37-62`, `:63-66`. Each names the candidate `59b8f4a8...` (`:7`) and the check run — cited
- **`quality/README.md:124-129` (table of runs):** a row for the run the new results are read from, if the four records stop being of the check run. An entry's baseline is matched to a run by substring (`crates/inference-docs/tests/quality/mod.rs:676-680`), so a new run name must not contain "check run" — inferred
- **`CHANGELOG.md`:** no entry; nothing a consumer can see changes (`AGENTS.md:45`) — inferred

### The check that holds the pages

- **Check:** `the_quality_documents_agree_with_each_other_and_with_the_taskfile` (`crates/inference-docs/tests/quality/mod.rs:1083-1087`), through `check_named_tests` (`crates/inference-docs/tests/quality/adapters.rs:359-421`) and `check_test` (`:338-354`). It reads all three pages (`mod.rs:86-92`). A page line "test `f` of `file`" goes red when the file does not exist or has no `fn f(` (`adapters.rs:345-352`). An entry that names a test its assessment does not list, or the reverse, goes red (`:389-400`). A listed test without a result in the record, or the reverse, goes red (`:410-420`). The baseline must start with "holds for N of N" as the record has it (`mod.rs:624-647`), and the record's location must end with the number of the run the baseline names (`mod.rs:692-707`) — cited
- **Planted cases of that check:** `a_function_or_a_file_that_left_the_tree_is_a_problem_for_what_names_it` (`adapters.rs:754-787`) and `an_entry_and_a_record_name_the_tests_their_plan_lists` (`:790-813`) plant on `cartesia-time-bounds` and its record; both keep working if the two existing Cartesia tests and their record items stay as written — cited

- **Also likely:** `crates/inference-docs/tests/quality/adapters.rs:718-736` holds the per-crate counts "37 tests" (Cartesia) and "126 tests" (Deepgram) as planted strings, and `quality/adapters.md:157-158`, `:217-218`, `:350-351`, `:725-726` with `quality/records/adapter-tests.evidence.json` hold the counts 37, 126, 163 and 31. They change only if the counts are read again from a new run. Deepgram would then read 127, the value `inference-audio-device` has at `quality/adapters.md:77`; `changed` replaces every occurrence (`mod.rs:1091-1094`), so the planted string at `adapters.rs:729` would hit both sections — inferred
- **Confidence:** high for the tests (the tree names each bound, its builder and the missing test); medium for the documents (which run the new results are read from is not decided in the tree)
- **Would collide with:** any unit that touches the `*-time-bounds/held` entries of `quality/adapters.md`, `quality/not-measured.md:72-118`, the `*-time-bounds` lists of `quality/adapters.ratchet.yaml:150-272`, the four `*-time-bounds.evidence.json` records, or the fixture and test files named above. story:time-bound-defaults-and-close-bounds-held lands on the same entries, lists and records (its file `:32-34`), and its Context names a wave that adds close-bound cases for ElevenLabs and Cartesia (`:26-28`), which would use the same fixtures
- **Safety fact:** no public API changes, because all five bounds are already set through public builders (`inference-cartesia/src/config.rs:72`, `inference-phonic/src/protocol.rs:152`, `inference-elevenlabs/src/agent.rs:147`, `inference-deepgram/src/config.rs:106`, `inference-elevenlabs/src/config.rs:163`) and an existing test sets one to 500 ms (`inference-elevenlabs/tests/stt.rs:502`) — step 2 (pointed at `file:line`), unproven
