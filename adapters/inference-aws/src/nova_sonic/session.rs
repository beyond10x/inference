//! One session's sequence of input events and its reader of output events, over two channels.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use super::Role;
use super::config::{AudioRate, SessionConfig};
use super::error::{NovaSonicError, ServiceError};
use super::input::{InputContent, InputEvent};
use super::output::{Decoder, OutputEvent};

/// One frame of the output side: the bytes of a `chunk`, or an exception the service reported.
pub type OutputFrame = Result<Vec<u8>, ServiceError>;

/// An open session: its writer and its reader, usable from two tasks.
#[derive(Debug)]
pub struct Session {
    /// Sends audio, tool results and the closing events.
    pub writer: SessionWriter,
    /// Reads the output events.
    pub reader: SessionReader,
}

impl Session {
    /// Opens a session over a transport: sends the session start, the prompt start and the system
    /// prompt into `input`, and reads output frames from `output`.
    ///
    /// The prompt name and every content name are fresh UUIDs, as in the reference's examples.
    pub async fn open(
        config: &SessionConfig,
        input: mpsc::Sender<Vec<u8>>,
        output: mpsc::Receiver<OutputFrame>,
    ) -> Result<Session, NovaSonicError> {
        let activity = Arc::new(Activity::new());
        let writer = SessionWriter {
            input,
            prompt_name: fresh_name(),
            input_rate: config.input_rate,
            audio_content: None,
            activity: Arc::clone(&activity),
        };
        writer
            .send(InputEvent::SessionStart {
                inference: config.inference,
                endpointing: config.endpointing,
            })
            .await?;
        writer
            .send(InputEvent::PromptStart {
                prompt_name: writer.prompt_name.clone(),
                voice_id: config.voice_id.clone(),
                output_rate: config.output_rate,
                tools: config.tools.clone(),
            })
            .await?;
        writer
            .send_content(
                InputContent::Text {
                    role: Role::System,
                    interactive: false,
                },
                |prompt_name, content_name| InputEvent::TextInput {
                    prompt_name,
                    content_name,
                    content: config.system_prompt.clone(),
                },
            )
            .await?;
        Ok(Session {
            writer,
            reader: SessionReader {
                output,
                decoder: Decoder::default(),
                read_timeout: config.read_timeout,
                activity,
            },
        })
    }
}

fn fresh_name() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The input side of a session.
#[derive(Debug)]
pub struct SessionWriter {
    input: mpsc::Sender<Vec<u8>>,
    prompt_name: String,
    input_rate: AudioRate,
    audio_content: Option<String>,
    activity: Arc<Activity>,
}

impl SessionWriter {
    /// The session's prompt name.
    pub fn prompt_name(&self) -> &str {
        &self.prompt_name
    }

    /// Sends PCM16 mono audio at the configured input rate, opening the audio content first if
    /// none is open.
    pub async fn send_audio(&mut self, pcm: &[u8]) -> Result<(), NovaSonicError> {
        let content_name = match &self.audio_content {
            Some(name) => name.clone(),
            None => {
                let name = fresh_name();
                self.send(InputEvent::ContentStart {
                    prompt_name: self.prompt_name.clone(),
                    content_name: name.clone(),
                    content: InputContent::Audio {
                        rate: self.input_rate,
                    },
                })
                .await?;
                self.audio_content = Some(name.clone());
                name
            }
        };
        self.send(InputEvent::AudioInput {
            prompt_name: self.prompt_name.clone(),
            content_name,
            pcm: pcm.to_vec(),
        })
        .await
    }

    /// Closes the open audio content, if any.
    pub async fn end_audio(&mut self) -> Result<(), NovaSonicError> {
        if let Some(content_name) = self.audio_content.take() {
            self.send(InputEvent::ContentEnd {
                prompt_name: self.prompt_name.clone(),
                content_name,
            })
            .await?;
        }
        Ok(())
    }

