---
format: aep.planning-md/3
id: review-result:adversary-sdk-agent-pass-2
kind: review-result
status: active
title: 'Adversary pass 2 on the Agent: a dropped device run keeps its sink and devices, an agent-side cut stalls a cascade run'
relations:
- reviews: story:sdk-agent
revision: 1
---
## Report

Adversary pass 2 on `impl/sdk-agent`, the whole story (`eeb1e00..29d536e`, 62 files), weighted
to what landed after pass 1: the switch, the STT control fix, the `Send` futures, the duplex
drain's answers, `native_voice`'s capability, `send_text` at commit, the event channel's item,
`devices.rs` and the per-run frame counts. Adversary tests committed on top as `ef631bc` on
branch `review/sdk-agent-adversary-2`, in `crates/inference/tests/adversary2_agent.rs`.

```
unit: story:sdk-agent pass 2, covering 29d536e plus test commit ef631bc
verdict: NEEDS-CHANGE
red: 3 cases
origin: introduced 6 / pre-existing 0 / undecided 0
```

Red cases:

- `after_a_dropped_device_run_the_reply_audio_is_on_the_stream`: after a dropped device run the
  reply's audio still went to the device and none reached the stream.
- `a_dropped_device_run_releases_the_microphone_and_the_speaker`: 2 s after the dropped run the
  microphone and the speaker were still open.
- `an_application_cut_of_a_cascade_reply_on_the_devices_closes_without_an_error`: after
  `agent.interrupt()` on a cascade device run the run failed with `Error::Layer` ("the played
  position stayed at Some(0) for 2s").

Green, not broken: frames fed while a close drains do not fail the device run (the R146 feeder
path); a commit of a cascade turn; concurrent `close()` with a held cascade sink.

Decisions:

- F1 to F3 are fixed on `impl/sdk-agent`.
- F4 is inferred from the code and was not built: it is recorded and left open.
- F5 is kept. A turn typed and not committed is the user's open turn, and it goes out at the
  next commit after a switch, as it would without one.
- F6 is answered by its wording: the drain's answer says the session closed before the call's
  result, which holds both for a call never started and for one whose executor already runs.

```findings
- file: crates/inference/src/devices.rs
  line: 282
  category: concurrency
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a dropped run_on_devices_with future never takes its sink out of the slot, so later reply audio still goes to the device and never onto the stream
- file: crates/inference/src/devices.rs
  line: 303
  category: concurrency
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a dropped run never clears listening and keeps the speaker in the sink, so the microphone and speaker streams stay open and a second run on the same host finds them busy
- file: crates/inference/src/devices.rs
  line: 113
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: an agent-side cut drops cascade frames the RoutedPlayout never counts as handed out or dropped, so after agent.interrupt() on a device run the cascade speaker stalls for 2 s and the run fails with Error::Layer
- file: crates/inference/src/agent.rs
  line: 1393
  category: concurrency
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: switch pauses the pump and then waits for the pump-spawned interrupt without aborting it; an interrupt blocked on a full Pause ingress waits for an egress only the paused pump drains (inferred, not built)
- file: crates/inference/src/agent.rs
  line: 679
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: texts typed with send_text survive a switch and reach the new recipe at the next commit; the story does not say whether they should
- file: crates/inference-pipeline/src/nodes/duplex.rs
  line: 511
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the drain answers every unanswered call rejected, including one whose executor is already running, so the model is told rejected for a tool whose side effect happens
```
