---
format: aep.planning-md/3
id: story:drop-tests-own-limit-and-stated-bound
kind: story
status: active
title: The drop tests of Cartesia and ElevenLabs text to speech end by a limit of their own and fail with an enlarged close bound
tags:
- from-finding
relations:
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T04:28:12Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T04:28:12Z", actor: "human:operator", revision: 3}
---
## Context

Found on 2026-11-25, when the pages of `quality/` were read against `main` after the wave of
2026-11-24. That wave added tests that drop a synthesis of Cartesia and of ElevenLabs text to
speech against a peer that floods pings and reads nothing
(`adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs`,
`adapters/inference-elevenlabs/tests/adversary_w2_drop_backed_up.rs`). Runs on the build node
with the source changed and restored showed three things these tests do not hold:

- **A test with no limit of its own.** With the bound gone from the wait outside a runtime and
  from `Connection::close`, the ElevenLabs test
  `dropping_an_idle_synthesis_whose_close_cannot_be_written_outside_a_runtime_waits_close_bound`
  runs until the runner ends it at 60 s (`TIMEOUT [ 60.004s]`). The Cartesia test of the same name
  fails at the 25 s limit of its fixture, and the ElevenLabs test
  `a_drop_inside_an_executor_that_is_not_tokio_waits_close_bound_and_not_the_grace` at a limit of
  30 s.
- **The tests follow the constant.** They compare the wait with `CLOSE_BOUND` itself. With the
  constant at 3 s every test of both crates passes (Cartesia: 44 passed; ElevenLabs: 202 passed;
  the drop outside a runtime held 3.001 s).
- **No test tells the three applications apart.** A drop has the bound three times: the wait
  outside a runtime, the close handed off inside a Tokio runtime, and `Connection::close`. With
  any one of the three removed every test of the crate passes. A test fails only when a whole
  path is left without a bound.

`quality/not-measured.md` names all three under "Time bounds no test fires".

## Planned scope

- The ElevenLabs test that hangs gets a limit of its own, a few seconds, so that a drop nothing
  ends fails the test and leaves no thread to the runner.
- The limits of the drop tests of both crates that are compared with `CLOSE_BOUND` are written
  out as numbers, so that an enlarged constant fails them.
- Per application of the bound, either a test that reaches it alone or a sentence on the page
  that says why none can.
- `quality/not-measured.md` follows.

## Acceptance

- With the bound removed from the wait outside a runtime and from `Connection::close`, no test of
  `inference-elevenlabs` runs longer than its own limit, shown by a run with that change planted
  (state transition: the bound is removed).
- With `CLOSE_BOUND` at 3 s the drop tests of both crates fail, shown red once (state transition:
  the constant is changed).
- For each of the three applications the story says whether its removal alone fails a test, from
  a run (state transition: one application is removed).
- No test changed here runs over 5 s; the gate's test step stays within its 120 s.

## Out of scope

- The cancel of Cartesia, which waits up to twice the constant and which no test reaches.
- Any change under `adapters/*/src`.

## Scope as built

Built on the branch `wave/u3-tests-under-load`, merged into `wave/2026-11-26-quality`:

- The drop tests of `inference-cartesia` and `inference-elevenlabs` take their limits from `STATED`
  (1 s) written out: outside a runtime at least 900 ms and at most 1 s plus `BOUND_SLACK` (100 ms)
  with ready time taken out; inside an executor that is not Tokio 900 ms to 1.1 s; the synthesis's
  own thread gone 900 ms to 2 s after a drop inside Tokio.
- The ElevenLabs drop tests that wait for the bound end by `LIMIT` (10 s).
- New Cartesia case `a_synthesis_that_fails_on_a_backed_up_connection_ends_at_the_close_bound`,
  through `Deaf::stalling` and `on_a_backed_up_connection_of` in `tests/support/deaf.rs`: it
  reaches the bound on `Connection::close` alone and holds the end to the stall and 900 ms to 1.2 s
  more (`STALL_SLACK`).
- `quality/not-measured.md` names these numbers and why no test of a drop fails with one of the
  other two applications of the bound removed; `quality/adapters.md` (ElevenLabs, close of text to
  speech) no longer says no test asserts the bound.
