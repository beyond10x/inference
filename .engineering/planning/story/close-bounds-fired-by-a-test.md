---
format: aep.planning-md/3
id: story:close-bounds-fired-by-a-test
kind: story
status: draft
title: Each close bound that can be reached is fired by a test
tags:
- from-finding
relations:
- serves: vision:inference
revision: 1
---
## Context

Split from story:time-bound-defaults-and-close-bounds-held after its scope read of 2026-11-25.
Five constants named `CLOSE_BOUND`, each 1 s and none settable, bound the close of eight bindings
(Cartesia text to speech, Deepgram transcription, the Deepgram Voice Agent, Deepslate Opal,
ElevenLabs text to speech, ElevenLabs speech to text, the ElevenLabs agent, Phonic). No test fires
any of them: the tests that compare a time with the constant close against a fixture that takes
the close at once, so they pass with a larger constant and with the bound removed
(review-result:quality-per-adapter-and-latency-defined-adversary-1, findings 1 and 10).

What the read found, by reading and not by a run:

- Every `CLOSE_BOUND` bounds the write of the close frame. A fixture that keeps reading and ignores
  the frame fires nothing; the peer has to stop reading, and the test has to fill the send path
  through the public interface before the close.
- In six bindings the constant is applied twice, on the frame write and on the wait for the task or
  in the drop. A test that times the drop sees the outer wait and stays green with the inner
  timeout removed.
- Deepgram transcription and ElevenLabs speech to text have nothing on the client to time: cancel
  and drop only send a command to the connection thread. The bound shows only at the server.
- Cartesia: a cancel waits for its send up to twice the constant and its test asserts once the
  constant; the longer wait may not be reachable without a change under `src/`.
- Amazon Nova Sonic has no bound on its close.

Another branch adds cases that fire the bound for a dropped synthesis of ElevenLabs text to speech
and of Cartesia against a server that stops reading; those two are not built again here.

## Planned scope

- First a read with a run: for each binding, how much has to be written before a write blocks on a
  peer that stops reading, and whether the adapter's own queue or write bound ends the session
  first. Its result decides which bindings get a test.
- For each binding where the bound can be reached: a mode of the existing fixture that stops
  reading, and one test in an existing file that holds that the close ends at the bound.
- For the bindings where it cannot: the page says so, with the reason.
- `quality/not-measured.md` follows; the page entries and records follow from a `check` run on
  `main`.

## Acceptance

- For each binding with a test: with `CLOSE_BOUND` enlarged the test fails, and the story says for
  each of the two places the constant is applied whether its removal fails the test; each planted
  change is shown red once (state transition: the bound is changed).
- No new test runs over 5 s; the gate's test step stays within its 120 s.

## Out of scope

- A bound on the close of Amazon Nova Sonic, and making any `CLOSE_BOUND` settable: both change an
  adapter and need a story of their own.
