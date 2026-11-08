---
format: aep.planning-md/3
id: story:cascade-turn-spans-in-a-burst
kind: story
status: draft
title: A cascade answers each of two turns that arrive in a burst with its own reply
tags:
- from-finding
relations:
- serves: vision:inference
revision: 1
---
## Context

Found while the fourth SDK wave diagnosed an intermittent cascade case of `story:sdk-agent`
(2026-11-08); it predates that story.

In `voice_cascade`, when two user turns arrive in a burst (audio fed from a file, or a test that
sends two utterances at once), the second turn's committed spans can be answered by the first
turn's generation, and the second turn then gets no answer at all:

- `AgentNode`'s `turn_completed` takes every committed span already on its input edge
  (`take_input`, `crates/inference-pipeline/src/nodes/agent.rs:465`).
- The turn events reach the node one hop later than the spans, through the cascade's turn
  fan-out, so the spans of turn 2 are often already on the edge when turn 1 completes.
- Turn 2's `TurnOpened` then supersedes that generation before any audio is routed (an
  `Interrupted`), and turn 2's `TurnCompleted` finds no span and starts no generation.

Measured with 16 agents at once over the two-utterance fixture: 44 of 960 rounds ended with no
reply audio, each stuck rather than slow (passing rounds took at most 34 ms). With one utterance
per round: 0 of 960.

A committed span (`TranscriptCommitted { span_id, text }`) names no turn, so the node cannot tell
which turn a span on its edge belongs to; the STT only promises that a turn's spans come before
that turn's `TurnCompleted`. Live microphone input leaves about 500 ms between turns, so the race
bites mainly on audio that arrives faster than real time.

## Acceptance

- Two turns of the cascade fixture fed at once over `voice_cascade` with the mocks yield two
  replies, each answering its own turn's text, in 960 of 960 rounds at 16 agents at once.
- The fix chooses one of: committed spans carry their turn, or text and turn events travel on one
  ordered edge; the choice is stated in the story before code.
