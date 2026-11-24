//! The one-call layer: `transcribe`, `chat`, `speak` and `talk`, and their `_with` forms over a
//! given stack (`docs/design/sdk.md` §3.1). `speak_with` and `talk_with` take an `AudioHost` and sit
//! behind the feature `audio-host`; `speak` and `talk` open the default host and sit behind
//! `audio-device`; `transcribe`, `chat` and their `_with` forms need neither.

use std::path::Path;

use inference_text::Span;

#[cfg(feature = "audio-host")]
use crate::Voice;
use crate::{Result, Stack, Transcriber};

/// The text of the WAV file at `path`, over the stack the process names: [`transcribe_with`]
/// over the stack resolved from the environment and the stack files.
///
/// # Errors
///
/// Those of [`Stack::from_env`] and of [`transcribe_with`].
pub async fn transcribe(path: impl AsRef<Path>) -> Result<String> {
    transcribe_with(&Stack::from_env()?, path).await
}

/// The text of the WAV file at `path` over `stack`'s `stt` binding: the texts of its committed
/// spans, in order, joined by a space. One session of the recipe `transcribe`, opened and
/// closed. To transcribe more than one recording, or to get the spans, build a [`Transcriber`].
///
/// # Errors
///
/// Those of [`Transcriber::new_with`] and [`Transcriber::transcribe_file`].
pub async fn transcribe_with(stack: &Stack, path: impl AsRef<Path>) -> Result<String> {
    let spans = Transcriber::new_with(stack)?.transcribe_file(path).await?;
    Ok(spans.iter().map(Span::text).collect::<Vec<_>>().join(" "))
}

/// Says `text` on the default speaker of the default audio host, over the stack the process
/// names: [`speak_with`] over the stack resolved from the environment and the stack files.
///
/// # Errors
///
/// Those of [`Stack::from_env`] and of [`speak_with`].
#[cfg(feature = "audio-device")]
pub async fn speak(text: impl Into<String>) -> Result<()> {
    speak_with(
        &Stack::from_env()?,
        text,
        inference_audio_device::CpalHost::new(),
    )
    .await
}

/// Says `text` on `host`'s default speaker over `stack`'s `tts` binding and returns once the
/// speaker played the last frame: one session of the recipe `speak`, opened and closed. No
/// microphone is opened. To say more than one text, or to get the frames, build a [`Voice`].
///
/// # Errors
///
/// Those of [`Voice::new_with`] and [`Voice::play`].
#[cfg(feature = "audio-host")]
pub async fn speak_with(
    stack: &Stack,
    text: impl Into<String>,
    host: impl inference_audio_device::AudioHost,
) -> Result<()> {
    Voice::new_with(stack)?.play(text, host).await
}
