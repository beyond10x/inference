---
format: aep.planning-md/3
id: story:native-speech-contract
kind: story
status: active
title: Native speech session contract and its mock
summary: ess declares the native speech session, inference-duplex carries the contract, the testkit mocks it.
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-duplex
- confidence: inferred
  path: crates/inference-testkit/Cargo.toml
- confidence: inferred
  path: crates/inference-testkit/src/duplex.rs
- confidence: inferred
  path: crates/inference-testkit/src/lib.rs
- confidence: inferred
  path: crates/inference-testkit/tests/target/duplex.rs
- confidence: inferred
  path: crates/inference-testkit/tests/target/mod.rs
- confidence: cited
  path: ess/SKIPPED.md
- confidence: cited
  path: ess/domains/duplex.yaml
- confidence: cited
  path: ess/system.yaml
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T20:56:22Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-19T20:56:23Z", actor: "human:timo", revision: 5}
---
## Context

The operator asked on 2026-10-19 for tests against two hosted realtime speech-to-speech models,
OpenAI Realtime and Amazon Nova Sonic (AWS Bedrock), with each provider in its own crate in this
repository. Both are native speech models: topology D (`docs/design/inference.md:790-801`), one
multiport node, never three fake components (§7.5, line 407; A-06, line 1142). Simultaneous input
and output belongs to the optional `inference-duplex` crate (§5.3, line 197), whose concepts
(§7.4, lines 392-403) are declared in the specification first (`epic:native-voice` Exit). Hosted
adapters must be enabled explicitly (lines 903-904); audio travels as bounded binary buffers, not
base64 JSON, on realtime paths (line 324). Capabilities `concurrent_input_output`, `native_duplex`
and `tool_calling` are declared in `ess/domains/model.yaml` (design 421-424).

`decision-blocker:native-duplex-evidence` (Q15) is cleared with its default on silence: one
conformance scenario decides whether an adapter may claim `native_duplex`.

## Planned scope

- Spec first: a new `ess/domains/duplex.yaml` declares the native speech session the two hosted
  models share: a session over one model, audio input appended and committed (or detected by the
  server's own turn detection), a response streaming audio and transcript, the input transcript,
  interruption by new speech (barge-in) cancelling the response in progress, and tool calls proposed
  inside the session and answered on it (reusing `inference.tools`). Add the Q15 scenario: with one
  open response, input accepted after output began, and output depending on it emitted before the
  response completes, with no second response id. Each declaration carries `read from:` lines.
  `ess specify validate`, synthesis (no refusal beyond the three ESS-SYNTH-001 ones), counts.
- `crates/inference-duplex`: the contract: a native speech model opens a session from a session
  configuration (voice, instructions, input and output audio formats from `inference-audio`, turn
  detection on or off, tools); the session takes audio frames, commit, cancel and tool results, and
  yields a typed event stream (output audio, output transcript delta, input transcript, speech
  started and stopped, response done, tool call, error with a typed failure kind). No provider type
  appears in it.
- What a telephony voice agent needs from it, each an explicit option or operation:
  - audio formats including G.711 mu-law at 8 kHz in 20 ms frames, passed through unresampled when
    the provider takes it, and PCM16 at 16 or 24 kHz;
  - turn detection server-side (plain or semantic VAD) or off, and automatic responses on or off,
    so the caller can create each response itself;
  - provider auto-interrupt on or off, so barge-in can be decided by the caller: cancel the
    response and truncate the output audio at the position the listener actually heard;
  - tool calls whose arguments arrive as JSON and whose results return by call id;
  - usage per response (audio and text tokens, cached tokens) and input transcription usage, as
    events a meter can read;
  - a liveness bound: a session whose provider sends nothing for a configured time yields a typed
    stall failure instead of silence; a closed transport yields a typed failure at once.
- Testkit: a scripted mock native speech model behind a `duplex` feature, and the conform target for
  the duplex domain.
- `inference-duplex` adds no dependency to any existing contract crate.

## Acceptance

`ess specify validate --path ess` exits 0 with `duplex.yaml`; the testkit conform run answers every
duplex scenario or lists it in `ess/SKIPPED.md` with the story that will answer it. A test drives the
mock through one session: audio in gives audio and transcript out on one response id; speech started
during the response cancels it and the next response has a new id; a proposed tool call answered on
the session yields a response that uses its result.

## Out of scope

Any provider (`story:openai-realtime-adapter`, `story:nova-sonic-adapter`); pipeline nodes and the
graph binding of a native speech node; WebRTC transport.
