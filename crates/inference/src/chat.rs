//! `Chat`: text in, text or a delta stream out, history kept, tools allowed
//! (`docs/design/sdk.md` §3.2).

use std::time::Duration;

use futures_core::Stream;
use futures_util::StreamExt as _;
use futures_util::stream;
use inference_facade::live::LiveSession;
use inference_pipeline::PlannedPipeline;
use inference_tools::IntoTool;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::agent::{Handle, failure};
use crate::recipe::ReplyPart;
use crate::{Description, Error, Event, Events, Payload, Recipe, Result, Role, Stack};

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Chat>();
};

/// How often a chat waiting for a reply denies the calls its tool authority holds for
/// confirmation.
const TICK: Duration = Duration::from_millis(20);

/// Text in, text out, over the recipe `chat`: [`Chat::send`] returns the reply to a text and
/// [`Chat::stream`] yields it delta by delta. A chat keeps its history: the first text opens one
/// session over the stack's `llm` binding and every later text is a turn of the same
/// conversation, so the model is asked each text after the turns before it, until
/// [`Chat::close`] closes the session. A chat dropped without it cancels its session.
///
/// [`Chat::tool`] gives the model a tool. A reply is whole once the model has answered and
/// awaits no call, so it holds the text of every turn of the tool loop, in order. A tool that
/// requires confirmation is never confirmed, since a chat has no one to ask: its call is
/// answered as rejected. Build an [`Agent`](crate::Agent) to confirm calls.
pub struct Chat {
    handle: Handle,
    live: Option<Live>,
}

/// The open session of a chat, as the chat reads it.
struct Live {
    /// What the session's model node answers.
    replies: UnboundedReceiver<ReplyPart>,
    /// The session's events, read for its failure and its end.
    events: Events,
    /// Whether a text was sent whose reply's end has not been read.
    unread: bool,
}

/// What a chat's session yielded next.
enum Next {
    Part(ReplyPart),
    Failed(Error),
    Closed,
    Tick,
}

/// Where a reply's stream is.
enum Stage {
    Send(String),
    Read,
    Done,
}

impl std::fmt::Debug for Chat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chat")
            .field("recipe", &self.handle.describe().recipe)
            .field("open", &self.live.is_some())
            .finish_non_exhaustive()
    }
}

/// The error of a text sent to a chat whose session is gone.
fn not_open() -> Error {
    Error::Closed {
        message: "the chat's session is not open".to_owned(),
    }
}

impl Chat {
    /// A chat over the stack the process names: [`Chat::new_with`] over the stack resolved from
    /// the environment and the stack files.
    ///
    /// # Errors
    ///
    /// Those of [`Stack::from_env`] and of [`Chat::new_with`].
    pub fn new() -> Result<Self> {
        Self::new_with(&Stack::from_env()?)
    }

    /// A chat over `stack`, which it shares and does not take: the recipe `chat` is planned over
    /// the stack's `llm` binding, and no session is opened yet.
    ///
    /// # Errors
    ///
    /// Those of [`Stack::resolve_with`] over no environment and of [`Recipe::instantiate`]:
    /// the error of an unbound role when the stack binds no `llm`.
    pub fn new_with(stack: &Stack) -> Result<Self> {
        Ok(Self {
            handle: Handle::plan(Recipe::chat(), stack)?,
            live: None,
        })
    }

    /// The same chat with `tool`, which its model may call: a `#[inference::tool]` function or a
    /// `(Tool, executor)` pair, as [`AgentBuilder::tool`](crate::AgentBuilder::tool) takes. The
    /// recipe is planned again with the tool loop [`Recipe::tools`] describes, which requires
    /// `tool_calling` of the `llm` binding. Give every tool before the first text: a session
    /// already open keeps the tools it was opened with.
    ///
    /// # Errors
    ///
    /// Those of [`Recipe::instantiate`], such as an `llm` binding that does not claim
    /// `tool_calling`; [`Error::Layer`] when the chat's session is already open.
    pub fn tool(mut self, tool: impl IntoTool) -> Result<Self> {
        if self.live.is_some() {
            return Err(Error::Layer {
                message: "the chat's session is open and keeps the tools it was opened with: \
                          give every tool before the first text"
                    .to_owned(),
                source: None,
            });
        }
        let (tool, executor) = tool.into_tool();
        self.handle.give(tool, executor)?;
        Ok(self)
    }

