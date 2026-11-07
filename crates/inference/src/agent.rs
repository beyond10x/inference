//! `Agent`: a session over a recipe that sends text or audio, yields `Event`s, interrupts, switches
//! and closes (`docs/design/sdk.md` §3.2, §8).
//!
//! An agent holds its session with no lock, in one `Arc<Inner>` it shares with its pump: the task
//! that is the session's one receiver, maps each session event to an [`Event`] and puts it on a
//! broadcast channel of capacity 1024, which [`Agent::events`] reads. The task the agent spawns
//! for the session's driver holds no `Arc<Inner>`; it reports how the run's driving ended through
//! a watch of its own.

use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use inference_audio::AudioFrame;
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_duplex::SessionConfig;
use inference_facade::events::SessionEvent;
use inference_facade::live::{InputError, LiveSession, LiveSessions, SessionDriver};
use inference_pipeline::PlannedPipeline;
use inference_pipeline::nodes::AuthorityHandle;
use inference_pipeline::nodes::duplex::CallIds;
use inference_pipeline::nodes::log::StreamEvent;
use inference_runtime::RunError;
use inference_text::SpanId;
use inference_tools::{
    CancellationSignal, ExecutionFuture, IntoTool, Tool, ToolCall, ToolCallId, ToolExecutor, ToolId,
};
use tokio::sync::{Notify, broadcast, watch};
use tokio::task::JoinHandle;

use crate::event::{Event, Events, Item};
use crate::stack::{Bound, default_session_config, role};
use crate::{BindingSource, Env, Error, Instantiation, Payload, Recipe, Result, Role, Stack};

/// The capacity of an agent's event channel: a reader more than this many events behind is told
/// [`Event::Lagged`].
const CAPACITY: usize = 1024;

/// How often the pump looks for a held tool call while no event arrives.
const TICK: Duration = Duration::from_millis(20);

const OPEN: u8 = 0;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "`switch` moves the state to it; it is not implemented yet"
    )
)]
const SWITCHING: u8 = 1;
const CLOSING: u8 = 2;
const CLOSED: u8 = 3;

const _: fn() = || {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<Agent>();
    assert_sync::<Agent>();
};

/// The process-wide live sessions every SDK session opens on.
#[must_use]
pub fn sessions() -> &'static LiveSessions {
    static SESSIONS: OnceLock<LiveSessions> = OnceLock::new();
    SESSIONS.get_or_init(LiveSessions::new)
}

/// The clock whose instant [`Event::Started`] carries.
pub trait Clock: Send + Sync + 'static {
    /// The instant now.
    fn now(&self) -> Instant;
}

/// The clock of the process: [`Instant::now`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// What an agent or its builder runs: the recipe chosen, each role's binding and its source, the
/// planned pipeline's description and the speech session's config (`None` for a cascade).
#[derive(Debug, Clone)]
pub struct Description {
    /// The name of the recipe chosen.
    pub recipe: String,
    /// Each bound role: its adapter's registered name, when bound by name, and where it came
    /// from.
    pub bindings: Vec<(Role, Option<String>, BindingSource)>,
    /// The `inference_facade::describe` output of the planned pipeline.
    pub pipeline: Option<inference_facade::describe::Description>,
    /// The speech session's config: the one the agent opens or opened with, or the one the last
    /// switch merged; `None` for a recipe without a `speech` role bound.
    pub session_config: Option<SessionConfig>,
}

/// A held lock, poison or not.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A device run's playback, attached to an agent's sink slot while it runs.
pub(crate) trait Playback: Send + Sync {
    /// Plays `frame` after every frame before it.
    fn play(&self, frame: AudioFrame);
    /// Whether reply audio is not yet played.
    fn pending(&self) -> bool;
    /// Stops the playback at once, dropping what is queued and in flight.
    fn cut(&self);
    /// How many frames were handed out.
    fn played(&self) -> u64;
}

/// The one route an agent's audio takes: to the attached sink, or onto the event channel.
pub(crate) struct Route {
    pub(crate) sink: Mutex<Option<Arc<dyn Playback>>>,
    sender: Mutex<Option<broadcast::Sender<Item>>>,
    routed: AtomicU64,
}

impl Route {
    fn new(sender: broadcast::Sender<Item>) -> Self {
        Self {
            sink: Mutex::new(None),
            sender: Mutex::new(Some(sender)),
            routed: AtomicU64::new(0),
        }
    }

    fn sink(&self) -> Option<Arc<dyn Playback>> {
        lock(&self.sink).clone()
    }

    /// Sends `event` to every reader; nothing once the sender is gone or no reader is left.
    fn send(&self, event: Event) {
        if let Some(sender) = lock(&self.sender).as_ref() {
            let _ = sender.send(Ok(event));
        }
    }

