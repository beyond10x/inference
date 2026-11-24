//! `Voice`: text in, audio frames out, played or returned (`docs/design/sdk.md` §3.2).

use inference_audio::AudioFrame;
use inference_facade::live::LiveSession;
use inference_pipeline::PlannedPipeline;

use crate::agent::{Handle, read_to_end};
use crate::{Agent, Description, Event, Payload, Recipe, Result, Stack};

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Voice>();
};

/// Text in, audio out, over the recipe `speak`: [`Voice::synthesise`] returns the frames of a
/// text and [`Voice::play`] plays them. Each call opens one session over the stack's `tts`
/// binding, sends the text as one committed segment and closes the session, so a voice says any
/// number of texts, one after another.
pub struct Voice {
    handle: Handle,
}

impl std::fmt::Debug for Voice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Voice")
            .field("recipe", &self.handle.describe().recipe)
            .finish_non_exhaustive()
    }
}

/// Gives `text` to the agent's session as one committed segment and closes the session, which
/// ends once the synthesiser has sent the last frame. The session is closed whether or not it
/// took the text.
async fn say(agent: &Agent, text: String) -> Result<()> {
    let sent = async {
        agent.send_text(text).await?;
        agent.commit().await
    }
    .await;
    let closed = agent.close().await;
    sent.and(closed)
}

impl Voice {
    /// A voice over the stack the process names: [`Voice::new_with`] over the stack resolved
    /// from the environment and the stack files.
    ///
    /// # Errors
    ///
    /// Those of [`Stack::from_env`] and of [`Voice::new_with`].
    pub fn new() -> Result<Self> {
        Self::new_with(&Stack::from_env()?)
    }

    /// A voice over `stack`, which it shares and does not take: the recipe `speak` is planned
    /// over the stack's `tts` binding, and no session is opened yet.
    ///
    /// # Errors
    ///
    /// Those of [`Stack::resolve_with`] over no environment and of [`Recipe::instantiate`]:
    /// [`Error::RoleUnbound`](crate::Error::RoleUnbound) when the stack binds no `tts`.
    pub fn new_with(stack: &Stack) -> Result<Self> {
        Ok(Self {
            handle: Handle::plan(Recipe::speak(), stack)?,
        })
    }

    /// The audio of `text`: every frame the synthesiser sent for it, in order. Opens one session
    /// and closes it.
    ///
    /// # Errors
    ///
    /// Those of [`AgentBuilder::start`](crate::AgentBuilder::start), [`Agent::commit`] and
    /// [`Agent::close`]; the error of the session's first [`Event::Error`], an
    /// [`Error::Provider`](crate::Error::Provider) when the synthesiser's provider failed.
    pub async fn synthesise(&mut self, text: impl Into<String>) -> Result<Vec<AudioFrame>> {
        let text = text.into();
        let agent = self.handle.open().await?;
        let mut frames = Vec::new();
        let read = read_to_end(agent.events(), |event| {
            if let Event::Audio(frame) = event {
                frames.push(frame);
            }
        });
        let (said, read) = tokio::join!(say(agent, text), read);
        said.and(read)?;
        Ok(frames)
    }

    /// Plays the audio of `text` on `host`'s default speaker and returns once the speaker played
    /// the last frame. Opens one session and closes it; no microphone is opened.
    ///
    /// # Errors
    ///
    /// [`Error::Device`](crate::Error::Device) when the speaker does not open, and those of
    /// [`Voice::synthesise`].
    #[cfg(feature = "audio-host")]
    pub async fn play(
        &mut self,
        text: impl Into<String>,
        host: impl inference_audio_device::AudioHost,
    ) -> Result<()> {
        let text = text.into();
        let agent = self.handle.open().await?;
        let read = read_to_end(agent.events(), drop);
        let played = async {
            let played = agent.play_during(&host, say(agent, text)).await;
            if played.is_err() {
                let _ = agent.close().await;
            }
            played
        };
        let (played, read) = tokio::join!(played, read);
        played.and(read)
    }

    /// What the voice runs: the recipe `speak`, the stack's bindings and their sources, and the
    /// planned pipeline's description.
    #[must_use]
    pub fn describe(&self) -> Description {
        self.handle.describe()
    }

    /// The live session of the last call, one layer down; none before the first call. After the
    /// call it is closed, and still answers for its ids and its usage.
    #[must_use]
    pub fn session(&self) -> Option<&LiveSession<Payload>> {
        self.handle.session()
    }

    /// The planned pipeline, one layer down: that of the last call's session, or, before the
    /// first call, the one planned when the voice was built.
    #[must_use]
    pub fn pipeline(&self) -> &PlannedPipeline {
        self.handle.pipeline()
    }
}
