---
format: aep.planning-md/3
id: story:quality-defined-in-ratchet-formats
kind: story
status: active
title: The quality of the repository is defined in Ratchet's formats
relations:
- serves: vision:inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T00:31:07Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-25T00:31:08Z", actor: "human:operator", revision: 3}
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