    fn subscribe(&self) -> Option<broadcast::Receiver<Item>> {
        lock(&self.sender)
            .as_ref()
            .map(broadcast::Sender::subscribe)
    }

    /// Drops the sender, so every reader ends after what it holds.
    fn hang_up(&self) {
        lock(&self.sender).take();
    }

    fn audio(&self, frame: AudioFrame) {
        if let Some(sink) = self.sink() {
            sink.play(frame);
        } else {
            self.routed.fetch_add(1, Ordering::AcqRel);
            self.send(Event::Audio(frame));
        }
    }

    fn pending(&self) -> bool {
        self.sink().is_some_and(|sink| sink.pending())
    }

    /// Cuts local playback when a sink is attached; frames already on the channel are the
    /// application's.
    fn cut_playback(&self) {
        if let Some(sink) = self.sink() {
            sink.cut();
        }
    }
}

#[cfg(feature = "audio-host")]
impl crate::recipe::AudioRoute for Route {
    fn route(&self, frame: AudioFrame) {
        self.audio(frame);
    }

    fn played(&self) -> u64 {
        self.sink()
            .map_or_else(|| self.routed.load(Ordering::Acquire), |sink| sink.played())
    }

    fn cut(&self) {
        self.cut_playback();
    }

    fn interrupted(&self) {
        self.send(Event::Interrupted { raw: None });
    }
}

/// The reply in progress and what was cut of it.
#[derive(Debug, Default)]
struct Tracking {
    /// The generation of the last `GenerationStarted`, to which an `AudioChunk` is attributed.
    last_started: Option<GenerationId>,
    /// The reply in progress.
    in_progress: Option<GenerationId>,
    /// The generations cut.
    cut: HashSet<GenerationId>,
    /// The interrupts the pump spawned whose `SessionInterrupted` it has not seen.
    spawned: usize,
    /// The application's interrupts whose `SessionInterrupted` the pump has not seen.
    requested: usize,
}

/// The calls already announced, and those whose `ToolCalled` was yielded.
#[derive(Debug, Default)]
struct Confirmations {
    announced: HashSet<ToolCallId>,
    called: HashSet<ToolCallId>,
}

/// What the pump reads and moves: everything of [`Inner`] but the session.
struct Core {
    state: AtomicU8,
    auto_interrupt: bool,
    started: AtomicBool,
    clock: Arc<dyn Clock>,
    reply: Mutex<Tracking>,
    route: Arc<Route>,
    authority: Option<AuthorityHandle>,
    call_ids: Option<CallIds>,
    names: Vec<(ToolId, String)>,
    confirmations: Mutex<Confirmations>,
}

impl Core {
    fn new(auto_interrupt: bool, route: Arc<Route>, clock: Arc<dyn Clock>) -> Self {
        Self {
            state: AtomicU8::new(OPEN),
            auto_interrupt,
            started: AtomicBool::new(false),
            clock,
            reply: Mutex::default(),
            route,
            authority: None,
            call_ids: None,
            names: Vec::new(),
            confirmations: Mutex::default(),
        }
    }

    /// Moves `Open` to `to`; whether this call moved it.
    fn leave_open(&self, to: u8) -> bool {
        self.state
            .compare_exchange(OPEN, to, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn state(&self) -> u8 {
        self.state.load(Ordering::Acquire)
    }

    fn name_of(&self, tool_id: ToolId) -> String {
        self.names
            .iter()
            .find(|(id, _)| *id == tool_id)
            .map_or_else(|| tool_id.to_string(), |(_, name)| name.clone())
    }

    /// The held or decided call `call_id` of the authority.
    fn call(&self, call_id: ToolCallId) -> Option<ToolCall> {
        let authority = self.authority.as_ref()?;
        authority
            .held()
            .into_iter()
            .chain(authority.calls())
            .find(|call| call.call_id() == call_id)
    }

    /// The pump's look at the authority's held calls: with the agent `Open`, one
    /// `ConfirmationRequested` for each held call whose `ToolCalled` was yielded and that was not
    /// announced; closing or closed, each held call is denied unannounced.
    fn check_held(&self) {
        let Some(authority) = self.authority.as_ref() else {
            return;
        };
        let open = self.state() == OPEN;
        for call in authority.held() {
            let call_id = call.call_id();
            if !open {
                let _ = authority.deny(call_id);
                continue;
            }
            let announce = {
                let mut confirmations = lock(&self.confirmations);
                confirmations.called.contains(&call_id) && confirmations.announced.insert(call_id)
            };
            if announce {
                self.route.send(Event::ConfirmationRequested {
                    call_id,
                    name: self.name_of(call.tool_id()),
                    arguments: call.arguments().clone(),
                });
            }
        }
    }
}

/// How the pump closes the session on an event: draining it (the model closed), or cancelling it
/// (the run or the model session failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Close {
    Drain,
    Cancel,
}

/// What one step of the pump asks of its task.
#[derive(Debug, Default, PartialEq, Eq)]
struct Act {
    /// Spawn the session's `interrupt`.
    interrupt: bool,
    /// Spawn a close.
    close: Option<Close>,
    /// The session's events ended.
    end: bool,
}

/// The pump's own state: the run whose events still come before the current run's, whether the
/// run's failure was reported, and the `SessionClosed` received.
#[derive(Debug, Default)]
struct Step {
    left: Option<RunId>,
    reported: bool,
    closed: Option<SessionEvent>,
}

impl Step {
    fn new(left: Option<RunId>) -> Self {
        Self {
            left,
            ..Self::default()
        }
    }

