---
format: aep.planning-md/3
id: story:elevenlabs-realtime-stt-adapter
kind: story
status: active
title: ElevenLabs realtime speech-to-text as a hosted STT adapter
summary: 'Scribe realtime over WebSocket behind Recogniser: μ-law 8 kHz and PCM16 in, hypotheses and timestamped committed spans out, usage in audio seconds.'
tags:
- downstream-input
relations:
- decomposes: epic:elevenlabs-route
- serves: vision:inference
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: adapters/inference-elevenlabs/Cargo.toml
- confidence: cited
  path: adapters/inference-elevenlabs/records
- confidence: cited
  path: adapters/inference-elevenlabs/src/error.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/lib.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/src/recogniser.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests
- confidence: cited
  path: website/docs/reference/environment.md
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-12T09:37:26Z", actor: "human:operator", revision: 5, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "proposed", to: "active", at: "2026-11-12T09:37:26Z", actor: "human:operator", revision: 6, decided_on: {"recorded":{"review_outcome":1}}}
---
## Context

No ElevenLabs speech-to-text binding exists: `git grep -i scribe` on `main` at `88f43b9c` finds
nothing in `adapters/` or the store. A phone voice-agent consumer needs the speech-to-text half of
an all-ElevenLabs cascade, fed G.711 μ-law 8 kHz frames.

ElevenLabs documents realtime speech-to-text as a WebSocket at
`wss://api.elevenlabs.io/v1/speech-to-text/realtime` (read 2026-11-12 from
<https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime>):

- auth: header `xi-api-key`;
- query: `model_id` (`scribe_v2_realtime` default), `audio_format` (`pcm_8000` … `pcm_48000`,
  `ulaw_8000`), `language_code`, `commit_strategy` (`manual` default, `vad`), the VAD
  thresholds, `include_timestamps`, `keyterms`;
- client: `input_audio_chunk` with `audio_base_64`, `commit`, `sample_rate`;
- server: `session_started`, `partial_transcript`, `committed_transcript`,
  `committed_transcript_with_timestamps` (words with `start` and `end` in seconds), `warning`,
  and fourteen error types (`auth_error`, `quota_exceeded`, `rate_limited`, …).

The protocol reports no usage, so usage is the audio duration the binding sent.

## Planned scope

- `adapters/inference-elevenlabs`, feature `stt`: `ElevenLabsRecogniser` implementing
  `inference_stt::Recogniser` (`crates/inference-stt/src/lib.rs:1006`) at locality
  `external_provider`.
- Input `ulaw_8000` and PCM16 at the documented rates; any other format refused at configuration.
- Configuration: model, language, key terms, commit strategy `vad` (with its thresholds) or
  `manual`; manual commit maps to the contract's input end or commit.
- Events: `partial_transcript` as `TranscriptEvent::Hypothesis` or `Revision`;
  `committed_transcript_with_timestamps` as `Committed` with the span's start and end; one
  `Usage` record carrying input audio seconds at the end.
- Each documented error type as a typed `ElevenLabsError` variant; a bounded handshake and write
  deadline, `ws://` only on loopback, the key only in the header and zeroized, as the TTS binding
  does.
- The protocol summary saved beside the fixtures with its date; a claims record
  `records/claims-stt.json` held equal to the code.
- The backend-neutral STT checks of the testkit run against a local WebSocket fixture server.
- Files: a new `adapters/inference-elevenlabs/src/recogniser.rs` and its protocol module, with
  `src/lib.rs`, `src/error.rs`, `Cargo.toml`, `records/claims-stt.json`,
  `tests/claims_record.rs`, `website/docs/reference/environment.md` and `CHANGELOG.md`. The crate
  files are shared with story:elevenlabs-tts-provider-cancel, which depends on this story.

## Acceptance

- A fixture test without network: μ-law 8 kHz frames in, `Hypothesis` then `Committed` spans with
  timestamps out, then one usage record with the input audio seconds.
- A fixture test each for manual commit, VAD commit, a documented error type and a stalled server.
- The backend-neutral STT checks pass against the fixture server.
- An ignored live test behind feature `live`, its ignore reason containing `billed`, reads a 30 s
  German G.711 μ-law clip from `INFERENCE_ELEVENLABS_STT_CLIP` and checks committed spans with
  timestamps and a usage record.
