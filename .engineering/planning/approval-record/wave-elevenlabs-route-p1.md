---
format: aep.planning-md/3
id: approval-record:wave-elevenlabs-route-p1
kind: approval-record
status: draft
title: 'Wave approved: ElevenLabs speech-to-text and provider-side TTS cancel, then the next minor release'
relations:
- decides: story:elevenlabs-realtime-stt-adapter
- decides: story:elevenlabs-tts-provider-cancel
- decides: task:release-elevenlabs-route-p1
revision: 1
---
## Decision

The operator approved P1 of epic:elevenlabs-route on 2026-11-12, option A of the question put in
the coordinating session: commit the plan, implement story:elevenlabs-realtime-stt-adapter, then
story:elevenlabs-tts-provider-cancel, open and merge the pull request through the bot route, and
tag the next minor release (task:release-elevenlabs-route-p1). The work was requested by a phone
voice-agent consumer.

On that word these three move from draft to active: story:elevenlabs-realtime-stt-adapter,
story:elevenlabs-tts-provider-cancel, task:release-elevenlabs-route-p1. The P2 and P3 items of the
epic stay in draft.

## Units

One unit builds at a time: U2 depends on U1 (`depends_on`, shared crate files).

| unit | story | branch | surface |
|---|---|---|---|
| U1 | story:elevenlabs-realtime-stt-adapter | `impl/elevenlabs-realtime-stt-adapter` | `adapters/inference-elevenlabs` feature `stt`, testkit STT checks, `website/docs/reference/environment.md` |
| U2 | story:elevenlabs-tts-provider-cancel | `impl/elevenlabs-tts-provider-cancel` | `adapters/inference-elevenlabs` synthesiser and connection, `crates/inference-tts`, latency report |

- Integration branch: `wave/elevenlabs-route-p1`, forked from `main` at `88f43b9c`.
- Check path: `aep plan artifact waves --kind story --status active`, typed scope entries.
- Review: one adversary pass per unit (`aep:adversary`), a security-reviewer pass on U1
  (`aep:security-reviewer`, a new credential path); at most two attacks each.
- Implementors: `aep:implementor`.
- Live check: no ElevenLabs API key is on the machine, so the billed live test of U1 is written
  and ignored, and not run in this wave.

## Commits this approval authorises

One commit for the plan; per unit its commits and one merge into the integration branch; the
closing store commit; the pull request into `main` merged through the bot route once
`inference / task check` and `Security and privacy` are green; then the release commit, the
annotated tag and the GitHub release of task:release-elevenlabs-route-p1. Nothing else.