    /// Yields one `Error::Layer` for the run, unless one was yielded.
    fn layer(&mut self, core: &Core, message: String, raw: Option<SessionEvent>) {
        if self.reported {
            return;
        }
        self.reported = true;
        core.route.send(Event::Error {
            error: Arc::new(Error::Layer {
                message,
                source: None,
            }),
            raw,
        });
    }

    /// Maps one session event.
    fn on_event(&mut self, core: &Core, event: SessionEvent) -> Act {
        let mut act = Act::default();
        let old = self.left.is_some();
        let route = &core.route;
        match &event {
            SessionEvent::SessionReady(_) => {
                if !core.started.swap(true, Ordering::AcqRel) {
                    route.send(Event::Started {
                        at: core.clock.now(),
                        raw: event,
                    });
                }
            }
            SessionEvent::SessionInterrupted(_) => {
                let answered = {
                    let mut reply = lock(&core.reply);
                    if reply.spawned > 0 {
                        reply.spawned -= 1;
                        true
                    } else if reply.requested > 0 {
                        reply.requested -= 1;
                        route.cut_playback();
                        true
                    } else {
                        false
                    }
                };
                if answered {
                    route.send(Event::Interrupted { raw: Some(event) });
                }
            }
            SessionEvent::SessionClosed(_) => {
                core.leave_open(CLOSING);
                self.closed = Some(event);
                act.end = true;
            }
            SessionEvent::RunCompleted(completed) => {
                if self.left == Some(completed.run_id) {
                    self.left = None;
                }
            }
            SessionEvent::RunFailed(failed) if !old => {
                let message = format!("the session's run failed: {:?}", failed.failure);
                self.layer(core, message, Some(event));
                if core.leave_open(CLOSING) {
                    act.close = Some(Close::Cancel);
                }
            }
            SessionEvent::SpeechSessionFailed(failed) if !old => {
                let message = format!("the speech model's session failed: {:?}", failed.failure);
                self.layer(core, message, Some(event));
                if core.leave_open(CLOSING) {
                    act.close = Some(Close::Cancel);
                }
            }
            SessionEvent::SpeechSessionClosed(_) if !old => {
                if core.leave_open(CLOSING) {
                    act.close = Some(Close::Drain);
                }
            }
            SessionEvent::SpeechStarted(_) if !old => {
                act.interrupt = barge_in(core);
            }
            SessionEvent::TranscriptHypothesis(heard) => route.send(Event::Heard {
                text: heard.text.clone(),
                final_: false,
                raw: event,
            }),
            SessionEvent::TranscriptRevision(revised) => route.send(Event::Transcript {
                text: revised.text.clone(),
                raw: event,
            }),
            SessionEvent::TranscriptCommitted(heard) => route.send(Event::Heard {
                text: heard.text.clone(),
                final_: true,
                raw: event,
            }),
            SessionEvent::TextSegmentCommitted(said) => route.send(Event::Said {
                text: said.text.clone(),
                raw: event,
            }),
            SessionEvent::AudioChunk(frame) => {
                let cut = {
                    let reply = lock(&core.reply);
                    reply
                        .last_started
                        .is_some_and(|generation| reply.cut.contains(&generation))
                };
                if !cut {
                    route.audio(frame.clone());
                }
            }
            SessionEvent::ToolCallProposed(proposed) => {
                lock(&core.confirmations).called.insert(proposed.call_id);
                let arguments = core
                    .call(proposed.call_id)
                    .map_or(serde_json::Value::Null, |call| call.arguments().clone());
                route.send(Event::ToolCalled {
                    name: core.name_of(proposed.tool_id),
                    arguments,
                    call_id: proposed.call_id,
                    speech_call_id: core
                        .call_ids
                        .as_ref()
                        .and_then(|ids| ids.speech_call_id(proposed.call_id)),
                    raw: event.clone(),
                });
            }
            SessionEvent::UsageRecorded(record) => route.send(Event::Usage(record.clone())),
            SessionEvent::SpeechResponseFailed(failed) => route.send(Event::Error {
                error: Arc::new(Error::Provider {
                    adapter: "speech".to_owned(),
                    message: format!("the response failed: {:?}", failed.failure),
                    source: None,
                }),
                raw: Some(event.clone()),
            }),
            SessionEvent::GenerationStarted(started) => {
                let mut reply = lock(&core.reply);
                reply.last_started = Some(started.generation_id);
                reply.in_progress = Some(started.generation_id);
            }
            SessionEvent::GenerationCompleted(ended) => end_reply(core, ended.generation_id),
            SessionEvent::GenerationCancelled(ended) => end_reply(core, ended.generation_id),
            _ => {}
        }
        act
    }

