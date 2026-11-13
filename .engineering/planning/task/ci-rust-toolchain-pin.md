---
format: aep.planning-md/3
id: task:ci-rust-toolchain-pin
kind: task
status: draft
title: A new stable Rust does not fail a PR that did not cause it
revision: 1
---
## Context

CI installs the current stable Rust (`.github/workflows/check.yml`, `toolchain: stable`), while
`Cargo.toml` declares `rust-version = "1.98"` and every other tool CI installs is pinned. Rust
1.100.0 (2026-11-09) failed `inference / task check` on a branch that did not cause it, three
ways: clippy `double_must_use`, the text of a trybuild case, and the directory of test binaries
(PR #45, 2026-11-12). `main` failed the same way until that PR merged.

## Planned scope

- CI installs a pinned Rust version, moved like the other pins at each release; or CI keeps
  stable and a scheduled job reports a new stable before a PR meets it. The choice is recorded in
  `AGENTS.md`.

## Acceptance

- A new stable Rust release does not fail a PR whose change does not touch the failing code.
- `AGENTS.md` names the Rust version rule next to the other CI pins.
