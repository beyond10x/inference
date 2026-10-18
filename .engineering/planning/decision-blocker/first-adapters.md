---
format: aep.planning-md/3
id: decision-blocker:first-adapters
kind: decision-blocker
status: open
title: Nobody has decided the first two adapters and the swap backend (Q11)
relations:
- blocks: epic:local-stt-adapter
- blocks: epic:local-llm-adapter
- blocks: epic:local-tts-adapter
- blocks: epic:backend-replacement
revision: 2
---
## Question

Open question 11 of the design: "What are the first two adapters to use when proving that backend
replacement is genuinely inexpensive?" (`docs/design/inference.md:1236`). §5.3 says two or three
adapters (line 273); §17 lists eleven candidates (lines 915-927); §17.1 names a reference stack
(lines 933-938).

## What it stops

The three local adapter epics and backend-replacement: each needs one named backend before its
first story can say what it wraps.

## Recommendation (review F6, `docs/design/review-2026-10-18.md`)

| role | first | swap |
|---|---|---|
| STT, TTS, VAD | sherpa-onnx (one toolkit, one FFI boundary, CPU-capable) | whisper.cpp for STT or Piper for TTS |
| LLM | llama.cpp server over its local HTTP API | vLLM |

Kyutai streaming STT and TTS and Moshi arrive with native-voice, where native streaming is the
point. Licences are verified at adoption (`decision-blocker:weight-licenses`), none assumed here.

## Default on silence

The recommendation above, subject to `decision-blocker:reference-hardware`.

## Clears when

The owner names the backends, or accepts the recommendation, and the answer is written into this
body and into the adapter epics.