    /// The driver's task ended, `Ok` or `Err`: one `Error::Layer` for an `Err`, then the close.
    fn on_driver(&mut self, core: &Core, driven: &std::result::Result<(), Arc<RunError>>) -> Act {
        if let Err(error) = driven {
            self.layer(
                core,
                format!("the session's run loop failed: {error}"),
                None,
            );
        }
        Act {
            close: core.leave_open(CLOSING).then_some(Close::Cancel),
            ..Act::default()
        }
    }

    /// The Agent's end: an `Error::Layer` for a driver `Err` not yet reported, `Closed`, the
    /// sender dropped and the state `Closed`.
    fn on_end(&mut self, core: &Core, driven: Option<&std::result::Result<(), Arc<RunError>>>) {
        core.leave_open(CLOSING);
        if let Some(Err(error)) = driven {
            self.layer(
                core,
                format!("the session's run loop failed: {error}"),
                None,
            );
        }
        core.state.store(CLOSED, Ordering::Release);
        core.route.send(Event::Closed {
            raw: self.closed.take(),
        });
        core.route.hang_up();
    }
}

/// The barge-in: a `SpeechStarted` while a reply is in progress or audio is pending at the sink.
/// With `auto_interrupt` on it cuts the reply and local playback; whether to spawn the interrupt.
fn barge_in(core: &Core) -> bool {
    let pending = core.route.pending();
    let mut reply = lock(&core.reply);
    if (reply.in_progress.is_none() && !pending) || !core.auto_interrupt {
        return false;
    }
    if let Some(generation) = reply.in_progress.take() {
        reply.cut.insert(generation);
    }
    reply.spawned += 1;
    drop(reply);
    core.route.cut_playback();
    true
}

fn end_reply(core: &Core, generation: GenerationId) {
    let mut reply = lock(&core.reply);
    if reply.in_progress == Some(generation) {
        reply.in_progress = None;
    }
}

/// How the driver's task returned, once it has, and its notification.
#[derive(Debug, Default)]
struct DriverWatch {
    driven: Mutex<Option<std::result::Result<(), Arc<RunError>>>>,
    notify: Notify,
}

impl DriverWatch {
    fn set(&self, driven: std::result::Result<(), Arc<RunError>>) {
        *lock(&self.driven) = Some(driven);
        self.notify.notify_one();
    }

    fn get(&self) -> Option<std::result::Result<(), Arc<RunError>>> {
        lock(&self.driven).clone()
    }
}

/// What an agent's builder was given, kept for a switch.
struct Settings {
    stack: Mutex<Stack>,
    recipe: String,
}

/// The state the agent, its pump, a close it spawns and `Drop` share.
struct Inner {
    session: LiveSession<Payload>,
    core: Core,
    settings: Settings,
    instantiation: Instantiation,
    config: Mutex<Option<SessionConfig>>,
    interrupting: Mutex<Option<JoinHandle<()>>>,
    driver: Mutex<Option<JoinHandle<()>>>,
    failure: Arc<DriverWatch>,
    ended: watch::Sender<bool>,
    first: Mutex<Option<broadcast::Receiver<Item>>>,
    stream_id: StreamId,
    sequence: AtomicI64,
}

impl Inner {
    fn envelope(&self, port: &str, kind: &str, payload: Payload) -> EventEnvelope<Payload> {
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.session.run_id(),
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence.fetch_add(1, Ordering::AcqRel) + 1,
            producer: NodeId::generate(),
            port: port.to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }

    /// Denies every held call, so the node's stop never waits on one.
    fn reject_held(&self) {
        if let Some(authority) = self.core.authority.as_ref() {
            for call in authority.held() {
                let _ = authority.deny(call.call_id());
            }
        }
    }

    /// The close path: drains the session, after denying every held call.
    async fn drain(&self) -> Result<()> {
        self.reject_held();
        match self.session.close().await {
            Ok(()) | Err(inference_facade::live::CloseError::Refused(_)) => Ok(()),
            Err(error) => Err(Error::Layer {
                message: error.to_string(),
                source: None,
            }),
        }
    }
}

/// Spawns the session's driver; its result goes to `watch`.
fn spawn_driver(driver: SessionDriver<Payload>, watch: Arc<DriverWatch>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let driven = driver.drive().await;
        watch.set(driven.map(drop).map_err(Arc::new));
    })
}

