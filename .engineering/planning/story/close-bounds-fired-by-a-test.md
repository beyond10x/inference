---
format: aep.planning-md/3
id: story:close-bounds-fired-by-a-test
kind: story
status: active
title: Each close bound that can be reached is fired by a test
tags:
- from-finding
relations:
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-deepgram/tests/agent_duplex.rs
- confidence: cited
  path: adapters/inference-deepgram/tests/cancel.rs
- confidence: cited
  path: adapters/inference-deepgram/tests/support/mod.rs
- confidence: cited
  path: adapters/inference-deepgram/tests/support/voice_agent.rs
- confidence: cited
  path: adapters/inference-deepslate/tests/opal_duplex.rs
- confidence: cited
  path: adapters/inference-deepslate/tests/support/mod.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/agent_duplex.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/stt.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/stt_support/mod.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/support/agent.rs
- confidence: cited
  path: adapters/inference-phonic/tests/duplex.rs
- confidence: cited
  path: adapters/inference-phonic/tests/support/mod.rs
- confidence: cited
  path: quality/adapters.md
- confidence: cited
  path: quality/not-measured.md
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T10:48:36Z", actor: "human:operator", revision: 8}
- {from: "proposed", to: "active", at: "2026-11-25T10:48:36Z", actor: "human:operator", revision: 9}
---
## Context

Split from story:time-bound-defaults-and-close-bounds-held after its scope read of 2026-11-25.
Five constants named `CLOSE_BOUND`, each 1 s and none settable, bound the close of eight bindings
(Cartesia text to speech, Deepgram transcription, the Deepgram Voice Agent, Deepslate Opal,
ElevenLabs text to speech, ElevenLabs speech to text, the ElevenLabs agent, Phonic). A test fixes
the value of each constant. No test fires any of them: the tests that compare a time with the
constant close against a fixture that takes the close at once, so they pass with a larger constant
and with the bound removed
(review-result:quality-per-adapter-and-latency-defined-adversary-1, findings 1 and 10).

What the scope read of this story found, by reading and not by a run:

- A close is one queued frame and a flush. Its write blocks only while bytes are left in the
  WebSocket write buffer in front of it. The adapter's own data writes do not leave it blocked: a
  data write that blocks ends the session at the read timeout, and the task returns without a
  close frame. (The first text of this story said the test has to fill the send path through the
  public interface; that was wrong.)
- A peer that sends many WebSocket pings and reads nothing does block it: each ping read queues a
  pong, the pongs fill the kernel buffers and then the client's write buffer, and the adapter
  writes nothing. Every binding's reader passes over pings, so the flood ends no session. The
  branch of the wave of 2026-11-24 measured this for the two text-to-speech bindings; its fixture
  doubles the flood until the server counts fewer pongs than pings.
- In the four session bindings (Phonic, the Deepgram Voice Agent, Deepslate Opal, the ElevenLabs
  agent) the constant is applied twice on a close: on the frame write in the task and on the
  caller's wait for the task. A test that times the close sees both against the same blocked
  write, so by reading it stays green with either one removed and fails with the constant
  enlarged.
- Deepgram transcription and ElevenLabs speech to text close on a connection thread: `cancel` and
  `Drop` send a command and return. What a test can observe is the end of that thread, and at the
  server a stream that ended with no close frame and fewer pongs than pings.
- Cartesia applies the constant in five places, among them a cancel that waits the constant for
  its send and then the constant again for the close.
- Amazon Nova Sonic has no bound on its close.

The branch of the wave of 2026-11-24 adds cases that fire the bound for a dropped synthesis of
ElevenLabs text to speech and of Cartesia; those two are not built again here.

## Planned scope

- First a run per binding, kept as output and not as a test: the fixture stops reading and sends a
  flood of pings, the close or cancel is called, and the time it held is printed. Its result
  decides which of the six bindings below get a test.
- Six bindings: Phonic, the Deepgram Voice Agent, Deepslate Opal, the ElevenLabs agent, Deepgram
  transcription, ElevenLabs speech to text. For each where the run shows the bound: the smallest
  addition to its existing fixture (a mode in which the peer stops reading, a step that sends the
  flood, for the two transcription fixtures a pong count and a close-frame flag) and one test in
  an existing test file.
  - Session bindings: a close on a backed-up connection ends no earlier than the constant and
    before twice the constant.
  - Transcription bindings: after a cancel on a backed-up connection the connection thread is
    gone by the constant plus a stated slack and not before the constant, or the server finds the
    stream ended with no close frame, whichever the run shows to be stable.
