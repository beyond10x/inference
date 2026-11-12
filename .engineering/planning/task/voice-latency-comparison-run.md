---
format: aep.planning-md/3
id: task:voice-latency-comparison-run
kind: task
status: active
title: Billed latency comparison of the ElevenLabs cascade and the realtime arms
summary: Median and p95 of end of speech to first audio per arm on the same clips, recorded as evidence and in the compare guide.
relations:
- decomposes: epic:elevenlabs-route
- depends_on: story:compare-elevenlabs-cascade-and-ollama
- serves: vision:inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-12T15:36:24Z", actor: "human:operator", revision: 4}
- {from: "proposed", to: "active", at: "2026-11-12T15:36:25Z", actor: "human:operator", revision: 5}
---
## Context

With `compare --cascade-stt elevenlabs` (story:compare-elevenlabs-cascade-and-ollama) the operator
can measure an ElevenLabs cascade against the realtime arms. The operator approved billed runs on
their own keys for this comparison (2026-11-12).

## Planned scope

- Run `compare --clips <dir> --runs-out <file>` (both from story:compare-elevenlabs-cascade-and-ollama)
  on at least 3 recorded clips, German and English, 10 runs per clip and arm, after one warm-up
  run per arm. The arms are: the ElevenLabs cascade with a local LLM served by Ollama; the local
  recogniser cascade speaking with ElevenLabs; and every realtime arm whose key is configured on
  the operator's machine. Arms without a key are listed as not run, never estimated. The clips
  are generated speech, not anyone's recorded voice, and are not committed.
- Record per arm the median and p95 of `first audio` (end of speech to first reply audio) and of
  each step as test_result evidence on this task, with the commit, the clips' description, the
  model names, the residency host and the date.
- Add the result table to `website/docs/guides/compare-voice-providers.md` with its date and the
  conditions it was measured under. No code changes in this task.

## Acceptance

- Evidence on this task names, per arm, its clip count (at least 3, German and English), its run
  count (at least 10 per clip after one warm-up run), and the median and p95 of end of speech to
  first audio, with the conditions.
- Every arm `compare` offers appears in the evidence, as measured or as "not run: no key".
- The guide shows the table with the measurement date.
