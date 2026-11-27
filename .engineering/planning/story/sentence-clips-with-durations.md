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
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: crates/inference/src/voice.rs
- confidence: cited
  path: crates/inference/tests/oneshot.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T13:16:54Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T13:16:54Z", actor: "human:operator", revision: 3}
---
## Context

A consumer dubbed a 60 s screencast on 2026-11-24 and cut its video scenes to the narration. Its
third gap: per-sentence clips with their durations, so a caller can cut video scenes to the speech.
It measured the durations of the files a local synthesiser binary wrote.

The TTS contract already reports audio per committed segment, and story:usage-tts counts output
audio per synthesis. No call returns the audio of one sentence with its duration. Serves O1 and O2.

Revised on 2026-11-27 (decision D64 A of the operator) after the story's scope read:

- No sentence segmenter exists in the tree. The earlier text named "the segmenter the pipeline
  already uses for committed text"; what cuts committed text today is the model wrapper, one text
  delta per segment. This story brings its own splitter.
- The SDK cannot say which committed segment a frame of one synthesis belongs to
  (`Event::Audio` carries no segment, and the agent's pump drops `SynthesisCompleted`). The earlier
  third clause of acceptance 1 needed that. It now compares with one synthesis per sentence, which
  keeps the story inside the `Voice` handle.

## Planned scope

- `crates/inference/src/voice.rs`: a call on `Voice` that takes a text, splits it into sentences,
  synthesises each sentence as its own `Voice::synthesise` call and returns one clip per sentence:
  the sentence text, its frames and its duration computed from the sample count and the sample
  rate.
- The splitter is private to `voice.rs`. A sentence ends at `.`, `!` or `?` (one or more) followed
  by whitespace or the end of the text; the text between ends is trimmed; a part that is empty
  after trimming is not a sentence. No abbreviation list.
- A file form that writes one WAV file per clip into an existing directory and returns the paths
  with their durations. Files are named `clip-<n>.wav`, `n` counting from 1 with three digits
  (`clip-001.wav`). A missing directory is an error; an existing file of that name is an error and
  is not overwritten; nothing is written when the check fails.
- One WAV writer in `voice.rs` (with `hound`, already a dependency of the crate), which
  story:speak-text-file-to-audio-file reuses. Integer and float PCM are written; mu-law and A-law
  frames are refused with an error that names the encoding.
- An empty or whitespace-only text returns no clip and no error, and opens no session.
- Tests in `crates/inference/tests/oneshot.rs` beside the `Voice` tests, over the scripted
  synthesiser.
- `CHANGELOG.md` under `## [Unreleased]`.

## Domain relations

A clip and a sentence are views, as the handles of story:sdk-one-call are: a clip is the audio
(`inference.audio` frames) of one committed output segment (`inference.stream.OutputSegment`) with
the segment's text. No entity, field or relation is added to `ess/`.

## Acceptance

- A test over the scripted synthesiser: a text of three sentences returns three clips in order;
  each clip's duration equals its sample count divided by the sample rate; each clip's frames equal
  the frames of `Voice::synthesise` called on that sentence alone (state transition: a text becomes
  an ordered list of clips).
- A text with no sentence end returns one clip. An empty or whitespace-only text returns no clip
  and no error, and the test sees no new session (state transition: none for an empty text).
- The file form writes one file per clip, named `clip-001.wav` onwards; the sum of the returned
  durations equals the summed length of the files read back; a missing directory and an existing
  `clip-001.wav` are errors and leave the directory as it was (state transition: a directory gains
  one file per clip, or is unchanged).
- Frames in mu-law or A-law are refused by the file form with an error that names the encoding.

## Out of scope

Word-level alignment (`audio_text_alignment` stays as each adapter's record declares it); video
editing; a pause model between sentences; abbreviations and other language rules of the splitter;
a per-segment audio reader inside one synthesis.
