---
format: aep.planning-md/3
id: story:sdk-agent
kind: story
status: draft
title: Agent runs a recipe as one session with one Event stream, devices wiring and escape methods
relations:
- decomposes: epic:sdk
- serves: vision:inference
- depends_on: story:sdk-recipes
- depends_on: story:audio-device-adapter
scope:
- confidence: cited
  path: adapters/inference-audio-device/src/fake.rs
- confidence: cited
  path: adapters/inference-audio-device/src/playout.rs
- confidence: cited
  path: crates/inference-facade/src/live.rs
- confidence: cited
  path: crates/inference-facade/src/switch.rs
- confidence: cited
  path: crates/inference-facade/tests
- confidence: cited
  path: crates/inference-testkit/src/duplex/mock.rs
- confidence: cited
  path: crates/inference-testkit/src/lib.rs
- confidence: cited
  path: crates/inference-testkit/src/mark.rs
- confidence: cited
  path: crates/inference-testkit/src/tts.rs
- confidence: inferred
  path: crates/inference/src/agent.rs
- confidence: inferred
  path: crates/inference/src/devices.rs
- confidence: inferred
  path: crates/inference/src/event.rs
- confidence: inferred
  path: crates/inference/tests/agent.rs
- confidence: cited
  path: crates/inference/tests/compile_fail
revision: 33
---
## Context

`docs/design/sdk.md` §3.2 and §3.5: `Agent`, the session over a recipe, with its builder, one
`Event` stream, devices wiring, and the escape methods. Replaces the hand loop of
`examples/realtime-voice/src/lib.rs:254-605`. Target T5. Serves O5.

## Domain relations

No new noun: an agent is an `inference.session.Session` over the pipeline of one
`inference.sdk.Instantiation`. `Event` is a view of `inference.session` and `inference.stream`
events.

## Planned scope

- `crates/inference-facade/src/live.rs` and `src/switch.rs`. This story owns them in the SDK plan; no
  other draft story scopes them. `LiveSession::receive` and `LiveSession::close` take `&self` (today
  `&mut self`, `live.rs:967`, `:881`):
  - The receive side keeps its own state behind a `std::sync::Mutex<Receiving<P>>` that only
    `poll_receive` takes, never across an await. That state is the egresses
    (`live.rs:736`) and the cursor of what was received.
  - The ingresses become `Mutex<Vec<Arc<Ingress<P>>>>`, because `Ingress<P>` is not `Clone`
    (`crates/inference-runtime/src/controller.rs:212`). `send`, `commit_input` and `interrupt`
    (`live.rs:814`, `:853`, `:909`) clone the `Arc`s out under that lock and await with no lock held.
  - `close` moves the session to Draining, then takes the ingress `Arc`s out with `mem::take`
    under the lock, so the ingresses are marked closed before any await. It then sends `Stop` on
    each with no lock held, and calls `shared.closing()`. No send ever holds a lock that `receive`
    needs.
  - `record_inputs` and `switch` keep `&mut self` (`live.rs:1116`, `switch.rs:77`). `switch` writes
    the new run's ingresses and egresses through `Mutex::get_mut` (`switch.rs:120-129`).
  - Taking `&mut self` to `&self` breaks no caller, because a `&mut` reborrows as `&`. A `let mut`
    that the change leaves unused would still fail `task check`, which runs clippy with
    `-D warnings` (`Taskfile.yml:15`) and so turns `unused_mut` into an error. This story drops
    those `mut`s in `crates/inference-facade/tests/`; 16 files there call `.receive()`.
  - `crates/inference-facade/tests/shared_session.rs` (new): the shared-session test below.
  - `interrupt` (`live.rs:909-930`) records `SessionInterrupted` after it has sent the `Cancel`
    into every ingress, not before (R178): it cuts the delivered generations, sends, then records,
    and records also when a send was refused (the cut holds; the error is returned as before). So
    once a reader has `SessionInterrupted`, every ingress holds the `Cancel` ahead of any input
    sent after it, and the node handles that `Cancel` before that input (story:sdk-duplex-node).
    `crates/inference-facade/tests/interrupt_and_cancel.rs` gains a case that holds the `Cancel`'s
    send (R184). The gate is a test node, `HeldIngressNode`, that reads nothing from its input
    until a `tokio::sync::Notify` fires. The test opens the session over the `text` helpers'
    declaration (`tests/text/mod.rs:162-167`: ingress capacity 4, `OverloadPolicy::Pause`), sends
    four prompts so the ingress is full, and polls `interrupt()` beside `receive()` (both take
    `&self`). Within 100 ms `interrupt()` has not returned and no `SessionInterrupted` is
    readable; after `notify_one()` the node drains its input, `interrupt()` returns `Ok(())`, and
    `SessionInterrupted` is readable, with the node's input holding the `Cancel` after the four
    prompts. Today's order (record before send, `live.rs:910-914`) fails the first assertion.