/// Acts on one step's requests: the interrupt and the close, each spawned.
fn act(inner: &Arc<Inner>, act: &Act) {
    if act.interrupt {
        let task = Arc::clone(inner);
        let handle = tokio::spawn(async move {
            if task.session.interrupt().await.is_err() {
                let mut reply = lock(&task.core.reply);
                reply.spawned = reply.spawned.saturating_sub(1);
            }
        });
        *lock(&inner.interrupting) = Some(handle);
    }
    match act.close {
        Some(Close::Drain) => {
            let task = Arc::clone(inner);
            tokio::spawn(async move {
                let _ = task.drain().await;
            });
        }
        Some(Close::Cancel) => {
            let _ = inner.session.cancel();
        }
        None => {}
    }
}

/// The pump: the session's one receiver, until the Agent's end.
async fn pump(inner: Arc<Inner>, mut step: Step) {
    let watch = Arc::clone(&inner.failure);
    let mut driven = false;
    loop {
        if !driven && let Some(result) = watch.get() {
            driven = true;
            let requests = step.on_driver(&inner.core, &result);
            act(&inner, &requests);
        }
        tokio::select! {
            biased;
            event = inner.session.receive() => {
                let Some(event) = event else { break };
                let requests = step.on_event(&inner.core, event);
                if requests.end {
                    break;
                }
                act(&inner, &requests);
            }
            () = watch.notify.notified(), if !driven => {}
            () = tokio::time::sleep(TICK) => {}
        }
        inner.core.check_held();
    }
    let driver = lock(&inner.driver).take();
    if let Some(driver) = driver {
        let _ = driver.await;
    }
    let result = watch.get();
    step.on_end(&inner.core, result.as_ref());
    inner.ended.send_replace(true);
}

/// Wraps a shared executor so one tool can be given to every plan of the builder.
struct SharedExecutor(Arc<dyn ToolExecutor>);

impl ToolExecutor for SharedExecutor {
    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        deadline: Instant,
        cancellation: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        self.0.execute(call, deadline, cancellation)
    }
}

/// Which recipe the builder plans.
#[derive(Clone)]
enum Choice {
    Voice,
    Text,
    Recipe(Box<Recipe>),
}

/// The stack of a builder: as given or bound, resolved once.
enum Staged {
    Unresolved { stack: Stack, given: bool },
    Resolved(Stack),
    Failed(String),
}

/// What a builder plans over its resolved stack.
struct Plan {
    recipe: Recipe,
    config: Option<SessionConfig>,
    auto_interrupt: bool,
}

/// The builder of an [`Agent`]: [`Agent::voice`], [`Agent::voice_with`], [`Agent::text`] or
/// [`Agent::from_recipe`], then [`AgentBuilder::start`].
pub struct AgentBuilder {
    choice: Choice,
    stack: Mutex<Staged>,
    instructions: Option<String>,
    voice: Option<String>,
    session_config: Option<SessionConfig>,
    tools: Vec<(Tool, Arc<dyn ToolExecutor>)>,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for AgentBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentBuilder")
            .field("instructions", &self.instructions)
            .field("voice", &self.voice)
            .field("session_config", &self.session_config)
            .finish_non_exhaustive()
    }
}

impl AgentBuilder {
    fn new(choice: Choice) -> Self {
        Self {
            choice,
            stack: Mutex::new(Staged::Unresolved {
                stack: Stack::new(),
                given: false,
            }),
            instructions: None,
            voice: None,
            session_config: None,
            tools: Vec::new(),
            clock: Arc::new(SystemClock),
        }
    }

    /// Replaces the stack: the agent runs over `stack`, resolved over no environment but its
    /// own bindings.
    #[must_use]
    pub fn stack(self, stack: Stack) -> Self {
        *lock(&self.stack) = Staged::Unresolved { stack, given: true };
        self
    }

    fn bind(self, binding: impl Into<crate::Binding>) -> Self {
        let staged = std::mem::replace(&mut *lock(&self.stack), Staged::Failed(String::new()));
        *lock(&self.stack) = match staged {
            Staged::Unresolved { stack, given } => Staged::Unresolved {
                stack: stack.bind(binding),
                given,
            },
            Staged::Resolved(stack) => Staged::Unresolved {
                stack: stack.bind(binding),
                given: true,
            },
            failed @ Staged::Failed(_) => failed,
        };
        self
    }

    /// Binds the `llm` role.
    #[must_use]
    pub fn llm(self, binding: Bound<role::Llm>) -> Self {
        self.bind(binding)
    }

    /// Binds the `stt` role.
    #[must_use]
    pub fn stt(self, binding: Bound<role::Stt>) -> Self {
        self.bind(binding)
    }

