---
format: aep.planning-md/3
id: story:time-bound-defaults-and-close-bounds-held
kind: story
status: active
title: Each time-bound default is asserted and each close bound is fired by a test
tags:
- from-finding
relations:
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-audio-device/tests/acknowledging.rs
- confidence: cited
  path: adapters/inference-aws/tests/nova_sonic_stall_and_bounds.rs
- confidence: cited
  path: adapters/inference-cartesia/tests/errors.rs
- confidence: cited
  path: adapters/inference-deepgram/tests/agent_duplex.rs
- confidence: cited
  path: adapters/inference-deepgram/tests/errors.rs
- confidence: cited
  path: adapters/inference-deepslate/tests/opal_duplex.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/agent_duplex.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/closed_stream.rs
- confidence: cited
  path: adapters/inference-llama-server/tests/offline.rs
- confidence: cited
  path: adapters/inference-openai/tests/gpt_live_duplex.rs
- confidence: cited
  path: adapters/inference-openai/tests/speech_request.rs
- confidence: cited
  path: adapters/inference-openai/tests/transport_deadlines.rs
- confidence: cited
  path: adapters/inference-phonic/tests/duplex.rs
- confidence: cited
  path: quality/not-measured.md
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T09:03:47Z", actor: "human:operator", revision: 8}
- {from: "proposed", to: "active", at: "2026-11-25T09:03:47Z", actor: "human:operator", revision: 9}
---
## Context

`quality/adapters.md` says for each time bound of an adapter whether a test asserts its default
value or only holds the mechanism with a shorter value
(story:quality-per-adapter-and-latency-defined). The scope read of 2026-11-25 counted 38
time-bound defaults in nine adapter crates. Four have a test that fixes the value: `OPEN_TIMEOUT`
of Amazon Nova Sonic, `DEFAULT_AGENT_KEEPALIVE_INTERVAL` of Deepgram, and `DEFAULT_READ_TIMEOUT`
and `CLOSE_TIMEOUT` of OpenAI. The other 34 have none, the five `CLOSE_BOUND` constants among
them. A change of one of them is a change a caller can observe, and the gate stays green on it.

Two things the read found:

- A comparison such as `endpoint.read_timeout == DEFAULT_READ_TIMEOUT` passes for any value. Two
  assertions of `inference-openai` are of this kind today.
- Eight of the 34 have no number outside their definition: the connect timeout of Cartesia,
  Deepgram, the ElevenLabs agent and Phonic, the read timeout of the Deepgram Voice Agent, and the
  three limits of llama-server, which are the defaults of a pinned dependency.

The draft of this story also held the close bounds that no test fires. That part is now
story:close-bounds-fired-by-a-test: it needs a peer that stops reading in each fixture and a
design of its own, and the read could not determine whether every bound can be reached.

## Planned scope

- Tests that compare each of the 34 defaults with its number written as a literal, read through
  the public default (`Endpoint::new()`, the default configuration, the public constant), in
  existing test files: about one test per binding, 13 in the scope read's cut.
- Where a document outside the source names the number (the changelog, the documentation site),
  the test's comment names that place. Where none does, the literal in the test is the reference.
- The three limits of llama-server are compared too: a bump of the pinned dependency that changes
  them is a change of this adapter's defaults and has to fail a test.
- The two empty comparisons of `inference-openai` get the numbers.
- `quality/not-measured.md`: the tests are named under "The command exists, the value is not
  kept"; the lines about defaults no test asserts leave the list of what nothing holds. The
  sentence about a wait of the close of Amazon Nova Sonic is corrected: the adapter has no such
  wait.
- `quality/adapters.md`, its plan and the records follow in a later change, from the first `check`
  run on `main` that ran the new tests.
- No file under `adapters/*/src` changes, and no test binary is added.

## Acceptance

- Each of the 34 defaults, changed to another value in the source, fails a test of its crate; each
  planted change is shown red once (state transition: a default is changed).
- No assertion of the new tests compares a default with the constant it is defined by (state
  transition: the tests are added).
- No new test runs over 5 s, none opens a connection, and the check of `quality/` stays green with
  the changed page.

## Out of scope

- Changing any default or bound.
- Firing a close bound (story:close-bounds-fired-by-a-test) and the connect timeouts
  (story:connect-timeouts-fired-by-a-test).
- The quiet windows and slacks that are not defaults a caller sets (`TRAILING_QUIET`,
  `MESSAGE_QUIET`, `VAD_END_QUIET`, `OPEN_SLACK`) and the defaults of the loopback device.