- The testkit's reply audio and its decoder (R105):
  - `crates/inference-testkit/src/mark.rs` (new; one `pub mod mark;` line in
    `crates/inference-testkit/src/lib.rs`). `mark::encode(mock, reply, frame)` and
    `mark::decode(&AudioFrame) -> Option<Mark { mock: MockId, reply: u32, frame: u32 }>`.
    `MockId` is `Speech` or `Synthesiser`.
  - A marked frame is never silent: its first three samples carry a magic value, the mock and the reply index, the fourth
    the frame index, and every later sample one fixed non-zero PCM16 value. The tap keeps frames as pushed, before the device resampling (`playout.rs:4`).
  - `crates/inference-testkit/src/duplex/mock.rs` (`AUDIO_FRAMES` frames per reply, `:50`, today
    silence, `:23`) and `crates/inference-testkit/src/tts.rs` (`MockSynthesis::silence`,
    `:322-335`) emit marked frames in place of silence. The duplex mock's input detection
    (`mock.rs:3-5`) reads only input, so it is unchanged.
  - The mock's history (R134, R140, R141), in the same `mock.rs`: `MockSpeechModel::history(&self)
    -> MockHistory`, a `Clone` handle on an `Arc<AtomicUsize>` that the model and every clone of
    it share with each session it opens. `MockHistory::completed() -> usize` counts the responses
    the mock completed, incremented in `Step::Complete` (`mock.rs:512`) after
    `response_completed`. A test keeps the handle before binding the model, and gates an
    utterance on it, so the utterance never lands in a response still in progress, where a `Say`
    reply would join that response (`mock.rs:448-461`) and a `Reply::Call` would be dropped
    (`:462`).
- `FakeHost` records what the agent plays (R105, R106, R118, R119). In `adapters/inference-audio-device/src/fake.rs`:
  - `FakeHost::played() -> Vec<Played>` returns the played log in order: `Played::Frame(AudioFrame)`
    for every frame written to its output sink, and `Played::Cleared` each time playback is stopped
    and cleared (`Speaker::stop`, `playout.rs:515`). `FakeHost::played_frames() -> Vec<AudioFrame>`
    returns the `Frame` entries only. `Played` is a `pub enum` in `fake.rs`.
  - Its sink accepts writes without a `pull`. `open_output` sets two crate-private hooks on the
    `PlayoutBuffer` it makes: a tap that keeps each pushed frame and records each stop as
    `Cleared`, and no capacity bound, so `Speaker::push` never answers `Full`.
  - `FakeHost::hold_sink() -> SinkHold`, a test gate: while the returned guard lives, a write to the
    output sink waits in the tap (a `Mutex<bool>` and `Condvar`) and nothing is recorded; dropping
    the guard releases the waiting writes in order. A write waiting in the hold when the speaker is
    stopped is discarded, not recorded, as `stop` discards what is queued (R135).
    `SinkHold::waiting() -> Vec<AudioFrame>` returns the writes waiting in the hold, in order, so a
    test can see that a frame reached the sink while nothing is recorded (R148).
  - The hooks are `pub(crate)` in `adapters/inference-audio-device/src/playout.rs`, which the cpal
    host never sets. `pull` behaves as before.
  - story:audio-device-adapter (active) lands on the same `fake.rs` and `playout.rs`, so this story
    depends on it (R121) and builds on its finished files.
- `crates/inference/src/agent.rs`:
  - `Agent::voice()`, `Agent::voice_with(stack)` (the same as `Agent::voice().stack(stack)`, the
    form the §3.2 program uses, `docs/design/sdk.md` §3.2), `Agent::text()`, `Agent::from_recipe`,
    and the builder methods
    `instructions`, `voice`, `session_config` (an `inference_duplex::SessionConfig` that replaces
    the speech adapter's default, below), `stack`, `llm`, `stt`, `tts`, `speech`, `tool` and `clock`:
    - The role is in the type (R104): `llm` takes only `Bound<role::Llm>`, `stt`
      `Bound<role::Stt>`, `tts` `Bound<role::Tts>` and `speech` `Bound<role::Speech>`, which
      story:sdk-stack's typed constructors return (`Binding::llm(..)` and the rest).
    - A runtime-roled `Binding::named(Role, ..)` is accepted only by `Stack::bind`, so
      `Agent::llm(Binding::tts(..))` and `Agent::llm(Binding::named(Role::Llm, ..))` do not
      compile.
    - Each method binds through `Stack::bind`.
    - `tool` takes `impl Into<(inference_tools::Tool, Box<dyn ToolExecutor>)>`, the pair
      story:sdk-tool-macro's struct converts into, so no edge to that story is needed. The pairs
      reach the recipe through story:sdk-recipes' `Recipe::tools`, and the speech
      `SessionConfig`'s `tools` through the merge below.
  - The shared state (R125). `pub struct Agent { inner: Arc<Inner>, pump: Option<JoinHandle<()>> }`,
    and a private `struct Inner` with these fields, each reached through `&Inner`:
    - `session: LiveSession<Payload>`, with no lock of the Agent's;
    - `state: AtomicU8`, the Agent's lifecycle (R145): `Open`, `Switching`, `Closing`, `Closed`.
      Every move out of `Open` is a `compare_exchange`, so of `switch` and a close exactly one
      wins. Only the pump moves `Closing` to `Closed`, at the Agent's end (R169, below);
    - `auto_interrupt: bool`, the Agent's own setting (R157, R171), fixed at build and unchanged by
      every switch: the `auto_interrupt` of the `SessionConfig` merged from `settings` (below)
      when the stack binds `speech`, whatever the first recipe. When it does not, the
      `auto_interrupt` of the builder's `session_config` if one was given, else `true`, the
      reference cascade's behaviour (its sink always takes turns,
      `examples/local-voice-cascade/src/lib.rs:875`). It controls barge-in on both topologies
      (R181): native through `SpeechStarted` (below), cascade through `TurnOpened`, where
      story:sdk-recipes builds `voice_cascade`'s sink with `cutting(auto_interrupt)` and, off, its
      `AgentNode` with `keeping_replies()`. Off, a `TurnOpened` cuts nothing;
    - `settings`: what the builder was given (`session_config`, `voice`, `instructions`, the tool
      pairs) and the resolved stack, so a switch merges the new recipe's config as `open` does
      (R157), and `config: std::sync::Mutex<Option<SessionConfig>>`, the config of the current
      recipe, which `describe()` returns;
    - `interrupting: std::sync::Mutex<Option<JoinHandle<()>>>`, the interrupt the pump spawned at
      a barge-in, if one runs;
    - `reply: std::sync::Mutex<Tracking>` (R156): `last_started` (the generation of the last
      `GenerationStarted` received), `in_progress` (the reply in progress, if any), `cut` (the
      cut set of generations) and `spawned` (the interrupts the pump spawned and has not yet seen
      the `SessionInterrupted` of). It lives in `Inner`, not in the pump, so a pump started after
      a switch or a failed switch reads the same tracking and cut set;
    - `driver: std::sync::Mutex<Option<JoinHandle<Result<Run, RunError>>>>` (R158), the task
      driving the current run, and `failure: Arc<DriverWatch>`, a `Mutex<Option<Result<(),
      Arc<RunError>>>>` (how `drive` returned, once it has) with a `tokio::sync::Notify`, the one
      thing that task shares with the Agent. Each run's driver gets a fresh watch: `switch`
      replaces `failure` before it spawns the new driver, so an old run's end never reaches a new
      pump (R169). The task holds
      the `SessionDriver` (`crates/inference-facade/src/live.rs:1241`, which holds the session's
      `Shared`, not `Inner`) and that `Arc<DriverWatch>`, never an `Arc<Inner>`, so `switch`'s
      `Arc::get_mut` is not blocked by it;
    - `ended: tokio::sync::watch::Sender<bool>`, set once the pump has dropped the event `Sender`
      (R149);
    - `authority: Option<AuthorityHandle>` (story:sdk-recipes' `native_voice`; `None` for the
      cascade recipes) and `confirmations: std::sync::Mutex<Confirmations>`, the calls already
      announced and the calls whose `ToolCalled` was yielded;
    - `route: Arc<Route>` (R168), holding `sink: std::sync::Mutex<Option<Sink>>`, the slot a
      device run attaches its playout channel to, `sender:
      std::sync::Mutex<Option<broadcast::Sender<Item>>>`, and `routed: AtomicU64`, the frames
      routed with no sink attached. It is an `Arc` of its own so the recipe can hold it (below)
      without an `Arc<Inner>`. Where this story says `Inner`'s `sink` or `sender`, it means these;
    - `first: std::sync::Mutex<Option<broadcast::Receiver<Item>>>`, the receiver made before the
      pump starts.
    The pump task, a close it spawns, and `Drop` each hold or use the `Arc<Inner>`; no task
    borrows `&Agent`. Every method but `switch` takes `&self`.
  - The driver (R158). `LiveSessions::open` answers the session and its `SessionDriver`
    (`crates/inference-facade/src/live.rs:594-600`), and `LiveSession::switch` answers the new
    run's (`crates/inference-facade/src/switch.rs:83`, `:130-133`). The Agent spawns
    `SessionDriver::drive` (`live.rs:1262-1291`) at open and after each successful switch, and
    keeps the `JoinHandle` in `driver`. `drive` returns once the run has ended and its end was
    delivered (`RunCompleted`, `RunCancelled` or `RunFailed`, `live.rs:1271-1287`). When it
    returns, `Ok` or `Err`, the task stores the result in `failure` and notifies. The pump waits
    on that notification beside `receive`. A failed run returns `Ok` too: the loop's error is the
    only `Err` (`live.rs:1262-1291`). The handle is awaited after the session ends: by the pump
    when it ends (below), before it yields `Closed`, and by `switch` for the old run (below).
  - The Agent's end (R169). Four things close the Agent: a current-epoch `RunFailed`, the driver
    task ending (`Ok` or `Err`), `SessionClosed`, and `receive` returning `None`. The first wins
    through the R145 `compare_exchange` of `state` from `Open` to `Closing`:
    - A current-epoch `RunFailed` or `SpeechSessionFailed` (R170, story:sdk-duplex-node: the
      model session failed) yields `Event::Error(Error::Layer)` first; the pump yields at most one
      `Error::Layer` per run, so a `RunFailed` after the run's `SpeechSessionFailed` yields none.
      It and the driver's end then try the exchange; the one that wins spawns the close path with
      `LiveSession::cancel()` (`live.rs:942`), because the run loop has ended and nothing is left
      to drain. One that loses (a close, the model's close or a switch already moved the state)
      does nothing more.
    - `SessionClosed`, or `receive` returning `None` before any `SessionClosed`, ends the pump:
      it moves `state` to `Closing` if it is still `Open`, takes the `driver` handle, if one is
      left, and awaits it, yields one `Event::Error(Error::Layer)` for a driver `Err` unless it
      already yielded one for that run's `RunFailed`, stores `Closed` in `state`, sends
      `Event::Closed`, drops the `Sender`, sets `ended` (R149) and returns. It returns there, so
      `Closed` is sent exactly once.
  - The pump (R96):
    - A task spawned when the session opens owns the receive loop. It holds a clone of the
      `Arc<Inner>` and is the only caller of `receive`, because the session's inbox has one waker
      slot (`live.rs:431-435`).
    - It maps each `SessionEvent` to an `Event` and sends it into a `tokio::sync::broadcast`
      channel of capacity 1024, whose `Item` is `Result<Event, Arc<Error>>`. It ignores the
      `SendError` of a send with no receiver left (R126).
    - `Agent` is not itself a `Stream` (R114). `Agent::events(&self) -> Events` returns the event
      stream, `Events: Stream<Item = Result<Event>> + Send + Sync + Unpin`. Its doc comment says:
      the first call takes `first`, the receiver made when the session opened, so it misses no
      event, and that holds after the session ended too: the first call then yields what the
      channel still holds and ends (R96, R138); a later call subscribes from that point and sees
      only what is sent after it, and a later call after the session ended returns a stream that
      ends at once. So one task can read `events()` while another calls `send_audio` on the same
      `&Agent`. It also says what order holds (R161): over `native_voice` the events come in the
      order the model produced them; over a cascade each node's events keep their own order, and
      no order between nodes is promised, so a reader may see a reply's `Said` before the `Heard`
      it answers.
    - `Events` wraps `tokio_stream::wrappers::BroadcastStream` (R112), which registers its waker
      and so loses no wakeup. Its lag error, `BroadcastStreamRecvError::Lagged(n)`, is yielded as
      `Event::Lagged(n)`, with `n` the events skipped, never a silent drop. Its `Err` item is the
      pump's terminal error, given once before the stream ends.
    - When the session ends, the pump sends `Closed`, then takes the `Sender` out of `sender` and
      drops it (R126). Every `Events` then ends after `Closed`, whenever it was taken. A call to
      `events()` that finds `first` already taken and no `Sender` returns an empty stream. `Events`
      also returns `None` on the poll after it has yielded `Closed` (R113).
    - Audio has one route, decided by the `sink` slot (R117, R127). While a sink is attached, the
      pump hands each audio frame to it and never puts it on the channel; with no sink attached,
      the frame goes on the channel as `Event::Audio`.
    - Both topologies take that one route (R168, R181). `Route` implements story:sdk-recipes'
      crate-private `AudioRoute` (`route(&self, AudioFrame)`, `played(&self) -> u64`, `cut(&self)`,
      `interrupted(&self)`), and the Agent hands a clone of its `Arc<Route>`, as `Arc<dyn
      AudioRoute>`, with its `auto_interrupt`, to the recipe at open and at each switch. The
      recipe never holds an `Arc<Inner>`, so `switch`'s `Arc::get_mut` and `Drop` are unaffected,
      as for the driver task. `route` sends the frame to the attached sink as the pump does, or
      onto the channel as `Event::Audio` with no sink attached, and drops it once the `Sender` is
      gone. `played` is the sink's handed-out count, or, with no sink attached, the count of
      frames routed. `cut` is the pump's cut (below): with a sink attached it raises the
      interruption count and calls `Speaker::stop()`; with none it does nothing, because frames
      already on the channel are the application's (R182). `interrupted` sends one
      `Event::Interrupted` onto the channel. `voice_cascade`'s sink, story:sdk-recipes'
      `AcknowledgingSink` over a `RoutedPlayout`, calls them from its node task, so a cascade reply
      plays through the sink slot, the `FakeHost` hold blocks it and the drain waits for it as for
      a native reply. Its frames bypass the pump's cut set; the cascade's barge-in is the sink's
      own (R181): with `auto_interrupt` on, a `TurnOpened` during a reply cuts the frames queued and
      in flight through `cut`, and each interruption the sink records yields one `Interrupted`
      through `interrupted`. A frame routed while a sink is attached carries the interruption
      count, so `agent.interrupt()` cuts it (R135). Every frame the sink routes is in the channel
      before `SessionClosed`, because the run ends only after the sink has ended, so the end
      marker below still follows the last frame.
    - The mapping (R150), the one table of what the pump yields. Over `native_voice` every model
      event arrives on the one egress, the `DuplexNode`'s `events` port (story:sdk-recipes, R142);
      over a cascade it arrives on the egress of the node that emits it. Session events come from
      the session's own inbox, which `poll_receive` reads before any egress (`live.rs:971-1000`).
      Over a cascade, `SttNode` sends its transcript to the next node before it sends it to its
      exposed egress (`crates/inference-testkit/src/stt/node.rs:443-448`), so the reply can be
      delivered before the transcript it answers: cascade order is not causal (R161).

      | `SessionEvent` | Arrives on | `Event` |
      |---|---|---|
      | `SessionReady`, the first | session inbox | `Started` |
      | `SessionInterrupted` | session inbox | `Interrupted` |
      | `SessionClosed` | session inbox | `Closed` |
      | `RunFailed` | session inbox | `Error(Error::Layer)`, current epoch only (below) |
      | `TranscriptHypothesis` | `events` | `Heard { final_: false }` |
      | `TranscriptRevision` | `events` | `Transcript` |
      | `TranscriptCommitted` | `events` | `Heard { final_: true }` |
      | `TextSegmentCommitted` | `events` | `Said` |
      | `AudioChunk` | `events` | `Audio`, or the sink; none for a cut generation (below) |
      | `ToolCallProposed` | `events` | `ToolCalled` |
      | `UsageRecorded` | `events` | `Usage` |
      | `SpeechResponseFailed` | `events` | `Error(Error::Provider)` (R143) |
      | `SpeechStarted` | `events` | none; barge-in input (below) |
      | `SpeechSessionClosed` | `events` | none; starts the close (below), current epoch only |
      | `SpeechSessionFailed` (R170) | `events` | `Error(Error::Layer)`, then the close (below), current epoch only |
      | `GenerationStarted`, `GenerationCompleted`, `GenerationCancelled`, `GenerationSuperseded` | `events` | none; reply tracking (below) |
      | `TextDelta`, `SynthesisCompleted`, `AudioPlayed`, every other `ToolCall*` event | `events` | none |
      | every other session lifecycle event, `RunCompleted`, `RunCancelled` | session inbox | none |

      A `Say` turn of the duplex mock reaches the session as `SpeechStarted`,
      `TranscriptCommitted`, `UsageRecorded`, `GenerationStarted`, `TextDelta`,
      `TextSegmentCommitted`, `AudioChunk` × `AUDIO_FRAMES`, `UsageRecorded`,
      `GenerationCompleted` (story:sdk-duplex-node, one egress). By the table it yields six
      events: `Heard`, `Usage`, `Said`, `Audio`, `Audio`, `Usage`.
    - Run epochs (R153). The epoch is the current run. After a successful switch the new pump is
      started with `left: Some(<the old run's RunId>)`, read with `LiveSession::run_id()`
      (`live.rs:764`) before the switch. An event's epoch is known by its place in the session's
      one ordered stream. `switch` records `PipelineSwitched`, then queues every event still on
      the old run's egresses among the session's own (`poll_drained`, `switch.rs:104-115`,
      `:138-160`), then the old run's end, `RunCompleted { run_id }` (`run_ended`,
      `live.rs:268-290`; `poll_left` and `release_left`, `:250-261`, `:158-161`). It returns `Ok`
      only once that end is queued, and the new run's events come after it, because
      `poll_receive` empties the inbox before it reads an egress (`live.rs:971-1000`). So while
      `left` is set, every event is the old epoch's, up to and including the `RunCompleted` whose
      `run_id` is `left`; that one clears `left`. The pump maps an old-epoch event's data as usual
      (`Heard`, `Said`, `Audio`, `Usage`, `SpeechResponseFailed`, the reply tracking), but drops
      its `SpeechStarted` (R172), `SpeechSessionClosed`, `SpeechSessionFailed`, `RunCompleted`, `RunCancelled` and
      `RunFailed`: it neither acts on them nor yields them, so an old run's `SpeechStarted` is no
      barge-in on the new one. A pump started at open, after a failed switch or after a
      `Drained` or `Start` switch error has `left: None`, so every event it reads is the current
      epoch's.
    - The reply in progress (R143, R154, R156). The pump tracks it in `Inner`'s `reply`. A reply
      starts at `GenerationStarted`, which the `DuplexNode` emits at the response's
      `ResponseCreated`, before the reply's first output. It ends at that generation's
      `GenerationCompleted`, its `GenerationCancelled`, the next `GenerationStarted`, or a cut: a
      barge-in's or an `agent.interrupt()`'s (R154). So a `SpeechStarted` after a cut, with no new
      `GenerationStarted`, finds no reply in progress and yields no `Interrupted`.
      `GenerationCancelled` is never read as a barge-in: `ResponseFailed` produces one too, and
      its `SpeechResponseFailed` yields `Event::Error`, never `Interrupted`.
    - The barge-in (R144). It is a `SpeechStarted` while a reply is in progress or while an
      attached sink's `pending()` is true (reply audio not yet played, below). What it does depends
      on `auto_interrupt` in `Inner`:
      - On: the pump adds the reply's generation, if one is in progress, to the cut set and ends
        the tracked reply (R154), cuts local playback (the cut, below), and `tokio::spawn`s `LiveSession::interrupt()` on its
        clone of the `Arc<Inner>`, keeping the `JoinHandle` in `interrupting`. It spawns because
        `interrupt` awaits a `cancel` send on each ingress under `OverloadPolicy::Pause`, which can
        wait on an egress only the pump drains. `interrupt` records `SessionInterrupted` once its `Cancel` sends are done (R178, above), and
        the pump maps it to the one `Interrupted` of this barge-in. If
        `interrupt` refuses (the session is no longer Ready), no `Interrupted` is yielded and the
        cut stands.
      - Off: nothing. The pump cuts nothing, spawns nothing and yields no `Interrupted`; the
        provider keeps the response and the application decides through `interrupt()`.
      - A cascade reports no `SpeechStarted`; its barge-in is the sink's at `TurnOpened`, under the
        same setting (R181, the route above).
      - A `SpeechStarted` with no reply in progress and nothing pending is the user's own turn
        starting and does nothing, so utterance 0 adds no event.
    - What marks a frame of the cut reply (R144, R155). An `AudioChunk` names no generation
      (`generation_of`, `live.rs:1209-1218`, has no arm for it), so the pump attributes each one
      to `last_started`, the generation of the last `GenerationStarted` before it on the one
      ordered egress (R142); a cut does not clear `last_started`. It drops every `AudioChunk`
      whose generation so attributed is in the cut set: it neither yields it nor hands it to the
      sink. The cut set
      covers the window before the spawned `interrupt` cuts the session's delivered generations;
      after that the session drops `g`'s events itself (`live.rs:915-919`, `:1029-1040`), so its
      later `GenerationCancelled` adds no second `Interrupted`.
    - `agent.interrupt()` first adds the reply in progress, if any, to the cut set and ends the
      tracked reply, under `reply`'s lock with no await held (R155), so a later frame of that
      reply is dropped even before the session's cut takes hold. It then calls
      `LiveSession::interrupt` (`live.rs:909`); the session records
      `SessionInterrupted` (`:914`) and the pump maps it to `Interrupted`, and cuts local playback
      when a sink is attached (R135). A `SessionInterrupted` that answers the pump's own barge-in
      does not cut a second time: the pump counts the interrupts it spawned and consumes one per
      `SessionInterrupted`.
    - The pump never awaits `close()` itself (R111). On a current-epoch
      `SessionEvent::SpeechSessionClosed` it moves `state` from `Open` to `Closing` by `compare_exchange` before anything else (R137,
      R145). Only if it won does it `tokio::spawn` the close path, a function of `Arc<Inner>`, on
      its own clone. It moves the state and spawns with no await between, so an abort from
      `switch` cannot fall between them. If it lost to `Switching`, it does nothing and the
      switch wins (R165): the switch's `Stop` drains the old run, whose `DuplexNode` has a closed
      model and ends `events` at once (story:sdk-duplex-node), the new run starts. A model close the
      pump had not read when it was aborted is drained into the inbox as the old epoch's and
      dropped (above). If it lost to
      `Closing`, a close already runs. Either way it keeps calling `receive` until the Agent's end
      (above). Awaiting
      `close()` inline would deadlock: `close` sends `Stop` on an ingress under
      `OverloadPolicy::Pause` while the `DuplexNode` waits on an egress that only the pump
      drains. Every recipe's exposures have capacity 64 under `OverloadPolicy::Pause`
      (story:sdk-recipes, `EXPOSURE_CAPACITY`).
  - `Agent::closed(&self) -> impl Future<Output = ()>` (R149) resolves once `ended` is set, so
    after the pump has dropped the `Sender` and every `Events` holds all it will ever hold. It
    resolves at once on an Agent whose pump already ended.
  - `switch(&mut self, recipe)` (R129, R145):
    - It moves `state` from `Open` to `Switching` by `compare_exchange`. If that fails (the state
      is `Closing` or `Closed`), it returns `Err(Error::Closed)` and touches nothing. The pump's
      model-initiated close moves `Open` to `Closing` the same way, so exactly one of the two
      wins; a switch in the window after the model's close is refused here.
    - It aborts the pump and awaits its `JoinHandle`, then aborts and awaits `interrupting`, if
      set, so no task but the Agent holds an `Arc<Inner>` clone; the driver task holds none
      (above) and keeps driving the old run, whose end `poll_drained` waits for. The abort lands
      at an await. The pump's only awaits are `receive`, the 20 ms `held()` tick and the
      driver's `Notify`, each cancel-safe; between them it sends to the
      channel and the sink without awaiting. An abort at `receive` loses no event, because
      `poll_receive` takes an event out of an egress or the inbox only when it returns `Ready`
      with it (`live.rs:971-1000`).
    - It resolves and plans the new recipe over the Agent's stack. The new recipe brings its
      models and its session config (R157): for `native_voice` the config is merged from
      `settings` as at open (the speech adapter's default, the stack's `voice`, the builder's
      `session_config`, the builder's `voice`), then its `auto_interrupt` is set to the Agent's,
      so the model and the pump agree; a cascade has no session config and takes the Agent's
      `auto_interrupt` at instantiation (R181). The Agent's own settings persist:
      `auto_interrupt`, the broadcast channel of capacity 1024 and its `Sender`, the `sink` slot
      with an attached device run, `confirmations` and `reply`.
    - It reads the old run's id (`LiveSession::run_id()`, `live.rs:764`), takes the session
      through `Arc::get_mut(&mut self.inner)` and calls `LiveSession::switch`. On `Ok(driver)` it
      awaits the old `driver` handle, which returns once the old run's end is delivered, the end
      `switch` already waited for (R158). It then spawns the new driver into `driver`, stores the
      new config in `config`, sets `authority` to the new recipe's handle, starts a new pump on
      the same `Sender` with `left` set to the old run's id (R153), and moves `state` back to
      `Open`. `confirmations` stays as it was, so a call announced before the switch is not
      announced again.
    - Every return after the abort leaves a running pump or a closed Agent that has emitted
      `Closed`:
      - A resolve or plan failure (`Error::RoleUnbound`, `ProfileDenies` and the rest), a
        `get_mut` that finds another clone (returned as `Err(Error::Closed)`), or
        `SwitchError::Refused` (nothing moved, the old run goes on, `switch.rs:77-130`): `switch`
        starts a new pump on the old session, moves `state` back to `Open`, and returns the
        error. The Agent works as before.
      - `SwitchError::Drained` (the session is Opened with no run) or `SwitchError::Start` (the
        session was closed): `switch` moves `state` to `Closing`, awaits the old `driver` handle
        (the old run has ended), calls `LiveSession::cancel()` when the session is not yet
        Closed, and starts a new pump with `left: None`, which delivers until `SessionClosed` and
        yields `Closed`. It returns the error. An `Err` from the old driver is reported as above.
    - It takes `&mut self` because `LiveSession::switch` does (`switch.rs:77`).
  - `impl Drop for Agent` (R115): if the session is not Closed, it calls `LiveSession::cancel()`
    (`live.rs:942`) through `inner` and aborts the pump's `JoinHandle` and `interrupting`; it
    leaves the driver task running, which ends with the cancelled run and holds no `Arc<Inner>`. So no task's `Arc<Inner>`
    clone keeps keep a session alive that the application let go of (`live.rs:1182-1188` runs
    only when the last `Arc<Inner>` drops).
  - `start`, `send_text`, `send_audio`, `commit`, `interrupt` and `close`; `confirm(call_id)` and
    `reject(call_id)` (R101):
    - They answer a held confirmation through the `AuthorityHandle` of story:sdk-recipes'
      `native_voice` (`crates/inference-pipeline/src/nodes/tool_authority.rs:434`, `:441`;
      `reject` is the handle's `deny`), and return the handle's `bool`.
    - The pump yields `Event::ConfirmationRequested` once for each call that newly appears in
      `AuthorityHandle::held()`. It checks `held()` on every event and every 20 ms, so the call is
      already held when the application sees the event and can answer it at once.
    - Order (R120): `Event::ToolCalled` comes from `SessionEvent::ToolCallProposed`
      (`crates/inference-facade/src/events.rs:97`), which the session delivers from the one exposed
      `events` egress (story:sdk-recipes, R142). The pump yields a call's `ConfirmationRequested`
      only after that call's `ToolCalled`; a call found held before its `ToolCalled` waits for it.
    - `close()` moves `state` from `Open` to `Closing` first (R145), then rejects every held confirmation
      (`AuthorityHandle::held()`, `:425`), so the node's stop never waits on one (R132). The pump
      reads `state` before it yields any `ConfirmationRequested`. With `Closing` or `Closed` (R116), it
      denies every call that becomes held at each `held()` check, until the run ends, and yields
      no `ConfirmationRequested` for it. So a call is either announced while the state is `Open` or
      denied without announcement.
  - The `UsageRecorded` records the agent delivered, kept for story:sdk-usage-cost.
  - The trait `inference::Clock`, whose instant `Event::Started` carries. No clock trait exists in
    the workspace today.
  - The escape methods (R97):
    - `session() -> &LiveSession<Payload>`, borrowed from the Agent's `Arc`.
    - `pipeline() -> &PlannedPipeline`, the type `LiveSessions::open` and `LiveSession::switch`
      take (`crates/inference-pipeline/src/planning.rs:80`, `live.rs:594-597`).
    - `describe()` returns `inference::Description`: the recipe chosen, each role's binding and
      `BindingSource`, the `inference_facade::describe` output and the duplex `SessionConfig` of
      the current recipe (`None` for a cascade): the one it opened with, or the one the last
      switch merged (R157).
    - That config is merged in one place, each later step winning: the speech adapter's default
      from its registry descriptor (story:sdk-stack), then the stack's `voice` key, then the
      builder's `session_config(..)`, which replaces the whole config, then the builder's
      `voice(..)`.
    - The builder's `instructions` and `tool`s fill `instructions` and `tools`.
    - The builder has the same `describe()`: it resolves the stack and plans the recipe but opens
      no session (`docs/design/sdk.md` §3.5).
  - The closed notification (story:sdk-duplex-node). The Agent's `close()` and the close the pump
    spawns on `SpeechSessionClosed` both run the runtime's normal stop path, `LiveSession::close`
    (`live.rs:881`). The run drains, the session delivers `SessionClosed`, and the agent yields
    `Event::Closed` once:
    - `SpeechSessionClosed` itself is not an `Event` variant; `Event::raw()` on `Closed` returns
      the `SessionClosed`.
    - Closing is idempotent. `state` moves from `Open` to `Closing` by `compare_exchange` in whichever
      close comes first, the application's `close()` or the pump on `SpeechSessionClosed` (R137,
      R145); the second finds `Closing` or `Closed` and returns `Ok(())` without calling
      `LiveSession::close` again (which refuses a session that is no longer `Opened` or `Ready`,
      `live.rs:881`).
  - Compile-time `Send` and `Sync` assertions for `Agent` in `agent.rs`
    (`const _: fn() = || { fn assert_send<T: Send>() {} fn assert_sync<T: Sync>() {}
    assert_send::<Agent>(); assert_sync::<Agent>(); };`), so a field that is not `Send` or not
    `Sync` fails the build. `Send` is §8's promise (`docs/design/sdk.md` §8). `Sync` is the one
    this story adds, because a handle is shared with another task (`&Agent` across a
    `tokio::spawn` for `close` or `send_audio` during a run).
  - `inference::sessions()`, public: the process-wide `LiveSessions`
    (`crates/inference-facade/src/live.rs:532`) every SDK session opens on. story:sdk-one-call
    uses it and does not define it.
- `crates/inference/src/event.rs`: `Events` (above) and `Event`, `#[non_exhaustive]` and `Clone`, with the twelve
  variants of the one list in `docs/design/sdk.md` §3.2 (R124): `Heard`, `Said`, `Audio`,
  `ToolCalled`, `Usage`, `Interrupted`, `Closed`, `Started`, `Transcript`, `Error`, `Lagged(u64)`
  and `ConfirmationRequested { call_id: ToolCallId, name, arguments }`. Also `Event::raw()`.
  `ToolCalled { name, arguments, call_id: ToolCallId, speech_call_id: Option<SpeechCallId> }`,
  mapped from `SessionEvent::ToolCallProposed`, carries the provider's call id from
  story:sdk-duplex-node's call-id map (R102). `Error` holds an
  `Arc<inference::Error>`, so `Event` is `Clone`. The doc comment of `Audio` states R182: it is
  yielded only while no sink is attached; then the application owns playback and cutting, a
  frame already yielded is not recalled by a cut (neither `agent.interrupt()` nor a barge-in),
  and `Interrupted` tells it when to cut, on both topologies.
- `crates/inference/src/devices.rs`: `run_on_devices_with(&self, host: impl AudioHost)`
  (`adapters/inference-audio-device/src/host.rs:150`) behind the feature `audio-host`, and
  `run_on_devices(&self)` on the default host (`CpalHost`) behind `audio-device` (story:sdk-crate):
  - Both take `&self`, so the agent stays readable afterwards (`usage()`,
    story:sdk-usage-cost). Both call a crate-private `run_until(host, stop)`, `stop` being
    `tokio::signal::ctrl_c()`.
  - `run_until` opens the `Speaker` and feeds it from a playout thread through an unbounded
    `std::sync::mpsc` channel, because `Speaker::push` sleeps (`playout.rs:486-505`). It puts
    that channel's sender, as a `Sink`, in `Inner`'s `sink` slot before it feeds the first input
    frame, and takes it out after the drain below (R127). The `Sink` holds the sender, an
    `Arc<Speaker>` (every `Speaker` method takes `&self`, `playout.rs:430-432`), an
    `Arc<AtomicU64>` interruption count, and two `AtomicU64` frame counts: sent by the pump, and
    handed out (pushed or discarded) by the playout thread.
  - The input feeder (R146) reads the host's input frames and sends each with `send_audio`. `send_audio`, like
    `send_text` and `commit`, maps `InputError::Refused` (the session is not Ready,
    `crates/inference-facade/src/live.rs:810-811`, `:489-491`) to `Error::Closed`. When it returns
    that, because the session left Ready after a model-initiated close or a `close()`, the feeder stops without an error, and `run_until` goes
    on to the drain below. Any other error ends `run_until` with that error.
  - `Sink::pending()` is true while reply audio is not yet played: sent exceeds handed out, or
    `Speaker::queued()` (`playout.rs:528`) is above zero. The pump reads it on `SpeechStarted`
    (R134, R144).
  - The cut (R130, R135, R144). At a barge-in with `auto_interrupt` on, and at a `SessionInterrupted`
    from the application's `interrupt()`, with a sink attached, the pump raises
    the interruption count, then calls `Speaker::stop()` (`playout.rs:515`) through the sink
    slot, at once, whatever is queued: stop clears the speaker's queue, so audio already pushed
    stops too. Each frame goes down the channel with the count at the time it was sent, and the
    playout thread discards, without pushing, every frame whose count is below the current one,
    so the frames still in the channel are dropped as well. A push in progress when the stop
    lands returns `PushError::Stopped` (`playout.rs:486-505`), and the frame is dropped.
  - It consumes nothing from the event stream. While the sink is attached, audio goes to it only,
    never onto the event channel (R117), so every non-audio event of the run stays readable after
    it returns, for runs that deliver fewer than 1024 of them, the channel's capacity; past that a
    reader gets `Event::Lagged(n)`.
  - It ends when the session ends (the pump has seen `SessionClosed`) or when `stop` completes,
    whichever is first, in the order below. `.closes()` on the duplex mock (story:sdk-duplex-node) ends the session
    once the mock's script is spent, through the closed notification above.
  - The drain (R106) means handed to the output sink. Before `run_until` drops the output stream
    and returns `Ok(())`, the playout thread has pushed every frame of the last reply and called
    `Speaker::finish()`.
  - The end marker (R136). When the pump receives `SessionClosed`, every audio frame the session
    delivered is already in the sink, because the session delivers `SessionClosed` after what the
    egresses held (`live.rs:316-325`). The pump then sends `End` down the same channel, after the
    last frame. The playout thread, on `End`, has handed out every frame before it; it calls
    `Speaker::finish()` and fires the drain `oneshot`. The playout thread never waits for the
    detach, so the drain does not depend on what follows it.
  - The order of `run_until`'s end, on both paths:
    - The session ends (the pump has seen `SessionClosed`): `run_until` awaits the `oneshot`, then
      the playback wait below, then takes the sink out of the slot.
    - `stop` completes first (`ctrl_c` or the test's stop future): `run_until` calls `close()`,
      which drains the session until `SessionClosed`, then awaits the `oneshot`, then the playback
      wait, then takes the sink out. Audio of the drain still goes to the sink, never onto the
      channel, because the slot is emptied only after `End`.
  - The drain wait never blocks the runtime (R128). `run_until` awaits the `oneshot`. A held
    `FakeHost` sink blocks only the playout thread, a `std::thread`, not a runtime worker.
  - On a real device it then waits for buffered playback with `Speaker::wait_played`
    (`playout.rs:534`), which sleeps in a loop (`:534-546`), so it runs in
    `tokio::task::spawn_blocking`. Its patience is the queued duration at that moment plus a
    margin of 250 ms. On a `FakeHost` that no test pulls, the wait ends at that bound, so no test
    needs a concurrent `pull` to end.
  - story:sdk-one-call's `talk` and `talk_with` call `run_until` too.
- `crates/inference/tests/agent.rs` (stub from story:sdk-crate, filled here):
  - Its mock scripts are reached through the feature `testing`. story:sdk-crate declares the
    `[[test]] agent` with `required-features = ["testing"]` (which implies `audio-host`, and so
    `FakeHost`), so Cargo builds the file only with it.
  - The tests bind the testkit mocks through the builder methods, as typed values (`Binding::stt`,
    `llm`, `tts`, `speech`, story:sdk-stack) or by the adapter name `mock` (story:sdk-stack's
    `registry/mock.rs`). This story adds no registration.
  - The file also holds the `trybuild` driver of `crates/inference/tests/compile_fail/`
    (new; `trybuild` is story:sdk-crate's dev-dependency). That is a directory of its own, apart
    from story:sdk-tool-macro's `tests/ui/`.

## Acceptance

In these acceptances, "the stream" is what the first `agent.events()` call returns. "Utterance i"
is `inference_testkit::duplex::mock::utterance(format, at)` (story:sdk-duplex-node, R162) in the
session's input format, starting where utterance i-1 ended (`next()`). "Sending utterance i" is
`send_audio` of each of its `frames()`, the speech then the silence, with no `commit()`: under
ServerVad, the mock's default, the silence ends the turn. "Feeding" it on a device run is
`host.feed(..)` of the same frames.

- Builder, one test each:
  - `instructions` reaches the generator's request.
  - `stack` replaces the resolved stack.
  - `llm`, `stt`, `tts` and `speech` each bind their role, reported with `BindingSource` `builder`
    by `describe()`.
  - `tool` makes the tool callable, shown by a `ToolCalled` event. The test hands it a hand-built
    `(Tool, Box<dyn ToolExecutor>)` pair, because this story does not depend on
    story:sdk-tool-macro, so `#[inference::tool]` may still be story:sdk-crate's `compile_error!`
    stub when it is implemented.
  - `clock` sets the instant `Event::Started` carries.
  - `Agent::voice_with(stack)` and `Agent::voice().stack(stack)` over the same stack give equal
    `describe()` output.
- The role is in the type (R104). A `trybuild` compile-fail case
  `crates/inference/tests/compile_fail/agent_llm_takes_llm.rs` calls
  `Agent::voice().llm(Binding::tts(<a MockSynthesiser>))`, and a second case calls
  `.llm(Binding::named(Role::Llm, "mock", keys))`. Neither compiles; the `.stderr` files name the
  expected `Bound<role::Llm>`. The driver in `tests/agent.rs` passes.
- `Agent::voice()` over a stack binding `speech` plans `native_voice`; over one binding `stt`,
  `llm`, `tts` and no `speech` plans `voice_cascade`; `describe()` names the recipe chosen.
- Each test that registers an adapter with story:sdk-stack's `Registry::register` does so under
  a name no other test registers (R221), as story:sdk-stack and story:sdk-recipes do, because the
  table is process-wide and append-only (decision D32 of `api-design:sdk`); the three below use
  `agent-hosted-switch`, `agent-hosted-first` and `agent-describe-no-open`.
- No switch across a profile boundary (`docs/design/sdk.md` §3.2, §4.3). A stack under
  `profile = "offline_strict"` binds `stt`, `llm` and `tts` to the mocks and `speech` to an adapter
  the test registers as `agent-hosted-switch` with locality `Locality::ExternalProvider` and an
  `open` that panics. `Agent::voice().stack(stack)`
  fails with `Error::ProfileDenies` from both the builder's `describe()` and `start()`. It plans no
  `voice_cascade` in its place, and the speech adapter's `open` is never called.
- A first hosted stack is told the fix (R203). A stack binding only `speech`, by
  `Binding::named(Role::Speech, "agent-hosted-first", keys)`, an adapter the test registers like the
  one above but under that name, resolved with
  `resolve_with(&Env::new())`, so no `profile` in any source (the default `offline_strict`,
  story:sdk-stack): the builder's `describe()` fails with an `Error` whose `kind()` is
  `inference::ErrorKind::ProfileDenies` and whose `to_string()` contains
  `profile = "hybrid_opt_in"` and `INFERENCE_PROFILE`, and also the role `speech`, that
  adapter's name and the locality `external_provider`, each found before the
  `profile = "hybrid_opt_in"` text (R212). The adapter's `open` is never called.
- A voice agent short of a role is told which (R210). `Agent::voice()` over a stack that binds
  only `stt` and `llm` to the cascade mocks, so `voice_cascade` is chosen: the builder's
  `describe()` fails with an `Error` whose `kind()` is `inference::ErrorKind::RoleUnbound` and
  whose `to_string()` contains `tts`.
- The merged `SessionConfig`, over a stack binding `speech` to the duplex mock, with the builder's
  `describe()`:
  - With no `voice` anywhere, `voice` is the mock registration's default.
  - With `voice = "cedar"` in the stack's `[speech]` table it is `cedar`; with that key and the
    builder's `.voice("verse")` it is `verse`.
  - In all three, formats, turn detection and `auto_interrupt` stay the registration's default.
  - With `.session_config(<a config at 16 kHz input, auto_interrupt on, voice "matthew">)` the
    merged config has that input rate, `auto_interrupt` and voice.
  - With `.session_config(..)` and `.voice("verse")` together, the voice is `verse` and the rest
    is the given config's.
- An AWS-bound stack opens nothing until `start()` (R110), gated `#[cfg(feature = "aws")]` and
  run under `--all-features`. Over a stack file with `profile = "hybrid_opt_in"` (without it the
  stack's default `offline_strict` denies the hosted adapter, story:sdk-stack, R201) and
  `[speech] adapter = "aws"`, and an `Env` with no AWS
  variable, the builder's `describe()` returns Nova Sonic's default `SessionConfig`.
- A tool call over `native_voice` (story:sdk-recipes). The stack binds `speech` to
  `MockSpeechModel::native(vec![<a Reply::Call turn naming "lookup">])`
  (the mock names its first call `call_1`, `crates/inference-testkit/src/duplex/mock.rs:491-492`), and the builder is given a hand-built `lookup` pair with `tool`. The agent yields
  `ToolCalled { name: "lookup", speech_call_id: Some(SpeechCallId("call_1")), .. }`, then `Said`, and
  the mock received the executor's answer under `call_1`.
- Confirmation (R101). Over the same mock with a `lookup` tool that requires confirmation:
  - The agent yields `ToolCalled { call_id, name: "lookup", .. }` and then
    `ConfirmationRequested` with the same `call_id`, in that order (R120); the test asserts the
    order.
  - `agent.reject(call_id)` makes the mock receive a rejection under `call_1`, and the executor never
    runs.
  - In a second case, `agent.confirm(call_id)` runs the executor, and the mock receives its answer
    under `call_1`.
  - In a third case, `agent.close()` with the confirmation still held returns `Ok(())` and the
    stream ends with `Closed`.
  - In a fourth case (R116, R132, R140), the mock's script has two `Reply::Call` turns naming
    `lookup`, and the test keeps `let history = model.history();`. It sends utterance 1 and
    reads the stream until `ConfirmationRequested` for `call_1`. It sends utterance 2 only once `history.completed()` is 1, so the call_1 response is complete, no
    response is in progress, and the mock proposes `call_2` in a new response instead of dropping
    it (`mock.rs:462`); `call_1` is still held, so no tool result can start one first. It reads
    the stream until `ToolCalled` for `call_2`, then calls `agent.close()`. The mock receives
    rejections under `call_1` and `call_2`, the executor never runs, the stream yields at most one
    `ConfirmationRequested` for `call_2`, `close()` returns `Ok(())`, and the stream ends with
    `Closed`.
  - The flag branch of R116, as a unit test in `agent.rs` of the pump's `held()` check: with `state`
    `Closing` and a test `AuthorityHandle` holding a call whose `ToolCalled` was yielded,
    one check denies it, records no announcement, and sends no `ConfirmationRequested` on the
    channel; with `state` `Open`, the same check sends one.
- The emission order, against the egress index (R142). Over `MockSpeechModel::native(<one Say
  turn>)` in `start()` mode, with `let history = model.history();` kept before binding, the test
  sends utterance 0 and reads nothing until `history.completed()` is 1, so every
  event of the turn is queued before the first read. The stream then yields `Heard`, `Usage`,
  `Said`, `Audio`, `Audio`, `Usage` in that order. The node emits the transcription
  `UsageRecorded` before the reply transcript; under the round-10 layout that record left on
  the control port, an egress of higher index than the transcript's, and a reader polling
  egresses in index order (`crates/inference-facade/src/live.rs:976-993`) would have yielded it
  after `Said`. With one egress it is second, in emission order.
- The barge-in mapping (R143, R144), a unit test in `agent.rs` of the pump's mapping step, fed
  session events directly with a recording stand-in for the spawned `interrupt`, so no timing is
  involved. With `auto_interrupt` on unless stated:
  - `SpeechStarted` with no reply in progress and no sink: no `Interrupted`, no interrupt
    requested.
  - `SpeechStarted`, then `GenerationStarted(g)` and an `AudioChunk` of `g` (the user's turn
    starting before the reply's first output): nothing requested, and the frame passes as
    `Audio`.
  - `GenerationStarted(g)`, `TextSegmentCommitted`, then `SpeechStarted`: one interrupt
    requested, `g` in the cut set, and a following `AudioChunk` dropped (no `Audio`). A
    `SessionInterrupted` fed next yields exactly one `Interrupted`.
  - `SpeechStarted` with no reply in progress and a sink whose `pending()` is true: one interrupt
    requested and the interruption count raised once.
  - `GenerationStarted(g)`, `SpeechResponseFailed`, `GenerationCancelled` of `g`: exactly one
    `Event::Error` holding `Error::Provider`, and no `Interrupted`.
  - `GenerationCancelled` with no `SpeechStarted` before it: no `Interrupted`.
  - With `auto_interrupt` off, `GenerationStarted(g)`, `TextSegmentCommitted`, `SpeechStarted`
    and a sink whose `pending()` is true: nothing requested, nothing cut, the count unchanged,
    and a following `AudioChunk` passes.
  - The cut ends the reply (R154): `GenerationStarted(g)`, `SpeechStarted`, `SpeechStarted`, no
    sink: one interrupt requested; with a `SessionInterrupted` fed after each, exactly one
    `Interrupted`.
  - Attribution (R155): `GenerationStarted(g)`, `SpeechStarted`, then an `AudioChunk`: dropped,
    because the frame is attributed to `g` though the cut ended the tracked reply.
    `GenerationStarted(h)` and an `AudioChunk` then pass as `Audio`.
  - The application's interrupt (R155): `GenerationStarted(g)`, then `agent.interrupt()`'s cut
    step, then an `AudioChunk`: dropped, `g` in the cut set, and a following `SpeechStarted`
    requests nothing.
  - A restarted pump (R156): a pump fed `GenerationStarted(g)` is dropped, and a new pump over the
    same `Inner` is fed `SpeechStarted`: one interrupt requested and a following `AudioChunk`
    dropped.
- Run epochs (R153, R165), a unit test in `agent.rs` of the same step:
  - A pump started with `left: Some(r0)` is fed `PipelineSwitched`, `SpeechSessionClosed`,
    `RunCompleted { run_id: r0 }`, then a `Say` turn's events. It moves no state, spawns no
    close, yields no `Closed` and no `Error`, and yields the turn's six events.
  - With `state` `Switching`, a pump with `left: None` fed `SpeechSessionClosed` leaves `state`
    `Switching` and spawns no close: the switch wins.
  - A pump with `left: None` fed `SpeechSessionClosed` with `state` `Open` moves it to `Closing`
    and spawns the close.
  - An old-epoch barge-in is dropped (R172): with `auto_interrupt` on, a pump started with
    `left: Some(r0)` is fed `PipelineSwitched`, `GenerationStarted(g)`, `SpeechStarted`, then
    `RunCompleted { run_id: r0 }`. No interrupt is requested, `g` is not in the cut set, the
    interruption count is unchanged and no `Interrupted` is yielded; a `SpeechStarted` fed after
    `GenerationStarted(h)` of the new epoch then requests one interrupt.
- The Agent's end (R169), a unit test in `agent.rs` of the same step, each case with `state`
  `Open` and a `Sender` with one live receiver:
  - The driver watch reports `Ok` and the pump is fed a current-epoch `RunFailed`, then `receive`
    returns `None`: the receiver gets `Error(Error::Layer)`, then `Closed`, then the channel's
    end; one close (`cancel`) is spawned; `state` is `Closed`.
  - The driver watch reports `Ok` with nothing fed, then `SessionClosed`: one close is spawned
    and `Closed` is yielded once, with no `Error`.
  - `SpeechSessionFailed`, then `RunFailed`, then `SessionClosed`: exactly one
    `Error(Error::Layer)`, one close spawned, `Closed` once.
  - With `state` `Closing`, a `RunFailed` then `SessionClosed`: no close spawned, `Closed` once.
  - The driver's `Err` arm (R185): the driver watch reports `Err(RunError::NodeFailed(..))` with
    nothing fed, then `receive` returns `None`: the receiver gets `Error(Error::Layer)` once, then
    `Closed` once, then the channel's end; one close (`cancel`) is spawned; `state` is `Closed`.
  No mock makes `drive()` return `Err` under the Agent: the node cannot fail its run itself, and
  the only `Err` path, a send refused after `SpeechSessionFailed` (story:sdk-duplex-node, "one
  frame of utterance 1"), is pre-empted by the Agent's cancel at that event (R170). So the `Err`
  arm is pinned here, at the pump step.
  A run failed by the egress watch, the ruling's case, is not reachable through the facade: the
  run's `RunRequests` are `pub(crate)` (`crates/inference-facade/src/live.rs:749`) and no facade
  code uses the egress watch. So it is pinned here, at the pump step.
  No test orders a model's own close inside a running switch without a hook in the node, so the
  race is pinned here, at the pump step.
- A native-to-native switch, end to end (R153, R157). Over `native_voice` and
  `MockSpeechModel::native(<two Say turns>)` built without `.closes()`, with `history` kept, and the
  builder's `.voice("verse")`: after utterance 0, once `history.completed()` is 1 (R177), so the
  switch's `Stop` finds no reply in progress to cancel (`ledger.rs:466-475`),
  `agent.switch(Recipe::native_voice())` returns `Ok`. The stream yields the old run's remaining
  events and no `Closed`. The test then sends utterance 1. The new session restarts the script
  (`mock.rs:306-319`), so the test asserts only presence: after the switch the stream yields a
  `Heard` and a `Said`, and no `Closed` up to that `Said`. `agent.describe()` returns a config whose voice is
  `verse` and whose `auto_interrupt` is the Agent's. `agent.close()` then returns `Ok(())`, and
  `inference::sessions().history(..)` holds the old run's `RunCompleted` before the new run's
  events.
- `agent.interrupt()` yields `Interrupted` (R134 (b), R152). Over `MockSpeechModel::native(<one Say
  turn>)` in `start()` mode, after the stream yields reply 0's `Said`, `agent.interrupt()` returns
  `Ok(())`. The test then calls `agent.close()` and reads the stream until `Closed`: over all
  events up to `Closed`, exactly one is `Interrupted`, after the `Said`, mapped from
  `SessionInterrupted` (`live.rs:914`).
- A failed response yields an error, not an interruption (R143). Over `MockSpeechModel::native(<one
  Say turn>).fails_responses().closes()` (story:sdk-duplex-node) in `start()` mode, with
  utterance 0 sent, the stream up to `Closed` holds exactly one `Event::Error`
  holding `Error::Provider` and no `Interrupted`.
- `close()` delivers every event received before it, then `Closed`, and nothing after.
- A model-initiated close through `start()` and the event stream, with no devices: over a stack
  binding `speech` to `MockSpeechModel::native(<one turn>).closes()`, `agent.start()`, sending
  utterance 0, then reading the stream until it ends yields the turn's events, then
  `Event::Closed` once, as the last event, with no `close()` call from the test.
- Closing is idempotent, both orders:
  - Model first, over the same mock: after `Event::Closed` from the model's close,
    `agent.close()` returns `Ok(())`, and the stream holds `Closed` once.
  - Application first (R164), over `MockSpeechModel::native(<one Say turn>)` built without
    `.closes()`, so the model never closes on its own and the only close is the application's:
    after utterance 0's `Said`, the test calls `agent.close()` twice, the second after the first
    returns. The drain's own model close is not emitted as `SpeechSessionClosed`
    (story:sdk-duplex-node). Both calls return `Ok(())`, and the stream holds `Closed` once.
- A failed model session closes the Agent (R158, R170; the driver's `Err` arm is the R169 unit
  test above, R185). Over
  `MockSpeechModel::native(<one Say turn>).with_faults(Faults { close_after: Some(3), .. })`
  (`mock.rs:93-98`, `:252`), whose transport closes after utterance 0's silence with the response
  created and in progress (`mock.rs:619-635`), so the model fails that response, then the
  session (`ledger.rs:490-497`), and the `DuplexNode` emits `SpeechSessionFailed` with no further
  input (R170, story:sdk-duplex-node): the test sends utterance 0 only and reads the stream under
  `tokio::time::timeout(5 s, ..)`. Its events hold exactly two `Event::Error`, in this order (R179):
  `Error::Provider`, for the failed response, then `Error::Layer`, for the failed session; then
  `Closed` as the last event, with no `close()` from the test. The Agent cancels the run, so the
  history ends it `RunCancelled`. The `sessions_view()` row is Closed, and `agent.close()` then
  returns `Ok(())`.
- `close()` from another task, with no lock to wait on (R95). While one task awaits
  `agent.run_on_devices_with(FakeHost::new(..))` over a duplex mock built without `.closes()`, a
  second task holding `&agent` (through an `Arc`) calls `agent.send_audio(..)` 200 times and then
  `agent.close()`. Each call returns `Ok`, the run returns `Ok(())` within 5 s, and `Closed` is
  the last event.
- One `Say` turn of the duplex mock, in `start()` mode, yields exactly six events (R139, R150), derived
  from the mapping table above and story:sdk-duplex-node's one-egress order: `Heard`, `Usage` (the transcription record),
  `Said`, `Audio`, `Audio` (`AUDIO_FRAMES` 2, `mock.rs:50`) and `Usage` (the response record,
  delivered at the response's end). The turn's `SpeechStarted` yields nothing (R134). Over
  `MockSpeechModel::native(<one Say turn>).closes()` with utterance 0 sent, the
  stream is `Started`, those six, `Closed`, and then `None`, checked with `assert_events!`'s
  matching rules (R70); the first `Usage` has no response id and the second has one.
- Lagged, with the exact count (R96, R117, R131, R139). In `start()` mode, so audio is on the
  channel, over `MockSpeechModel::native(<171 Say turns, turn i saying "reply i">).closes()`,
  with `let history = model.history();` kept before binding. The test sends utterance i once `history.completed()` is i, so no utterance arrives during a response (`mock.rs:16-19`).
  It reads the stream only after `agent.closed()` resolves (R149), so the pump has sent `Closed`
  and dropped the `Sender`. The run delivers
  exactly 1028 = 1024 + 4 events: `Started`, 6 per turn, `Closed`; so `k` is 4. The stream yields
  `Event::Lagged(4)` first. It then yields the last 1024 events in order: turn 0's `Audio`,
  `Audio`, `Usage`; for each turn i from 1 to 170, `Heard`, `Usage`, `Said { text: "reply i" }`,
  `Audio`, `Audio`, `Usage`; and then `Closed`. Each is compared by `assert_events!`'s matching
  rules (R70).
- Every stream ends after `Closed` (R113, R126, R138):
  - After `events()` yields `Closed`, the next poll returns `None`, also after a `switch`.
  - A second `Events`, taken before the session ended, also returns `None` after `Closed`.
  - The first `events()` call, made only after the session ended, yields the buffered events,
    the last of them `Closed`, and then `None` (R96).
  - A later `events()` call after the session ended returns `None` on its first poll.
- Reading and sending at once (R114, R133, R141). Over `MockSpeechModel::native(<two Say
  turns>)`, with `let history = model.history();` kept before binding, reply 1 starts only when
  the second utterance is sent, so the test gates it:
  1. One task reads the first `agent.events()`.
  2. A second task, holding `&agent` through an `Arc`, sends utterance 0; each `send_audio`
     returns `Ok`.
  3. The first stream yields reply 0's six events through its response `Usage`, the last of the
     turn.
  4. The test takes a second `events()`.
  5. Once `history.completed()` is 1, the second task sends utterance 1, so it
     cannot join reply 0's response (`mock.rs:448-461`).
  6. The second stream yields reply 1's `Heard`, `Usage`, `Said`, `Audio`, `Audio`, `Usage`, and
     no event of reply 0.
- `switch` keeps the confirmation state and refuses after close (R129), over `native_voice` with
  a `lookup` tool that requires confirmation:
  - After one `ConfirmationRequested` for `call_1`, still held, `agent.switch(Recipe::native_voice())`
    returns `Ok`. The stream yields no second `ConfirmationRequested` for `call_1`.
  - After `agent.close()`, `agent.switch(..)` returns `Err(Error::Closed)`.
  - Right after a model-initiated close (R137, R151): over `MockSpeechModel::native(<one Say
    turn>).closes()` in `start()` mode, the test sends utterance 0, so the turn runs and the mock, its script spent, closes the session. Once the
    session's `sessions_view()` row has left Ready (Draining or Closed),
    `agent.switch(Recipe::native_voice())` returns `Err(Error::Closed)`, the session is not
    switched, and the stream ends with `Closed` once.
  - A switch whose resolve fails after the abort leaves a working Agent (R145, R156). Over a
    stack binding only `speech`, to `MockSpeechModel::native(<three Say turns>).closes()`, with
    the builder's `.session_config(..)` setting `auto_interrupt` on and `let history =
    model.history();` kept, in `start()` mode: after utterance 0's `Said`,
    `agent.switch(Recipe::voice_cascade())` returns an `Err` whose `kind()` is
    `inference::ErrorKind::RoleUnbound` and whose `to_string()` contains `stt`, the first of the
    cascade's roles the stack leaves unbound (R210). A barge-in still
    cuts after the restart, so reply 1 is produced inside a device run (R174): the test takes
    `let hold = host.hold_sink();`, spawns `agent.run_on_devices_with(host.clone())`, and, once
    `history.completed()` is 1, feeds utterance 1. The stream yields reply 1's `Heard` and `Said`,
    and no `Closed` up to that `Said`. The test waits until `history.completed()` is 2 and
    `hold.waiting()` holds a frame that decodes to reply 1, feeds utterance 2's `speech()`, waits
    for `Interrupted` (the gate, R178, below), feeds its `silence()` and drops the hold. The run returns `Ok(())`;
    `FakeHost::played()` holds `Played::Cleared` and no frame of reply 1 after it, and the stream
    holds exactly one `Interrupted`.
- A model-initiated close while the egress is full (R111). Over
  `Agent::from_recipe(Recipe::native_voice().exposure_capacity(NonZeroUsize::MIN))` and a stack
  binding `speech` to `MockSpeechModel::native(<one Say turn>).closes()`, whose reply puts more
  than one event on the capacity-1 egress before the close (`AUDIO_FRAMES` 2 frames and the
  transcript, `mock.rs:50`), with utterance 0 sent: the close completes within 5 s, the stream ends with `Closed`, and
  `inference::sessions().history(agent.session().session_id())` (`live.rs:574`) holds
  `RunCompleted` before `SessionClosed`.
- Dropping an unclosed Agent (R115), a unit test in `agent.rs` under `cargo test -p inference
  --features testing --lib`: over a duplex mock built without `.closes()`, the test keeps the
  `session_id` and a `Weak` of the Agent's `Arc<Inner>`, then drops the Agent. Within 5 s the
  `sessions_view()` row with that `session_id` is Closed, and the `Weak` no longer upgrades, which
  holds only once the pump task, which owned the other clone, has ended.
- The rest of the surface, one test each. The tests run over the testkit mocks bound with
  `Stack::bind(Binding::<role>(<mock>))`, the seam `Stack::scripted()` binds through;
  story:sdk-testing depends on this story, so `Stack::scripted()` itself does not exist yet.
  - `send_audio`: over the cascade mocks, the frames of
    `inference_testkit::stt::synthetic_audio::tone_silence_tone_silence()` (`synthetic_audio.rs:114`)
    are sent with `send_audio`, then `commit()` (a manual turn). The stream holds a `Heard` with
    the recogniser's scripted text and a `Said`. A cascade's events come from several nodes on
    several egresses, and the speech-to-text node sends its transcript downstream before its
    exposed egress (`crates/inference-testkit/src/stt/node.rs:443-448`), so no order between them is
    promised (R161); the cascade cases check presence only.
  - `commit`: `send_text("a")`, `send_text("b")`, then `commit()` reach the generator as one user
    turn, `a b` (the session joins a turn's texts with a space, `crates/inference-facade/src/live.rs:33-34`).
  - `switch`: after `agent.switch(Recipe::voice_cascade())` over the cascade mocks, the session row
    with `agent.session().session_id()` still exists, its `pipeline_id` is the new
    `agent.pipeline()`'s, and the next utterance yields a `Heard` and a `Said`.
  - `Agent::text()`: over a stack binding only `llm`, `describe()` names `chat`, and `send_text`
    then `commit` yields `Said` with the generator's text.
  - `Agent::from_recipe(Recipe::voice_cascade())`: over the cascade mocks, `describe()` names
    `voice_cascade`, and an utterance yields a `Heard` and a `Said`, and, with no sink
    attached, `Audio` events whose frames decode to `MockId::Synthesiser` (R168).
- A unit test in `event.rs` matches every `Event` variant with no wildcard arm, so adding or
  removing a variant fails it. `rg -n -B1 'pub enum Event' crates/inference/src/event.rs` shows
  `#[non_exhaustive]`.
- Each escape method returns the type the design names, checked by a test that binds each to a
  typed `let` and calls one method of it:
  - `let s: &LiveSession<Payload> = agent.session();`
  - `let p: &PlannedPipeline = agent.pipeline();` (`crates/inference-pipeline/src/planning.rs:80`)
  - `describe()` returns a `Description` holding the output of
    `crates/inference-facade/src/describe.rs:64`.
  - `Event::raw()` returns the `SessionEvent` or duplex event.
- The builder's `describe()` over a stack binding `speech` to the duplex mock:
  - It returns the `SessionConfig` the agent then opens with; its `instructions` are the builder's
    and its `tools` hold the tool given.
  - It opens nothing. Over a `speech` adapter the test registers as `agent-describe-no-open`,
    whose `open` panics, the
    builder's `describe()` still returns the `SessionConfig`.
  - After `start()`, `agent.describe()` returns an equal `SessionConfig`.
  - Over the cascade mocks the `SessionConfig` is `None`.
- `agent.run_on_devices_with(FakeHost::new(..))`
  (`adapters/inference-audio-device/src/fake.rs:122`), over a stack binding `speech` to
  `MockSpeechModel::native(<script>).closes()`:
  - It plays the agent's audio: `FakeHost::played_frames()` holds the agent's reply frames.
  - It stops playback at a barge-in (R119, R130, R134, R144, R148). The script is two `Say` turns, and
    `.closes()` takes effect only once both are spent. The builder's `.session_config(..)` sets
    `auto_interrupt` on. The barge-in falls after reply 0 is complete, so the mock cancels nothing
    (it cancels only a response in progress, `mock.rs:549-553`), and the barge-in is the
    `SpeechStarted` with reply 0's audio pending at the sink; its `Interrupted` is mapped from the
    `SessionInterrupted` that the pump's spawned `interrupt()` records. The test keeps
    `let history = model.history();` before binding, then:
    1. `let hold = host.hold_sink();`, then the run is spawned.
    2. `host.feed(..)` feeds utterance 0.
    3. The test waits until `history.completed()` is 1, so reply 0 is complete, and until
       `hold.waiting()` holds at least one frame that decodes (`inference_testkit::mark::decode`)
       to reply 0, so reply 0's audio has reached the held sink and is pending.
       `played_frames()` cannot serve as this gate: a held write is not recorded (the hold, above).
    4. While the sink is still held, `host.feed(..)` gives utterance 1's `speech()`, the
       barge-in. The test keeps the hold until the stream yields `Interrupted`, then feeds
       utterance 1's `silence()`, which ends its turn (R163). The gate (R178): the session
       records `SessionInterrupted` only after the `Cancel` is on every ingress (the `interrupt`
       change above), so the silence lands behind it and the node handles the `Cancel` first,
       with no response in progress, before the turn's new response exists.
    5. `drop(hold)`.
    6. The run returns `Ok(())`. `FakeHost::played()` holds `Played::Cleared`. No `Played::Frame`
       after it decodes (`inference_testkit::mark::decode`) to reply 0, and reply 1's frames
       follow it. Over all events up to `Closed`, exactly one is `Interrupted` (R152).
  - A cut after every frame reached the speaker (R135). The same script, `auto_interrupt` on, no
    hold: after utterance 0, the test waits until `history.completed()` is 1 and
    `played_frames()` holds both frames of reply 0. They are still queued in the speaker, since no
    task pulls (`Speaker::queued()` above zero). `host.feed(..)` then gives utterance 1's `speech()`;
    the stream yields `Interrupted`, and the test then feeds its `silence()` (R163; the gate, R178,
    as above). After the run returns `Ok(())`, `played()` is reply 0's two
    frames, `Played::Cleared`, then reply 1's frames: nothing of reply 0 follows `Cleared`.
  - No cut with `auto_interrupt` off (R144, R173). The same script as the R135 case, with the
    builder's `.session_config(..)` setting `auto_interrupt` off: after utterance 0, once
    `history.completed()` is 1 and `played_frames()` holds both frames of reply 0, the test feeds
    utterance 1's `speech()` and then its `silence()` at once, with no wait between. After the run
    returns `Ok(())`,
    `played()` holds no `Played::Cleared`, its frames decode to reply 0's two frames then reply
    1's, and the stream up to `Closed` holds no `Interrupted`.
  - An application interrupt mid-reply cuts that reply (R155). The script is two `Say` turns, no
    barge-in: the test takes `let hold = host.hold_sink();`, spawns the run, feeds utterance 0,
    waits until `hold.waiting()` holds a frame that decodes to reply 0, calls `agent.interrupt()`,
    waits for `Interrupted`, drops the hold, then feeds utterance 1. The run returns `Ok(())`;
    `played()` holds `Played::Cleared`, and no `Played::Frame` after it decodes to reply 0.
  - A model-initiated close during input (R146). Over `MockSpeechModel::native(<one Say
    turn>).closes()`: the test takes `let hold = host.hold_sink();`, spawns the run, feeds
    utterance 0, and waits until the session's
    `sessions_view()` row has left Ready. It then feeds ten more frames, which the feeder's
    `send_audio` refuses with `Error::Closed`. `tokio::time::timeout(1 s, &mut run)` elapses,
    because the held sink keeps the drain open. After `drop(hold)`, the run returns `Ok(())`.
  - It returns `Ok(())` once the script is spent and the mock closed the session; no test task
    calls `pull`.
  - The run consumed nothing from the stream (R96). Read after the run returns, `events()` yields
    the run's non-audio events and ends with exactly one `Event::Closed`. It yields no `Audio`,
    because the sink was attached before the first input frame and detached only after the drain
    (R117, R127).
  - `agent.describe()` is callable on the same `agent` afterwards.
- A cascade reply plays through the sink slot (R168). Over `Agent::from_recipe(Recipe::voice_cascade())`
  and the cascade mocks, whose synthesiser emits marked frames (`MockId::Synthesiser`), and no
  task pulls: the test takes `let hold = host.hold_sink();`, spawns
  `agent.run_on_devices_with(host.clone())`, and feeds the frames of
  `inference_testkit::stt::synthetic_audio::synthesize(&FIXTURE[..2])` (`synthetic_audio.rs`),
  one turn, whose pause ends it at the VAD (R183). Once `hold.waiting()` holds a frame that
  decodes to `MockId::Synthesiser`, a second task holding `&agent` through an `Arc` calls
  `agent.close()`. `tokio::time::timeout(1 s, &mut run)` elapses, because the cascade sink waits
  for `played()` and the drain for the held frames. After `drop(hold)`, the run and `close()`
  return `Ok(())`, `FakeHost::played_frames()`, decoded, holds every frame of the reply of
  `MockId::Synthesiser` in order, and the stream holds no `Audio`. It runs under
  `#[tokio::test(flavor = "multi_thread")]`.
- A second turn cuts a cascade reply (R181, R189). Over the same setup with `auto_interrupt` on,
  the stack binding `llm` to a `MockGenerator` (`crates/inference-testkit/src/llm.rs:189`) whose
  first reply is `one two three four`, so the mock synthesiser yields four frames for reply 0, one
  per word (`crates/inference-testkit/src/tts.rs:275`). The test feeds
  `synthesize(&FIXTURE[..2])`, waits until `hold.waiting()` holds a frame that decodes to
  `MockId::Synthesiser`, feeds `synthesize(&FIXTURE[2..3])`, drops the hold and closes.
  `FakeHost::played()` holds `Played::Cleared`; fewer than four frames of
  `FakeHost::played_frames()` decode to reply 0 of `MockId::Synthesiser`; and the stream yields
  `Interrupted` exactly once. With `auto_interrupt` off (`Agent::builder` with a session
  config whose `auto_interrupt` is false), the same feed cuts nothing: `played_frames()`, decoded,
  holds all four frames of reply 0 in order, `played()` holds no `Played::Cleared` before them,
  and the stream yields no `Interrupted`.
- Every frame of the last reply is handed to the sink before the run returns (R105, R118). Reply
  indices are 0-based, so of two replies the last is reply 1. The stack binds `speech` to
  `MockSpeechModel::native(<two Say turns>).closes()`, the mock closing right after reply 1, with
  `let history = model.history();` kept before binding, and no task pulls:
  - The test takes `let hold = host.hold_sink();`, spawns `agent.run_on_devices_with(host.clone())`,
    feeds utterance 0, and, once `history.completed()` is 1, utterance 1 (R175). It reads
    `events()` until `Closed`, so reply 1 has been produced.
  - The run is then still pending: `tokio::time::timeout(1 s, &mut run)` elapses.
  - After `drop(hold)`, the run returns `Ok(())`, and `FakeHost::played_frames()`, decoded with
    `inference_testkit::mark::decode`, holds every frame index of reply 1 of `MockId::Speech`, in
    order.
  - Without the drain, `run_until` returns once the pump has seen `SessionClosed`, while the sink
    is held, so the pending check fails on every run. The implementor runs the test once with the
    drain removed, records the failure in the story's evidence, and restores the drain.
  - The test runs twice (R128): under `#[tokio::test]`, the current-thread flavour, and under
    `#[tokio::test(flavor = "multi_thread")]`. On the current-thread runtime the timeout can fire
    only because the drain wait awaits a `oneshot` and never blocks the runtime's one thread.
- A unit test in `src/devices.rs` calls `run_until(FakeHost::new(..), rx)` over a duplex mock built
  without `.closes()`, whose session therefore stays open, with `rx` a
  `tokio::sync::oneshot::Receiver`. Sending on its sender makes `run_until` return `Ok(())` and the
  session closed. It passes under `cargo test -p inference --features testing --lib`.
- A stop fired mid-reply drains first (R136), a unit test in `src/devices.rs` under the same
  command, over `MockSpeechModel::native(<two Say turns>)` built without `.closes()`, with
  `let history = model.history();` kept: the test takes `let hold = host.hold_sink();`, spawns
  `run_until(host.clone(), rx)`, feeds utterance 0, waits until `history.completed()` is 1, so
  both of reply 0's frames reach the pump before `SessionClosed`, and sends on the stop sender.
  `tokio::time::timeout(1 s, &mut run)` elapses while the sink is held. After `drop(hold)`, the
  run returns `Ok(())`; `played_frames()`, decoded, holds reply 0's frame indices 0 and 1 in order,
  read when it returns and again 500 ms later with no change; and `events()` holds no `Audio` and
  ends with one `Closed`.
- `rg -n 'assert_send::<Agent>\(\)' crates/inference/src/agent.rs` and
  `rg -n 'assert_sync::<Agent>\(\)' crates/inference/src/agent.rs` match, and
  `cargo check -p inference --all-features` exits 0 with them.
  `rg -n 'inner: Arc<Inner>' crates/inference/src/agent.rs` and
  `rg -n 'session: LiveSession<Payload>' crates/inference/src/agent.rs` match, and
  `rg -n 'Mutex<LiveSession' crates/inference/src/agent.rs` exits 1.
- `inference::sessions()`: after starting an `Agent` over the cascade mocks,
  `inference::sessions().sessions_view()` (`live.rs:561`) holds a row whose `session_id`
  (`crates/inference-facade/src/session.rs:473`) is `agent.session().session_id()` (`live.rs:758`).
  The test asserts that row, not a row count, so sessions other tests open in parallel do not
  matter.
- `cargo test -p inference --features testing --test agent` passes.

In `crates/inference-facade/tests/shared_session.rs`, passing in
`cargo test -p inference-facade --test shared_session` (R95):

- A session is opened over a pipeline whose ingress has capacity 1 under `OverloadPolicy::Pause`
  (`crates/inference-runtime/src/queue.rs:11-12`), wrapped in `Arc<LiveSession<_>>`.
- One task sends 100 inputs with `send`, faster than the run consumes. Another task loops on
  `receive()` on the same `Arc`.
- Within 5 s, every input is received, and `close()` from the sending task returns `Ok(())`, after
  which `receive()` answers `None`. The test cannot be written against the `&mut self` shape:
  with that shape, the second task cannot call `receive` through an `Arc`.

`crates/inference-facade/tests/`, `crates/inference-testkit` and `adapters/inference-audio-device`:
- `cargo test -p inference-facade --all-features` passes.
- `cargo test -p inference-testkit --all-features` passes with the marked reply audio.
- `cargo test -p inference-audio-device --all-features` passes with `played()`.
- A unit test in `mark.rs` round-trips `encode` and `decode` for both mocks, and `decode` of a
  frame of silence is `None`.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` exits 0.

Its diff lists none of `crates/inference/src/lib.rs`, `crates/inference/src/prelude.rs` and
`crates/inference/Cargo.toml` (story:sdk-crate wrote them once, decision D9 of `api-design:sdk`).

## Out of scope

`Chat`, `Transcriber`, `Voice` and the one-call functions (story:sdk-one-call); `usage()` and
`cost()` (story:sdk-usage-cost); `FakeClock` and the scripted stack (story:sdk-testing).
`Chat`, `Transcriber` and `Voice` carry their own `Send` assertions (story:sdk-one-call).
