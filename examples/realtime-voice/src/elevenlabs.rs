//! The ElevenLabs bindings `talk` and `compare` make: every one at the hosted endpoint
//! [`endpoint`] reads from `ELEVENLABS_RESIDENCY`, so a key of a data-residency stack reaches
//! the host that takes it; the key both read ([`key`]); and, with feature `local`, the LLM of
//! their ElevenLabs cascades (`generator`).

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use inference_elevenlabs::{
    CommitStrategy, ConfigError, CredentialSource, ELEVENLABS_KEY_NAME, ELEVENLABS_KEY_VARIABLE,
    ElevenLabsRecogniser, ElevenLabsRecognition, ElevenLabsSynthesiser, Endpoint,
    EnvironmentSource, InputFormat, KeychainSource, OutputFormat, TranscriptionConfig, VadSettings,
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

/// The recogniser of `compare`'s ElevenLabs cascade arm at `endpoint`: `pcm_16000`, committed
/// manually, its key resolved through `source` under `ELEVENLABS_KEY_NAME`. The arm's own
/// detector ends each turn by ending the recognition's input, which sends the commit, so the
/// recognition ends at the commit's answer and not after the endpoint's voice activity detection
/// has stayed quiet for `inference_elevenlabs::VAD_END_QUIET`. Making it sends nothing.
///
/// # Errors
///
/// The [`ConfigError`] of the endpoint's URL.
pub fn manual_recogniser(
    endpoint: Endpoint,
    source: Arc<dyn CredentialSource>,
) -> Result<ElevenLabsRecogniser, ConfigError> {
    ElevenLabsRecogniser::new(
        endpoint,
        TranscriptionConfig::new(InputFormat::Pcm16000)
            .with_commit_strategy(CommitStrategy::Manual),
        source,
        ELEVENLABS_KEY_NAME,
    )
}

/// Why `recognition` failed, once it has: the failure a cascade runner reports.
#[must_use]
pub fn failure(recognition: &ElevenLabsRecognition) -> Option<String> {
    recognition.failure().map(ToString::to_string)
}

/// The ElevenLabs key: `ELEVENLABS_API_KEY` when it is set, the OS keychain's
/// `elevenlabs-api-key` otherwise, resolved once here so a missing key stops before anything
/// opens. Before it asks the keychain it says so on one line of standard error, which leaves
/// standard output to what the caller prints, as the keychain may wait for an unlock prompt, and
/// it gives up after [`KEY_WAIT`]. The key is never printed.
///
/// # Errors
///
/// [`Refused::Invalid`] when the keychain does not open or neither source gives a key.
pub fn key() -> Result<Arc<dyn CredentialSource>, Refused> {
    let (source, named): (Arc<dyn CredentialSource>, String) =
        if std::env::var_os(ELEVENLABS_KEY_VARIABLE).is_some() {
            (
                Arc::new(EnvironmentSource::elevenlabs()),
                ELEVENLABS_KEY_VARIABLE.to_owned(),
            )
        } else {
            let keychain = KeychainSource::native().map_err(|error| {
                Refused::Invalid(format!(
                    "{ELEVENLABS_KEY_VARIABLE} is not set and the OS keychain did not open: \
                     {error}"
                ))
            })?;
            eprintln!(
                "{ELEVENLABS_KEY_VARIABLE} is not set: waiting up to {} s for the OS \
                 keychain's {ELEVENLABS_KEY_NAME}; unlock the keychain if it asks",
                KEY_WAIT.as_secs()
            );
            (
                Arc::new(keychain),
                format!(
                    "{ELEVENLABS_KEY_VARIABLE} is not set and the OS keychain's \
                     {ELEVENLABS_KEY_NAME}"
                ),
            )
        };
    resolves(&*source).map_err(|why| Refused::Invalid(format!("{named} gave no key: {why}")))?;
    Ok(source)
}

/// The value of the variable `name`, trimmed.
///
/// # Errors
///
/// [`Refused::Missing`] when it is unset or blank.
#[cfg(feature = "local")]
fn read(name: &'static str) -> Result<String, Refused> {
    std::env::var(name)
        .map(|value| value.trim().to_owned())
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or(Refused::Missing(name))
}

/// The generator of the server the local voice cascade's variables name,
/// `INFERENCE_LLAMA_SERVER_URL` on loopback and `INFERENCE_LLAMA_SERVER_MODEL`, with the local
/// cascade's `MAX_OUTPUT_TOKENS`: a `llama-server`, or another server of the OpenAI-compatible
/// Chat Completions API such as Ollama's. Nothing is sent before the first turn.
///
/// # Errors
///
/// [`Refused::Missing`] naming a variable not set, or [`Refused::Invalid`] for a URL whose host
/// is not loopback or a server the `llama-server` adapter does not bind.
#[cfg(feature = "local")]
pub fn generator() -> Result<inference_llm::Generator, Refused> {
    use inference_llama_server::{Config, LlamaServer};
    use local_voice_cascade::local::{
        MAX_OUTPUT_TOKENS, SERVER_MODEL_VARIABLE, SERVER_URL_VARIABLE, loopback, server_adapter,
    };

    let url = read(SERVER_URL_VARIABLE)?;
    loopback(&url)
        .map_err(|refused| Refused::Invalid(format!("{SERVER_URL_VARIABLE}: {refused}")))?;
    let mut server = Config::new(url, read(SERVER_MODEL_VARIABLE)?);
    server.max_output_tokens = MAX_OUTPUT_TOKENS;
    let refused = |detail: String| {
        Refused::Invalid(format!(
            "{SERVER_URL_VARIABLE} and {SERVER_MODEL_VARIABLE} name no server the \
             llama-server adapter binds: {detail}"
        ))
    };
    let adapter = server_adapter(&server).map_err(|error| refused(error.to_string()))?;
    LlamaServer::new(&server)
        .map_err(|error| refused(error.to_string()))?
        .generator(adapter)
        .map_err(|error| refused(error.to_string()))
}

/// How long [`resolves`] waits for the key source to answer: long enough for an unlocked
/// keychain, not for an unlock prompt nobody answers.
pub const KEY_WAIT: Duration = Duration::from_secs(8);

/// Whether the ElevenLabs key resolves through `source` under `ELEVENLABS_KEY_NAME` within
/// [`KEY_WAIT`]. The source's future is driven on a Tokio runtime of its own, on a thread of its
/// own, since a source such as the keychain's reads on Tokio's blocking pool, and the caller may
/// be on a thread with no runtime or inside one. When the wait passes, the runtime is shut down
/// without waiting for a read still blocked, which is left to end on its own. The wait bounds a
/// source that awaits, as the keychain's does, not one that blocks the thread polling it. The
/// key is dropped unread.
///
/// # Errors
///
/// Why it did not: the source's refusal, which never holds the key, no answer within
/// [`KEY_WAIT`], no runtime to drive it on, or a panic of the source.
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
                let answered = runtime.block_on(async {
                    tokio::time::timeout(KEY_WAIT, source.resolve(ELEVENLABS_KEY_NAME)).await
                });
                runtime.shutdown_background();
                answered
                    .map_err(|_| {
                        format!(
                            "the key source gave no answer within {} s",
                            KEY_WAIT.as_secs()
                        )
                    })?
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
