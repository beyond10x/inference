---
format: aep.planning-md/3
id: epic:elevenlabs-route
kind: epic
status: draft
title: ElevenLabs speech route for a phone voice-agent consumer
summary: ElevenLabs speech-to-text, text-to-speech and agent from a tagged release, fed μ-law 8 kHz frames, with provider-side cancel, usage and async handles.
tags:
- downstream-input
relations:
- serves: vision:inference
revision: 2
---
## Context

A phone voice-agent consumer, a tokio Rust service that already depends on these crates by git
tag, wants to run its speech on ElevenLabs through inference: speech-to-text, text-to-speech and
the realtime agent, fed G.711 μ-law 8 kHz audio frames. Phone transports (SIP, RTP, media
streams, call control) stay in the consumer; inference takes and returns audio frames only.

What `main` at `88f43b9c` has and lacks for that route:

- ElevenLabs TTS exists (`adapters/inference-elevenlabs/src/synthesiser.rs:149`), but a cancel
  stops nothing at the provider (`adapters/inference-elevenlabs/src/lib.rs:26-31`).
- The ElevenLabs agent binding exists (`adapters/inference-elevenlabs/src/agent_duplex.rs:395`),
  but sends neither the session's voice nor its instructions (`agent_duplex.rs:23-24`) and
  records no usage.
- No ElevenLabs speech-to-text binding exists.
- Every handle is blocking (`crates/inference-tts/src/lib.rs:1271`,
  `crates/inference-stt/src/lib.rs:1031`, `crates/inference-duplex/src/contract.rs:361`), so a
  tokio service holds one thread per call.
- `main` holds changes no tag carries (`#39`).
- The consumer also drives OpenAI Realtime through its own adapter, because the neutral duplex
  session configuration lacks controls its voice loop uses.

## Outcome

1. The consumer binds ElevenLabs speech-to-text and text-to-speech from a tagged release, feeds
   μ-law 8 kHz frames, cuts provider-side generation on barge-in and reads time to first audio;
   that release lists the breaking changes since the consumer's pinned minor (`0.12.0`).
2. A later tagged release carries the ElevenLabs agent with the session's prompt, first message,
   voice and language and its usage, and async forms of the speech handles.
3. The neutral duplex session configuration carries the controls the consumer's OpenAI Realtime
   loop uses, so the consumer can use inference's OpenAI Realtime binding instead of its own.
4. The status page and the adapter reference list every hosted voice adapter with the release
   that shipped it, so the consumer can pick bindings.

## Order

The `depends_on` edges are the order; the shared files each edge protects are named in the
dependent's body.

- P1: `story:elevenlabs-realtime-stt-adapter`, then `story:elevenlabs-tts-provider-cancel`, then
  `task:release-elevenlabs-route-p1`.
- P2: `story:elevenlabs-agent-session-overrides`, then `story:async-speech-handles` (worked alone),
  then `task:release-elevenlabs-route-p2`.
- P3: `story:duplex-session-config-additions` (worked alone) after the P2 release;
  `story:status-page-voice-adapters` after the P1 release.

## Out of scope

Telephony transport and call control. Code in the consumer's own repository.
