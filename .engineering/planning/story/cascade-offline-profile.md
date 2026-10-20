---
format: aep.planning-md/3
id: story:cascade-offline-profile
kind: story
status: active
title: 'offline_strict enforced: planning refuses an adapter the profile does not admit, and a run fails on an outbound connection'
relations:
- decomposes: epic:local-voice-cascade
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T08:01:30Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-20T08:01:30Z", actor: "human:timo", revision: 4}
---
## Context

A-04 (`docs/design/inference.md:1140`): the local cascade executes with external network access
denied; section 23.2 (1120): local-only profiles prohibit external networking. The profile table
(1046) gives `offline_strict` in-process or local workers only, egress denied.
`ess/domains/graph.yaml` declares the `locality-denied` outcome of `inference.graph.PlanPipeline`
as external, the planner to decide. `inference-pipeline` does not decide it: `PlanPipelineError`
has no such variant (`crates/inference-pipeline/src/planning.rs:93-107`), and the testkit target
forces the outcome (`crates/inference-testkit/tests/target/graph.rs:27-32`, "check 8 is not
planned yet"). At run time nothing watches the network: every run is offline today only because
its mocks open no socket.

This story needs no other story of the epic: its tests run the existing testkit voice cascade
(`inference_testkit::cascade`). `story:local-voice-cascade-example` puts the same watch around
the cascade with the real adapters.

Decisions taken on silence:

- Planning: a binding whose adapter `locality` is not in the `admitted` list of the pipeline
  profile refuses planning with `PlanPipelineError::LocalityDenied`, naming the node, the adapter,
  the locality and the profile, and the pipeline stays Validated. The planner reads `admitted` as
  declared and adds no rule about which localities a kind admits beyond the table rules
  `DeclareProfile` already holds (`crates/inference-core/src/policy.rs`, `TableRule`).
- Forty files under `crates/` and `adapters/` declare a profile with `admitted` empty (count of
  files containing `admitted: Vec::new()` or `admitted: vec![]`, 2026-10-20), while the mocks they
  bind declare `Locality::SameProcess`. Each of them whose pipeline binds an adapter changes to
  `admitted: vec![Locality::SameProcess]` in this story, so the tree stays green in one commit.
  It is a mechanical one-line edit per file and the only edit to those files.
- Loopback is not egress. A local worker reached on 127.0.0.1 or ::1 is admitted compute under
  `offline_strict`; egress is a connection to any other address. Read from the table row (1046)
  and the owner choice of `llama-server` over its local HTTP API for the LLM (epic Decisions),
  and recorded so the A-04 run with the real LLM does not fail on its own backend.
- Run time: an egress watch in `inference-runtime` (new module, `cfg(target_os = "linux")`, no
  `unsafe`). Given a run whose pipeline profile has egress `denied`, it polls the sockets of the
  current process (the socket inodes under `/proc/self/fd` matched against
  `/proc/self/net/tcp`, `tcp6`, `udp` and `udp6`) and, on a socket whose remote address is neither
  unspecified nor loopback, fails the run with a `FailureReason` naming the address.
  `RunRequests` (`crates/inference-runtime/src/drive.rs`), which offers cancel and drain today,
  gains the request to fail the run. Polling can miss a connection opened and closed between two
  polls: the watch is a tripwire and a test instrument, not a sandbox, and its documentation says
  so. On another operating system it is absent and the tests that need it are not compiled.
- The negative test opens, from a mock node, a UDP socket connected to a TEST-NET-1 address
  (192.0.2.1, RFC 5737): connecting a UDP socket sends nothing and leaves the remote address
  visible to the watch.

## Planned scope

- `crates/inference-pipeline/src/planning.rs`: the locality check and the error variant; cases in
  `crates/inference-pipeline/tests/planning.rs`.
- The fixture edits of the second decision.
- `crates/inference-runtime/src/egress.rs` (new), its export in `src/lib.rs`, the fail request in
  `src/drive.rs`; `crates/inference-runtime/tests/offline_profile.rs` (new).
- Not changed: `ess/` (the outcome is declared), the testkit conformance target, which may keep
  forcing `locality-denied`.

## Domain relations

- Pipeline references DeploymentProfile, one: `ess/domains/graph.yaml`, `inference.graph.Pipeline`,
  relation `profile`.
- Pipeline owns Binding, many; Binding references Node and Adapter, one each:
  `ess/domains/graph.yaml`, `inference.graph.Pipeline` relation `bindings`, `inference.graph.Binding`
  relations `node`, `adapter`.
- The bound adapter `locality` (`ess/domains/model.yaml`, `inference.model.Adapter`, field
  `locality`) against the profile `admitted` list (`ess/domains/policy.yaml`,
  `inference.policy.DeploymentProfile`, field `admitted`) is the external outcome
  `locality-denied` of `inference.graph.PlanPipeline` in `ess/domains/graph.yaml`.
- Run references Pipeline, one, and owns NodeInstance, many: `ess/domains/graph.yaml`,
  `inference.graph.Run`, relations `pipeline`, `instances`.

## Acceptance

On Linux, `cargo test -p inference-runtime --test offline_profile` shows planning refused with
`LocalityDenied` for an `external_provider` binding under `offline_strict`
(`inference.graph.PlanPipeline/outcome/locality-denied`), the testkit voice cascade completing
under the egress watch, a node connecting to 127.0.0.1 leaving the run unaffected, and the run
Failed with a reason naming 192.0.2.1 once a node connects a socket to it; `task check` passes.

## Out of scope

A sandbox that prevents connections (network namespace, seccomp); the egress rules of the other
three profiles; the watch around the real adapters (`story:local-voice-cascade-example`); making
the testkit target answer `locality-denied` from the planner.