- The flood doubles until the server counts fewer pongs than pings, so its size does not depend on
  the host.
- For a binding where the run does not show the bound, no test: the page says so, with the reason.
- `quality/not-measured.md` follows: the new tests are named under "The command exists, the value
  is not kept", and the section "Time bounds no test fires" keeps what stays unfired. The sentence
  of `quality/adapters.md` that says no test closes against a peer that does not take the write is
  corrected; its entries, the plan lists and the records follow from a `check` run on `main`.
- No file under `adapters/*/src` changes, and no test binary is added.

## Acceptance

- For each binding with a test: with `CLOSE_BOUND` enlarged in the source the test fails, shown
  red once (state transition: the bound is changed).
- For each session binding with a test, the story says for each of the two places the constant is
  applied whether its removal alone fails the test, from a run with that change planted (state
  transition: one application of the bound is removed).
- No new test runs over 5 s, each ends its own wait when nothing blocks, and the gate's test step
  stays within its 120 s (state transition: the tests are added).

## Out of scope

- A test that fires one application of the constant alone in a session binding: the frame write
  through a failed open behind a flood, the wait for the task through a close while the task sits
  in a blocked write.
- The close as an exchange of Cartesia and of ElevenLabs text to speech, the cancel of Cartesia,
  and the deadline of a Deepgram transcription handle that adds the constant to the read timeout.
- The drop of a synthesis of Cartesia and of ElevenLabs text to speech: the branch of the wave of
  2026-11-24 fires it.
- A bound on the close of Amazon Nova Sonic, and making any `CLOSE_BOUND` settable: both change an
  adapter and need a story of their own.
- The entries of `quality/adapters.md`, the lists of `quality/adapters.ratchet.yaml` and the
  records under `quality/records/`.

## Scope

Coordinator's ruling, 2026-11-25: this unit builds the smallest cut of "Size" below, six tests and six fixture additions, after a run per binding. The cut that fires each application of the constant alone is out of scope. Of `quality/adapters.md` only the sentence at lines 62 to 69 changes; its entries, the plan lists and the records follow after a `check` run on `main` that ran the new tests.

Derived 2026-11-25 by `story-scoper` from the tree at `05df3ffea`; the other branch
(`wave/2026-11-24-dub-voice`) was read at `81b2e8690`. Every line is **cited** (read from the
story or the tree) or **inferred** (a reading that could be wrong). Nothing was run: "by reading"
and "needs a run" say how far a claim was taken.

- **Primary surface:** the integration tests and local fixtures of six bindings (Phonic, Deepgram Voice Agent, Deepslate Opal, ElevenLabs agent, Deepgram transcription, ElevenLabs speech to text), and one section of `quality/not-measured.md` — inferred
- **Files:** `adapters/inference-phonic/tests/duplex.rs`, `adapters/inference-phonic/tests/support/mod.rs` — inferred
- **Files:** `adapters/inference-deepgram/tests/agent_duplex.rs`, `adapters/inference-deepgram/tests/support/voice_agent.rs` — inferred
- **Files:** `adapters/inference-deepgram/tests/cancel.rs`, `adapters/inference-deepgram/tests/support/mod.rs` — inferred
- **Files:** `adapters/inference-deepslate/tests/opal_duplex.rs`, `adapters/inference-deepslate/tests/support/mod.rs` — inferred
- **Files:** `adapters/inference-elevenlabs/tests/agent_duplex.rs`, `adapters/inference-elevenlabs/tests/support/agent.rs` — inferred
- **Files:** `adapters/inference-elevenlabs/tests/stt.rs`, `adapters/inference-elevenlabs/tests/stt_support/mod.rs` — inferred
- **Symbols:** `CLOSE_BOUND`, 1 s, five declarations: `adapters/inference-cartesia/src/connection.rs:20`, `adapters/inference-deepgram/src/connection.rs:21`, `adapters/inference-deepslate/src/connection.rs:23`, `adapters/inference-elevenlabs/src/connection.rs:21`, `adapters/inference-phonic/src/duplex.rs:121` — cited
- **Also likely:** `quality/adapters.md:62-69` — inferred, the paragraph ends "No test closes against a peer that does not take the write", which this unit makes untrue; the page is written by hand
- **Documents:** `quality/not-measured.md:72-107`, section "Time bounds no test fires" (lead-in `:74-77`, "Every `CLOSE_BOUND`" `:82-86`, closing sentence `:106-107`) — cited, the story names the page
- **Not touched:** any file under `adapters/*/src`; the story adds tests and fixture modes and makes no constant settable — cited (story, Out of scope)

