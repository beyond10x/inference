---
format: aep.planning-md/3
id: story:quality-defined-in-ratchet-formats
kind: story
status: implemented
title: The quality of the repository is defined in Ratchet's formats
relations:
- serves: vision:inference
scope:
- confidence: cited
  path: .github/workflows/check.yml
- confidence: cited
  path: AGENTS.md
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: crates/inference-docs/Cargo.toml
- confidence: cited
  path: crates/inference-docs/tests/quality
- confidence: cited
  path: crates/inference-docs/tests/workflows.rs
- confidence: cited
  path: quality
- confidence: cited
  path: ratchet.toml
revision: 14
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T00:31:07Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-25T00:31:08Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-25T01:48:04Z", actor: "human:operator", revision: 14, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":1}}}
---
## Context

The repository checks its quality in several places, each with its own form: the bounds of
`task test-run` (120 s for the step, 60 s per test), zero warnings from lint and the documentation
build, the drift check of generated pages, the conformance suite synthesised from `ess/`, the slow
lane of compile-fail cases, the documentation site's `ux/baseline.json`, the pinned sha256 of the
native archive. No one document says which of these count as the quality of the project, what
each one's value is today, or what is not measured at all.

Ratchet (`beyond10x/ratchet`) is the organisation's tool for such a definition. At its commit
`f139eccc17a548444f0f4e1dc06520120c28739f` it holds a specification and four formats
(`ratchet/1`, `ratchet-assessor/1`, `ratchet-evidence/1`, `ratchet-reference/1`) and no command
that works: every field name is a proposal until a release ships it. This repository is its first
consumer. On 2026-11-25 the owner ordered the quality of this repository to be defined with
Ratchet, in this repository.

Two readings were confirmed by Ratchet's maintainer session on 2026-11-25: a consumer writes its
own assessor declarations under `quality/assessors/`, without `command` where no program writes
evidence yet; and a baseline read from a CI run is one `ratchet-evidence/1` document per
assessment in a tracked directory, with no hand-written `ratchet-reference/1`.

## Planned scope

- `ratchet.toml` at the root and `quality/*.ratchet.yaml`: subjects, assessments with their
  measurements (kind, output, scope, unit, `better`), requirements and gates for what the
  repository measures today, written against the pinned commit.
- `quality/assessors/<name>@<version>.assessor.yaml`: one declaration per assessor the
  assessments name.
- `quality/records/`: one `ratchet-evidence/1` document per assessment whose value was read from
  a CI run on `main`, naming the run.
- `quality/README.md`: each measure with its source command, unit, direction and baseline with
  the run it was read from. `quality/not-measured.md`: what has no command that produces it.
- A check in the gate that fails when a document under `quality/` stops parsing, names a task or
  an assessor that does not exist, or when the README's table and the documents disagree.
- What a format cannot say goes to Ratchet as a finding; no field is invented here.

## Acceptance

- Every measure in `quality/README.md` names a command of this repository that produces it, a
  unit, a direction and a baseline with the CI run it was read from; a measure without such a
  command is in `quality/not-measured.md` and has no number (state transition: the documents are
  added).
- The gate's check goes red when an assessment names a task that `Taskfile.yml` does not have
  (state transition: the task is renamed).

## Scope

As built, 2026-11-25.

- `ratchet.toml`: the three specification files and the assessor directory.
- `quality/gate.ratchet.yaml`, `quality/docs-site.ratchet.yaml`, `quality/adapters.ratchet.yaml`:
  24 assessments, 43 measures, 36 requirements, 5 gates.
- `quality/assessors/`: 9 declarations, none with `command`.
- `quality/records/`: 21 `ratchet-evidence/1` documents. 18 name a run of 2026-11-24 on `main` at
  `59b8f4a858ab9c4eef94bc21de8e7bb85f90ddad` (`check`, `slow`, `Shared source gates`); the three of
  the documentation site name the `Docs UX` run of a pull request whose merge result has the tree
  of that commit.
- `quality/README.md`: one entry per measure with command, unit and direction, bound, baseline
  and the run or file it was read from. `quality/not-measured.md`: 16 entries with no command, 3
  whose command exists and whose value is not kept, 9 that the formats at the pin cannot say.
- `crates/inference-docs/tests/quality/mod.rs` with `adversary_q1.rs`, a module of the test binary
  `workflows` of `inference-docs`, part of `task test-run`: the check of the acceptance. It reads
  no CI run.
- `crates/inference-docs/Cargo.toml`, `Cargo.lock`: `serde_norway` and `toml` as development
  dependencies; both were in `Cargo.lock` before.
- `.github/workflows/check.yml`, `crates/inference-docs/tests/workflows.rs`, `AGENTS.md`: a pull
  request that changes a file under `quality/` runs the full gate, also for a `*.md` file. Without
  this a change of `quality/README.md` alone skipped the check that reads it.
- `CHANGELOG.md`: one `Added` and one `Changed` entry.

Not built, and said on the page: no record for `conformance/passing-scenarios` and for the six
resource values of the local speech-to-text adapters; the check does not read the number in a
baseline that names a file, nor a bound without a number; four commands that stand in no `cmds`
list of `Taskfile.yml` are held against nothing; the three `*.ratchet.yaml` files were decoded by
this repository's test only, since no tool of Ratchet reads them at the pin.

Sent to Ratchet as findings, with no field invented here: 16 from the first round, of which one
was withdrawn, and 3 from the correction (the candidate of a merge result, one observation kind
per plan, the machine as a condition).
