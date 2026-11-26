---
format: aep.planning-md/3
id: story:ticker-test-holds-under-contention
kind: story
status: active
title: The ElevenLabs ticker test fails on its block also when processors are contended
relations:
- serves: vision:inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T07:09:13Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-26T07:09:14Z", actor: "human:operator", revision: 4}
---
## Context

`adapters/inference-elevenlabs/tests/adversary_w2_ticker_blocks.rs` holds two tests that fail
under contention for one cause: the ticker subtracts the time its thread waited for a processor
(`/proc/thread-self/schedstat`), and under contention that subtraction swallows the 200 ms block
the test plants.

- `a_thread_asleep_for_200_ms_fails_the_tests` (about line 77) failed once on 2026-11-26 on the
  build node at a load average of 86 on 32 CPUs with "ticker's largest gap is 11.2 ms". Found by
  the adversary pass over unit U2 of the wave of 2026-11-26 (review of
  story:test-run-step-keeps-its-margin, finding F3).
- `a_thread_computing_for_200_ms_fails_the_tests` failed once on 2026-11-26 on the build node at a
  load average of 90.5 with "busy loop held 200.000867ms and the ticker's largest gap is
  47.095422ms": a spinning thread that is preempted counts as ready, and that time is subtracted.
  Found by the implementor of unit U3 of the same wave, in a planted run of the drop tests.

Both were there before that wave. The default nextest profile runs 8 tests at once since that
wave, which raises the contention on a 4-processor runner. The hold tests of the drop of a
synthesis in `inference-cartesia` and `inference-elevenlabs` subtract the same ready time since
that wave; a drop that spins instead of parking could be partly hidden under load (the drop code
parks).

## Acceptance

- The test fails on the blocking it exists for (a thread asleep for 200 ms) on a machine whose
  processors are contended, shown by a run beside a parallel load, and passes 20 runs in a row
  without the planted block under the same load.
- The test says in its doc what share of a gap it attributes to processor wait and why.

## Out of scope

- The library's ticker and the adapters' runtime use.

## Scope as built

Built on the branch `wave2/tests-under-contention`, merged into `wave/2026-11-26-adoption`:

- `adapters/inference-elevenlabs/tests/support/ticker.rs` keeps its rule: the whole time the
  thread was ready to run comes out of a gap; its doc states the limit (a thread that computes or
  yields is counted only for the time it ran).
- `adapters/inference-elevenlabs/tests/adversary_w2_ticker_blocks.rs`: each case pins itself beside
  8 spinning threads; the computing case plants 200 ms of its own processor time; the sleep case
  fails with its own message when the kernel counts more than a quarter of the sleep as ready;
  each wait case checks that the ticker took out no more than the kernel counted; two controls
  (nothing held, 15 ms of computing) pass beside the load; the loop of yields stays ignored with
  a `limit:` reason.
