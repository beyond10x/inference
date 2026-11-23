---
format: aep.planning-md/3
id: story:test-run-step-under-budget
kind: story
status: draft
title: The test-run step ends within 120 s and synthesises the conformance suite once per run
revision: 2
---
## Context

The test step of the gate is over its 120 s budget. On the head 9950ff98 of pull request 61, CI's
`task test-run` step took 154 s (15:57:33 to 16:00:07 UTC, job 137801812823) against the 120 s
step budget. Locally the same step took 448 s at a load average above 120, and 11 tests of
`inference-testkit`, each of which synthesises the conformance suite, were killed at the 60 s per
test limit.

The cost is the shape the test budget names: an expensive product, the synthesised conformance
suite, is made inside each test, so it is paid once per test. Under cargo-nextest each test is its
own process, so an in-process global does not share it.

## Planned scope

- `crates/inference-testkit`: the synthesised suite is made once per run and read by every test,
  either as a committed generated file with a digest of its sources that a fast check compares, or
  as one on-disk fixture built once per run under `target/` and keyed by the digest of its sources.
- Full regeneration from the sources runs in the slow lane, only when the sources changed, and
  compares bytes with the committed file.
- The step prints its ten slowest tests, in CI and locally.

## Acceptance

- The `task test-run` step ends within 120 s of wall time in CI, measured on a pull request that
  changes code, and prints its ten slowest tests.
- The conformance suite is synthesised at most once per test run, not once per test: a run of the
  testkit's tests calls synthesis once, or never when the committed file's digest matches.
- No test of `inference-testkit` runs over 60 s locally at a load average above 120, and none is
  killed by the 60 s limit.