    /// The model's reply to `text`, whole: the deltas of [`Chat::stream`], joined.
    ///
    /// # Errors
    ///
    /// Those of [`Chat::stream`]'s items.
    pub async fn send(&mut self, text: impl Into<String>) -> Result<String> {
        let mut reply = String::new();
        let mut deltas = std::pin::pin!(self.stream(text));
        while let Some(delta) = deltas.next().await {
            reply.push_str(&delta?);
        }
        Ok(reply)
    }

    /// The model's reply to `text`, as the text deltas the model streams, in order; their
    /// concatenation is the reply. The text is sent when the stream is first polled, which opens
    /// the chat's session if none is open. The stream ends once the reply is whole: the model
    /// has answered and awaits no tool call. A stream dropped before its end leaves the rest of
    /// its reply unread, and the next call reads past it before it sends its text.
    ///
    /// # Errors
    ///
    /// The stream yields one error and then ends: those of
    /// [`AgentBuilder::start`](crate::AgentBuilder::start) and
    /// [`Agent::commit`](crate::Agent::commit); [`Error::Provider`] when the model refused to
    /// answer, its message the refusal, after which the conversation goes on; the error of the
    /// session's first [`Event::Error`], such as a turn the model failed, and [`Error::Closed`]
    /// when the session ended before the reply was whole, as when the model's tool calls reached
    /// the bound of the tool loop. After either of those two the session is closed, and the next
    /// text starts a new conversation.
    pub fn stream(
        &mut self,
        text: impl Into<String>,
    ) -> impl Stream<Item = Result<String>> + Send + '_ {
        stream::unfold(
            (self, Stage::Send(text.into())),
            |(chat, stage)| async move {
                let sent = match stage {
                    Stage::Send(text) => chat.say(text).await,
                    Stage::Read => Ok(()),
                    Stage::Done => return None,
                };
                let delta = match sent {
                    Ok(()) => chat.delta().await,
                    Err(error) => Err(error),
                };
                match delta {
                    Ok(Some(delta)) => Some((Ok(delta), (chat, Stage::Read))),
                    Ok(None) => None,
                    Err(error) => Some((Err(error), (chat, Stage::Done))),
                }
            },
        )
    }

    /// Whether a text was sent on the open session whose reply's end has not been read.
    fn unread(&self) -> bool {
        self.live.as_ref().is_some_and(|live| live.unread)
    }

    /// Sends `text` as the next turn of the conversation, on a session opened first if none is
    /// open, once the reply before it was read to its end, whatever that reply came to.
    async fn say(&mut self, text: String) -> Result<()> {
        while self.unread() {
            let _ = self.delta().await;
        }
        if self.live.is_none() {
            let agent = self.handle.open().await?;
            let replies = agent.replies().ok_or_else(|| Error::Layer {
                message: "the chat's run built no model node to read replies from".to_owned(),
                source: None,
            })?;
            self.live = Some(Live {
                replies,
                events: agent.events(),
                unread: false,
            });
        }
        let (Some(agent), Some(live)) = (self.handle.agent(), self.live.as_mut()) else {
            return Err(not_open());
        };
        let sent = async {
            agent.send_text(text).await?;
            agent.commit().await
        };
        match sent.await {
            Ok(()) => {
                live.unread = true;
                Ok(())
            }
            Err(error) => {
                self.end().await;
                Err(error)
            }
        }
    }

    /// The next delta of the reply being read; none once it is whole, or when no reply is
    /// unread.
    async fn delta(&mut self) -> Result<Option<String>> {
        loop {
            let Some(live) = self.live.as_mut().filter(|live| live.unread) else {
                return Ok(None);
            };
            let next = tokio::select! {
                biased;
                part = live.replies.recv() => part.map_or(Next::Closed, Next::Part),
                event = live.events.next() => match event {
                    Some(Ok(Event::Error { error, .. })) => Next::Failed(failure(&error)),
                    Some(Ok(Event::Closed { .. })) | None => Next::Closed,
                    Some(Err(error)) => Next::Failed(error),
                    Some(Ok(_)) => continue,
                },
                () = tokio::time::sleep(TICK) => Next::Tick,
            };
            match next {
                Next::Part(ReplyPart::Delta(delta)) => return Ok(Some(delta)),
                Next::Part(ReplyPart::Refused(refusal)) => {
                    return Err(Error::Provider {
                        adapter: self.adapter(),
                        message: format!("the model refused to answer: {refusal}"),
                        source: None,
                    });
                }
                Next::Part(ReplyPart::End { ended: false }) => {
                    live.unread = false;
                    return Ok(None);
                }
                Next::Part(ReplyPart::End { ended: true }) | Next::Closed => {
                    let failed = self.end().await;
                    return Err(failed.unwrap_or_else(|| Error::Closed {
                        message: "the chat's session ended before the reply was whole, as when \
                                  the model's tool calls reach the bound of the tool loop; the \
                                  next text starts a new conversation"
                            .to_owned(),
                    }));
                }
                Next::Failed(error) => {
                    self.end().await;
                    return Err(error);
                }
                Next::Tick => {
                    if let Some(agent) = self.handle.agent() {
                        agent.deny_held();
                    }
                }
            }
        }
    }

    /// The name of the adapter bound to `llm`, or `llm` when the binding has none.
    fn adapter(&self) -> String {
        self.handle
            .describe()
            .bindings
            .into_iter()
            .find(|(role, _, _)| *role == Role::Llm)
            .and_then(|(_, name, _)| name)
            .unwrap_or_else(|| Role::Llm.as_str().to_owned())
    }

    /// Closes the chat's session, if one is open, and forgets it, so the next text opens another;
    /// the first error the session reported and the chat has not returned, if any.
    async fn end(&mut self) -> Option<Error> {
        let mut live = self.live.take()?;
        let closed = match self.handle.agent() {
            Some(agent) => agent.close().await.err(),
            None => None,
        };
        let mut failed = None;
        while let Some(event) = live.events.next().await {
            if let Ok(Event::Error { error, .. }) = event {
                failed.get_or_insert_with(|| failure(&error));
            }
        }
        failed.or(closed)
    }

    /// Closes the chat's session by draining it, which ends the conversation; nothing when none
    /// is open. A later text opens a new session and starts a new conversation.
    ///
    /// # Errors
    ///
    /// The error of the session's first [`Event::Error`] the chat has not returned, or that of
    /// [`Agent::close`](crate::Agent::close).
    pub async fn close(&mut self) -> Result<()> {
        self.end().await.map_or(Ok(()), Err)
    }

    /// What the chat runs: the recipe `chat`, the stack's bindings and their sources, and the
    /// planned pipeline's description, with the tool loop once a tool was given.
    #[must_use]
    pub fn describe(&self) -> Description {
        self.handle.describe()
    }

    /// The live session of the conversation, one layer down; none before the first text. After
    /// [`Chat::close`] it is closed, and still answers for its ids and its usage.
    #[must_use]
    pub fn session(&self) -> Option<&LiveSession<Payload>> {
        self.handle.session()
    }

    /// The planned pipeline, one layer down: that of the last session, or, before the first
    /// text, the one planned when the chat was built or last given a tool.
    #[must_use]
    pub fn pipeline(&self) -> &PlannedPipeline {
        self.handle.pipeline()
    }
}
