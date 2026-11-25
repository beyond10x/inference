---
format: aep.planning-md/3
id: story:close-bounds-fired-by-a-test
kind: story
status: implemented
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
revision: 12
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T10:48:36Z", actor: "human:operator", revision: 8}
- {from: "proposed", to: "active", at: "2026-11-25T10:48:36Z", actor: "human:operator", revision: 9}
- {from: "active", to: "implemented", at: "2026-11-25T12:15:47Z", actor: "human:operator", revision: 12, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":1}}}
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

What the runs of this unit found, on the build node, 2026-11-25:

- The flood backs up all six bindings: 36,000 pings in five of them, 72,000 in Deepslate Opal,
  whose first 36,000 pongs arrived whole in most runs. No read stall, ping or keep-alive ended a
  session first, each set to 30 s or 60 s through the public configuration.
- With the constant at 3 s and at 500 ms the test of each of the six bindings fails. With 1,500 ms,
  1,900 ms and 999 ms it passes: the limits are 1 s and 2 s on the time measured.
- The two places the constant is applied in a session binding, from runs with each change
  planted alone, by the implementor and again by the adversary pass: with the timeout on the
  frame write removed the test of Phonic, of the Deepgram Voice Agent, of Deepslate Opal and of
  the ElevenLabs agent passes (the close held 1.001 s), and with the timeout on the wait for the
  task removed it passes too (1.001 s to 1.002 s). With both removed each test fails at its own
  deadline of 4 s. So the removal of either one alone fails no test, as the scope read had
  inferred.
- In the two transcription bindings the constant is applied once on the connection thread; with
  it removed each test fails at its own deadline of 4 s.

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

As built, 2026-11-25. Commits `edeb51720`, `08c0f5bc5`, `b736f6be8` and `a1dd00dab`. The scope
read of the same day is replaced by this. No file under `adapters/*/src` and no manifest changed,
and no test binary was added.

- Six tests, one per binding, each in an existing test file:
  - `a_close_on_a_backed_up_connection_ends_at_the_close_bound` in
    `adapters/inference-phonic/tests/duplex.rs`,
    `adapters/inference-deepgram/tests/agent_duplex.rs`,
    `adapters/inference-deepslate/tests/opal_duplex.rs` and
    `adapters/inference-elevenlabs/tests/agent_duplex.rs`: the time a close holds its caller,
    called outside an async runtime.
  - `a_cancel_on_a_backed_up_connection_ends_its_thread_at_the_close_bound` in
    `adapters/inference-deepgram/tests/cancel.rs` and `adapters/inference-elevenlabs/tests/stt.rs`:
    the time from the cancel to the end of the recognition's connection thread, found by its name
    under `/proc/self/task`. These two run on Linux and rely on cargo-nextest's process per test;
    every job of the forge runs on Linux.
- The peer of each test: the local fixture sends a flood of WebSocket pings and reads nothing
  more. The binding answers each ping with a pong; the pongs fill the socket buffers and the
  close frame behind them cannot be written. The first flood is 36,000 pings, 72,000 for Deepslate
  Opal. A flood whose pongs all arrived is doubled up to a largest flood the test can read within
  its own waits (288,000 in the two transcription tests, 144,000 for Phonic and the ElevenLabs
  agent, 72,000 for the Deepgram Voice Agent and Deepslate Opal); at the largest the test fails
  and says that nothing was asserted of the bound.
- What a test asserts of a backed-up run: the fixture read no pong before the test let it read
  again, fewer pongs than pings at the end, no close frame, and the time no sooner than 1 s and
  before 2 s. Both limits are numbers written out, never `CLOSE_BOUND`. Every test ends its own
  wait after 4 s and lets the server read again; the two agent tests bound the join of the
  closing thread with 5 s more.
- Fixture additions, in the six existing fixture files: a step that sends the flood, a mode in
  which the server reads nothing until `read_again`, a count of pongs before and after that call,
  and a flag for a close frame.
- Keep-alive, ping and read stall are set to 30 s or 60 s through the public configuration, so
  none ends a session before the close. The Deepgram transcription test sets the public field
  `keepalive_interval`; Deepslate's endpoint takes a read timeout longer than its ping interval.
- What the tests do not tell apart, named on the page:
  - in the four session bindings, which of the two applications of the constant ended the close:
    the test passes with either timeout removed and fails with both removed;
  - a bound between a little under 1 s and a little under 2 s;
  - a dropped handle, the end of input, and a failed open.
- `quality/not-measured.md`: a new list "The close bound of six bindings" under "The command
  exists, the value is not kept" names the six tests, what the fixture tells and what the tests do
  not tell apart. Under "Time bounds no test fires" the entry "Every `CLOSE_BOUND`" keeps what is
  still unfired: Cartesia and ElevenLabs text to speech, each application of the constant alone in
  the session bindings, and the deadline of a Deepgram transcription handle, which two named tests
  pass without reaching.
- `quality/adapters.md`: the one paragraph under "How a time bound is written" says that six
  bindings have a test of the peer that does not take the write and what its limits pass and fail.

Not built, as planned: the entries of `quality/adapters.md`, the plan lists and the records, which
follow from a run of the workflow `check` on `main` that ran the six tests; a test per application
of the constant; the close of Cartesia and of ElevenLabs text to speech, which the wave of
2026-11-24 covers for a dropped synthesis.

Written on a tree without the wave of 2026-11-24: when this branch takes `main` in, the sentences
of `quality/not-measured.md` about Cartesia and ElevenLabs text to speech are to be read against
the tests that wave added.
