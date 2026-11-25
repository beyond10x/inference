---
format: aep.planning-md/3
id: story:time-bound-defaults-and-close-bounds-held
kind: story
status: draft
title: Each time-bound default is asserted and each close bound is fired by a test
tags:
- from-finding
relations:
- serves: vision:inference
revision: 1
---
## Context

`quality/adapters.md` says for each time bound of an adapter whether a test asserts its default
value or only holds the mechanism with a shorter value
(story:quality-per-adapter-and-latency-defined). `quality/not-measured.md` lists what is held by
nothing:

- Defaults no test asserts, among them the default read timeout and idle limit of Amazon Nova
  Sonic, `PLAYBACK_PATIENCE` of the audio device, `DEFAULT_CONNECT_TIMEOUT` of Deepgram
  transcription, and the default limits of the HTTP transport of llama-server.
- Every `CLOSE_BOUND`: the tests that compare a time with the constant close against a fixture
  that takes the close at once, so they pass with a larger constant and with the bound removed
  (review-result:quality-per-adapter-and-latency-defined-adversary-1, findings 1 and 10).
- Cartesia: a cancel waits for its send up to twice `CLOSE_BOUND`
  (`adapters/inference-cartesia/src/synthesiser.rs`), and its test asserts once the constant.

The wave of 2026-11-24 adds cases that fire `CLOSE_BOUND` for a dropped synthesis of ElevenLabs
and of Cartesia against a server that stops reading. Once it is on `main`, those two lines of
the page are corrected from it and are not built again here.

## Planned scope

- One test per binding that compares each default constant with its documented number.
- For each binding whose close is bounded by `CLOSE_BOUND` and has no test that fires it: one
  test against a fixture that does not take the close, holding that the close ends at the bound.
- The page and `quality/not-measured.md` follow: a line moves when its test exists.

## Acceptance

- A changed default fails the test of its binding; a removed close bound fails the close test of
  its binding. Each is shown red once with the change planted.
- No new test runs over 5 s; the gate's test step stays within its 120 s.

## Out of scope

- Changing any default or bound.
- The connect timeouts (story:connect-timeouts-fired-by-a-test).
