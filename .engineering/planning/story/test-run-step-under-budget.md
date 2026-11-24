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
  path: AGENTS.md
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: Taskfile.yml
- confidence: cited
  path: adapters/inference-deepgram/tests/security2_agent_release.rs
- confidence: cited
  path: adapters/inference-openai/src/live.rs
- confidence: cited
  path: adapters/inference-openai/src/live_duplex.rs
- confidence: cited
  path: adapters/inference-openai/tests
- confidence: cited
  path: crates/inference-testkit/Cargo.toml
- confidence: cited
  path: crates/inference-testkit/tests
- confidence: cited
  path: examples/realtime-voice/src/elevenlabs.rs
- confidence: cited
  path: examples/realtime-voice/tests
- confidence: cited
  path: examples/repo-podcast/src/show.rs
- confidence: cited
  path: examples/repo-podcast/tests/adversary1_show.rs
- confidence: inferred
  path: website/docs/guides/run-the-checks.md
revision: 28
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

Rewritten 2026-11-24 from the built unit (`impl/test-run-step-under-budget` at `7f43e7dd9`, base
`0fe11ff08`): 51 files, read from `git diff --name-status`. The scoper's section of the same day
named the testkit tests and `Taskfile.yml`; it did not foresee the adapters and the examples.

- **Primary surface:** `crates/inference-testkit/tests` (30 files) and
  `crates/inference-testkit/Cargo.toml`
  - `suite/mod.rs` and `conformance_suite.rs`: the shared suite, synthesised once per distinct
    suite per run and kept under `target/` by the digest of its sources; two witness scenarios
    under `suite/witness/`
  - `target_self.rs` and `target_self/`: the 65 `target::` tests, run in one binary; `target/*.rs`
    keep the code the other binaries use
  - nine tests that synthesised on their own now read the shared suite: `conform.rs`,
    `adversary11_duplex_close_mutants.rs`, `adversary12_duplex_close_witness.rs`,
    `adversary1_target.rs`, `adversary1_tools_target.rs`, `adversary2_stream_deferred.rs`,
    `adversary_usage_view.rs`, `adversary_w23_descriptor_view.rs`, `duplex_close_mutants.rs`; and
    `adversary3_target_sequence.rs`
  - `adversary_w1_test_budget.rs`: the first adversary pass's cases
- **`Taskfile.yml`:** `test-run` ends at its bound of 120 s with a line naming it and exit 201,
  and prints the ten slowest tests of the run that just ended
- **`adapters/inference-openai`:** `src/live.rs`, `src/live_duplex.rs` (the close timeout as a
  value) and eight test files (bounds as values, reads that end at a marker,
  `adversary2_w1_script_end.rs`)
- **`adapters/inference-deepgram/tests/security2_agent_release.rs`:** stalled openings wait 200 ms
- **`examples/repo-podcast`:** `src/show.rs` (`Settings::silent_turn`), `tests/adversary1_show.rs`
- **`examples/realtime-voice`:** `src/elevenlabs.rs` (`resolves_within`),
  `tests/adversary2_u3_talk.rs`, `tests/adversary2_w1_key_check.rs`
- **Documents:** `website/docs/guides/run-the-checks.md`, `AGENTS.md`; `Cargo.lock`
- **Not touched, though the scoper thought it likely:** `.github/workflows/check.yml`,
  `.github/workflows/slow.yml`, `crates/inference-docs/tests/workflows.rs`. The unit took the
  cache-on-disk form, so no committed suite file and no slow lane were added.
- **Known limits:** on the forge's runner image `timeout` is uutils coreutils, which kills only
  its own child at the second signal; a part that ignored the first signal can run on until the
  job ends (the guide says so). The local step fails on two ticker tests of
  `inference-elevenlabs` when the build node is loaded; a story
  of the next wave fixes them, and the forge's runner passes them.
