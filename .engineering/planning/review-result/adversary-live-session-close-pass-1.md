---
format: aep.planning-md/3
id: review-result:adversary-live-session-close-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the live-session close and bounded-log fixes
relations:
- reviews: story:live-session-close-and-bounded-log
revision: 1
---
unit: 0.18.0 RF-facade story:live-session-close-and-bounded-log, unit head 176cc84; the adversary's case is commit 511e476 (one test file)
verdict: NEEDS-CHANGE
cases: executed 13→15, red 1
origin: introduced 1 / pre-existing 0 / undecided 0

**Case** (`crates/inference-facade/tests/adversary_rf1_generation_release.rs`): one generation with one segment, then N later generations that start and complete, then the first segment's `AudioPlayed`, then `interrupt()` and `close()`. N=7 green; N=8 red at `:190` (`left: 1 right: 0`): the first generation's `AudioPlayed` is delivered after `SessionInterrupted`.

**Suite**: lib 3/0, adversary1_delivery_guards 2/0, interrupt_and_cancel 4/0, live_session_close_and_bounded_log 4/0; clippy clean; `ess specify validate` valid.

**Attacked, not broken**: failure, cancel and a left run's end all close a draining session with `SessionClosed` last; a Ready session whose run fails stays Ready until `close()`; a concurrent `cancel()` answers Refused as documented; the log bound holds at exactly 4096; API changes additive.

Cargo about 0.3 min, wall about 40 min, peak `target/` 312M.

```findings
[
{"file":"crates/inference-facade/src/live.rs","line":183,"category":"acceptance","severity":"warning","verdict":"INFEASIBLE","origin":"introduced","message":"ENDED_KEPT=8 releases a delivered generation's bookkeeping, so an interrupt after eight later generations ended delivers that generation's AudioPlayed, contradicting interrupt's promise; no pipeline in the repository shown to reach it."},
{"file":"crates/inference-facade/src/live.rs","line":78,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"Bookkeeping is released only on generation-end events delivered through an egress, so a pipeline whose egress carries none still grows without bound."},
{"file":"crates/inference-testkit/src/session.rs","line":263,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The testkit's Sessions::events doc still says every event emitted, and LiveSessions::events exposes no way to detect truncation."},
{"file":"crates/inference-facade/src/session.rs","line":643,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A >= to > mutant in Sessions::log lets the log reach 4097 and survives both bound tests."},
{"file":"crates/inference-facade/src/live.rs","line":1361,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"SessionDriver::drive doc still says only a run that completed closes a draining session."},
{"file":"crates/inference-facade/src/live.rs","line":34,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The crate doc claims kept state grows with neither session length nor input rate, yet history grows per turn."}
]
```
