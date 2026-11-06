//! The one-call layer: `transcribe`, `chat`, `speak` and `talk`, and their `_with` forms over a
//! given stack (`docs/design/sdk.md` §3.1). `speak_with` and `talk_with` take an `AudioHost` and sit
//! behind the feature `audio-host`; `speak` and `talk` open the default host and sit behind
//! `audio-device`; `transcribe`, `chat` and their `_with` forms need neither.
