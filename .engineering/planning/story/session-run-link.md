---
format: aep.planning-md/3
id: story:session-run-link
kind: story
status: implemented
title: Session to Run link declared and the session store answering its scenarios
summary: A session references its current run, attach and ready are guarded, and the testkit answers every inference.session scenario.
relations:
- decomposes: epic:session-facade
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-testkit/src/session.rs
- confidence: inferred
  path: crates/inference-testkit/tests/conform.rs
- confidence: inferred
  path: crates/inference-testkit/tests/target/mod.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/session.rs
- confidence: cited
  path: ess/SKIPPED.md
- confidence: cited
  path: ess/domains/session.yaml
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T20:42:41Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-19T20:42:41Z", actor: "human:timo", revision: 5}
- {from: "active", to: "implemented", at: "2026-10-19T21:20:17Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1,"review_outcome":5,"verification":1}}}
---
## Context

`epic:session-facade` waits for the Session to Run relation, which `ess/domains/session.yaml:50-54`
leaves `UNMAPPED`: a session "potentially spans multiple graph runs" (`docs/design/inference.md:105`)
and the envelope's `session_id` is optional (`docs/design/inference.md:310`), so a run may exist with
no session. The design does not say which side holds the link. The testkit has no session store
(`crates/inference-testkit/src/session.rs` is one line), so the 21 `inference.session` scenarios are
the only ones `ess/SKIPPED.md` lists.

Decisions taken by the coordinator on the operator's standing rule (decide the reversible option,
report it), recorded here so the decomposition of `epic:session-facade` starts from them:

- The session holds the link: it references its current run, zero or one, by a run id field on the
  session. A run carries no session field; the envelope's optional `session_id` stays the only
  run-side trace, so a run without a session stays valid and `ess/domains/graph.yaml` is unchanged.
  Over its lifetime a session may reference several runs, one at a time.
- Attaching a run is a session command. It is refused when the session is Draining or Closed, or
  when the run's pipeline is not the session's pipeline. Whether a session may replace a current run
  that is not terminal is the topology-switch question and stays `UNMAPPED` for the facade stories.
- `MarkSessionReady` requires a current run. Readiness stays an explicit command; ess reads a related
  row's fields, not its held state, so "the run is Active" is the host's to assert.
- History, the facade operations (send, receive, commit_input, interrupt, describe), the cancel
  branch of close and the topology switch stay `UNMAPPED`; the decomposition of
  `epic:session-facade` owns them.

## Planned scope

- Spec first, in `ess/domains/session.yaml`: the run id field and its `relations:` entry on
  `inference.session.Session`, the attach command with its outcomes and refusals, and the
  `MarkSessionReady` guard, each with `read from:` lines (105, 310, 339, 713-719). Replace the
  Session to Run `UNMAPPED` marker; keep the others. Run `ess specify validate --path ess` and
  `ess verify conform synthesize` (no refusal beyond the three ESS-SYNTH-001 ones), and record the
  scenario counts.
- The testkit's session store in `crates/inference-testkit/src/session.rs` and its conform target in
  `crates/inference-testkit/tests/target/session.rs`, reading the graph store for the pipeline's
  existence, plan state and mode, as the other domains' targets read theirs.
- `ess/SKIPPED.md`: every `inference.session` line removed once its scenario passes.

## Domain relations

- Session references Pipeline, one: `ess/domains/session.yaml`, `inference.session.Session`, relation
  `pipeline` (declared).
- Session references Run, zero or one, held on the session: declared by this story.
- Run references Pipeline, one: `ess/domains/graph.yaml`, `inference.graph.Run` (declared).

## Acceptance

`ess specify validate --path ess` exits 0 with the Session to Run relation declared and its
`UNMAPPED` marker gone. The testkit conform run answers every `inference.session` scenario, the
existing 21 and the ones the attach command adds, with none unsupported, and `ess/SKIPPED.md` lists
no `inference.session` id. Attaching a run whose pipeline is not the session's is refused, and
`MarkSessionReady` on a session with no current run is refused.

## Out of scope

The facade API and its crate, the topology switch, session history, subgraphs (`epic:session-facade`
stories that follow); any change to `ess/domains/graph.yaml` or the runtime.