    /// Binds the `tts` role.
    #[must_use]
    pub fn tts(self, binding: Bound<role::Tts>) -> Self {
        self.bind(binding)
    }

    /// Binds the `speech` role.
    #[must_use]
    pub fn speech(self, binding: Bound<role::Speech>) -> Self {
        self.bind(binding)
    }

    /// Gives the agent a tool: a `#[inference::tool]` function or a `(Tool, executor)` pair.
    #[must_use]
    pub fn tool(mut self, tool: impl IntoTool) -> Self {
        let (tool, executor) = tool.into_tool();
        self.tools.push((tool, Arc::from(executor)));
        self
    }

    /// The instructions of the speech session and of every request of the generator.
    #[must_use]
    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// The voice of the speech session; wins over every other source.
    #[must_use]
    pub fn voice(mut self, voice: impl Into<String>) -> Self {
        self.voice = Some(voice.into());
        self
    }

    /// The speech session's config, replacing the speech adapter's default and the stack's
    /// voice; [`AgentBuilder::voice`] still wins over its voice. Its `auto_interrupt` is the
    /// agent's.
    #[must_use]
    pub fn session_config(mut self, config: SessionConfig) -> Self {
        self.session_config = Some(config);
        self
    }

    /// The clock whose instant [`Event::Started`] carries.
    #[must_use]
    pub fn clock(mut self, clock: impl Clock) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Runs `f` over the resolved stack, resolving it first.
    fn with_stack<T>(&self, f: impl FnOnce(&mut Stack) -> Result<T>) -> Result<T> {
        let mut staged = lock(&self.stack);
        if let Staged::Unresolved { .. } = &*staged {
            let Staged::Unresolved { stack, given } =
                std::mem::replace(&mut *staged, Staged::Failed(String::new()))
            else {
                unreachable!("matched above");
            };
            let env = if given { Env::new() } else { Env::process() };
            *staged = match stack.resolve_with(&env) {
                Ok(stack) => Staged::Resolved(stack),
                Err(error) => {
                    *staged = Staged::Failed(error.to_string());
                    return Err(error);
                }
            };
        }
        match &mut *staged {
            Staged::Resolved(stack) => f(stack),
            Staged::Failed(message) => Err(Error::Layer {
                message: message.clone(),
                source: None,
            }),
            Staged::Unresolved { .. } => unreachable!("resolved above"),
        }
    }

    /// The recipe, config and `auto_interrupt` the builder plans over `stack`.
    fn plan(&self, stack: &Stack) -> Plan {
        let recipe = match &self.choice {
            Choice::Voice if stack.adapter(Role::Speech).is_some() => Recipe::native_voice(),
            Choice::Voice => Recipe::voice_cascade(),
            Choice::Text => Recipe::chat(),
            Choice::Recipe(recipe) => (**recipe).clone(),
        };
        let recipe = if self.tools.is_empty() {
            recipe
        } else {
            recipe.tools(self.tools.iter().map(|(tool, executor)| {
                let executor: Box<dyn ToolExecutor> =
                    Box::new(SharedExecutor(Arc::clone(executor)));
                (tool.clone(), executor)
            }))
        };
        let recipe = match &self.instructions {
            Some(instructions) => recipe.instructions(instructions.clone()),
            None => recipe,
        };
        let config = stack.adapter(Role::Speech).is_some().then(|| {
            let mut config = self.session_config.clone().unwrap_or_else(|| {
                stack
                    .session_config()
                    .cloned()
                    .unwrap_or_else(default_session_config)
            });
            if let Some(voice) = &self.voice {
                config.voice.clone_from(voice);
            }
            if let Some(instructions) = &self.instructions {
                config.instructions.clone_from(instructions);
            }
            for (tool, _) in &self.tools {
                if !config.tools.iter().any(|given| given.name() == tool.name()) {
                    config.tools.push(tool.clone());
                }
            }
            config
        });
        let auto_interrupt = config.as_ref().map_or_else(
            || {
                self.session_config
                    .as_ref()
                    .is_none_or(|config| config.auto_interrupt)
            },
            |config| config.auto_interrupt,
        );
        Plan {
            recipe,
            config,
            auto_interrupt,
        }
    }

    /// What the agent would run: the stack resolved and the recipe planned; no session is opened.
    ///
    /// # Errors
    ///
    /// Those of [`Stack::resolve_with`] and [`Recipe::instantiate`].
    pub fn describe(&self) -> Result<Description> {
        self.with_stack(|stack| {
            let plan = self.plan(stack);
            let instantiation = plan.recipe.instantiate(stack)?;
            Ok(describe(stack, &instantiation, plan.config))
        })
    }