- The entries of `quality/adapters.md`, the lists of `quality/adapters.ratchet.yaml` and the
  records under `quality/records/`.

## Scope

Coordinator's ruling, 2026-11-25: this story builds the default tests alone, the value of the five `CLOSE_BOUND` constants among them. The close tests below ("Close bounds", "Close tests", "Not determined") are story:close-bounds-fired-by-a-test. Of the files under "Documents" this unit changes `quality/not-measured.md` alone; the entries of `quality/adapters.md`, the plan and the records follow after a `check` run on `main` that ran the new tests.

Derived 2026-11-25 by `story-scoper` at `f91b78b6b`, read-only. Every line is **cited** (read
from the story or the tree) or **inferred** (a reading that could be wrong). The tables are
cited: each row was read at the place it names. The choice of test file is inferred throughout:
the story names none.

- **Primary surface:** existing test files and fixtures under `adapters/*/tests/` of nine
  crates, and `quality/not-measured.md` — inferred for the test files, cited for the document
  (story, Planned scope).
- **Not touched:** `adapters/*/src`, any manifest, `quality/adapters.md`,
  `quality/adapters.ratchet.yaml`, `quality/records/` — cited, the rule in use on this branch
  (`.engineering/planning/story/connect-timeouts-fired-by-a-test.md`, Out of scope).
- **Confidence:** medium — the inventory is read line by line; whether each close bound can be
  reached through the public API is not proven, and for the Cartesia cancel it looks unreachable.

### Defaults: 38, of which 34 have no test that fixes the value

`task test-run` runs `cargo nextest run --workspace --all-features` (`Taskfile.yml:29`, `:37-42`).
Searched: each constant name, and every `assert` line that names `*_timeout`, `idle_limit` or
`*_interval`, under `adapters/`, `crates/` and `examples/`. No adapter crate has a README.
"Page" is `quality/adapters.md`.

