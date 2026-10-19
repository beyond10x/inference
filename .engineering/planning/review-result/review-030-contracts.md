---
format: aep.planning-md/3
id: review-result:review-030-contracts
kind: review-result
status: active
title: Read-only review of the STT and TTS contracts since 0.3.0 (haiku)
relations:
- reviews: story:audio-and-stt-contract
- reviews: story:tts-contract
prose_only: read-only general-purpose reviewer requested by the operator; findings are prose, each traced to a line
revision: 1
---
defects found: 6 (2 behavioural, 4 doc or mock mismatches; all from reading the code, nothing built or run, since cargo would write to the repo's target/)

1. **Verified, medium: a segment can be offered to two syntheses.** `crates/inference-text/src/lib.rs:6` derives `Clone` on `OutputSegment`. `crates/inference-tts/src/lib.rs:20-21` says "a segment offered to one synthesis cannot be offered to another", and `:344-358` (`offer`) only checks held ids within one synthesis. Trigger: `let s2 = seg.clone(); a.offer(seg); b.offer(s2);`. Both queue, and both can later `acknowledge_playback`, so one segment is played twice. The `compile_fail` doctest (`lib.rs:23-33`) only shows the move case. `tests/adversary1_segment_identity.rs:4-6` repeats the false guarantee. Fix: drop `Clone` or make the segment identity single-use. Sure: verified by reading the derive and `offer`.

2. **Verified, low: envelope sequence overflows at i64::MAX.** `crates/inference-stt/src/lib.rs:489-492` does `self.next_sequence += 1` with no check. `RecognitionRequest::continuing` (`:105-121`) accepts any `i64`. Trigger: `continuing(…, i64::MAX, …)` then `hypothesis("x")`. Debug panics on overflow. Release wraps to i64::MIN, so the stream goes backwards. Public API, so reachable. Sure: verified by reading. Fix: `checked_add` and refuse, or document a range.

3. **Verified, low: mock declares max_concurrency 1 but does not enforce it.** `crates/inference-testkit/src/stt.rs:180-181` and `:213-220` (`start` returns an owned `MockRecognition`, so two can run at once). Same in `crates/inference-testkit/src/tts.rs:77-78` and `:111-119`. Design §23.2 says capability declarations must match observed behaviour. Sure: verified by reading.

4. **Verified, low: mock doc says frames are taken "as offered", but they are taken on `next()`.** `crates/inference-testkit/src/stt.rs:11-13` says "takes each frame as it is offered". `offer_frame` (`:296-298`) only queues; `take_input` runs in `advance()` (`:251-252`). Same mismatch in `crates/inference-testkit/src/tts.rs:12-13` vs `next()` at `:142-147`. `output().frames_taken()` lags after `offer`. Sure: verified.

5. **Suspected, low-medium: playback confirmation is not kept from the adapter.** `crates/inference-tts/src/lib.rs:16-18` says acknowledgement is "the media sink's call, never the synthesiser's", and `:583-585` says "an implementation never calls them itself". But `SynthesisOutput::acknowledge_playback` (`:470-478`) and `discard` are public, and an adapter owns its `SynthesisOutput` (the mock holds it as `output`, `tts.rs:131-132`). Nothing in the types stops the adapter calling it. The design rule (§7.3, line 388) is actor-level, and the contract only documents it. Suspected: I read the call path but did not compile a counterexample.

6. **Verified, low: doc contradicts itself on frames emitted at Synthesized.** `crates/inference-tts/src/lib.rs:44-49` says "no frame of a segment is emitted once it is Synthesized". `complete()` (`:439-459`) moves the segment to Synthesized (`:449`) and returns its frames in the same call (`:453-457`), with `frames_emitted` documented as final once Synthesized (`:527-528`). The counts are consistent; the wording is not. Verified by reading.

Checked and found consistent: STT and TTS state transitions against `ess/domains/stream.yaml` (Tentative queue refused, Played discard refused, Synthesized-only playback); envelope `kind` names and per-stream monotonicity within one recognition; `samples_at`, `data_len` (including the u16::MAX × u32::MAX test value), `taken_through` saturation; base64 strictness; design line citations 362, 386, 388, 390; the `stt-only` tree test. Not checked: the stream domain `EventLog` (testkit `stream.rs`, outside scope) and whether the test suites pass.