    /// Opens the agent's session and starts its pump. Needs a Tokio runtime.
    ///
    /// # Errors
    ///
    /// Those of [`AgentBuilder::describe`] and [`Instantiation::open`].
    pub async fn start(self) -> Result<Agent> {
        let (mut stack, plan) = self.with_stack(|stack| {
            let plan = self.plan(stack);
            let taken = std::mem::take(stack);
            Ok((taken, plan))
        })?;
        if plan.config.is_some() {
            stack.set_session_config(plan.config.clone());
        }
        let (sender, first) = broadcast::channel(CAPACITY);
        let route = Arc::new(Route::new(sender));
        let mut instantiation = instantiate(&plan.recipe, &stack, &route, plan.auto_interrupt)?;
        let (session, driver) = instantiation.open(&stack, sessions()).await?;
        let mut core = Core::new(plan.auto_interrupt, route, Arc::clone(&self.clock));
        core.authority = instantiation.authority().cloned();
        core.call_ids = instantiation.call_ids().cloned();
        core.names = self
            .tools
            .iter()
            .map(|(tool, _)| (tool.tool_id(), tool.name().to_owned()))
            .collect();
        let failure = Arc::new(DriverWatch::default());
        let handle = spawn_driver(driver, Arc::clone(&failure));
        let (ended, _) = watch::channel(false);
        let inner = Arc::new(Inner {
            session,
            core,
            settings: Settings {
                stack: Mutex::new(stack),
                recipe: plan.recipe.name().to_owned(),
            },
            instantiation,
            config: Mutex::new(plan.config),
            interrupting: Mutex::new(None),
            driver: Mutex::new(Some(handle)),
            failure,
            ended,
            first: Mutex::new(Some(first)),
            stream_id: StreamId::generate(),
            sequence: AtomicI64::new(0),
        });
        let pump = tokio::spawn(pump(Arc::clone(&inner), Step::new(None)));
        Ok(Agent {
            inner,
            pump: Some(pump),
        })
    }
}

/// Instantiates `recipe` over `stack`, a cascade's audio taking `route`.
fn instantiate(
    recipe: &Recipe,
    stack: &Stack,
    route: &Arc<Route>,
    auto_interrupt: bool,
) -> Result<Instantiation> {
    #[cfg(feature = "audio-host")]
    {
        let routed: Arc<dyn crate::recipe::AudioRoute> = Arc::clone(route) as _;
        crate::recipe::instantiate_routed(recipe, stack, routed, auto_interrupt)
    }
    #[cfg(not(feature = "audio-host"))]
    {
        let _ = (route, auto_interrupt);
        recipe.instantiate(stack)
    }
}

fn describe(
    stack: &Stack,
    instantiation: &Instantiation,
    session_config: Option<SessionConfig>,
) -> Description {
    Description {
        recipe: instantiation.recipe().to_owned(),
        bindings: Role::ALL
            .into_iter()
            .filter_map(|role| {
                stack
                    .adapter(role)
                    .map(|bound| (role, bound.name().map(str::to_owned), bound.source()))
            })
            .collect(),
        pipeline: inference_facade::describe::describe(
            instantiation.declaration(),
            instantiation.pipeline_id(),
        ),
        session_config,
    }
}

fn input_error(error: InputError<Payload>) -> Error {
    match error {
        InputError::Refused(refused) => Error::Closed {
            message: format!("the session admits no input: {refused}"),
        },
        other => Error::Layer {
            message: other.to_string(),
            source: None,
        },
    }
}

/// A session over a recipe: send text or audio, read [`Event`]s, interrupt, switch, close.
pub struct Agent {
    inner: Arc<Inner>,
    pump: Option<JoinHandle<()>>,
}