| Default | Value | Defined at | Number outside its definition | Test that fixes the value |
|---|---|---|---|---|
| **inference-audio-device** | | | | |
| `PLAYBACK_PATIENCE` (pub) | 2 s | `src/acknowledging.rs:180` | page `:90-91`; `CHANGELOG.md:748` | none found; used as a value at `tests/acknowledging.rs:479`, `:1033`, `:1045` |
| **inference-aws** | | | | |
| `OPEN_TIMEOUT` (pub) | 10 s | `src/nova_sonic/client.rs:40` | page `:117`; `CHANGELOG.md:804-805` | `the_open_deadline_is_stated`, `tests/nova_sonic_stall_and_bounds.rs:279` |
| read timeout of `SessionConfig::new` (pub field, no constant) | 30 s | `src/nova_sonic/config.rs:134` | page `:124`; doc comment `src/nova_sonic/config.rs:124` | none found; `tests/nova_sonic_session.rs:394` asserts only over zero |
| idle limit of `default_session_config()` (pub fn) | 30 s | `src/duplex.rs:176` | page `:128`; doc comment `src/duplex.rs:162` | none found; no test calls the function |
| **inference-cartesia** | | | | |
| `DEFAULT_READ_TIMEOUT` (pub) | 10 s | `src/config.rs:25` | page `:169` | none found |
| `DEFAULT_CONNECT_TIMEOUT` (pub) | 10 s | `src/config.rs:28` | none | none found |
| `CLOSE_BOUND` (pub) | 1 s | `src/connection.rs:20` | page `:172` | none found |
| **inference-deepgram** | | | | |
| `DEFAULT_CONNECT_TIMEOUT` (pub) | 10 s | `src/config.rs:22` | none | none found |
| `DEFAULT_READ_TIMEOUT` (pub) | 10 s | `src/config.rs:26` | page `:232` | none found |
| `DEFAULT_KEEPALIVE_INTERVAL` (pub) | 4 s | `src/config.rs:31` | page `:229`; the doc comment `src/config.rs:28-30` gives a range, not the number | none found |
| `DEFAULT_AGENT_OPEN_TIMEOUT` (pub) | 15 s | `src/config.rs:168` | page `:240` | none found |
| `DEFAULT_AGENT_READ_TIMEOUT` (pub) | 30 s | `src/config.rs:172` | none | none found |
| `DEFAULT_AGENT_KEEPALIVE_INTERVAL` (pub) | 8 s | `src/config.rs:176` | page `:243`; doc comment `src/config.rs:174-175`; `src/agent.rs:29` | unit test `the_agent_endpoint_takes_a_plain_scheme_and_a_checked_url`, `src/config.rs:457` |
| `CLOSE_BOUND` (pub) | 1 s | `src/connection.rs:21` | page `:235` | none found |
| **inference-deepslate** | | | | |
| `DEFAULT_OPEN_TIMEOUT` (pub) | 15 s | `src/config.rs:23` | page `:310` | none found |
| `DEFAULT_READ_TIMEOUT` (pub) | 30 s | `src/config.rs:27` | page `:317` | none found; `src/config.rs:300-318` holds only the rule against the ping |
| `DEFAULT_PING_INTERVAL` (pub) | 10 s | `src/config.rs:31` | page `:313`; doc comment `src/config.rs:29`; `protocol/README.md:13` | none found |
| `CLOSE_BOUND` (pub) | 1 s | `src/connection.rs:23` | page `:321` | none found |
| **inference-elevenlabs** | | | | |
| `DEFAULT_READ_TIMEOUT`, endpoint of text to speech and speech to text (pub) | 10 s | `src/config.rs:23` | page `:362`; `website/docs/guides/compare-voice-providers.md:95` | none found |
| `DEFAULT_CONNECT_TIMEOUT`, same endpoint (pub) | 10 s | `src/config.rs:26` | page `:370` | none found |
| agent `DEFAULT_CONNECT_TIMEOUT` (pub) | 10 s | `src/agent.rs:91` | none | none found |
| agent `DEFAULT_METADATA_TIMEOUT` (pub) | 10 s | `src/agent.rs:94` | page `:377` | none found |
| agent `DEFAULT_READ_TIMEOUT` (pub) | 30 s | `src/agent.rs:97` | page `:381` | none found |
| `CLOSE_BOUND` (pub) | 1 s | `src/connection.rs:21` | page `:366-367` | none found |
| **inference-llama-server** | | | | |
| `Config::limits` = `Limits::default()` of the pinned `llm-http` (pub field `src/lib.rs:155`): response headers, idle, total | 60 s, 180 s, 600 s | taken at `src/lib.rs:172`; the numbers stand only in the dependency (`crates/llm-http/src/transport.rs:46-48` at tag 0.20.0, pinned by `Cargo.toml:23` of the crate) | none in this tree (`quality/not-measured.md:121-122`) | none found; `tests/cancellation.rs:44` sets its own |
| **inference-openai** | | | | |
| `DEFAULT_READ_TIMEOUT` (pub) | 30 s | `src/config.rs:22` | page `:623`; doc comment `src/speech/config.rs:40-41` | `endpoint_is_built_and_destructured_with_its_0_16_0_fields`, `tests/endpoint_literal.rs:24` |
| `DEFAULT_CONNECT_TIMEOUT` (pub) | 10 s | `src/config.rs:28` | page `:610`; `src/lib.rs:13` | none fixes it: `tests/transport_deadlines.rs:145`, `:147` hold "is the constant" and "under 80 s" |
| `DEFAULT_WRITE_TIMEOUT` (pub) | 10 s | `src/config.rs:35` | page `:610-611`; `src/lib.rs:15` | none fixes it: `tests/transport_deadlines.rs:146`, `:148` |
| `CLOSE_TIMEOUT` (pub) | 15 s | `src/live.rs:106` | page `:637`; `CHANGELOG.md:9-10`; doc comment `src/live.rs:104` | `tests/gpt_live.rs:560`; `tests/gpt_live_duplex.rs:1459` |
| Realtime idle limit of `default_session_config()` (pub fn) | 30 s | `src/duplex.rs:178` | page `:633`; doc comment `src/duplex.rs:159` | none found; no test calls the function |
| GPT-Live idle limit of `default_session_config()` (pub fn) | 30 s | `src/live_duplex.rs:202` | doc comment `src/live_duplex.rs:182-183` only; not on the page | none found |
| **inference-phonic** | | | | |
| `DEFAULT_CONNECT_TIMEOUT` (pub) | 10 s | `src/protocol.rs:100` | none | none found |
| `DEFAULT_READY_TIMEOUT` (pub) | 10 s | `src/protocol.rs:103` | page `:737` | none found |
| `DEFAULT_READ_TIMEOUT` (pub) | 30 s | `src/protocol.rs:106` | page `:740` | none found |
| `DEFAULT_PING_INTERVAL` (pub) | 10 s | `src/protocol.rs:109` | page `:740` | none found |
| `CLOSE_BOUND` (pub) | 1 s | `src/duplex.rs:121` | page `:743` | none found |

