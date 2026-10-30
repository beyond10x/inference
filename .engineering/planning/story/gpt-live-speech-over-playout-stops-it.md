---
format: aep.planning-md/3
id: story:gpt-live-speech-over-playout-stops-it
kind: story
status: draft
title: Speech over a GPT-Live answer stops local playout
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: inferred
  path: examples/realtime-voice/src/lib.rs
- confidence: cited
  path: examples/realtime-voice/tests/acceptance.rs
revision: 3
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
