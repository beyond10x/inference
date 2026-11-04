---
format: aep.planning-md/3
id: story:gpt-live-speech-over-playout-stops-it
kind: story
status: active
title: Speech over a GPT-Live answer stops local playout
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-openai/src/live_duplex.rs
- confidence: cited
  path: adapters/inference-openai/tests/gpt_live_speech_over_output.rs
- confidence: cited
  path: examples/realtime-voice/src/lib.rs
- confidence: cited
  path: examples/realtime-voice/tests/acceptance.rs
revision: 10
transitions:
- {from: "draft", to: "proposed", at: "2026-11-04T03:53:28Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-11-04T03:53:28Z", actor: "human:timo", revision: 5}
---
## Context

Found on 2026-10-30 while writing `examples/realtime-voice/tests/acceptance.rs`
(approval-record:wave-0-9-0). The GPT-Live binding reports no speech start and cancels no response
(`adapters/inference-openai/src/live_duplex.rs:31-37`); `Conversation` stops playout only on
`SpeechStarted` or a provider-side cancel (`examples/realtime-voice/src/lib.rs`, the barge-in
paragraph of its module comment). With `--provider openai-live`, when the user speaks over an
answer, the model goes quiet but audio already queued at the speaker still plays. Asserted as
today's behaviour by `gpt_live_conversation_is_interrupted_and_answers_the_time`.

## Planned scope

- Decide which GPT-Live signal marks the user speaking over playout (for example an input
  transcript or input-audio event arriving while the speaker holds unplayed audio) and stop
  playout on it in `Conversation`, for configurations whose binding takes no truncation.
- Change the GPT-Live acceptance test to assert that nothing more plays after the user speaks.

## Acceptance

`cargo test -p realtime-voice --test acceptance` passes with the GPT-Live case asserting that the
fake speaker's queue is empty after speech over an answer, and the OpenAI Realtime and Nova Sonic
cases unchanged.

## Scope

Confirmed against the merged diff (0.13.0 wave, d2fccdf to 0365212).

| path | planned | landed |
|---|---|---|
| `examples/realtime-voice/src/lib.rs` | inferred | cited: module comment only; the transcript rule of the first attempt was reverted |
| `examples/realtime-voice/tests/acceptance.rs` | cited | cited |
| `adapters/inference-openai/src/live_duplex.rs` | not planned | cited: the fix (`Spoken`, `speech_over`) |
| `adapters/inference-openai/tests/gpt_live_speech_over_output.rs` | not planned | cited, new |
| `adapters/inference-openai/tests/adversary1_gpt_live_speech_over_timing.rs` | not planned | cited, new (adversary) |
| `examples/realtime-voice/tests/adversary1_gpt_live_transcript.rs` | not planned | cited, new (adversary) |

Correction: the planned surface was the conversation in `realtime-voice`; the change landed in
the GPT-Live binding, because an untimed transcript cannot tell the question's tail from speech
over the answer.
