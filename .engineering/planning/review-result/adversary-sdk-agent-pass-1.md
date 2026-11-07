---
format: aep.planning-md/3
id: review-result:adversary-sdk-agent-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on the Agent: an event after SessionClosed, the fake sink''s tap and holds, a drop without Closed'
relations:
- reviews: story:sdk-agent
revision: 1
---
## Report

Adversary pass 1 on `impl/sdk-agent` at `d45a0e5`; adversary tests committed on top as `e7de5c5`
on branch `review/sdk-agent-adversary-1`, in three files:
`crates/inference-facade/tests/adversary1_interrupt_after_close.rs`,
`adapters/inference-audio-device/tests/adversary1_fake_tap.rs` and
`crates/inference/tests/adversary1_agent_drop.rs`.

```
unit: story:sdk-agent pass 1, covering d45a0e5 plus test commit e7de5c5
verdict: NEEDS-CHANGE
red: 8 cases
origin: introduced 8 / pre-existing 0 / undecided 0
```

Decisions: F1, F3 to F8 are fixed on `impl/sdk-agent`. F2 is a no-op: the fake host's sink keeps
its capacity bound by default, because existing playout and loopback tests rely on `Full`, and
`FakeHost::unbounded()` opts out of it; the story says so, and the F2 test runs on
`FakeHost::new(..).unbounded()`. F7 was inferred from the code: a `Forward` from a live Agent that
refuses the cancel while the session stays Ready cannot be built from a test, so its fix is held by
unit tests of `Core::settle`.

```findings
- file: crates/inference-facade/src/live.rs
  line: 1125
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: an interrupt admitted while Ready whose cancel waits on a full ingress records SessionInterrupted after a concurrent close delivered SessionClosed, so receive yields an event after SessionClosed
- file: adapters/inference-audio-device/src/fake.rs
  line: 236
  category: acceptance
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: FakeHost::new's sink answers Full with no pull, while the story said the fake sink is never full
- file: adapters/inference-audio-device/src/playout.rs
  line: 339
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the tap logs a write refused as Full or for its rate in played(), and logs a push that waits for room once per retry
- file: adapters/inference-audio-device/src/fake.rs
  line: 213
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: holds are a bool, so dropping one of two holds releases the sink
- file: adapters/inference-audio-device/src/playout.rs
  line: 390
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: an unbounded sink refuses a 257th discontinuous frame as Full
- file: crates/inference/src/agent.rs
  line: 1244
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a stream held across the drop of an unclosed Agent ends without Event::Closed
- file: crates/inference/src/agent.rs
  line: 643
  category: judgement
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the barge-in gives its interrupt count back on any error, also a refused cancel after which the session still records SessionInterrupted, so that event goes unanswered (inferred)
- file: adapters/inference-audio-device/src/playout.rs
  line: 433
  category: judgement
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: Cleared is logged outside the queue's lock, so it can be logged after a frame queued later (inferred)
```
