---
format: aep.planning-md/3
id: approval-record:talk-picks-elevenlabs-into-p1
kind: approval-record
status: draft
title: 'Operator chose A: live checks with the operator''s key, and a talk demo that picks ElevenLabs joins P1'
relations:
- decides: story:talk-picks-elevenlabs
revision: 1
---
## Decision

The operator chose option A on 2026-11-12 for trying the ElevenLabs route against the real API:
the operator stored an ElevenLabs key in the keychain secret `elevenlabs-api-key`; the coordinator
runs the billed live checks of text-to-speech, speech-to-text and the agent; and a `talk` demo
that can pick ElevenLabs joins P1, so the next release ships it. The podcast on ElevenLabs waits
for story:elevenlabs-agent-session-overrides in P2.

On that word story:talk-picks-elevenlabs moves from draft to active and joins
wave/elevenlabs-route-p1 as U3, after U2 (shared `examples/realtime-voice`).

## Commits this approval authorises

U3's commits and its merge into the integration branch, within the commits
approval-record:wave-elevenlabs-route-p1 already authorises. Nothing else.
