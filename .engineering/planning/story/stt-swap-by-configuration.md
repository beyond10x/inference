---
format: aep.planning-md/3
id: story:stt-swap-by-configuration
kind: story
status: implemented
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
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T19:17:05Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-20T19:17:05Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-20T22:31:21Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1}}}
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

## Closing note

### Preconditions, as read before any change

1. Endpointing does not depend on signals only the sherpa-onnx recogniser emits. The cascade's
   turns come from `inference-pipeline`'s energy voice activity detector (`SPEECH_VAD`: 0.02,
   30 ms attack, 600 ms hold) through the turn policy to the `stt` node's `turns` port; the
   `SttNode` acts on `TurnOpened` and `TurnCompleted` only, and no recogniser event reaches the
   turn policy. Holds.
2. The STT node the example runs is the testkit's `SttNode`. It offers each frame with
   `offer_frame` (which `inference-stt` accepts from any recogniser; only `take_input` before the
   end of input needs `incremental_input`), and at the turn's `TurnCompleted` calls `end_input`
   and drains the recognition. Its constructor documentation said "The recogniser is to claim
   `incremental_input`, so that a turn's frames reach its recognition before the turn completes";
   that is a statement of when frames are taken, not a check: nothing in the node refuses a
   recogniser that claims neither. Holds; confirmed by the run below (whisper.cpp, which claims
   neither, committed the clip's transcript through this node).
3. The node lives under `crates/` (`inference-testkit`) and already handles a recogniser claiming
   neither capability (point 2). Holds. Only its documentation line is stale; a patch for it was
   left to the coordinator, not applied.

### What changed

- `INFERENCE_CASCADE_STT` (`sherpa-onnx`, the default, or `whispercpp`) selects the recogniser
  bound at `stt`; `talk` reads it as `--stt` with the variable as fallback, beside its other
  options. It decides which adapter crate opens its checkpoint (`INFERENCE_SHERPA_ONNX_STT_DIR`
  or `INFERENCE_WHISPERCPP_STT_MODEL`), whose adapter record the catalog holds, and which adapter
  the `stt` node is bound to. Both adapter crates are dependencies of the example's existing
  feature `local`, which keeps the native builds out of the default build; no cargo feature
  decides the backend.
- The `stt` node's declaration was lowered: the cascade story declared it requiring
  `incremental_input` and `output_streaming`; it now requires nothing (`STT_REQUIRES`). The
  stricter declaration stays reachable (`STREAMING_STT_REQUIRES`, `declare_requiring`), and one
  added test asserts that binding whisper.cpp to it is refused with `CapabilityMismatch` naming the
  `stt` node and a capability whisper.cpp lacks. Planning names `output_streaming`, the first
  unmet capability in `Capability` order.
- Conditions added to the cascade tests on the bound adapter's claimed capabilities: none. No
  existing case asserted what only a streaming recogniser shows (a hypothesis before the turn
  completes); the A-08 case asserts the LLM never sees a hypothesis, which holds under both.
- The spoken clip of the weighted cases is the sherpa-onnx checkpoint's `test_wavs/0.wav`, read
  from `INFERENCE_SHERPA_ONNX_STT_DIR` under both values, so both checkpoints are needed for the
  whisper.cpp weighted run.

### Effective capabilities, as observed (the `Pipelines` view row of the planned cascade)

| `INFERENCE_CASCADE_STT` | effective capabilities |
|---|---|
| `sherpa-onnx` | `[OutputStreaming]` |
| `whispercpp` | `[]` |

The cascade has no source node, so every bound adapter contributes: sherpa-onnx claims both
streaming capabilities, the llama-server generator `output_streaming` only, the mock synthesiser
both. effective(whispercpp) = effective(sherpa-onnx) minus {`incremental_input`,
`output_streaming`}, and the difference is visible in the view. Asserted without weights by
`under_whispercpp_the_effective_capabilities_lose_incremental_input_and_output_streaming` and
with the opened adapters by `the_clip_is_answered_offline_and_every_segment_is_played_through_receipts`.

### Weighted runs (reference machine, CPU, both checkpoints present, loopback llama-server serving qwen2.5-1.5b-instruct q4_k_m)

`cargo test -p local-voice-cascade --all-features --test local_adapters -- --ignored --nocapture --test-threads=1`

`INFERENCE_CASCADE_STT` unset (sherpa-onnx):

```
test a_revised_committed_span_and_a_tentative_delta_of_the_server_are_stopped_at_the_gates ... recogniser: sherpa-onnx
committed input to first playable audio, turn 00000000-0000-4000-8000-000000000001: Some(374.800352ms)
ok
test the_clip_is_answered_offline_and_every_segment_is_played_through_receipts ... recogniser: sherpa-onnx
effective capabilities: [OutputStreaming]
committed input to first playable audio, turn 00000000-0000-4000-8000-000000000001: Some(35.577341ms)
heard: [(SpanId(305ef8a5-cab7-4086-a1e6-fa2e177a6330), "AFTER EARLY NIGHTFALL THE YELLOW LAMPS WOULD LIGHT UP HERE AND THERE THE SQUALID QUARTER OF THE BROTHELS")]
answered: "This appears to be a poem or short passage written in the style of a travel or travelogue. It describes a city or town at night, with the streets illuminated by yellow lamps and brothels visible in the dim light. The imagery evokes a sense of decay or a certain kind of atmosphere, but I can't"
ok
test the_clip_said_again_while_the_answer_plays_stops_it_and_supersedes_its_generation ... recogniser: sherpa-onnx
committed input to first playable audio, turn 00000000-0000-4000-8000-000000000001: Some(42.206535ms)
committed input to first playable audio, turn 00000000-0000-4000-8000-000000000002: Some(1.002260774s)
interruption to playback stop, turn 00000000-0000-4000-8000-000000000002: 20.425µs
heard: [(SpanId(48d8dc5e-32cb-4ac2-bef2-0391f6792b0f), "AFTER EARLY NIGHTFALL THE YELLOW LAMPS WOULD LIGHT UP HERE AND THERE THE SQUALID QUARTER OF THE BROTHELS"), (SpanId(4d492a6f-a77d-4f2f-96c8-754842c90312), "AFTER EARLY NIGHTFALL THE YELLOW LAMPS WOULD LIGHT UP HERE AND THERE THE SQUALID QUARTER OF THE BROTHELS")]
ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 38.74s
```

`INFERENCE_CASCADE_STT=whispercpp`, nothing else changed but its checkpoint variable:

```
test a_revised_committed_span_and_a_tentative_delta_of_the_server_are_stopped_at_the_gates ... recogniser: whispercpp
committed input to first playable audio, turn 00000000-0000-4000-8000-000000000001: Some(164.011451ms)
ok
test the_clip_is_answered_offline_and_every_segment_is_played_through_receipts ... recogniser: whispercpp
effective capabilities: []
committed input to first playable audio, turn 00000000-0000-4000-8000-000000000001: Some(37.738537ms)
heard: [(SpanId(189f588e-5060-4ab5-b9f7-00c2c3837475), "After early nightfall the yellow lamps would light up here and there the squalid quarter of the brothels.")]
answered: "The phrase you've provided appears to be a fragment of a larger narrative or description, likely from a story, poem, or descriptive text. It describes a scene at nightfall in a squalid area, where brothels are illuminated by yellow lamps. This image evokes a dark, possibly illegal or morally dubious setting"
ok
test the_clip_said_again_while_the_answer_plays_stops_it_and_supersedes_its_generation ... recogniser: whispercpp
committed input to first playable audio, turn 00000000-0000-4000-8000-000000000001: Some(39.00443ms)
committed input to first playable audio, turn 00000000-0000-4000-8000-000000000002: Some(147.143345ms)
interruption to playback stop, turn 00000000-0000-4000-8000-000000000002: 22.832µs
heard: [(SpanId(4949cf51-9555-4c25-95ca-d5cf8c135967), "After early nightfall the yellow lamps would light up here and there the squalid quarter of the brothels."), (SpanId(0f3fb3cf-c21a-4bb1-b005-aa73b8714483), "After early nightfall the yellow lamps would light up here and there the squalid quarter of the brothels.")]
ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 27.34s
```

The A-04 network-denied check is `assert_completed` in each case: the egress watch polled while
the run was Active and was not tripped. A-08 is the second case, A-10 the third.

### The diff gate

The story's wording: "`git diff --stat <base>...HEAD`, from the commit before
`story:whispercpp-stt-adapter` to the end of this story, lists no path under `crates/`." That
cannot hold as worded: between the commit before `story:whispercpp-stt-adapter` and this story's
base, other stories of waves 21 to 23 changed 66 files under `crates/`. The gate was applied to
this story's own change instead: `git diff --stat` from its base, the wave 23 integration commit
it was forked from, lists no path under `crates/`:

```
 AGENTS.md                                          |   2 +-
 Cargo.lock                                         |   1 +
 examples/local-voice-cascade/Cargo.toml            |   5 +-
 examples/local-voice-cascade/examples/talk.rs      |  36 +-
 examples/local-voice-cascade/src/lib.rs            |  50 ++-
 examples/local-voice-cascade/src/local.rs          | 388 +++++++++++++++++++--
 .../local-voice-cascade/tests/local_adapters.rs    | 313 ++++++++++++++---
 7 files changed, 698 insertions(+), 97 deletions(-)
```

Paths outside `adapters/` and the example: `Cargo.lock` (the example now depends on
`inference-whispercpp`, one line) and the `AGENTS.md` layout row of the example. No CI step
changed: `task check` already builds every workspace member with all features, the whisper.cpp
adapter among them.

### Review (adversary pass 1)

No defect against the acceptance; the same three weighted cases and one of the adversary's pass
under both values, network denied. Taken:

- `talk` read an empty checkpoint variable of the recogniser not selected as an error; it now
  ignores it, as its documentation says, and refuses an empty value of the selected one as missing.
- Unknown, empty, wrong-case, padded and aliased values of `INFERENCE_CASCADE_STT` are refused
  before any checkpoint is read (`adversary1_stt_swap.rs`).

Recorded, not changed:

- With the `stt` node requiring nothing, the positional `declare_local_adapters` plans the
  llama-server generator's record at `stt`; planning has never checked a binding's modality (the
  mock synthesiser was accepted at `stt` before this story too). `declare_local` passes the
  recogniser's record from a typed field and cannot swap them.
- `talk` links the sherpa-onnx C library dynamically, so it needs it under `whispercpp` too.
- The Context's "no hypothesis or revision precedes" a batch commit: whisper.cpp emits a
  Hypothesis and then the Committed span, both after the end of input.
