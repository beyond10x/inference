---
format: aep.planning-md/3
id: story:release-check-for-added-ignores
kind: story
status: draft
title: The check on added ignored tests exists, or AGENTS.md stops naming it
tags:
- from-finding
relations:
- informed_by: review-result:synthesiser-drop-inside-runtime-adversary-1
- serves: vision:inference
revision: 1
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
