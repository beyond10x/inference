---
format: aep.planning-md/3
id: story:facade-crate
kind: story
status: active
title: inference-facade crate holds the session domain, and the testkit store calls it
summary: The session state machine of ess/domains/session.yaml moves from the testkit into a new inference-facade crate; every inference.session scenario still passes.
relations:
- decomposes: epic:session-facade
- serves: vision:inference
scope:
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: crates/inference-facade/Cargo.toml
- confidence: cited
  path: crates/inference-facade/src/lib.rs
- confidence: cited
  path: crates/inference-facade/src/session.rs
- confidence: cited
  path: crates/inference-facade/tests/session.rs
- confidence: cited
  path: crates/inference-testkit/Cargo.toml
- confidence: cited
  path: crates/inference-testkit/src/session.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T22:56:32Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-19T22:56:33Z", actor: "human:timo", revision: 4}
---
## Context

`epic:session-facade` builds the optional session API of `docs/design/inference.md` §13 (703-743)
in the crate the design names for it, `inference-facade` (201; 243: it depends on `runtime` and the
domain contracts), which §5.3 (273) splits out once its contract is clear. Today the session nouns
live only in the testkit: `crates/inference-testkit/src/session.rs:4-5` says "No contract crate
declares a session yet, so the nouns of the domain are declared here", and the store there answers
the `inference.session` scenarios `story:session-run-link` made pass. Every later story of the epic
adds operations to the session, so the session has to live where an application can depend on it.

This story is the prefactoring the other session stories depend on: it moves, it does not extend.
`ess/domains/session.yaml` does not change.

Decisions taken on silence (the design does not settle them; each is reversible and holds until
somebody decides otherwise):

- The session state machine (Session, its state, the five commands, their refusals and events) is
  public API of `inference-facade`. The testkit's `SessionDomain` stays the conformance target's
  store and calls the facade's state machine instead of carrying one, as the testkit graph store
  wraps `inference_pipeline::Declaration` and `inference_runtime::run::Run`
  (`crates/inference-testkit/src/graph.rs:50-59`).
- What the session reads of the graph (a pipeline's existence, state and mode; a run's pipeline)
  it reads through a trait the facade declares and the testkit implements over `GraphDomain`, as
  `inference-pipeline` reads profiles and adapters through its `Catalog`
  (`crates/inference-pipeline/src/builder.rs:12-17`).
- ADR-005 (1190): sessions are optional and sit above graph executions. No workspace crate depends
  on `inference-facade` except `inference-testkit`, and that dependency sits behind a testkit
  feature, so a testkit user who wants no runtime builds none.
- The two rules `session.yaml` leaves `UNMAPPED` and the testkit does not enforce (a retried attach
  of the run already held; a run current in two sessions) stay unenforced: enforcing either in the
  facade would fail the synthesized attach scenarios the testkit now delegates to it.

## Planned scope

- New crate `crates/inference-facade` (`Cargo.toml`, `src/lib.rs`, `src/session.rs`), a workspace
  member through the existing `crates/*` glob, so the root `Cargo.toml` does not change;
  `Cargo.lock` gains the crate.
- `crates/inference-testkit/src/session.rs` keeps the interface the conformance target calls, so
  `crates/inference-testkit/tests/target/session.rs` does not change, and delegates every command
  to the facade. `crates/inference-testkit/Cargo.toml` gains the dependency behind a feature.
- Tests of the moved state machine in `crates/inference-facade/tests/session.rs`: each command's
  accepting branch and each refusal, read against `session.yaml`.

## Domain relations

- Session references Pipeline, one: `ess/domains/session.yaml`, `inference.session.Session`,
  relation `pipeline`.
- Session references Run, zero or one: `ess/domains/session.yaml`, `inference.session.Session`,
  relation `run` (the `Optional` field `run_id` carries the zero).
- Run references Pipeline, one: `ess/domains/graph.yaml`, `inference.graph.Run`, relation
  `pipeline`.

None is added or changed.

## Acceptance

`cargo test -p inference-testkit --all-features --test conform` answers every `inference.session`
scenario with none unsupported and `ess/SKIPPED.md` unchanged while
`crates/inference-testkit/src/session.rs` holds no session state machine of its own,
`cargo test -p inference-facade` passes the moved state machine's tests, and
`cargo tree --workspace -i inference-facade` names no dependent other than `inference-testkit`.

## Out of scope

Any change to `ess/`; the facade operations, history and the topology switch (the later stories of
`epic:session-facade`); a session bound to a running run.