impl fmt::Debug for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Agent")
            .field("session_id", &self.inner.session.session_id())
            .field("recipe", &self.inner.settings.recipe)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// A voice agent: `native_voice` when the stack binds `speech`, otherwise `voice_cascade`;
    /// the stack is resolved from the process unless [`AgentBuilder::stack`] gives one.
    #[must_use]
    pub fn voice() -> AgentBuilder {
        AgentBuilder::new(Choice::Voice)
    }

    /// `Agent::voice().stack(stack)`.
    #[must_use]
    pub fn voice_with(stack: Stack) -> AgentBuilder {
        Self::voice().stack(stack)
    }

    /// A text agent over `chat`.
    #[must_use]
    pub fn text() -> AgentBuilder {
        AgentBuilder::new(Choice::Text)
    }

    /// An agent over `recipe`.
    #[must_use]
    pub fn from_recipe(recipe: Recipe) -> AgentBuilder {
        AgentBuilder::new(Choice::Recipe(Box::new(recipe)))
    }

    /// The event stream. The first call takes the receiver made when the session opened, so it
    /// misses no event, and that holds after the session ended too: the first call then yields
    /// what the channel still holds and ends. A later call subscribes from that point and sees
    /// only what is sent after it; a later call after the session ended returns a stream that
    /// ends at once. So one task can read `events()` while another calls `send_audio` on the same
    /// `&Agent`.
    ///
    /// Over `native_voice` the events come in the order the model produced them; over a cascade
    /// each node's events keep their own order, and no order between nodes is promised, so a
    /// reader may see a reply's `Said` before the `Heard` it answers.
    #[must_use]
    pub fn events(&self) -> Events {
        if let Some(first) = lock(&self.inner.first).take() {
            return Events::over(Some(first));
        }
        Events::over(self.inner.core.route.subscribe())
    }

    /// Resolves once the pump has dropped the event sender, so every [`Events`] holds all it
    /// will ever hold; at once when it already has.
    pub fn closed(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut ended = self.inner.ended.subscribe();
        async move {
            let _ = ended.wait_for(|ended| *ended).await;
        }
    }

    /// Sends a text turn's text on the recipe's `user_input`.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`] once the session admits no input; [`Error::Layer`] otherwise.
    pub async fn send_text(&self, text: impl Into<String>) -> Result<()> {
        let event = StreamEvent::TranscriptCommitted(inference_text::TranscriptCommitted {
            span_id: SpanId::generate(),
            text: text.into(),
        });
        let kind = event.name().to_owned();
        let envelope = self
            .inner
            .envelope("user_input", &kind, Payload::Stream(event));
        self.inner.session.send(envelope).await.map_err(input_error)
    }

    /// Sends a frame of the user's audio on the recipe's `audio`.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`] once the session admits no input; [`Error::Layer`] otherwise.
    pub async fn send_audio(&self, frame: AudioFrame) -> Result<()> {
        let envelope =
            self.inner
                .envelope("audio", "inference.audio.AudioFrame", Payload::Audio(frame));
        self.inner.session.send(envelope).await.map_err(input_error)
    }

    /// Commits the user's turn.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`] once the session admits no input; [`Error::Layer`] otherwise.
    pub async fn commit(&self) -> Result<()> {
        self.inner.session.commit_input().await.map_err(input_error)
    }

    /// Cuts the reply in progress: its later frames are dropped at once, the session cuts it, and
    /// the stream yields [`Event::Interrupted`].
    ///
    /// # Errors
    ///
    /// [`Error::Closed`] once the session is not ready; [`Error::Layer`] otherwise.
    pub async fn interrupt(&self) -> Result<()> {
        {
            let mut reply = lock(&self.inner.core.reply);
            if let Some(generation) = reply.in_progress.take() {
                reply.cut.insert(generation);
            }
            reply.requested += 1;
        }
        let interrupted = self.inner.session.interrupt().await;
        if let Err(InputError::Refused(_)) = &interrupted {
            let mut reply = lock(&self.inner.core.reply);
            reply.requested = reply.requested.saturating_sub(1);
        }
        interrupted.map_err(input_error)
    }

    /// Confirms the held call `call_id`; whether it was held.
    pub fn confirm(&self, call_id: ToolCallId) -> bool {
        self.inner
            .core
            .authority
            .as_ref()
            .is_some_and(|authority| authority.confirm(call_id))
    }

    /// Rejects the held call `call_id`; whether it was held.
    pub fn reject(&self, call_id: ToolCallId) -> bool {
        self.inner
            .core
            .authority
            .as_ref()
            .is_some_and(|authority| authority.deny(call_id))
    }

    /// Closes the session by draining it: every held confirmation is rejected, every event
    /// received before the close is delivered, then [`Event::Closed`]. Idempotent, whichever of
    /// the application and the model closes first; it returns once the pump has ended.
    ///
    /// # Errors
    ///
    /// [`Error::Layer`] when an ingress refused the end of input.
    pub async fn close(&self) -> Result<()> {
        let closed = self.closed();
        if self.inner.core.leave_open(CLOSING) {
            self.inner.drain().await?;
        }
        closed.await;
        Ok(())
    }

    /// The live session, shared with the agent's pump.
    #[must_use]
    pub fn session(&self) -> &LiveSession<Payload> {
        &self.inner.session
    }

    /// The planned pipeline of the current recipe.
    #[must_use]
    pub fn pipeline(&self) -> &PlannedPipeline {
        self.inner.instantiation.planned()
    }

    /// The recipe, bindings, pipeline and speech config the agent runs.
    #[must_use]
    pub fn describe(&self) -> Description {
        describe(
            &lock(&self.inner.settings.stack),
            &self.inner.instantiation,
            lock(&self.inner.config).clone(),
        )
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.inner.session.cancel();
        if let Some(pump) = self.pump.take() {
            pump.abort();
        }
        if let Some(interrupting) = lock(&self.inner.interrupting).take() {
            interrupting.abort();
        }
    }
}

#[cfg(test)]
mod tests;
