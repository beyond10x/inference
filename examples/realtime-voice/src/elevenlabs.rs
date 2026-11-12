//! The ElevenLabs bindings `talk` and `compare` make: every one at the hosted endpoint
//! [`endpoint`] reads from `ELEVENLABS_RESIDENCY`, so a key of a data-residency stack reaches
//! the host that takes it.

use std::sync::Arc;
use std::thread;

use inference_elevenlabs::{
    CommitStrategy, ConfigError, CredentialSource, ELEVENLABS_KEY_NAME, ElevenLabsRecogniser,
    ElevenLabsSynthesiser, Endpoint, InputFormat, OutputFormat, TranscriptionConfig, VadSettings,
};

use crate::compare::hosted_tts;
use crate::talk::Refused;

/// The hosted endpoint of the residency `ELEVENLABS_RESIDENCY` names
/// ([`Endpoint::from_env`]): the global host when it is unset or empty.
///
/// # Errors
///
/// [`Refused::Invalid`] naming the variable, never its value, when it names no residency.
pub fn endpoint() -> Result<Endpoint, Refused> {
    Endpoint::from_env().map_err(|error| Refused::Invalid(error.to_string()))
}

/// The recogniser of the ElevenLabs cascade at `endpoint`: `pcm_16000` from the microphone, the
/// endpoint's voice activity detection committing each turn, its key resolved through `source`
/// under `ELEVENLABS_KEY_NAME`. Making it sends nothing.
///
/// # Errors
///
/// The [`ConfigError`] of the endpoint's URL.
pub fn recogniser(
    endpoint: Endpoint,
    source: Arc<dyn CredentialSource>,
) -> Result<ElevenLabsRecogniser, ConfigError> {
    ElevenLabsRecogniser::new(
        endpoint,
        TranscriptionConfig::new(InputFormat::Pcm16000)
            .with_commit_strategy(CommitStrategy::Vad(VadSettings::default())),
        source,
        ELEVENLABS_KEY_NAME,
    )
}

/// Whether the ElevenLabs key resolves through `source` under `ELEVENLABS_KEY_NAME`. The
/// source's future is driven to its end on a Tokio runtime of its own, on a thread of its own,
/// since a source such as the keychain's reads on Tokio's blocking pool, and the caller may be
/// on a thread with no runtime or inside one. The key is dropped unread.
///
/// # Errors
///
/// Why it did not: the source's refusal, which never holds the key, no runtime to drive it on,
/// or a panic of the source.
pub fn resolves(source: &dyn CredentialSource) -> Result<(), String> {
    thread::scope(|scope| {
        scope
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| {
                        format!("no runtime to resolve the ElevenLabs key on: {error}")
                    })?;
                runtime
                    .block_on(source.resolve(ELEVENLABS_KEY_NAME))
                    .map(drop)
                    .map_err(|error| format!("the ElevenLabs key did not resolve: {error}"))
            })
            .join()
            .unwrap_or_else(|_| Err("the ElevenLabs key source panicked".to_owned()))
    })
}

/// The synthesiser of a cascade at `endpoint`: the voice `voice` names in `pcm_24000`, as
/// [`hosted_tts`] makes it.
///
/// # Errors
///
/// As [`hosted_tts`].
pub fn synthesiser(
    endpoint: Endpoint,
    source: Arc<dyn CredentialSource>,
    voice: Option<&str>,
) -> Result<ElevenLabsSynthesiser, String> {
    hosted_tts(endpoint, source, voice, OutputFormat::Pcm24000)
}
