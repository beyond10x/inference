---
format: aep.planning-md/3
id: story:sentence-clips-with-durations
kind: story
status: active
title: Synthesis returns one clip per sentence with its duration
tags:
- from-consumer-gap
relations:
- decomposes: epic:sdk
- serves: vision:inference
- depends_on: story:sdk-one-call
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T13:16:54Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T13:16:54Z", actor: "human:timo", revision: 3}
---
## Context

A consumer dubbed a 60 s screencast on 2026-11-24 and cut its video scenes to the narration. Its
third gap: per-sentence clips with their durations, so a caller can cut video scenes to the speech.
It measured the durations of the files a local synthesiser binary wrote.

The TTS contract already reports audio per committed segment, and story:usage-tts counts output
audio per synthesis. No call returns the audio of one sentence with its duration. Serves O1 and O2.

## Planned scope

- A call on `Voice` that takes a text, splits it into sentences with the segmenter the pipeline
  already uses for committed text, synthesises each as its own segment and returns one clip per
  sentence: the sentence text, its frames and its duration computed from the frame count and the
  sample rate.
- A file form that writes one WAV file per sentence into a directory and returns the list of paths
  with their durations.

## Acceptance

- A test over the scripted synthesiser: a text of three sentences returns three clips in order;
  each clip's duration equals its sample count divided by the sample rate; the concatenation of the
  clips' frames equals the frames of the whole text synthesised segment by segment.
- A text with no sentence end returns one clip. An empty text returns no clip and no error.
- The file form writes one file per clip, and the sum of the returned durations equals the summed
  length of the files.

## Out of scope

Word-level alignment (`audio_text_alignment` stays as each adapter's record declares it); video
editing; a pause model between sentences.
