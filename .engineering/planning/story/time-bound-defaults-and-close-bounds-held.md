---
format: aep.planning-md/3
id: story:time-bound-defaults-and-close-bounds-held
kind: story
status: implemented
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
  path: adapters/inference-openai/tests/duplex.rs
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
revision: 14
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T09:03:47Z", actor: "human:operator", revision: 8}
- {from: "proposed", to: "active", at: "2026-11-25T09:03:47Z", actor: "human:operator", revision: 9}
- {from: "active", to: "implemented", at: "2026-11-25T10:29:32Z", actor: "human:operator", revision: 13, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":1}}}
---
## Context

`quality/adapters.md` says for each time bound of an adapter whether a test asserts its default
value or only holds the mechanism with a shorter value
(story:quality-per-adapter-and-latency-defined). The scope read of 2026-11-25 counted 38
time-bound defaults in nine adapter crates. Four had a test that fixes the value: `OPEN_TIMEOUT`
of Amazon Nova Sonic, `DEFAULT_AGENT_KEEPALIVE_INTERVAL` of Deepgram, and `DEFAULT_READ_TIMEOUT`
and `CLOSE_TIMEOUT` of OpenAI. The other 34 had none, the five `CLOSE_BOUND` constants among
them. A change of one of them is a change a caller can observe, and the gate stayed green on it.
The adversary pass found a 39th the read had missed: the bound of 15 s on establishing a
connection that llama-server takes from its pinned dependency.

Two things the read found:

- A comparison such as `endpoint.read_timeout == DEFAULT_READ_TIMEOUT` passes for any value. Two
  assertions of `inference-openai` were of this kind.
- Nine of the 34 have no number outside the source: the connect timeout of Cartesia, Deepgram,
  the ElevenLabs agent and Phonic, the read timeout of the Deepgram Voice Agent, the idle limit
  of GPT-Live, which only a doc comment states, and the three limits of llama-server, which are
  the defaults of a pinned dependency. (The first text of this story said eight; the count was
  corrected from the tree.)

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

As built, 2026-11-25. Commits `6d83c041f`, `1b4697831`, `0701c4c97` and `12d77e8e3`. The scope
derived on the same day at `f91b78b6b` is replaced by this. No file under `adapters/*/src`, no
fixture and no manifest changed, and no test binary was added.

- Thirteen tests named `the_time_bound_defaults_are_the_stated_numbers`, one per binding, each in
  an existing test file, and the existing `the_deadlines_are_stated_and_settable` of
  `adapters/inference-openai/tests/transport_deadlines.rs`, whose two empty comparisons now
  compare with 10 s:
  - `adapters/inference-audio-device/tests/acknowledging.rs`: `PLAYBACK_PATIENCE`, 2 s.
  - `adapters/inference-aws/tests/nova_sonic_stall_and_bounds.rs`: the read timeout of a new
    session configuration and the idle limit of the default session configuration, 30 s each.
  - `adapters/inference-cartesia/tests/errors.rs`: read 10 s, connect 10 s, `CLOSE_BOUND` 1 s.
  - `adapters/inference-deepgram/tests/errors.rs`: connect 10 s, read 10 s, keep-alive 4 s,
    `CLOSE_BOUND` 1 s.
  - `adapters/inference-deepgram/tests/agent_duplex.rs`: open 15 s, read 30 s.
  - `adapters/inference-deepslate/tests/opal_duplex.rs`: open 15 s, read 30 s, ping 10 s,
    `CLOSE_BOUND` 1 s.
  - `adapters/inference-elevenlabs/tests/closed_stream.rs`: read 10 s, connect 10 s,
    `CLOSE_BOUND` 1 s.
  - `adapters/inference-elevenlabs/tests/agent_duplex.rs`: connect 10 s, metadata 10 s, read
    30 s.
  - `adapters/inference-llama-server/tests/offline.rs`: 60 s to the response headers, 180 s idle,
    600 s in total, and the bound of 15 s on establishing a connection, read from the constant of
    the pinned dependency and from a client built as the adapter builds its own.
  - `adapters/inference-openai/tests/duplex.rs`: the Realtime idle limit, 30 s, and the connect
    and write deadline of a model built with `OpenAiRealtime::new`, 10 s each. The scope read
    had placed the idle limit in `tests/transport_deadlines.rs`, a binary without the feature
    that module needs.
  - `adapters/inference-openai/tests/gpt_live_duplex.rs`: the GPT-Live idle limit, 30 s, the read
    timeout of the endpoint `Endpoint::live` gives, 30 s, and the connect and write deadline of a
    model built with `OpenAiLive::new`, 10 s each.
  - `adapters/inference-openai/tests/speech_request.rs`: connect 10 s and read 30 s of
    `SpeechEndpoint::new`.
  - `adapters/inference-phonic/tests/duplex.rs`: connect 10 s, ready 10 s, read 30 s, ping 10 s,
    `CLOSE_BOUND` 1 s.
- Each test reads the public constant where there is one, and the value from what a caller gets
  without setting it where that needs no connection: the field of a new endpoint or
  configuration, the reader of a new model. Each comparison is with a literal, never with the
  constant the default is defined by. A test with several defaults collects every difference and
  names each with both values.
- One more test, `a_sink_given_no_patience_waits_the_stated_two_seconds_for_a_stalled_speaker` in
  `adapters/inference-audio-device/tests/acknowledging.rs`, reads the patience from a sink that
  was given none: it stalls a drain, reads 2 s from the fault and asserts that the fault did not
  come earlier. It runs about 2 s and ends its own wait after 4 s.
- What the tests do not hold:
  - the deadlines `RealtimeClient::connect` and `LiveClient::connect` of OpenAI start with: a
    client gives no reader of them before its connection is open;
  - the bound on establishing a connection of the client the llama-server adapter builds: the
    adapter gives no reader of its client, so only a move of the pin fails the test;
  - the places where an adapter uses a value later, the firing of a close bound among them
    (story:close-bounds-fired-by-a-test).
- `quality/not-measured.md`: a new list "The value of each time-bound default" under "The
  command exists, the value is not kept" names every test above, says what a test reads and names
  the two defaults read by no test. The section before named "Time bounds no test fires, and
  defaults no test asserts" is "Time bounds no test fires" and keeps what is unfired: every
  `CLOSE_BOUND`, the cancel of Cartesia, ElevenLabs text to speech, and the connect bound of
  llama-server. It says that Amazon Nova Sonic has no bound on its close. The section on the
  default limits of llama-server is gone; its statement is in the new list.

Not built, as planned: the entries of `quality/adapters.md`, the lists of
`quality/adapters.ratchet.yaml` and the records under `quality/records/`. They follow from the
first `check` run on `main` that ran these tests. Until then three lines of `quality/adapters.md`
say that no test fixes a default that a test now fixes: the close of ElevenLabs text to speech,
the default limits of llama-server, and the connect and write defaults of OpenAI.

Added on the same day, after `main` took the wave of 2026-11-24 and this branch took it in
(merge `8c240e3ee`, commit `cdfa965b1`): the wave brought one new time-bound constant,
`CLOSE_GRACE`, 5 ms, in `inference-cartesia` and `inference-elevenlabs`. The two tests
`the_time_bound_defaults_are_the_stated_numbers` of
`adapters/inference-cartesia/tests/errors.rs` and
`adapters/inference-elevenlabs/tests/closed_stream.rs` compare it with the literal 5 ms, and the
list "The value of each time-bound default" of `quality/not-measured.md` names it. With the
constant at 6 ms each of the two tests fails and names it with both values. The page conflict of
the merge was resolved so that each sentence is true on the merged tree: the entries about the
wave's tests that fire a close bound and a connect timeout stay, and "no test asserts its value"
is gone where a test of this unit asserts it.
