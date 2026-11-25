---
format: aep.planning-md/3
id: story:tool-future-runs-inside-the-callers-runtime
kind: story
status: draft
title: A tool's future is polled inside a Tokio runtime
tags:
- from-finding
relations:
- informed_by: review-result:sdk-one-call-adversary-2
- serves: vision:inference
revision: 1
---
## Context

The second adversary pass on the one-call SDK functions (review-result:sdk-one-call-adversary-2,
finding 5) gave a chat a tool whose future awaits `tokio::time::sleep`. The call failed with
"there is no reactor running, must be called from the context of a Tokio 1.x runtime": the
executor node polls a tool's future on a thread of its own, without a runtime
(`crates/inference-pipeline/src/nodes/mod.rs:299-301`,
`crates/inference-pipeline/src/nodes/tool_executor.rs:466`). The thread and the polling are the
base's; before the unit the panic ended the node, now it fails the call and the model is told so.
`docs/design/sdk.md` §5 defines tools as `async fn` and does not name a runtime. A tool that calls
an HTTP client or waits on a timer is the usual tool. The docs of `Chat::tool` and
`AgentBuilder::tool` state the limit and the way around it (a handle the tool holds).

## Planned scope

- A tool given through the SDK (`Chat::tool`, `AgentBuilder::tool`, `#[inference::tool]`) has its
  future polled inside a Tokio runtime: the runtime current where the tool was given, or the
  SDK's own.
- The pipeline crate stays without a Tokio dependency.

## Acceptance

- A tool whose future awaits a Tokio timer answers its call through `Chat::tool` and through
  `Agent::voice().tool(..)`, and the reply carries its answer (state transition: the authority
  admits the call).
- A tool given outside any runtime behaves as the docs say.
