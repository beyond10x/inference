---
format: aep.planning-md/3
id: story:realtime-voice-demo
kind: story
status: active
title: Talk to a realtime model from a laptop
summary: examples/realtime-voice wires microphone, native speech model and speaker, with barge-in and one demo tool.
relations:
- decomposes: epic:native-voice
- serves: vision:inference
- depends_on: story:audio-device-adapter
- depends_on: story:openai-realtime-adapter
- depends_on: story:nova-sonic-adapter
scope:
- confidence: inferred
  path: examples/realtime-voice
- confidence: cited
  path: examples/realtime-voice/tests/acceptance.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T22:29:55Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-19T22:29:55Z", actor: "human:timo", revision: 4}
---
## Context

The operator wants to talk to the realtime models from a laptop. Once the device adapter and the
provider bindings exist, one example wires them: microphone, native speech model, speaker.

## Planned scope

- `examples/realtime-voice`: choose the provider (OpenAI Realtime or Amazon Nova Sonic) by flag,
  capture from the default microphone, stream to the model through `inference-duplex`, play its
  audio to the default output, print both transcripts; barge-in stops playout at once and truncates
  the model's output at the played position; one demo tool (the local time) is offered to show a
  tool round trip. Keys from the local keychain or the AWS default chain, as the provider stories
  say.

## Acceptance

On a laptop with a headset, the operator holds a spoken conversation with each provider, interrupts
it mid-answer and hears it stop, and asks for the time and hears the tool's answer.

## Re-check 2026-10-29

Re-checked against `origin/main` at `de88601` on 2026-10-29 for the wave that moved `ess` to 0.76.0,
and left out of that wave.

- Built on `main`: `examples/realtime-voice` with the `talk` example (`--provider openai`,
  `openai-live` or `nova`), barge-in that stops playout and truncates at the heard position, and the
  `local_time` tool. Its tests that need no device or key run in that wave's full gate.
- `dependency-blocker:nova-bedrock-access` is cleared (2026-10-21).
- Open: the acceptance itself, a spoken conversation on a laptop with a headset with each provider,
  an interruption and the tool's answer heard. It needs the operator at a laptop with a headset; no
  code is planned for it. The story stays active until that run is recorded here.

## Covered without a headset 2026-10-30

Covered without a headset on 2026-10-30 (approval-record:wave-0-9-0), merged into the wave branch
at `7761004`:

- `examples/realtime-voice/tests/acceptance.rs`: one continuous scripted conversation per provider
  configuration (`openai_config()`, `openai_live_config()`, `nova_config()`) through `Conversation`,
  the testkit's mock model and the device adapter's fake host: the answer reaches the speaker with
  both transcripts, speech over the answer stops playout, and the `local_time` tool is answered by
  call id and its answer played. `cargo test -p realtime-voice --test acceptance`: 3 passed. The
  implementor broke `src/lib.rs` six ways, one at a time; each break turned at least one of the
  three red.
- GPT-Live: its binding reports no speech start and cancels nothing
  (`adapters/inference-openai/src/live_duplex.rs:31-37`, `examples/realtime-voice/examples/talk.rs:19`),
  so the conversation does not stop local playout when the user speaks over it; audio already
  queued at the speaker still plays. The test asserts that as today's behaviour. The story's planned
  scope names OpenAI Realtime and Nova Sonic; the gap is story:gpt-live-speech-over-playout-stops-it.

Open: the acceptance itself, a spoken conversation on a laptop with a headset with each provider,
an interruption heard, and the tool's answer heard. It is the operator's run:

```
cargo run -p realtime-voice --example talk -- --provider openai
cargo run -p realtime-voice --example talk -- --provider nova
```
