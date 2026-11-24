//! The one-call layer: `transcribe`, `chat`, `speak` and `talk`, and their `_with` forms over a
//! given stack (`docs/design/sdk.md` §3.1). `speak_with` and `talk_with` take an `AudioHost` and sit
//! behind the feature `audio-host`; `speak` and `talk` open the default host and sit behind
//! `audio-device`; `transcribe`, `chat` and their `_with` forms need neither.

#[cfg(feature = "audio-host")]
use crate::{Result, Stack, Voice};

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
