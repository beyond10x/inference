---
format: aep.planning-md/3
id: story:speak-text-file-to-audio-file
kind: story
status: active
title: A text file becomes an audio file in one call, with the voice as configuration
tags:
- from-consumer-gap
relations:
- decomposes: epic:sdk
- serves: vision:inference
- depends_on: story:sdk-one-call
- depends_on: story:local-tts-adapter-under-licence-policy
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T13:16:54Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T13:16:54Z", actor: "human:operator", revision: 3}
---
## Context

A consumer dubbed a 60 s screencast on 2026-11-24 (754 characters of narration) and could not use
this repository for it. Its second gap, in its words: one call or one command, "text file in, audio
file out", with the voice as configuration. It ran a local synthesiser binary by hand instead.

story:sdk-one-call gives `speak_with(&stack, text, host)`, which plays on an audio host, and
`Voice::synthesise(text)`, which returns frames. Neither writes a file, and no synthesiser other
than the mock is bound by name (`crates/inference/src/registry/mock.rs:46`), so `Stack::from_env()`
cannot name a voice. Serves O1: one adapter used alone.

The repository ships libraries and no binary (`AGENTS.md`, Rules). The command form is therefore a
cargo example, like `talk` and `compare`.

## Planned scope

- `Voice::synthesise_to_file(text, path)` and the one-call `speak_to_file[_with]`: the text is
  synthesised and written as one WAV file in the synthesiser's own sample rate.
- The local synthesiser of story:local-tts-adapter-under-licence-policy is bound by name behind a
  feature of the `inference` crate, its voice directory read from the stack's configuration
  (builder, environment or TOML), so the voice is configuration and not code.
- An example that reads a text file and writes an audio file through that call.
- The documentation names which voice the example uses and where a caller sets another one.

## Acceptance

- A test over the scripted synthesiser: `speak_to_file_with(&stack, text, path)` writes a WAV file
  whose samples are the synthesiser's frames in order and whose header carries its sample rate and
  channel count.
- A stack built from TOML that names the local synthesiser and a voice directory binds it; a stack
  that names a voice directory that does not exist is refused with an error naming the path.
- The example turns a text file into a WAV file with no hosted call; its run is ignored without the
  voice directory in the environment.

## Out of scope

Compressed formats (MP3, Opus); a binary or an installed command; a default voice downloaded by a
build or a test.