- Counts: audio-device 1, aws 3, cartesia 3, deepgram 7, deepslate 4, elevenlabs 6, llama-server
  3, openai 6, phonic 5. Four are fixed by a test today; of the 34 others, 5 are `CLOSE_BOUND`
  and 29 are other defaults — cited.
- `inference-sherpa-onnx` and `inference-whispercpp` have no time-bound constant (page `:775`,
  `:793`; no `Duration` constant under their `src/`) — cited.
- The speech binding of OpenAI has no constant of its own: `SpeechEndpoint::new` takes
  `DEFAULT_CONNECT_TIMEOUT` and `DEFAULT_READ_TIMEOUT`
  (`adapters/inference-openai/src/speech/config.rs:45-46`) — cited.

### Close bounds: five `CLOSE_BOUND`, all 1 s, fixed, none fired

| Binding | Where the bound is applied | Test that compares a time with it | Fired |
|---|---|---|---|
| Cartesia text to speech | frame write `src/connection.rs:146`; cancel send `src/synthesiser.rs:473` (timeout at `src/connection.rs:92`), waited for until twice the constant at `:479`; close after a failed cancel `:485`, `:492`; orderly close `:498-504`; drop `:633` | `a_cancel_does_not_wait_for_the_server`, `tests/cancel.rs:112`, asserts `<= CLOSE_BOUND` at `:124`; the fixture answers the cancel at once (`tests/support/mod.rs:291-295`). The drop test `tests/cancel.rs:93` asserts no time | no |
| Deepgram transcription | frame write `src/connection.rs:88`, called at `src/recogniser.rs:846`, `:850`; an addend of the summary deadline at `src/recogniser.rs:377` | `tests/security1_boundaries.rs:290` asserts `<= CLOSE_BOUND + 500 ms` at `:323`, with the handshake never answered; `tests/cancel.rs:12`, `:43` assert no time | no |
| Deepgram Voice Agent | frame write `src/connection.rs:88`, called at `src/agent_duplex.rs:541`, `:856`; wait for the connection task `src/agent_duplex.rs:1009` (close `:1419`, drop `:933`) | `tests/agent_duplex.rs:560` (`< CLOSE_BOUND` at `:574`, `:579`); `:583` (`< CLOSE_BOUND * 2` at `:596`) | no |
| Deepslate Opal | frame write `src/connection.rs:71`; wait for the task `src/opal.rs:956` | `tests/opal_duplex.rs:1046` (`<= CLOSE_BOUND * 2` at `:1062`) | no |
| ElevenLabs text to speech | farewell and frame `src/connection.rs:150`; drop `src/synthesiser.rs:773`; close as an exchange `src/synthesiser.rs:544` | none asserts a time on a close (`quality/not-measured.md:105-114`) | no; left to the other branch |
| ElevenLabs speech to text | `src/connection.rs:150`, called at `src/recogniser.rs:891`, `:951`, `:955` | none; no speech-to-text test names `CLOSE_BOUND` | no |
| ElevenLabs agent | frame write `src/agent_duplex.rs:658`; wait for the task `src/agent_duplex.rs:971` | `tests/agent_duplex.rs:485` (`:498`, `:503`); `:507` (`< CLOSE_BOUND * 2` at `:519`) | no |
| Phonic | frame write `src/duplex.rs:540`; wait for the task `src/duplex.rs:900` | `tests/duplex.rs:402` (`:418`, `:421`); `:359` (`< CLOSE_BOUND * 2` at `:373`) | no |
| OpenAI GPT-Live | `CLOSE_TIMEOUT`, settable (`src/live.rs:892`, `src/live_duplex.rs:407`), applied at `src/live.rs:1063`, `src/live_duplex.rs:1290` | `close_gives_up_at_its_bound_across_every_read`, `tests/gpt_live.rs:588`, at 2 s | yes; nothing to add |
| OpenAI Realtime | the read timeout, `src/client.rs:502-508`, `:1103-1125` | `tests/adversary16_u1_split.rs:343` | no `CLOSE_BOUND`; nothing to add |
| Amazon Nova Sonic | none: the close sends a command (`src/duplex.rs:1362`) and the task runs `writer.close()` with no timeout (`src/duplex.rs:671-674`) | none | no bound exists |

