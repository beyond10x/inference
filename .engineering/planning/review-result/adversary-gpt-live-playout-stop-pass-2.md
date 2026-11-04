---
format: aep.planning-md/3
id: review-result:adversary-gpt-live-playout-stop-pass-2
kind: review-result
status: active
title: 'Adversary pass 2 on GPT-Live playout stop: a second interruption of one response is not reported'
relations:
- reviews: story:gpt-live-speech-over-playout-stops-it
revision: 1
---
## Pass

Adversary pass 2 on story:gpt-live-speech-over-playout-stops-it, against the correction `0d105d3`
on `impl/gpt-live-speech-over-playout-stops-it`; the cases are `5d7e2e6`,
`adapters/inference-openai/tests/adversary1_gpt_live_speech_over_timing.rs` (test file only), run
through the real binding over a local WebSocket replay.

verdict: CONFIRMED (1 red case)
cases: executed 115→120, red 1
origin: introduced 1 / pre-existing 0 / undecided 0

## Cases

- `a_second_interruption_of_the_same_response_is_reported`: RED. The user says "wait" at 1250 ms
  over the answer; the model is silent 300 ms (under the 800 ms gap), speaks again from 1900 ms on
  the same response, and the user says "stop" at 2100 ms: `both interruptions of the answer are
  speech over it … left: [SampleClockPosition(30000)] right: [SampleClockPosition(30000),
  SampleClockPosition(50400)]`.
- Green: `the_next_response_is_interrupted_as_the_first`, `the_stretch_edges` (a fragment at the
  answer's start is reported, one at its latest end is not),
  `out_of_order_output_fragments_widen_the_stretch`,
  `a_previous_responses_stretch_reports_nothing_over_the_next`.
- Regression (`-p inference-openai --features gpt-live,duplex`): gpt_live_speech_over_output 3,
  gpt_live_duplex 23, gpt_live 13, gpt_live_split 3, gpt_live_ping 2, gpt_live_stall_bound 7,
  duplex 32; realtime-voice acceptance 3, adversary1_gpt_live_transcript 6,
  adversary2_nova_barge_in 1, wiring 12, usage 10; fmt and clippy clean.

## Finding

`adapters/inference-openai/src/live_duplex.rs:703` (`|| spoken.interrupted`, set at :709): speech
over a response is reported once per response, so a second interruption after the model yields
for less than the segmentation gap and continues the same response is not reported and playout is
not stopped. Reached by the default 800 ms `Segmentation::gap` and the hosted model's recorded
continuation of a response after input during output (the binding's claims doc, 2026-10-20).

Tried and not broken: the stretch's edges, out-of-order output timings, the reset between
responses, a stale stretch from the previous response, a response with audio and no transcript;
OpenAI Realtime `duplex` 32/32; Nova Sonic's binding unchanged. The pass-1 realtime-voice cases
build `SpeechStarted` by hand and no longer test the binding; `gpt_live_speech_over_output.rs` and
this pass's file do.

```findings
[{"file":"adapters/inference-openai/src/live_duplex.rs","line":703,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"U1-A2-F1: speech_over reports SpeechStarted once per response, so when the model yields for less than the 800 ms segmentation gap, goes on with the same response and the user interrupts it again, no SpeechStarted is reported and playout is not stopped (red through the binding: adversary1_gpt_live_speech_over_timing.rs:240, [30000] against [30000, 50400]). Reached by: the default Segmentation::gap of 800 ms keeps a yield-and-continue as one response, and the hosted model was recorded continuing the same response after input during output (live_duplex.rs claims doc, 2026-10-20). Possible fix: re-arm when an output fragment starts at or after the interrupting fragment's end_ms."}]
```
