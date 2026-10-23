---
format: aep.planning-md/3
id: story:repo-podcast-fixtures-removed-on-failure
kind: story
status: implemented
title: A failing repo-podcast test leaves no git checkout under target/tmp
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T12:40:04Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-23T12:40:04Z", actor: "human:timo", revision: 5}
- {from: "active", to: "implemented", at: "2026-10-23T12:59:43Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1}}}
---
## Context

`examples/repo-podcast/tests/title.rs` builds fake git checkouts (directories holding a `.git`)
under `CARGO_TARGET_TMPDIR`: `repo-podcast-title-worktree/main-checkout/.git` and
`repo-podcast-title-no-origin/my-project/.git`. Before 28e680c (2026-10-21, on main) the tests left
them in place. Since 28e680c each test calls `remove_dir_all` on its root as its last statement
(title.rs:49, :62), so a passing run leaves nothing; a failing assertion panics before that line and
the nested `.git` stays.

A nested `.git` under `target/tmp` makes `worktree gc` retain the tree as `worktree-hidden-state`,
and `worktree discard-cache` keeps it as a repository. Eight managed trees built before 28e680c
(waves 22 and 23) are retained for this reason (2026-10-21 cleanup); they are about 12M each and are
kept, not repaired.

## Planned scope

- The fixtures in `title.rs` (and any other test under `examples/` or `crates/` that writes a
  `.git` below `CARGO_TARGET_TMPDIR`; `brief.rs` and `adversary1_brief.rs` write there but no `.git`)
  live in a temporary directory owned by a guard whose `Drop` removes it, so a panicking test
  removes them too.

## Expected before

Machine check that fails before the change and passes after it: a test in
`examples/repo-podcast/tests/` that creates a fixture through the same helper, panics inside
`std::panic::catch_unwind`, and then asserts that the fixture's directory no longer exists. Plus,
after `cargo test -p repo-podcast`, `find target/tmp -name .git` prints nothing.

## Acceptance

After any run of the workspace tests, passing or failing, no `.git` exists below `target/tmp`.

## Scope

- `examples/repo-podcast/tests/title.rs`: its two fixtures.
- A guard type local to `examples/repo-podcast/tests/` (a shared test module), and the new test
  of "Expected before" beside it.
- No other test writes a `.git` below `CARGO_TARGET_TMPDIR` (grep on 2026-10-23: `title.rs` only).

## Decision, 2026-10-23

This story stops new leftovers only. The eight trees retained before it stay as they are: each
still holds `target/tmp/repo-podcast-title-no-origin/my-project/.git`, and `worktree gc` retains
them as `worktree-hidden-state`. They are about 12M each, which is not worth deleting data for.
They are freed when `worktree` can archive a finished tree with a nested fixture repository.