### How a close frame comes to block

- The close of a socket is one queued frame and a flush; it does not wait for the peer's close frame (tokio-tungstenite 0.30.0 `src/lib.rs:289-294`, `:357-386`; pinned at `Cargo.lock:4539-4540`, `:4718-4719`) — cited
- The frame write blocks only while bytes are left in the WebSocket write buffer in front of it (tungstenite 0.30.0 `src/protocol/frame/mod.rs:250-291`, `src/protocol/mod.rs:528-537`, `:597-607`) — cited
- A read never waits for a write: each ping read queues a pong, and a flush that would block is left for later (tungstenite 0.30.0 `src/protocol/mod.rs:449-466`, `:672`, `:773-781`). So a peer that sends many pings and reads nothing fills the kernel buffers and then the client's write buffer with pongs, while the adapter writes nothing — cited
- Every binding's reader passes over pings, so the flood ends no session: Cartesia `connection.rs:138`, ElevenLabs `connection.rs:141`, Phonic `duplex.rs:520`, `:745`, Deepgram agent `agent_duplex.rs:827`, Deepslate `opal.rs:797`, ElevenLabs agent `agent_duplex.rs:692`, Deepgram transcription `recogniser.rs:902` — cited
- The adapter's own data writes do not leave a blocked close: in the four session bindings a data write is awaited outside the select, ends at the read timeout, and the task returns with no close frame (Phonic `duplex.rs:719-727`, Deepgram agent `agent_duplex.rs:804-810`, Deepslate `opal.rs:773-779`, ElevenLabs agent `agent_duplex.rs:793-799`) — cited; by reading
- No adapter queue takes part in the flood. The queue of 64 commands (`adapters/inference-phonic/src/duplex.rs:127`) matters only for the case "close while the task sits in a blocked write" — cited
- The other branch measured the flood for the two text-to-speech bindings: 9,000 and 18,000 pings of 125 bytes did not back up, 36,000 did (`.engineering/planning/review-result/synthesiser-drop-inside-runtime-adversary-2.md:51-53` on that branch). Its fixture doubles the flood until the server counts fewer pongs than pings, so the number does not depend on the host — cited
- That the same flood backs up the six bindings of this unit — inferred; needs a run per binding

### Per binding: from the public call to the constant

