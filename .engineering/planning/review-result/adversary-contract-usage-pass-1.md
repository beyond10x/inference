---
format: aep.planning-md/3
id: review-result:adversary-contract-usage-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the contract usage and naming fixes
relations:
- reviews: story:contract-usage-and-naming-drift
revision: 1
---
unit: 0.19.0 RF-contracts story:contract-usage-and-naming-drift, unit head 6150375; the adversary's case is commit 9db6465 (one test file)
verdict: INFEASIBLE (one red case no implementer in this repository reaches), plus five notes
cases: executed 112→113, red 1
origin: introduced 4 / pre-existing 1 / undecided 0

**Case** (red): `crates/inference-stt/tests/adversary_rf1_untaken_tail.rs`. A call not marked failed that offered 1.5 s and took 1.0 s records final usage of 1000 ms; TTS refuses its record in the same state.

**Suite**: testkit conform 66; inference-stt usage 10, taken_audio 2, adversary1_usage 4; testkit stt_usage 5, adversary1_stt_usage_node 2, stt_conformance 13, adversary1_stt_mutants 10; `ess specify validate` valid.

**Attacked, not broken**: the additive contract (private fields only, no serde on the changed types; `AdapterDescriptor` has `Default`, `serde(default)`, `deny_unknown_fields`); facade totals go through `add`; N6 refuses alike in both paths; nothing reads `SynthesisEvent::name()` as an envelope kind; every STT implementer and the conformance kit stay green.

Cargo 2.1 min, wall about 12 min, peak `target/` 1.6G.

```findings
[
{"file":"crates/inference-stt/src/lib.rs","line":754,"category":"acceptance","severity":"warning","verdict":"INFEASIBLE","origin":"introduced","message":"usage() on a call not marked failed records a final input_audio_ms that leaves out frames offered and never taken, where TTS refuses its record; no implementer in this repository reaches it (red case crates/inference-stt/tests/adversary_rf1_untaken_tail.rs:81)"},
{"file":"crates/inference-testkit/src/stt/conformance.rs","line":578,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"nine doc lines still say input_audio_ms is the audio offered: deepgram lib.rs:41, recogniser.rs:281, sherpa-onnx stt.rs:27, whispercpp stt.rs:28, inference-stt tests/usage.rs:3, testkit stt.rs:19, conformance.rs:160, :314, :578"},
{"file":"ess/domains/stream.yaml","line":920,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"the UNMAPPED comment lists AudioChunk as an undeclared design event though audio.yaml now declares inference.audio.AudioChunk"},
{"file":"crates/inference-duplex/src/ledger.rs","line":1309,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"response_usage doc still says it takes a report in any state of the response, but it now refuses once the usage record is made"},
{"file":"crates/inference-duplex/tests/late_response_usage.rs","line":122,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"the drop is recorded by the test itself; the only non-test caller of response_usage (testkit duplex mock.rs:496) passes the refusal on with ? instead of counting a drop"},
{"file":"adapters/inference-deepgram/src/recogniser.rs","line":315,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"forward_input takes the whole batch and stops at the first forward failure, so frames never sent count as taken in input_audio_ms (read from the code, not run)"}
]
```
