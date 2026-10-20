---
format: aep.planning-md/3
id: story:stt-swap-by-configuration
kind: story
status: active
title: Cascade selects sherpa-onnx or whisper.cpp STT by configuration, same tests, no crates/ change
summary: A configuration value binds the cascade's stt node to either adapter; the same cascade test functions pass under both; effective capabilities drop incremental_input and output_streaming under whisper.cpp; git diff --stat of the swap lists no path under crates/ (A-05).
relations:
- decomposes: epic:backend-replacement
- serves: vision:inference
- depends_on: story:whispercpp-stt-adapter
- depends_on: story:local-voice-cascade-example
scope:
- confidence: cited
  path: examples/local-voice-cascade
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T19:17:05Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-20T19:17:05Z", actor: "human:timo", revision: 4}
---
## Context

Step 6 of the first demonstrator, "replace one backend from configuration and pass the same tests"
(`docs/design/inference.md:1175`), and A-05, "a different STT/TTS/LLM adapter can replace its
predecessor without changing domain contracts" (line 1140). The epic's decision: the swap is STT,
sherpa-onnx to whisper.cpp. The whisper.cpp adapter is `story:whispercpp-stt-adapter`; this story
puts it into the cascade by configuration.
The cascade with real adapters comes from `epic:local-voice-cascade`; no story of it existed when
this was drafted (2026-10-20), so the `depends_on` edge names the epic and should move to the
story that lands the real-adapter cascade example once it is drafted.

What the swap changes, read from the code:

- A node's `requires` is mandatory: planning refuses a binding whose adapter lacks one with
  `CapabilityMismatch` (`crates/inference-pipeline/src/lib.rs:326-345`). The mock cascade declares its
  `stt` node with `requires: [IncrementalInput, OutputStreaming]`
  (`crates/inference-testkit/src/cascade.rs:369` and 375), so a whisper.cpp binding to a node
  declared that way is refused before the run.
- Effective capabilities are the intersection of what the bound adapters implement over every
  required path, source to sink (`crates/inference-pipeline/src/planning.rs:5-11`, design §8.3
  line 443). The whisper.cpp record claims neither `incremental_input` nor `output_streaming`
  (design line 362); the sherpa-onnx record claims both (`story:sherpa-onnx-recogniser`). So
  effective(whisper.cpp) = effective(sherpa-onnx) minus {`incremental_input`, `output_streaming`}, and
  nothing is added. The difference is visible in the `inference.graph.Pipelines` view
  (`ess/domains/graph.yaml:2259`) only where the sherpa-onnx configuration's effective set holds one
  of the two; if it holds neither, both sets are equal and the difference shows only in the bound
  adapters' records.
- Behaviour: with a batch recogniser the transcript arrives once, at the endpoint, as committed
  spans; no hypothesis or revision precedes it.

## Domain relations

- `inference.graph.Binding -> inference.graph.Node`, one: inferable, `ess/domains/graph.yaml`, entity
  `inference.graph.Binding`, relation `node` (`references`, cardinality `one`, via `node_id`).
- `inference.graph.Binding -> inference.model.Adapter`, one: inferable, `ess/domains/graph.yaml`,
  entity `inference.graph.Binding`, relation `adapter` (`references`, cardinality `one`, via
  `adapter_id`). The swap is which adapter the `stt` node's binding references; no entity is added.
- `inference.model.Adapter -> inference.model.Model`, one: inferable, `ess/domains/model.yaml`, entity
  `inference.model.Adapter`, relation `model`.

## Planned scope

- Preconditions, checked first against the cascade's real-adapter example as landed. Each one that
  fails stops the story with a report, because fixing it under `crates/` would break A-05:
  1. endpointing does not depend on signals only the sherpa-onnx recogniser emits (the cascade
     epic allows the STT adapter's own signals or an energy VAD; only the VAD path survives a swap);
  2. the STT node the example runs drives any `Recogniser` through offer and end of input at the
     endpoint, and does not assume `incremental_input` (the testkit `SttNode` does,
     `crates/inference-testkit/src/stt/node.rs:122-124`);
  3. the node implementation the example uses lives outside `crates/`, or inside it already handles
     a recogniser that claims neither capability.
- A configuration value of the cascade example selects the STT backend, `sherpa-onnx` (default) or
  `whispercpp`, read where the example reads its other configuration (for tests, the variable
  `INFERENCE_CASCADE_STT`). It decides which adapter crate builds the `Recogniser`, whose
  `Adapter` record goes into the catalog, and which `adapter_id` the `stt` node is bound to
  (`BindNode`). Both adapter crates are ordinary dependencies of the example: no cargo feature
  decides the backend, so the switch is configuration alone.
- The example's `stt` node requires nothing the whisper.cpp record lacks. If the cascade story
  declared it with `incremental_input` or `output_streaming`, this story lowers that in the example's
  declaration and says so in the closing note; one added test keeps the stricter declaration and
  asserts that binding whisper.cpp to it is refused with `CapabilityMismatch` naming the node and
  the capability (A-13, line 1148).
- The cascade example's tests run once per configuration value, with the same test functions. A
  test asserting what only a streaming recogniser shows (a hypothesis before the endpoint) becomes
  conditional on the bound adapter's claimed capabilities, as the testkit's capability tests are
  (`crates/inference-testkit/tests/stt.rs:132`); every such condition added here is listed in the
  closing note. Weighted runs need `INFERENCE_SHERPA_ONNX_STT_DIR` and `INFERENCE_WHISPERCPP_STT_MODEL`
  and are ignored without them, as the
  two adapter stories define.
- One test plans the cascade under each value and asserts effective(whisper.cpp) =
  effective(sherpa-onnx) minus {`incremental_input`, `output_streaming`}, read from the
  `Pipelines` view; the closing note records both sets as observed.
- The diff gate: `git diff --stat <base>...HEAD`, from the commit before
  `story:whispercpp-stt-adapter` to the end of this story, lists no path under `crates/`. Paths
  outside `adapters/` and the example (`Cargo.lock`, a CI step, the AGENTS.md layout row) are
  listed in the closing note.

## Acceptance

With the configuration value set to `whispercpp` and nothing else changed, the cascade example plans,
speaks a response with network denied and passes the same cascade test functions it passes with
sherpa-onnx, its effective capabilities equal the sherpa-onnx set minus `incremental_input` and
`output_streaming`, and the swap's `git diff --stat` shows no path under `crates/` (A-05,
`docs/design/inference.md:1140`).

- The A-04 network-denied check and the A-08 and A-10 scenarios of the cascade pass under both
  values on the reference machine (CPU), with both checkpoints present; the run output goes in the
  closing note.
- Without checkpoints, `cargo test --workspace --all-features` passes and lists the weighted
  cascade runs as ignored.
- `task check` passes.