    /// Sends the result of the tool use `tool_use_id` as its own content block.
    pub async fn send_tool_result(
        &mut self,
        tool_use_id: &str,
        content: &str,
    ) -> Result<(), NovaSonicError> {
        self.send_content(
            InputContent::ToolResult {
                tool_use_id: tool_use_id.to_owned(),
            },
            |prompt_name, content_name| InputEvent::ToolResult {
                prompt_name,
                content_name,
                content: content.to_owned(),
            },
        )
        .await
    }

    /// Ends the session as the reference orders it: the open audio content, the prompt, the
    /// session; then closes the input side.
    pub async fn close(mut self) -> Result<(), NovaSonicError> {
        self.end_audio().await?;
        self.send(InputEvent::PromptEnd {
            prompt_name: self.prompt_name.clone(),
        })
        .await?;
        self.send(InputEvent::SessionEnd).await
    }

    async fn send(&self, event: InputEvent) -> Result<(), NovaSonicError> {
        self.input
            .send(event.encode())
            .await
            .map_err(|_| NovaSonicError::Closed)?;
        self.activity.touch();
        Ok(())
    }

    /// One whole content block: its start, the event `body` builds from the prompt and content
    /// names, and its end.
    async fn send_content(
        &self,
        content: InputContent,
        body: impl FnOnce(String, String) -> InputEvent,
    ) -> Result<(), NovaSonicError> {
        let content_name = fresh_name();
        self.send(InputEvent::ContentStart {
            prompt_name: self.prompt_name.clone(),
            content_name: content_name.clone(),
            content,
        })
        .await?;
        self.send(body(self.prompt_name.clone(), content_name.clone()))
            .await?;
        self.send(InputEvent::ContentEnd {
            prompt_name: self.prompt_name.clone(),
            content_name,
        })
        .await
    }
}

/// The output side of a session.
#[derive(Debug)]
pub struct SessionReader {
    output: mpsc::Receiver<OutputFrame>,
    decoder: Decoder,
    read_timeout: Duration,
    activity: Arc<Activity>,
}

/// When a frame last crossed the session, either way: the start of the stall clock.
#[derive(Debug)]
struct Activity {
    origin: Instant,
    since_origin_nanos: AtomicU64,
}

impl Activity {
    fn new() -> Self {
        Activity {
            origin: Instant::now(),
            since_origin_nanos: AtomicU64::new(0),
        }
    }

    fn touch(&self) {
        let nanos = u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.since_origin_nanos.fetch_max(nanos, Ordering::Relaxed);
    }

    fn last(&self) -> Instant {
        self.origin + Duration::from_nanos(self.since_origin_nanos.load(Ordering::Relaxed))
    }
}

impl SessionReader {
    /// The next output event; `None` once the stream ended.
    ///
    /// [`NovaSonicError::Stall`] when the session was quiet for the read timeout: the clock runs
    /// from the later of the last frame the writer sent and the last frame the reader received.
    /// While the user speaks the writer sends audio, so a turn longer than the read timeout is not
    /// a stall; the clock counts once both sides are quiet. A stall leaves the reader usable.
    pub async fn recv(&mut self) -> Result<Option<OutputEvent>, NovaSonicError> {
        let frame = loop {
            let deadline = self.activity.last() + self.read_timeout;
            match tokio::time::timeout_at(deadline.into(), self.output.recv()).await {
                Ok(frame) => break frame,
                Err(_) if self.activity.last() + self.read_timeout <= Instant::now() => {
                    return Err(NovaSonicError::Stall {
                        after: self.read_timeout,
                    });
                }
                Err(_) => {}
            }
        };
        self.activity.touch();
        match frame {
            None => Ok(None),
            Some(Err(error)) => Ok(Some(OutputEvent::Error(error))),
            Some(Ok(bytes)) => self.decoder.decode(&bytes).map(Some),
        }
    }
}
