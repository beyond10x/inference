---
format: aep.planning-md/3
id: story:test-run-step-keeps-its-margin
kind: story
status: active
title: The forge's test step ends well inside its 120 s budget
relations:
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T04:28:11Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T04:28:11Z", actor: "human:operator", revision: 3}
---
## Context

The test step of the gate ends within 2 minutes of wall time (`AGENTS.md` of the folder the
repository lives in, "Tests stay fast"); `task test-run` enforces it with a bound of 120 s. On the
forge the step `task test-run` of the workflow `check` took 91 s at `fb7192ac8` (2026-11-25), 99 s
at the head of pull request 75 and 109 s at the head of pull request 76 (2026-11-26), with 3059,
3175 and 3207 tests in the workspace run. On the build node the same step ran 52 s, 97 s, 94 s and
125 s on 2026-11-25 and 2026-11-26; the run of 125 s was ended by the bound with every test run
and the last one killed. The ten slowest tests of one run take 9.1, 9.0, 8.0, 6.2, 6.0, 5.9, 5.6,
4.6 and 4.0 s.

## Acceptance

- The forge step `task test-run` of the workflow `check` on `main` ends under 90 s in five runs
  one after another, with the same tests and none of them removed or skipped to get there.
- Every test over 5 s in the step's list of the ten slowest is named in this story's scope with
  what makes it slow and what was done: made faster, or moved to the lane `slow` with the reason.
- No bound of the step and no shard is added.

## Out of scope

- The machines the step runs on.
- The tests of the lane `slow`.

## Scope as built

Built on the branch `wave/u2-test-margin`, merged into `wave/2026-11-26-quality`:

- `Taskfile.yml`: `task test-build` ends with `task test-fixtures`, which runs the nextest profile
  `setup` and synthesizes the testkit's suites once into a cache keyed by the SHA-256 of `ess/`,
  `ess --version` and the witness scenarios; `task test-run` synthesizes nothing when the cache is
  current, and a test without the setup step still synthesizes as before.
- `.config/nextest.toml`: the profile `setup`; `test-threads = 8` in `[profile.default]` (the
  coordinator's decision D57); `NEXTEST_TEST_THREADS`, which the shared build pool sets per job,
  wins over the file.
- Tests made cheaper with the same inputs and assertions: `usage_view` runs its mutants over the
  scenarios that name `UsageRecords` (`suite::cut_by`); the resampler pairs and the TUI widths run
  on threads; `cascade_burst` makes its fixture audio once; the stall cases of
  `examples/realtime-voice/tests/adversary_compare_cascade_2.rs` run with read timeout 1.2 s,
  limit 1.5 s and drain 3 s, with a case for a commit 150 ms late.
- `crates/inference-testkit/tests/async_handles.rs` takes the ticker of
  `adapters/inference-elevenlabs/tests/support/ticker.rs`, which leaves out the time the thread
  waited for a processor.
- Pages: `AGENTS.md` and `website/docs/guides/run-the-checks.md` name the setup step, the 8
  threads and the variable that wins over them.

Left open: the step's wall time on the forge is measured by the wave's own check run; a cheaper
build profile for two crates (D2 of the implementor's report) is held until that run.
