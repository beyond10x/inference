---
format: aep.planning-md/3
id: story:test-run-step-under-budget
kind: story
status: active
title: The test-run step ends within 120 s and synthesises the conformance suite once per run
relations:
- serves: vision:inference
scope:
- confidence: inferred
  path: .github/workflows/check.yml
- confidence: inferred
  path: .github/workflows/slow.yml
- confidence: inferred
  path: AGENTS.md
- confidence: cited
  path: Taskfile.yml
- confidence: inferred
  path: crates/inference-docs/tests/workflows.rs
- confidence: cited
  path: crates/inference-testkit/tests
- confidence: inferred
  path: website/docs/guides/run-the-checks.md
revision: 13
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T12:32:05Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-24T12:32:05Z", actor: "human:operator", revision: 4}
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
- The conformance suite is synthesised at most once per distinct suite per test run, not once per
  test: a run of the testkit's tests calls synthesis once for the authored suite and once for the
  witnessed suite, or never for a suite whose file on disk matches the digest of its sources.
- No test of `inference-testkit` runs over 60 s locally at a load average above 120, and none is
  killed by the 60 s limit.

Corrected on 2026-11-24: the second line read "calls synthesis once, or never when the committed
file's digest matches". The testkit's tests read two suites, the authored one and the witnessed
one; each is synthesised once per run and kept in a cache keyed by the digest of its sources
(review-result:test-run-step-under-budget-adversary-1 finding 3, carried in
review-result:test-run-step-under-budget-adversary-2). Measured on the forge's runner at the
unit's head: the step took 90 s.

## Scope

Derived 2026-11-24 by `story-scoper` at `745fe77b6`; nothing was built or run. Every line is
**cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-testkit/tests` — cited (the story names the crate; all 13
  synthesising tests are there)
- **Files:** `conform.rs`, `adversary11_duplex_close_mutants.rs`,
  `adversary12_duplex_close_witness.rs`, `adversary1_target.rs`, `adversary1_tools_target.rs`,
  `adversary2_stream_deferred.rs`, `adversary_usage_view.rs`, `adversary_w23_descriptor_view.rs`,
  `duplex_close_mutants.rs` under `crates/inference-testkit/tests/` — cited
- **Files:** `Taskfile.yml:32-64` (`test-run`, `test-workspace`, `test-slowest`, `test-slow`) — cited
- **Symbols:** nine private copies of `synthesized()` / `synthesized_suite()` / `ess()`, each
  `Command::new("ess")` — cited
- **Also likely:** `.github/workflows/slow.yml` (installs no `ess`, has no `ess/**` path) and
  `.github/workflows/check.yml:143-145` — inferred, the slow-lane regeneration and a 120 s step bound
- **Also likely:** `crates/inference-docs/tests/workflows.rs` — inferred, it pins the content of
  `check.yml` and `slow.yml`
- **Also likely:** a new shared module and a committed suite file under
  `crates/inference-testkit/tests/` — inferred
- **Documents:** `AGENTS.md:54-58`, `website/docs/guides/run-the-checks.md:36-77` — inferred, they
  quote the task commands
- **Confidence:** high for the testkit tests and `Taskfile.yml` (read in the tree); medium for the
  workflow and document lines (they depend on which of the two accepted forms is built)
- **Would collide with:** any unit touching `crates/inference-testkit/tests`, `Taskfile.yml`,
  `.github/workflows/` or the `ess/` tree — cited
- **Safety fact:** 10 of the 13 calls pass the same arguments (`--path ess --scenarios
  ess/scenarios`) and every test edits only an in-memory copy of the suite
  (`adversary1_target.rs:446`, `adversary2_stream_deferred.rs:244-256`), so one read-only file
  serves them — cited, unproven for all nine files
- **Safety fact:** whether `ess verify conform synthesize` writes the same bytes for the same
  inputs was not checked; a byte comparison in the slow lane depends on it — inferred