- **Phonic.** `close` `duplex.rs:1193-1202` and `Drop` `:829-836` call `shutdown(true)` `:890-905`: the task leaves its loop `:718` and writes the close frame under the constant (`:768`, `:540`), and the caller waits for the task under the constant `:900`. A failed open reaches the frame write alone: `open` `:333`, `connect` `:419`. Reachable through the public interface: yes, by reading; needs a run — cited
- Phonic, what ends a backed-up session first: the client ping after `ping_interval` and the read stall after `read_timeout` (`duplex.rs:603-643`); a test closes before the first of them or sets them long — cited
- **Deepgram Voice Agent.** `close` `agent_duplex.rs:1419-1429`, `Drop` `:933-940`, `shutdown` `:999-1014`: frame write `:856` to `connection.rs:88`, wait for the task `:1009`. Failed open: `:432`, `:541`. First other bound: `KeepAlive` after `keepalive_interval` (`agent_duplex.rs:694-697`, `config.rs:238`). Reachable: yes, by reading; needs a run — cited
- **Deepslate Opal.** `close` `opal.rs:1426`, `Drop` `:902-909`: frame write `:821` to `connection.rs:71`, wait for the task `:956`. Failed open: `:433`, `:515`. First other bound: the protocol ping every `ping_interval` whatever was read (`opal.rs:676`, `:690-693`); the test helper sets 1 s (`tests/opal_duplex.rs:103`), so the case needs an endpoint of its own. Reachable: yes, by reading; needs a run — cited
- **ElevenLabs agent.** `close` `agent_duplex.rs:1312-1322`, `Drop` `:910-917`, `shutdown` `:961-976`: frame write `:847` to `:658`, wait for the task `:971`. Failed open: `:431`, `:552`; a format the session does not have: `:446-449` (frame write alone, no wait). First other bound: the read stall; WebSocket pings do not move its clock (`:692`, `:784`), so flood and close have to end within `read_timeout` of the last text. Reachable: yes, by reading; needs a run — cited
- **Masking in these four.** On a close or drop outside an async runtime both applications run at 1 s against the same blocked write. By reading, a test that times the close stays green with either one removed and fails only with the constant enlarged or both removed. The frame write alone is seen through a failed open behind a flood (`open` returns only after it). The wait alone is seen through a close while the task sits in a blocked data write — inferred; needs a run with each planted change
- **Deepgram transcription.** `cancel` `recogniser.rs:299-303` and `Drop` `:571-575` send `Command::Cancel` `:305-309`; the connection thread takes it `:732` and closes `:845-848` to `connection.rs:88`. End of input closes at `:849-852`, and only then is the last word sent `:855-857`. Third use the story does not count: the handle's deadline `read_timeout + CLOSE_BOUND` `:377`. A cancel is read only after the command in progress ends `:808`. First other bound: `KeepAlive` every 4 s (`config.rs:31`, `recogniser.rs:831-837`). Reachable: yes, by reading; needs a run — cited
- **ElevenLabs speech to text.** `cancel` `recogniser.rs:389-393` and `Drop` `:703-707` send `Command::Cancel` `:395-399`; the thread takes it `:853` and closes `:950-953` to `connection.rs:150`. End of input: `:954-957`. Handle gone at open: `:891`. Reachable: yes, by reading; needs a run — cited
- **What a test of these two can observe.** At the server today: `Seen.client_closed`, a bool that a close frame and a dropped socket set alike and that carries no time (`adapters/inference-deepgram/tests/support/mod.rs:159-160`, `:431`, `:437`; `adapters/inference-elevenlabs/tests/stt_support/mod.rs:203-204`, `:469`, `:475`) — cited
- What would tell a fired bound at the server: the server reads again later than the constant after the cancel and finds the stream ended with no close frame and fewer pongs than the pings it sent. That needs a pong count and a close-frame flag in `Seen` — inferred
- In the test process: the connection thread, named `inference-deepgram` (`recogniser.rs:622-624`) or `inference-elevenlabs-stt` (`recogniser.rs:758`), is still listed shortly after the cancel and gone by the constant plus slack. Linux only (`/proc/self/task`). The 15-byte kernel name of the ElevenLabs thread is the same as that of the text-to-speech runtime thread (`synthesiser.rs:358`) — inferred
- **Cartesia.** Frame write `connection.rs:146`. Drop: `synthesiser.rs:619-638` (close started `:630`, wait `:633`). Close as an exchange: `:498-504`, settled `:362-365` with no deadline of its own `:589`. Cancel: `discard` `:675-696`, `cancel_live` `:463-487` (send under the constant `:473`, wait until twice the constant `:479`), on a failed send `close_now` `:490-494` (wait `:492`) — cited
- Cartesia cancel, reachable without a change under `src/`: yes, by reading. After a flood and one chunk without `done`, `discard` of the segment being spoken waits the constant for its send and then the constant again in `close_now`. The other branch measured 2.0016 s (`.engineering/planning/review-result/synthesiser-drop-inside-runtime-adversary-1.md:125` on that branch). The deadline of twice the constant at `:479` is not what ends the wait and is unreachable by reading — cited
- **ElevenLabs text to speech.** Frame write `connection.rs:150`. Drop: `synthesiser.rs:759-778` (close `:770`, wait `:773`). Close as an exchange: `close_connection` `:541-548` at the end of an utterance `:582-584`, settled `:388`. `discard` does no I/O `:814-827` — cited
- Both text-to-speech bindings: the other branch fires the drop. The close as an exchange is fired by no test on either branch, and it is the one place there where the frame write is the only bound — cited

### Fixtures

