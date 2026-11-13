---
format: aep.planning-md/3
id: story:voice-cascade-recipe-tools
kind: story
status: draft
title: The voice_cascade recipe offers its tools to the cascade's model node
revision: 1
---
## Context

`Recipe::tools` gives tools to the `native_voice` recipe only
(`crates/inference/src/recipe.rs:307-311`: "Every other recipe keeps them unused"), and the run
builds the tool list only for `Topology::NativeVoice` (`crates/inference/src/recipe/run.rs:229-234`).
The pipeline itself can run a tool in the cascade (story:cascade-read-only-tool, implemented), but
an application using the `voice_cascade` recipe cannot hand its model node a tool, so a spoken,
tool-using agent on the local pipeline is not reachable through the SDK.

## Planned scope

- `Recipe::tools` applies to `voice_cascade`: its model node is offered the tools and the run's
  tool executor node runs each authorized call on the executor given for it, as `native_voice`
  does.
- The commit-gate and basis-span rules of the cascade (side-effecting tools only on committed
  input) hold for these calls.

## Acceptance

- A test runs `voice_cascade` with one read-only tool on mock adapters: the model proposes the
  call, the executor runs it, and the reply spoken carries its result.
- A side-effecting tool proposed on uncommitted input is refused, as in the pipeline today.
- The doc comment of `Recipe::tools` names both recipes.
