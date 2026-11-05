---
format: aep.planning-md/3
id: story:long-talk-over-does-not-resume-the-cut-answer
kind: story
status: draft
title: A long talk-over never resumes the cut answer
summary: 'Barge-in scenarios: short interruption, long talk-over and backchannel, each held by a case.'
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 1
---
## Context

A downstream adopter reports this barge-in failure in its own voice agent, not in inference's
code: when the caller talks over the agent for more than about 6 seconds, the agent resumes the
sentence that was cut. Short interruptions work. Barge-in quality is one of the adopter's three
criteria, beside latency and voice.

inference's `Conversation` with `auto_interrupt` stops playout when speech starts. The cascade's
`barge_in.rs` shows a second utterance superseding the first answer. No case covers a long
talk-over, a short interruption and a backchannel ("mm-hm") as separate scenarios, or says what
happens to the cut answer once a long talk-over ends.

## Planned scope

- Scenario cases over the testkit's scripted duplex model and the cascade's mocks, with no
  timing sleeps:
  - a short interruption;
  - a talk-over of 6 s and of 12 s;
  - a backchannel during the answer.
- For each: what playout does, whether any frame of the cut answer is played after the talk-over,
  and what the next answer answers.
- Measure first. Change code only if a case shows the cut answer resuming.

## Acceptance

- After a talk-over of any length, no frame of the cut answer plays again, and the next answer
  responds to what the caller said during the talk-over.
- A short interruption and a backchannel are each held by a case that records today's behaviour,
  named for what it shows.