- Phonic: has the mode. `Script.deaf` `tests/support/mod.rs:88-90`, served at `:289-300` (sends the ready messages, runs the steps, reads nothing). Smallest addition: one `Step` that sends N WebSocket pings (arm in `run` `:349-388`); for a failed-open case, `deaf` has to honour `ready` — cited for what exists, inferred for the addition
- Deepgram Voice Agent: no such mode; the reader task always runs (`tests/support/voice_agent.rs:295-336`) and the server waits for the client's first message `:344`. Smallest addition: one `Script` field (`:79-97`) that answers the opening without starting the reader, and the ping step (`run` `:385-416`) — cited, addition inferred
- Deepslate Opal: no such mode; reader task `tests/support/mod.rs:338-394`, first message awaited `:396`. Same addition: one `Script` field (`:99-117`) and the ping step (`run` `:429-457`) — cited, addition inferred
- ElevenLabs agent: no such mode; reader task `tests/support/agent.rs:264-290`, first message awaited `:292`. Same addition: one `Script` field (`:62-81`) and the ping step (steps `:313-345`) — cited, addition inferred
- Deepgram transcription: `Action::Wait` already keeps the single loop from reading (`tests/support/mod.rs:61-62`, `:384`, read at `:387`). Smallest addition: one `Action` that sends N pings (`:372-384`), a pong count where pongs are passed over `:430`, and a close-frame flag where close and error are taken alike `:431` — cited, addition inferred
- ElevenLabs speech to text: the same shape in `tests/stt_support/mod.rs` (`Action::Wait` `:69-70`, `:433`; read `:436`; pongs `:468`; close and error alike `:469`) and the same three additions — cited, addition inferred
- Cartesia and ElevenLabs text to speech: `tests/support/mod.rs` has no mode that stops reading (`Script` at `adapters/inference-cartesia/tests/support/mod.rs:46-63`, `adapters/inference-elevenlabs/tests/support/mod.rs:56-76`). The other branch adds a separate file `tests/support/deaf.rs` in each crate instead — cited
- Shape to reuse from the other branch: a flood of a fixed first size that doubles until the server, reading again on release, counts fewer pongs than pings; a count of the frames read after release; a list of the process's threads by name (`tests/support/deaf.rs`, `tests/support/dropped.rs` on that branch) — cited

### Size

- Smallest cut: six tests and six fixture additions. Four session bindings: a close outside a runtime on a backed-up connection; it fails with the constant enlarged and does not tell the two applications apart. Two transcription bindings: a cancel on a backed-up connection — inferred
- Cut that meets the Acceptance sentence about each of the two places: two more tests per session binding (a failed open behind a flood; a close while the task sits in a write), twelve there, plus the two transcription tests, and one more if the Deepgram deadline `recogniser.rs:377` is to be fired — inferred
- No new test binary when the tests go into the existing files named above — inferred
- Each test waits one constant and one flood. The other branch records no backed-up case over 5 s (`.engineering/planning/review-result/synthesiser-drop-inside-runtime-adversary-1.md:46` on that branch). The time of each case here and its share of the 120 s test step (`Taskfile.yml:32-35`, 60 s kill in `.config/nextest.toml`) need a run — cited for the record, inferred for this unit

- **Confidence:** medium — the story names no test file and no fixture; every close path was read to its `file:line` and the blocking mechanism is read from the pinned library and measured on another branch for two bindings, and it is not run for the six bindings of this unit
- **Would collide with:** any unit that edits the twelve test and fixture files above; `adapters/inference-elevenlabs/tests/agent_duplex.rs` in particular, which this branch and the other branch both changed; any unit that edits `quality/not-measured.md:72-107` or `quality/adapters.md:62-69`; any new `tests/support/deaf.rs` or `tests/support/dropped.rs` in the Cartesia and ElevenLabs crates (the other branch creates them); the other branch's draft on the wait of Cartesia's `discard`, which a test pinning about twice the constant would contradict; line numbers of `adapters/inference-elevenlabs/src/agent_duplex.rs`, which move by about 91 lines when the other branch merges — inferred
- **Safety fact:** the unit changes no file under `src/`, and a fixture addition reaches only the fixture: a new `Action` is matched only in the fixture's own loop (`adapters/inference-deepgram/tests/support/mod.rs:372-384`, `adapters/inference-elevenlabs/tests/stt_support/mod.rs:421-433`), every `Seen` derives `Default`, and every fixture file allows dead code. Not proven: that every `Script` literal in the tests of the three session crates still compiles with one more field — step 2, unproven — cited
