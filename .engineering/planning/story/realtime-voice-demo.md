---
format: aep.planning-md/3
id: story:realtime-voice-demo
kind: story
status: draft
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
revision: 2
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
