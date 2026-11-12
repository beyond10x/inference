---
format: aep.planning-md/3
id: approval-record:wave-elevenlabs-route-p2
kind: approval-record
status: draft
title: 'Wave approved: async speech handles, status page, a flaky test, and a billed ElevenLabs cascade latency comparison, release 0.29.0'
relations:
- decides: story:async-speech-handles
- decides: story:status-page-voice-adapters
- decides: story:realtime-voice-barge-in-stop-flake
- decides: story:compare-elevenlabs-cascade-and-ollama
- decides: task:voice-latency-comparison-run
- decides: task:release-elevenlabs-route-p2
revision: 1
---
## Decision

On 2026-11-12 the operator chose option B for the next wave of epic:elevenlabs-route ("would then
do as recommended. Including the comparison."):

- story:async-speech-handles
- story:status-page-voice-adapters
- story:realtime-voice-barge-in-stop-flake
- story:compare-elevenlabs-cascade-and-ollama
- task:voice-latency-comparison-run, billed runs on the operator's own keys
- task:release-elevenlabs-route-p2, release 0.29.0

Out of this wave: story:duplex-session-config-additions, story:eleven-realtime-duplex (parked), and
the SDK epic.

The stories go through the scopers and the plan-critic panel first. Findings that only sharpen
scope or acceptance are fixed without asking again. A finding that changes what the wave delivers
goes back to the operator.
