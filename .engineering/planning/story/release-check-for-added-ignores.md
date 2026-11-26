---
format: aep.planning-md/3
id: story:release-check-for-added-ignores
kind: story
status: active
title: The check on added ignored tests exists, or AGENTS.md stops naming it
tags:
- from-finding
relations:
- informed_by: review-result:synthesiser-drop-inside-runtime-adversary-1
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T04:28:12Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T04:28:12Z", actor: "human:operator", revision: 3}
---
## Context

`AGENTS.md` says of a live test against a hosted provider that its ignore reason contains the
word `billed`, and that "the release check admits an added `#[ignore]` only with that word". The
adversary pass on the synthesiser drop (review-result:synthesiser-drop-inside-runtime-adversary-1,
finding 11) looked for that check in `Taskfile.yml`, `.github` and `crates` and found none; 64 of
the 81 `#[ignore]` lines in the repository lack the word, among them the local adapters' tests
that wait on a checkpoint path. The sentence describes a check that does not exist.

## Planned scope

- Either the check exists: a test of the documentation crate, or a step of the release
  procedure, that lists every `#[ignore]` and admits the reasons the repository's rules name (a
  billed live test; a local test that names the environment variable it waits on).
- Or `AGENTS.md` stops naming a check and states the rule for ignore reasons alone.

## Acceptance

- An added `#[ignore]` with no reason, or with a reason of neither admitted kind, fails the gate
  or the release procedure's check (state transition: the line is added), or `AGENTS.md` no longer
  says a check exists.

## Scope as built

Built on the branch `wave/u4-ignore-check`, merged into `wave/2026-11-26-quality`:

- `crates/inference-docs/tests/ignores/mod.rs`, a module of the integration binary
  `tests/workflows.rs`, run in `task test-run`: reads every `#[ignore]` of the workspace's Rust
  sources, and a `#[cfg_attr(…)]` with an `ignore` item over as many lines as it spans; leaves out
  `<root>/target`, directories holding `CACHEDIR.TAG`, hidden directories and `node_modules`.
- It admits five kinds of reason and refuses the rest, naming file, line, reason and the bullet of
  `AGENTS.md`: billed (`not billed` does not count; a test target built only with feature `live`
  needs `billed` unless its package's description begins "Local adapter"), local (each
  `INFERENCE_*` name stands whole in a string literal of the package), by hand (names a microphone
  or a speaker), child (a parent of the same file names the case as a whole identifier or a
  literal), limit (names an existing file of the repository that states the limit).
- `crates/inference-docs/tests/ignores/adversary.rs`: the adversary's ten cases.
- Reasons rewritten: three of `inference-aws` and two of `inference-openai` say `billed`; the one
  limit of `inference-elevenlabs` names the file that states it. 73 ignores read; none removed, no
  test un-ignored.
- `AGENTS.md`: the bullet "Every `#[ignore]` gives a reason of one of five kinds" replaces the
  sentence about a release check.

Left open: telling a hosted test from a local one outside the `live` gate rests on the wording of
its reason.
