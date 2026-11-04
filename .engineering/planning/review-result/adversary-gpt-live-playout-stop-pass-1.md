---
format: aep.planning-md/3
id: review-result:adversary-gpt-live-playout-stop-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on GPT-Live playout stop: 2 introduced findings on transcript timing'
relations:
- reviews: story:gpt-live-speech-over-playout-stops-it
revision: 1
---
## Pass

Adversary pass 1 on story:gpt-live-speech-over-playout-stops-it, against `b043cba` on
`impl/gpt-live-speech-over-playout-stops-it`; the cases are `3de9007`,
`examples/realtime-voice/tests/adversary1_gpt_live_transcript.rs` (test file only).

verdict: undecided (2 red cases; whether the hosted server sends events in that order is not observed)
cases: executed 26→32, red 2
origin: introduced 2 / pre-existing 0 / undecided 0

## Cases

- `a_trailing_fragment_of_the_question_does_not_cut_its_answer`: RED, `every frame of the answer
  is still to play, none dropped … left: 2400 right: 7200`, with a `BargeIn{Some(answer), heard:
  Some(0)}` note.
- `the_next_question_over_the_answers_trailing_silence_is_no_barge_in`: RED, `the user spoke after
  the answer, over silence … left: 1 right: 0`.
- Green: `a_transcript_over_an_idle_speaker_reports_no_barge_in`,
  `repeated_fragments_stop_the_answer_once_and_later_audio_plays` (one stop, no cancel or truncate,
  later audio plays), `a_transcript_during_the_tool_round_trip_leaves_the_answer_to_play`,
  `with_auto_interrupt_off_a_transcript_stops_nothing`.
- Regression: acceptance 3 passed, adversary2_nova_barge_in 1, wiring 12, usage 10, lib 0; fmt and
  clippy clean.

## Findings

- F1 `examples/realtime-voice/src/lib.rs:432` (the new `InputTranscribed` arm): any input-transcript
  fragment arriving while audio is queued stops the answer and drops that audio, though the model
  continues the same response. Reachable through the binding whenever the server sends that order:
  `live_duplex.rs:881-884` records each fragment with `start_ms`/`end_ms` dropped and
  `ledger.rs:1123` accepts it in any state. Not reachable through the scripted mock
  (`Shape::GptLive` puts the whole transcript at speech start, `acceptance.rs:287-293`). Hosted
  ordering not observed.
- F2 `lib.rs:432`: the binding delivers a response's trailing silent frames until the 800 ms gap
  (`live_duplex.rs:702-712`); a next question starting in that gap is reported as `Note::BargeIn`
  on the finished answer. Only silence is dropped; the harm is a false report. Hosted timing not
  observed.

Tried and not broken: OpenAI Realtime (`truncation` true, the branch never runs); Nova Sonic (the
binding reports `SpeechStarted` and completes the response at the user text block,
`aws/src/duplex.rs:806-818`, before any `InputTranscribed`); empty speaker, repeated fragments,
tool round trip; no cancel and no truncate on GPT-Live; the acceptance's new assertions fail
without the change.

```findings
[{"file":"examples/realtime-voice/src/lib.rs","line":432,"category":"concurrency","severity":"warning","verdict":"INFEASIBLE","origin":"introduced","message":"U1-A1-F1: a late input-transcript fragment of the user's own question, arriving after the answer's first audio, stops playout and drops the answer's queued audio while the model keeps answering (red: adversary1_gpt_live_transcript.rs:325, 2400 samples queued against 7200 expected). Reached by: the GPT-Live binding passes every session.input_transcript.delta through with start_ms/end_ms dropped (live_duplex.rs:881-884), and the ledger accepts it in any state (ledger.rs:1123). The scripted mock cannot reach it, because Shape::GptLive emits the transcript at speech start (acceptance.rs:287-293). Whether the hosted server sends events in this order has not been observed."},{"file":"examples/realtime-voice/src/lib.rs","line":432,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"U1-A1-F2: when the user's next question starts over the answer's trailing silence, it is reported as Note::BargeIn on the finished answer (red: adversary1_gpt_live_transcript.rs:354, 1 barge-in against 0 expected). Reached by: the binding keeps delivering silent output on the response until Segmentation::gap ends it (live_duplex.rs:702-712, 800 ms by default), so a user who speaks within that gap triggers it. Hosted timing has not been observed."}]
```
