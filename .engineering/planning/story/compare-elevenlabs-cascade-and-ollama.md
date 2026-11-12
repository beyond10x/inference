---
format: aep.planning-md/3
id: story:compare-elevenlabs-cascade-and-ollama
kind: story
status: active
title: compare measures an ElevenLabs cascade with a local LLM on Ollama against the realtime arms
summary: compare's cascade arm can listen with ElevenLabs speech-to-text, its LLM is checked against Ollama, and the report names end of speech to first audio per arm.
relations:
- decomposes: epic:elevenlabs-route
- serves: vision:inference
- depends_on: story:talk-picks-elevenlabs
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: adapters/inference-llama-server
- confidence: inferred
  path: examples/realtime-voice/Cargo.toml
- confidence: cited
  path: examples/realtime-voice/examples/compare.rs
- confidence: cited
  path: examples/realtime-voice/examples/talk.rs
- confidence: cited
  path: examples/realtime-voice/src/compare.rs
- confidence: cited
  path: examples/realtime-voice/src/elevenlabs.rs
- confidence: cited
  path: examples/realtime-voice/tests
- confidence: cited
  path: website/docs/guides/compare-voice-providers.md
- confidence: cited
  path: website/docs/guides/run-the-local-voice-cascade-offline.md
- confidence: inferred
  path: website/docs/reference/environment.md
revision: 11
transitions:
- {from: "draft", to: "proposed", at: "2026-11-12T15:36:17Z", actor: "human:operator", revision: 10}
- {from: "proposed", to: "active", at: "2026-11-12T15:36:17Z", actor: "human:operator", revision: 11}
---
## Context

The operator wants to know, measured, how a stitched ElevenLabs cascade (ElevenLabs speech-to-text,
an LLM, ElevenLabs text-to-speech) compares in response time with the realtime speech-to-speech
arms `compare` already has. `compare`'s cascade arm listens with a local recogniser
(sherpa-onnx or whisper.cpp) and only speaks with ElevenLabs or Cartesia
(`examples/realtime-voice/examples/compare.rs`, `CascadeVoice`); the ElevenLabs recogniser of
story:elevenlabs-realtime-stt-adapter is used only by `talk --provider elevenlabs-cascade`
(story:talk-picks-elevenlabs). The cascade's LLM is any OpenAI-compatible chat-completions server
at `INFERENCE_LLAMA_SERVER_URL` (`adapters/inference-llama-server/src/lib.rs:165`); Ollama serves
that API at `http://127.0.0.1:11434/v1`, which has not been run against the adapter.

## Planned scope

- `compare --cascade-stt elevenlabs`: the cascade arm listens with `ElevenLabsRecogniser`, built
  through `realtime_voice::elevenlabs`; the local recognisers stay the default. The LLM is opened
  without the local recogniser's variables: `talk`'s private `generator()` and `key()` move from
  `examples/talk.rs` into `src/` and both examples use them.
- Fair timing: the ElevenLabs recognition commits manually when the arm's own end-of-input
  detector ends the caller's turn, the way the local cascade is driven. VAD commit would add its
  500 ms quiet window (`adapters/inference-elevenlabs/src/recogniser.rs:39-43`) to the cascade's
  first audio. The guide states the commit mode. The cascade runner reports a recogniser failure
  (`src/compare.rs:1662` passes `|_| None` today) instead of discarding it.
- Input and statistics for the comparison: `compare` takes question clips from a directory
  (`--clips <dir>`, WAV) in place of the one built-in question, and the table gives p95 next to
  median, lowest and highest (`Spread`, `src/compare.rs:2030-2039`). A `--runs-out <file>` option
  writes every run's per-step times as JSON lines.
- The cascade's LLM against Ollama: the existing ignored live test of `inference-llama-server`
  (`tests/live.rs`) is run with `INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:11434/v1` and a small
  model, after a warm-up request. Anything the adapter refuses is fixed in the smallest change.
  `website/docs/guides/run-the-local-voice-cascade-offline.md` names Ollama as a tested server,
  with the model and the date. The adapter reference (`crates/inference-docs/src/adapters.rs`) is
  not changed; that file belongs to story:status-page-voice-adapters.
- The test `examples/realtime-voice/tests/compare.rs::two_scripted_providers_give_their_figures_their_order_and_the_table`
  keeps its name; `crates/inference-docs/src/status.rs:328` names it.
- Docs: `website/docs/guides/compare-voice-providers.md` (the new flags, the `--help` block),
  `website/docs/reference/environment.md`, `CHANGELOG.md`.

## Acceptance

- A fixture test: `compare --cascade-stt elevenlabs` against a loopback ElevenLabs recogniser
  server and a fixture LLM produces a cascade row with every per-step column present.
- In that test, the cascade row's `first audio` is at least the fixture's injected delays and at
  most those delays plus 250 ms, so a run that waits out the recogniser's 500 ms quiet window
  fails.
- A fixture test: `--clips` with two WAV clips runs each clip on each arm.
- A fixture test: with more than one run, the table shows p95 next to median, lowest and highest.
- A fixture test: `--runs-out` writes one JSON line per run, each with its per-step times.
- A fixture test: a recogniser failure in the cascade arm is reported in the arm's row, not
  dropped.
- A recorded run: `inference-llama-server`'s live test passes against Ollama at
  `http://127.0.0.1:11434/v1`, recorded as test_result evidence with the model name.