- No `CLOSE_BOUND` is settable: no endpoint has a field for it (for example
  `adapters/inference-cartesia/src/config.rs:35-41`,
  `adapters/inference-phonic/src/protocol.rs:113-125`) — cited.
- Cartesia cancel, verified: the send inside the task gives up at once the constant
  (`src/synthesiser.rs:473`), the caller waits for that task until twice the constant (`:479`),
  and a failed send is followed by a close that waits up to the constant again (`:485`,
  `:490-493`). The test asserts once the constant (`tests/cancel.rs:124`) — cited.
- Left out, as the story says: the drop of a synthesis of ElevenLabs text to speech and of
  Cartesia. No fixture of this tree stops reading for them (Cartesia `Script`,
  `adapters/inference-cartesia/tests/support/mod.rs:47-63`; ElevenLabs `Script`,
  `adapters/inference-elevenlabs/tests/support/mod.rs:56-78`) — cited.

### Files the unit would touch

Default tests, one per binding, each a comparison of `Endpoint::new()` or the default
configuration with literal numbers — all inferred:

- `adapters/inference-audio-device/tests/acknowledging.rs`: `PLAYBACK_PATIENCE` (the file
  imports it at `:18`).
- `adapters/inference-aws/tests/nova_sonic_stall_and_bounds.rs`: read timeout and idle limit of
  Nova Sonic, beside `the_open_deadline_is_stated` (`:275-281`).
- `adapters/inference-cartesia/tests/errors.rs`: read and connect.
- `adapters/inference-deepgram/tests/errors.rs`: connect, read, keep-alive of transcription.
- `adapters/inference-deepgram/tests/agent_duplex.rs`: open and read of the Voice Agent.
- `adapters/inference-deepslate/tests/opal_duplex.rs`: open, read, ping.
- `adapters/inference-elevenlabs/tests/closed_stream.rs`: read and connect of the endpoint that
  text to speech and speech to text share.
- `adapters/inference-elevenlabs/tests/agent_duplex.rs`: connect, metadata, read of the agent.
- `adapters/inference-llama-server/tests/offline.rs`: the three limits (a default `Config` is
  built at `:251`).
- `adapters/inference-openai/tests/transport_deadlines.rs`: connect and write as numbers, and
  the Realtime idle limit.
- `adapters/inference-openai/tests/gpt_live_duplex.rs`: the GPT-Live idle limit.
- `adapters/inference-openai/tests/speech_request.rs`: connect and read of `SpeechEndpoint::new`
  (the file asserts its URL at `:187`).
- `adapters/inference-phonic/tests/duplex.rs`: connect, ready, read, ping.

Close tests, one per binding with an unfired `CLOSE_BOUND` that this story keeps — all inferred:

- `adapters/inference-phonic/tests/duplex.rs`: no fixture change. `Script.deaf`
  (`tests/support/mod.rs:88-90`, served at `:289-300`) already reads nothing; it records no
  close, so the test times the drop itself.
- `adapters/inference-deepgram/tests/cancel.rs` and `tests/support/mod.rs`: `Action::Wait`
  (`:61-62`, run at `:384`) already keeps the server from reading, since it reads only at
  `:387`. The end of the loop (`:432`) records a close frame and a dropped socket as the same
  `client_closed` (`:159-160`); one more field of `Seen` would tell them apart.
- `adapters/inference-elevenlabs/tests/stt.rs` and `tests/stt_support/mod.rs` (not under
  `tests/support/`): the same shape: `Action::Wait` `:69-70`, run at `:434`, read at `:436`,
  end at `:473`, `client_closed` at `:204-205`.
- `adapters/inference-deepgram/tests/agent_duplex.rs` and `tests/support/voice_agent.rs`: the
  reader task (`:295-335`) always reads and `Step::Pause` (`:50-51`) stops only the writer. One
  field of `Script` (`:79-85`, `:89-97`) that stops the reader, like the `deaf` field of Phonic.
- `adapters/inference-deepslate/tests/opal_duplex.rs` and `tests/support/mod.rs`: the same
  addition, to `Script` (`:101-119`) and the reader (`:337-393`).
- `adapters/inference-elevenlabs/tests/agent_duplex.rs` and `tests/support/agent.rs`: the same
  addition, to `Script` (`:62-81`) and the reader (`:264` on).
- `adapters/inference-cartesia/tests/cancel.rs` and `tests/support/mod.rs`: only if the longer
  wait of a cancel can be reached at all; see "Not determined".

