---
format: aep.planning-md/3
id: story:local-voice-cascade-example
kind: story
status: implemented
title: 'The local voice cascade over the local STT, LLM and TTS adapters, microphone and speaker: an offline spoken answer'
relations:
- decomposes: epic:local-voice-cascade
- serves: vision:inference
- depends_on: story:cascade-commit-gates
- depends_on: story:cascade-barge-in
- depends_on: story:cascade-offline-profile
- depends_on: story:sherpa-onnx-recogniser
- depends_on: story:llama-server-adapter
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T10:21:04Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-20T10:21:04Z", actor: "human:timo", revision: 5}
- {from: "active", to: "implemented", at: "2026-10-20T13:52:24Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1}}}
---
## Context

The exit of P2 (`docs/design/inference.md:1160`): an entirely offline voice pipeline speaks a
response. A-04 (1140) with the real adapters; A-08 (1144) with the scenarios
`inference.stream.Span/state/Committed/refuses/inference.stream.ReviseSpan` and
`inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.QueueForSynthesis` passing
with real adapters in the loop; A-10 (1146) with them; the two section 20 measures (1022-1023)
emitted. The cascade, its gates, its sink, barge-in and the egress watch are proven on the mocks
by `story:cascade-commit-gates`, `story:cascade-barge-in` and `story:cascade-offline-profile`;
this story binds the local adapters and the audio device to the same declaration (A-05, 1141).

The adapters, as drafted when this story was written:

- STT: `story:sherpa-onnx-recogniser`, crate `inference-sherpa-onnx` feature `stt`, checkpoint
  directory from `INFERENCE_SHERPA_ONNX_STT_DIR`, locality `same_process`.
- LLM: `story:llama-server-adapter`, crate `inference-llama-server`, a `Generator` over a running
  `llama-server` at `INFERENCE_LLAMA_SERVER_URL` serving `INFERENCE_LLAMA_SERVER_MODEL`, locality
  `local_worker`, claiming `output_streaming` only and committing each delta as it streams.
- TTS: no adapter story yet. `epic:local-tts-adapter` has only the backend-neutral
  `story:tts-adapter-conformance`; its sherpa-onnx synthesiser waits on
  `decision-blocker:sherpa-tts-gpl-phonemiser` (open). This story depends on that adapter in
  words: once its story exists, a `depends_on` edge to it is added, and this story is not
  scheduled before.

The STT and TTS nodes of the testkit take the concrete mocks (`SttNode` holds a `MockRecogniser`,
`crates/inference-testkit/src/stt/node.rs:98-126`; `TtsNode` a `MockSynthesiser`,
`src/tts/node.rs:89-124`); the LLM node takes any `inference_llm::Generator`
(`src/llm/node.rs:252-266`).

Decisions taken on silence:

- `SttNode` and `TtsNode` become generic over `inference_stt::Recogniser` and
  `inference_tts::Synthesiser`, with the mock as default type parameter so every existing caller
  compiles unchanged. They stay in `inference-testkit`, and the example depends on it (features
  `stt`, `llm`, `tts`) as a normal dependency, which an unpublished example may. Moving the generic
  model nodes into a production crate is a later milestone.
- The LLM node is the testkit `ModelNode` over the generator of the LLM adapter, with the identity
  as its reply function.
- The profile of the declaration admits `same_process` and `local_worker`; the egress watch of
  `story:cascade-offline-profile` treats the loopback connection to `llama-server` as admitted
  compute. A server URL whose host is not loopback is refused by the example and the tests before
  the run starts.
- Environment: `INFERENCE_SHERPA_ONNX_STT_DIR`, `INFERENCE_LLAMA_SERVER_URL`,
  `INFERENCE_LLAMA_SERVER_MODEL`, and the variable the TTS adapter story names for its checkpoint
  (`INFERENCE_SHERPA_ONNX_TTS_DIR` if it names none). Each test needing them is `#[ignore]` with a
  reason naming them and fails with a message naming the missing one when run with `--ignored`.
- The spoken question is the fixture `question-16k.wav` (eSpeak NG, "What is the capital of
  France?") with its provenance file, copied into the example `tests/fixtures` as the adapter
  stories copy it. No new recording.
- The ignored tests play to the fake audio host, pulled at the real-time rate by the test, so they
  need no sound card; the laptop run uses the cpal host.

## Planned scope

- `crates/inference-testkit/src/stt/node.rs` and `src/tts/node.rs`: the type parameter.
- `examples/local-voice-cascade`: bindings of the local adapters in `src/lib.rs` behind a feature
  `local`; `examples/talk.rs` (clap derive: STT checkpoint directory, server URL and model, TTS
  checkpoint, input and output device by name or default), printing both measures per turn;
  `Cargo.toml` dependencies on the three adapter crates; `tests/local_adapters.rs` (new, ignored):
  1. offline answer: the fixture fed at the audio ingress, under the egress watch, yields a
     committed span containing "capital", committed assistant text, segments Played through sink
     receipts, and a run that reaches Completed after `stop` with no failure;
  2. A-08: a revision naming a Committed span the real recogniser produced, injected at the input
     gate, is refused; a tentative delta of a generation of the real LLM, injected at the output
     gate before its commit, is never queued for synthesis;
  3. A-10: the fixture fed again while the first answer plays stops it, the generation is
     Superseded, and no frame of it is played after the stop.
- `AGENTS.md` Layout row for the example (packages, variables, run command).

## Domain relations

- Pipeline owns Binding, many; Binding references Node and Adapter, one each:
  `ess/domains/graph.yaml`, `inference.graph.Pipeline` relation `bindings`, `inference.graph.Binding`
  relations `node`, `adapter`; one declaration, two sets of bindings.
- Pipeline references DeploymentProfile, one: `ess/domains/graph.yaml`, `inference.graph.Pipeline`,
  relation `profile`; the adapter `locality` against the profile `admitted` list is the
  `locality-denied` outcome of `inference.graph.PlanPipeline`.
- Generation owns OutputSegment, many: `ess/domains/stream.yaml`, `inference.stream.Generation`,
  relation `segments`.

No noun is added and `ess/` does not change.

## Acceptance

With the STT checkpoint, a running loopback `llama-server` and the TTS checkpoint named by their
variables, `cargo test -p local-voice-cascade --all-features -- --ignored` passes the offline
answer, A-08 and A-10 tests on the reference machine on CPU, and `task check` passes without
them.

## To try

Not the acceptance: on a laptop with a headset, `cargo run -p local-voice-cascade --example talk`
with the three locations, ask a question and hear the answer, then speak over it and hear it stop.

## Out of scope

Tools in the local cascade with real adapters (`story:cascade-read-only-tool` names its
demonstration); the whisper.cpp swap (`epic:backend-replacement`); GPU execution; echo
cancellation; moving the generic model nodes out of the testkit.

## Amendment

Amended on 2026-10-20 by the coordinating session, before the story started:

- TTS: no local TTS adapter exists yet (`decision-blocker:sherpa-tts-gpl-phonemiser` is open). The
  example binds the testkit's mock synthesiser behind the same binding the TTS adapter will take;
  `talk` plays its output. The TTS checkpoint variable and the A-08/A-10 parts that need real
  synthesis use the mock until the adapter lands, and the story that adds the adapter swaps the
  binding.
- STT input: the pinned sherpa-onnx checkpoint does not recognise the eSpeak fixture (it hears
  "capitals"). The offline-answer test feeds the checkpoint's LibriSpeech clip `test_wavs/0.wav`
  (read from `INFERENCE_SHERPA_ONNX_STT_DIR`) and asserts the committed span contains "nightfall";
  the A-10 test feeds the same clip again while the first answer plays.
- Acceptance otherwise unchanged: the ignored tests pass with the STT checkpoint and a loopback
  `llama-server` named by their variables, and `task check` passes without them.
