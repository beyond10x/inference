---
format: aep.planning-md/3
id: story:gate-test-budget-and-planning-skip
kind: story
status: draft
title: The test step holds the 60 s and 120 s budget, and a planning-only pull request skips the build
summary: 'Review findings 16 and 17: nextest with a 60 s kill, one trybuild run in a slow lane, a pinned rustc, and an early exit for planning-only pull requests'
relations:
- serves: vision:inference
revision: 1
---
## Context

Review finding 16 (release 0.31.0): CI's `cargo test --workspace --all-features` took 16 min 23 s,
7 min 43 s of it building the tests; one `trybuild` compile-check fixture ran 174 s. On 0.32.0 the
test step ran 17 min 24 s (05:06:53 to 05:24:17 UTC, run 45115048722) and `task check` about 25 min.
The workspace budget is 60 s per test and 120 s per test step. Locally, `trybuild` targets ran 174 s
(`adversary_w25_tool_macro_pass2_ui`), 338 s (`adversary_w25_tool_macro_ui`, three tests over 60 s)
and 48 s (`tool_macro`), and `agent.rs` held a 122 s compile-fail case and a 76 s loop. The UI
snapshot `tool_argument_not_deserialize.stderr` was recorded on rustc 1.100.0 (CI's `stable`) while
the repository pinned no toolchain.

Review finding 17: PR 55 changed two files under `.engineering/` and its `task check` ran 24 min
06 s; the workflow runs the whole gate for every pull request.

## Planned scope

- `.config/nextest.toml`: a test over 60 s is killed and fails; the run prints its ten slowest
  tests. `task check` and CI run the test phase with cargo-nextest at one pinned version.
- Every `trybuild` case of `crates/inference` runs in one `trybuild` run in its own test binary,
  in a `slow` lane (`task test-slow`, workflow `slow.yml`) that runs only when the macro crate, the
  SDK crate's sources or the UI cases change.
- The slow `agent.rs`, `cascade_burst.rs` and `adversary_u2_turn_at_close.rs` tests prove the same
  property in less time.
- `rust-toolchain.toml` pins rustc 1.100.0; CI installs the toolchain it names.
- Test builds carry no debug info.
- A pull request whose changed files are all planning or prose files passes the required check
  without building or testing; one path list in a POSIX sh script with a test of both sides.

## Acceptance

- A test that runs over 60 s in the gate's test step is killed and fails the step, locally and in
  CI, and the step prints its ten slowest tests (finding 16).
- No test of `crates/inference` runs over 60 s in the gate's test step, and every case the removed
  per-test `trybuild` runs checked is still checked by the one `trybuild` run of the slow lane
  (finding 16).
- Local gate and CI use the same rustc, read from `rust-toolchain.toml`, and the UI snapshots pass
  on it (finding 16).
- A test binary built by the gate has no `.debug_*` section (finding 16).
- A pull request that changes only `.engineering/` files or `*.md` files outside `website/` passes
  the required check `inference / task check` without building or testing; any other changed path
  runs the full gate, and a `website/` change still runs the documentation check (finding 17).