Documents:

- `quality/not-measured.md` — cited (story, Planned scope). Under the rule in use, a new test is
  named under "The command exists, the value is not kept" (`:252`), in a list like the one at
  `:290-311`. The lines that would leave "Time bounds no test fires, and defaults no test
  asserts":
  - `:78-79` Cartesia, `:80-81` Phonic, `:82-84` ElevenLabs agent, `:85-86` ElevenLabs text to
    speech and speech to text, `:87-88` Deepgram transcription, `:89-90` Deepgram Voice Agent,
    `:91-92` Deepslate, `:93-95` OpenAI, `:98` audio device.
  - `:96-97` Amazon Nova Sonic: the first sentence moves; the second stays, since no bound
    exists.
  - `:119-124`, the section on the limits of llama-server.
  - `:99-101` "Every `CLOSE_BOUND`" shrinks to the closes that stay unfired.
  - `:102-104` Cartesia cancel moves only if a test reaches the longer wait.
  - `:105-114` ElevenLabs text to speech stays for the other branch.
  - `:74-76` and `:116-117`, the lead-in and the closing paragraph, are reworded.
- `quality/adapters.md` is not changed in this unit — cited. Its entries that go stale until a
  `check` run on `main`: `:90-97`, `:124-131`, `:169-176`, `:229-252`, `:310-324`, `:362-388`,
  `:462-465`, `:610-614`, `:633-636`, `:737-746`. The paragraph at `:62-69` is outside any entry
  and becomes untrue with the first close test ("No test closes against a peer that does not
  take the write").
- `quality/adapters.ratchet.yaml` is not changed — cited. The lists are at `:118-131` (audio
  device), `:133-148` (aws), `:150-163` (cartesia), `:165-183` (deepgram), `:185-200`
  (deepslate), `:202-219` (elevenlabs), `:221-233` (llama-server), `:235-257` (openai),
  `:259-272` (phonic); the requirements at `:657-700`; the gate names them at
  `quality/gate.ratchet.yaml:371-379`.
- `quality/records/*-time-bounds.evidence.json` are not changed — cited. Their test items:
  audio-device `:41`, `:49`; aws `:43-67`; cartesia `:41`, `:49`; deepgram `:46-94`; deepslate
  `:43-67`; elevenlabs `:45-85`; llama-server `:40`; openai `:50-130`; phonic `:42-58`.

### Size

- 13 default tests, one per row of the first list above; 14 if ElevenLabs speech to text gets
  its own beside text to speech, 9 if cut one per crate — inferred.
- 6 close tests (Phonic, Deepgram transcription, ElevenLabs speech to text, Deepgram Voice
  Agent, Deepslate, ElevenLabs agent), and a seventh for the Cartesia cancel if it is reachable
  — inferred.
- No new file under `tests/`: every file named above exists — cited.
- None would run over 5 s by the constants: a default test waits for nothing, a close test waits
  for one `CLOSE_BOUND` of 1 s, the Cartesia cancel for two. The time a test needs to fill the
  socket buffer before the close is not known — inferred.

### Would collide with

- Any unit that touches the test files and fixtures named above; `adapters/inference-phonic/tests/duplex.rs`,
  `adapters/inference-elevenlabs/tests/agent_duplex.rs`, `adapters/inference-cartesia/tests/errors.rs`
  and `adapters/inference-deepgram/tests/errors.rs` were last changed by
  story:connect-timeouts-fired-by-a-test — cited.
- The branch that fires the drop of Cartesia and of ElevenLabs text to speech: by its
  description it adds a mode that stops reading to `adapters/inference-cartesia/tests/support/mod.rs`
  and `adapters/inference-elevenlabs/tests/support/mod.rs`, and changes
  `quality/not-measured.md:99-114` — inferred; that branch was not read.
- Any unit that edits `quality/not-measured.md:72-124` or `:252-311` — cited.

### Safety fact

- The check of `quality/` binds each `*-time-bounds/held` entry to the list of its plan and to
  its record (`crates/inference-docs/tests/quality/adapters.rs:370-421`), and holds for every
  page, `quality/not-measured.md` included, only that a test named as "test `<fn>` of `<file>`"
  exists (`:303-321`, `:365-369`). So new tests named only in `quality/not-measured.md` keep the
  check green, as long as no entry, list or record changes and no listed test is renamed — cited
  at step 2 (pointed at `file:line`), unproven.
