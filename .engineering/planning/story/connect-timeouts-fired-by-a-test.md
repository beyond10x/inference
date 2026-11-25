---
format: aep.planning-md/3
id: story:connect-timeouts-fired-by-a-test
kind: story
status: implemented
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
  path: quality/not-measured.md
revision: 12
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T07:49:25Z", actor: "human:operator", revision: 8}
- {from: "proposed", to: "active", at: "2026-11-25T07:49:25Z", actor: "human:operator", revision: 9}
- {from: "active", to: "implemented", at: "2026-11-25T08:46:16Z", actor: "human:operator", revision: 12, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":1}}}
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

As built, 2026-11-25. Commits `d99652d48`, `473046a3b`, `f95709ed1` and `d75990f20`. The scope
derived on the same day at `d99471c4a` is replaced by this. No file under `adapters/*/src`, no
fixture and no manifest changed, and no test binary was added.

- Five tests, one per binding, each in an existing test file:
  - Cartesia text to speech: `a_handshake_never_answered_fails_the_synthesis_at_the_connect_timeout`
    in `adapters/inference-cartesia/tests/errors.rs`.
  - Phonic: `a_handshake_never_answered_fails_the_open_at_the_connect_timeout` in
    `adapters/inference-phonic/tests/duplex.rs`.
  - ElevenLabs agent: `a_handshake_never_answered_fails_the_open_at_the_connect_timeout` in
    `adapters/inference-elevenlabs/tests/agent_duplex.rs`.
  - ElevenLabs text to speech:
    `a_handshake_never_answered_fails_the_synthesis_at_the_connect_timeout` in
    `adapters/inference-elevenlabs/tests/closed_stream.rs`.
  - Deepgram transcription:
    `a_handshake_never_answered_fails_the_recognition_at_the_connect_timeout` in
    `adapters/inference-deepgram/tests/errors.rs`.
- Each opens against a listener on loopback that takes the connection and never answers the
  handshake, with a connect timeout of 500 ms set through the endpoint's builder. It asserts the
  connect error with no status and the detail `no handshake within 500ms`, that the listener
  received one connection, that the open ended no earlier than the timeout and before twice the
  timeout, and it ends its own wait after 4 s. Four tests bind a listener of their own; the
  agent's uses the stalled handshake and the connection count of its existing fixture.
- What the tests do not hold: a timeout multiplied by less than two passes, and so does a
  credential lookup moved inside the timeout.
- `quality/not-measured.md`: a new list "The connect timeout of five bindings" under "The command
  exists, the value is not kept" names the five tests and says that no record of a run holds
  their results yet. The list of bounds no test fires keeps, for these bindings, only the
  defaults no test asserts.

Not built, as planned: the entries of `quality/adapters.md`, the lists of
`quality/adapters.ratchet.yaml` and the records under `quality/records/`. They follow from the
first `check` run on `main` that ran the five tests.
