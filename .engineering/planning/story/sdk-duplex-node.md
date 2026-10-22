---
format: aep.planning-md/3
id: story:sdk-duplex-node
kind: story
status: draft
title: A native speech model runs as a graph node inside a session
relations:
- decomposes: epic:sdk
- serves: vision:inference
- depends_on: story:sdk-model-nodes
scope:
- confidence: cited
  path: crates/inference-facade/Cargo.toml
- confidence: cited
  path: crates/inference-facade/src/events.rs
- confidence: inferred
  path: crates/inference-pipeline/Cargo.toml
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/duplex.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/mod.rs
- confidence: inferred
  path: crates/inference-testkit/src/duplex/mock.rs
- confidence: inferred
  path: crates/inference-testkit/tests/duplex_node.rs
revision: 18
---
## Context

G2 of `docs/design/sdk.md` §1: no `NodeImplementation` wraps a `NativeSpeechModel`
(`crates/inference-duplex/src/contract.rs:338`), so a native speech model never runs under the
runtime or a session; `examples/realtime-voice/src/lib.rs:254-605` drives the handle in its own
loop. Without this node the SDK's `Agent::voice()` has two code paths, one of them outside the
session. Serves O2 and O5.

Ordered after story:sdk-model-nodes because both edit `crates/inference-pipeline/src/nodes/mod.rs`
and `crates/inference-pipeline/Cargo.toml`, and because this node emits the `StreamEvent` that story
moves into `crates/inference-pipeline/src/nodes/log.rs`.

## Domain relations

No new noun and no specification change. `ess/domains/duplex.yaml` declares
`inference.duplex.SpeechSession` (`:127`) with no relation to `inference.graph.NodeInstance` and
no UNMAPPED marker for one; the domain says "a session is opened over an adapter the caller already
holds" (`:124-126`), and the design keeps the contracts as they are (`docs/design/sdk.md` §1). The
node is such a caller.

## Planned scope

- `crates/inference-pipeline/src/nodes/duplex.rs` (new): `DuplexNode<M: NativeSpeechModel>` and the
  payload trait it needs, `DuplexPayload`, which `inference::Payload` implements in
  story:sdk-payload. Its constructors `speech_started`, `closed` and `response_failed` (below) are
  required trait methods with no default, as are the stream, audio and usage ones. The node's ports
  are audio in, control in (interrupt), the two tool ports below and one output, `events`
  (R142). `events` is `Essentiality::Essential` and `tool_call` is `Essentiality::Optional`
  (`crates/inference-pipeline/src/lib.rs:167-173`), so a pipeline of the node alone, with audio in
  and control in as ingresses and `events` as its egress, passes check 6
  (`crates/inference-pipeline/src/validation.rs:276-327`: no unresolved essential output and no
  `Sink` node; R159). Every payload the application reads leaves on `events`, in the order the node emits
  it, so that order is the delivery order:
  - An exposure names one port (`crates/inference-pipeline/src/lib.rs:999-1006`), the node is
    handed one endpoint per exposure (`crates/inference-pipeline/src/node.rs:80-84`), and an
    exposure's queue gives envelopes in arrival order and loses none under `Pause`
    (`crates/inference-runtime/src/queue.rs:8-12`). The node awaits each send before the next.
  - Separate output ports would each be their own egress, and the session takes the first ready
    egress in index order (`crates/inference-facade/src/live.rs:976-993`), so delivery would not
    follow emission. No fan-in can restore that order: `FanIn` is a role with no node
    implementation (`lib.rs:136`), and a fan-in would read one queue per incoming edge.
  - What the node emits on `events` for each model event, each mapped by `SessionPayload` to the
    `SessionEvent` named:

    | model event (`crates/inference-duplex/src/lib.rs`) | `SessionEvent` |
    |---|---|
    | `SpeechStarted` | `SpeechStarted` (below) |
    | `InputTranscribed` | `TranscriptCommitted` of the basis span below |
    | `UsageRecorded` (`inference.usage`) | `UsageRecorded` |
    | `ResponseCreated` | `GenerationStarted` of the response's generation |
    | `OutputTranscriptDelta` | `TextDelta`, then `TextSegmentCommitted` of a segment holding that text, both on the response's generation |
    | `OutputAudio` | `AudioChunk` |
    | `ResponseCompleted` | `GenerationCompleted` |
    | `ResponseCancelled` | `GenerationCancelled` |
    | `ResponseFailed` | `SpeechResponseFailed` (below), then `GenerationCancelled` |
    | `SpeechToolCallProposed` | `ToolCallProposed`, sent on `events` before the same update leaves on `tool_call` |
    | `SpeechSessionClosed` | `SpeechSessionClosed` (below) |
    | `SpeechSessionFailed` (`crates/inference-duplex/src/lib.rs:478-483`) | `SpeechSessionFailed` (R170, below) |
    | `SpeechStopped`, `InputCommitted`, `ResponseUsageRecorded`, `TranscriptionUsageRecorded`, `OutputTruncated`, `SpeechToolCallAnswered` | nothing |

  A failed model session (`SpeechSessionFailed`, R158, R170). The node polls the model's
  `next_event` in its own loop whether or not input arrives, so it sees the failure as soon as the
  model yields it, with no further frame. It emits `DuplexPayload::failed(SpeechSessionFailed) ->
  Self` once on its `events` port, then nothing more, ends its exchange (its input receivers go
  with it) and answers the run's `drain` with a `NodeError` naming the `SessionFailure`.
  story:sdk-agent maps the event to `Error::Layer` and cancels the run, so the session ends it
  `RunCancelled`. The node cannot fail its run itself: the run loop wakes on a node only when a
  send to it is refused as closed (`crates/inference-runtime/src/drive.rs:19-25`, `:426-428`),
  and `NodeImplementation` has no fail path (`crates/inference-pipeline/src/node.rs:100-143`).
  Without that cancel, the next envelope sent into its audio ingress is refused as closed, the
  loop takes the drain, and the error fails the node and the run (`controller.rs:1001-1040`): the
  session delivers `RunFailed` and `SessionDriver::drive` returns
  `Err(RunError::NodeFailed(..))`.

  `DuplexPayload: ToolPayload`
  (`crates/inference-pipeline/src/nodes/mod.rs:98`), and the node has two tool ports shaped as the
  runtime's tool nodes expect (`crates/inference-runtime/tests/tool_feedback_loop.rs:233-237`):
  - `tool_call` (out, Structured). Each `SpeechToolCallProposed` the model session yields
    (`crates/inference-duplex/src/lib.rs:458`) leaves as `ToolCallUpdate { call, event: Proposed }`
    (`nodes/mod.rs:57`, `:91`):
    - `call` is a `ToolCall` from `ToolCall::propose(tool, &generation, &basis, arguments)`
      (`crates/inference-tools/src/call.rs:125`), with a fresh `ToolCallId`. The generation and
      the basis come from the source below.
    - The node is built with the tools it may propose, `DuplexNode::with_tools(Vec<Tool>)`. A
      proposal naming a tool outside that set never leaves the node; it is answered to the model at
      once as denied, as the testkit's LLM node does (`crates/inference-testkit/src/llm/node.rs:892-897`).
    - A `ToolCall` carries no provider id (`call.rs:107`), so the node keeps a map from each
      `ToolCallId` it mints to the model's `SpeechCallId` (`lib.rs:77`).
    - `DuplexNode::call_ids() -> CallIds` returns a cloneable read handle on that map:
      `CallIds::speech_call_id(ToolCallId) -> Option<SpeechCallId>`. story:sdk-recipes passes it to
      story:sdk-agent, whose `Event::ToolCalled` carries the provider's id (R102).
  - `tool_result` (in, Structured). A `ToolCallUpdate` whose event is `Result`, `Failed` or
    `Rejected` is looked up in that map and answered to the model by its `SpeechCallId`
    (`submit_tool_result`, the model then yielding `SpeechToolCallAnswered`, `lib.rs:467`). A
    failure or rejection is answered as an error result:
    - The entry is then marked answered, not removed, so a late lookup through `CallIds` still
      finds it.
    - An update for an id not in the map, for an entry already answered, or with another event,
      is dropped.
  story:sdk-recipes wires the two ports to `ToolAuthority` and `ToolExecutorNode`.
- The generation and the basis span of a proposed call (R103). `ToolCall::propose` refuses a
  generation that is not Active (`StaleGeneration`) and a basis that is not Committed (`Barred`,
  `call.rs:144-160`). The node keeps both as the testkit's LLM node does
  (`crates/inference-testkit/src/llm/node.rs:899-903`, `:1002-1012`):
  - Generation, one per model response:
    - `Generation::start(StartGeneration { run_id, node_id, epoch })`
      (`crates/inference-core/src/generation.rs:136`, `:46`) on `ResponseCreated`, `epoch`
      counting the session's responses from 1.
    - `complete` (`:183`) on `ResponseCompleted`; `cancel` (`:213`) on `ResponseCancelled` or
      `ResponseFailed`.
    - A proposal names the response it belongs to (`SpeechToolCallProposed::response_id`) and
      takes that response's generation.
  - Turn: one per user turn, `Turn::open(OpenTurn { run_id })` (`crates/inference-text/src/inbound.rs:229`)
    at the session's first input and again after each `InputCommitted`.
  - Basis span:
    - `InputTranscribed` emits a span on the open turn with
      `Span::emit_as(span_id, &turn, run_id, stream_id, sequence, text)` (`inbound.rs:329`) and
      commits it at once (`Span::commit`, `:392`); that span becomes the basis.
    - At `InputCommitted` with no transcript yet, the node emits and commits an empty span, so a
      proposal on an untranscribed turn still has a committed basis.
- `crates/inference-pipeline/src/nodes/mod.rs`: `pub mod duplex`.
- `crates/inference-pipeline/Cargo.toml`: `inference-duplex` as a normal dependency;
  `inference-duplex` does not depend on the pipeline (`crates/inference-duplex/Cargo.toml`).
- `crates/inference-testkit/tests/duplex_node.rs` (new), gated in the file by
  `#![cfg(all(feature = "run", feature = "duplex"))]` so the testkit's `Cargo.toml` is not edited,
  with a test-local payload implementing `DuplexPayload` and `SessionPayload`
  (`crates/inference-facade/src/events.rs:124`), over the duplex mock
  (`crates/inference-testkit/src/duplex/mock.rs`).
- The closed notification. A node has no way to end its run: the runtime drives `prepare`, `start`,
  `drain`, `cancel` and `stop` (`crates/inference-pipeline/src/node.rs:100-143`), and a run ends only
  through halt, drain or a gone consumer (`crates/inference-runtime/src/controller.rs:951-972`). So
  the node reports the close and the session ends the run. When the model session yields
  `SpeechSessionClosed` (`crates/inference-duplex/src/lib.rs:472-476`), `DuplexNode` emits
  `DuplexPayload::closed(SpeechSessionClosed) -> Self` once on its `events` port, then emits
  nothing more. After the close it drops every frame that arrives on its audio input and every
  tool result, without appending them to the closed model session and without failing, so a
  microphone that keeps feeding until the session stops does not fail the run. The live session delivers only what `SessionPayload::session_event` maps to a
  `SessionEvent` (`crates/inference-facade/src/live.rs:1017`), and no variant fits
  (`crates/inference-facade/src/events.rs:48`), so this story adds one:
  - `crates/inference-facade/src/events.rs`: `SessionEvent::SpeechSessionClosed(inference_duplex::SpeechSessionClosed)`,
    documented as `inference.duplex.SpeechSessionClosed` (an event `ess/domains/duplex.yaml`
    already declares), and a row for it in the module table. No match over `SessionEvent` in the
    workspace lacks a wildcard arm, so nothing else changes.
  - The same file gains `SessionEvent::SpeechStarted(inference_duplex::SpeechStarted)`
    (`crates/inference-duplex/src/lib.rs:372-377`), documented as `inference.duplex.SpeechStarted`,
    with a row in the module table (R134). The node emits `DuplexPayload::speech_started(SpeechStarted)
    -> Self` on its `events` port for each `SpeechStarted` the model session yields, so the
    agent sees the user start speaking; no session variant carried it before.
  - The same file gains `SessionEvent::SpeechResponseFailed(inference_duplex::ResponseFailed)`
    (`crates/inference-duplex/src/lib.rs:434-439`), documented as `inference.duplex.ResponseFailed`,
    with a row in the module table (R143). The node emits `DuplexPayload::response_failed(ResponseFailed)
    -> Self` for each `ResponseFailed`, before the `GenerationCancelled` that cancelling the
    generation emits. Both a cancelled and a failed response cancel the generation, so this is
    the only event that tells them apart.
  - The same file gains `SessionEvent::SpeechSessionFailed(inference_duplex::SpeechSessionFailed)`
    (`crates/inference-duplex/src/lib.rs:478-483`), documented as
    `inference.duplex.SpeechSessionFailed`, with a row in the module table (R170). The node emits
    it through `DuplexPayload::failed(SpeechSessionFailed) -> Self` (above).
  - Usage (R139). The ledger records each usage twice: as the duplex's own
    `ResponseUsageRecorded` or `TranscriptionUsageRecorded`, and as an `inference.usage` record,
    `SessionEvent::UsageRecorded` (`push_record`, `crates/inference-duplex/src/ledger.rs:965-969`).
    The node forwards every `UsageRecorded` as `Payload::Usage`, and neither duplex-specific event,
    because they carry the same counts and forwarding both would count them twice in `usage()`.
    The duplex mock produces two per turn:
    - the transcription record, naming no response, when the turn ends (`mock.rs:433-440`, through
      `transcription_usage`, `ledger.rs:616-622`, to `record_of_session`, `:838`);
    - the response record, naming the response, at the response's end: its `Usage` step only
      settles the counts (`count`, `ledger.rs:884-891`), and `Complete` (`mock.rs:512`) records
      them just before `ResponseCompleted` (`ended`, `:894-899`; `record_response`, `:925-943`).
    So a mock `Say` turn reaches the session as the transcript, the transcription `UsageRecorded`,
    the reply transcript, `AUDIO_FRAMES` frames and the response `UsageRecorded`, in that order,
    and the response `UsageRecorded` is delivered only once that response is complete. The order
    holds because all of them leave on `events` (above).
  - `crates/inference-facade/Cargo.toml`: `inference-duplex` as a normal dependency;
    `inference-duplex` depends on `inference-audio`, `inference-core` and `inference-tools` only, so
    there is no cycle.
  - The test-local payload of `duplex_node.rs` maps its closed variant to `SpeechSessionClosed`,
    its speech-started variant to `SpeechStarted`, its response-failed variant to
    `SpeechResponseFailed` (R147) and its failed variant to `SpeechSessionFailed` (R170); `inference::Payload` does the same in story:sdk-payload, and story:sdk-agent closes the live
    session on it (`LiveSession::close`, `live.rs:881`). The run then drains and the session
    delivers `SessionClosed`.
- `crates/inference-testkit/src/duplex/mock.rs`: `MockSpeechModel::closes(self) -> Self`, a builder beside
  `with_faults` (`:252`). A session opened from a model built with it closes once its script is
  spent. When `next_event` (`:619`) finds all six of these, it runs `self.ledger.close()`
  (`crates/inference-duplex/src/ledger.rs:468`), which queues `SessionEvent::SpeechSessionClosed`
  (`:477`):
  - `script` is empty;
  - `plan` is empty;
  - `ledger.in_progress()` is `None` (`ledger.rs:275`);
  - `pending` is `None`, so no reply waits;
  - `after_result` is `None`, so no tool result is awaited (`mock.rs:597-611`);
  - `speaking` is false, so the user is not mid-utterance.
  Every later `next_event` answers `None`, because the session is no longer open (`mock.rs:623`).
  `end_turn` on a spent script stays as it is (`:427-429`), and a model built without `.closes()`
  behaves exactly as today, so the existing tests of the mock are untouched. story:sdk-stack does
  not touch this file.
- In the same `mock.rs`: `MockSpeechModel::fails_responses(self) -> Self`, a builder beside `closes`
  (R143). A session opened from a model built with it ends each response, in place of its
  `Complete` step, with `ledger.response_failed(response_id, SessionFailure { kind:
  SessionFailureKind::ProviderError, detail: "mock failure" }, now)`
  (`crates/inference-duplex/src/ledger.rs:739-749`, `lib.rs:139-166`). A model built without it
  behaves as today.
- In the same `mock.rs`, the one utterance every SDK test feeds (R162):
  `inference_testkit::duplex::mock::utterance(format: AudioFormat, at: u64) -> Utterance`, with
  `speech() -> Vec<AudioFrame>` (two 20 ms frames of byte `0x01` from sample `at`, built with
  `frame`, `:138`), `silence() -> AudioFrame` (one frame of `format.encoding.silence()` after
  them), `frames()` (speech, then silence) and `next() -> u64` (the sample after its last frame,
  where the next utterance starts). The turn ends by ServerVad silence with no `commit()`: the
  mock's registration and `MockSpeechModel::native` default to ServerVad (story:sdk-stack), and
  the silence frame after speech commits the turn and runs `end_turn` (`append_audio`,
  `mock.rs:555-569`). Byte `0x01` is not an ASCII graphic, so the recogniser reads no words
  (`:415-420`) and the turn is heard as the script's `heard`. No SDK test calls `commit()` on top
  of the silence: `commit_input` (`:573-576`) would end a second turn.
- How the node handles the runtime's controls and lifecycle (`crates/inference-pipeline/src/node.rs:100-143`,
  `ControlKind`, `crates/inference-core/src/envelope.rs:64-77`; R160). The node handles every
  input envelope already queued before its next `next_event`, so a control sent behind a frame
  acts before the model's next step:
  - A `Cancel` control on any input (`LiveSession::interrupt` sends one into every ingress,
    `crates/inference-facade/src/live.rs:909-930`): with a response in progress, the node calls
    `cancel_response` (`crates/inference-duplex/src/contract.rs:395`) on it, at most once per
    response, and drops the call's error; with none, it ignores the `Cancel`. The model's
    `ResponseCancelled` then leaves as `GenerationCancelled` (table above).
  - Drain, on a `Stop` control (`LiveSession::close`, `live.rs:881`) or the runtime's `drain`:
    the node admits no new audio and no tool result, calls the model's `close()`
    (`contract.rs:427`), which records every response not yet recorded and cancels the one in
    progress (`crates/inference-duplex/src/ledger.rs:460-480`), and forwards on `events` every
    event `next_event` yields until the `SpeechSessionClosed` that `close()` queued. That close is
    the application's own and is not emitted as `DuplexPayload::closed`; the session reports it
    as `SessionClosed`. The node then ends `events`. A model already closed (the closed
    notification below) answers `close()` with `NotOpen`, and the node ends `events` at once.
  - `stop` releases the model session handle. When no drain ran (a `cancel` of the run), it calls
    `close()` first, drops its result and forwards nothing.

## Acceptance

In `crates/inference-testkit/src/duplex/mock.rs`'s own `mod tests`, passing in
`cargo test -p inference-testkit --features duplex --lib duplex::mock`:

- A session of `MockSpeechModel::native(vec![<one turn>]).closes()`, driven through that turn, yields
  the turn's events, then `SpeechSessionClosed`, then `None` from `next_event` on every later call,
  and its state is `SpeechSessionState::Closed`.
- The same script without `.closes()`, driven the same way and then polled with `next_event` past the
  turn, yields no `SpeechSessionClosed` and stays `Open`.
- A `Reply::Call` turn as the last turn, with `.closes()`: after the call's response completes, polling
  `next_event` yields no `SpeechSessionClosed` and the state stays `Open`, because `after_result` is
  set. After `submit_tool_result`, the reply's response streams and completes, and only then does
  `SpeechSessionClosed` follow.
- A barge-in at the last turn, with `.closes()` and `auto_interrupt` on: a speech frame appended while
  the last reply plays cancels that response; polling `next_event` while the user is speaking yields
  no `SpeechSessionClosed`, and the state stays `Open`.
- A session of `MockSpeechModel::native(vec![<one Say turn>]).fails_responses()`, driven through
  that turn, yields `ResponseFailed` with kind `ProviderError` for its response, and no
  `ResponseCompleted`.

Run by `cargo test -p inference-testkit --features run,duplex --test duplex_node`:

The test pipeline is `DuplexNode` alone over the duplex mock, with audio in and control in as its
ingresses and `events` as its egress (R159); the tool-port cases also expose `tool_call` and
`tool_result`. Every case feeds `utterance(..).frames()` with no `commit()` (R162).

- The test pipeline's `declaration.validate` reports no finding.
- The test pipeline, opened as a live session, delivers the mock's transcript, response audio and `UsageRecorded` as session events. Over one
  `Say` turn it delivers exactly two `UsageRecorded`, one naming the response and one naming
  none, and no event for `ResponseUsageRecorded` or `TranscriptionUsageRecorded` (R139).
- The same pipeline delivers `SessionEvent::SpeechStarted` once for each user turn's first speech
  frame, before that turn's transcript (R134). Both leave on `events`, so the order is the node's
  emission order.
- One egress (R142): the pipeline exposes exactly one egress, the node's `events` port, and one
  `Say` turn reaches the session as `SpeechStarted`, `TranscriptCommitted`, `UsageRecorded`,
  `GenerationStarted`, `TextDelta`, `TextSegmentCommitted`, `AudioChunk`, `AudioChunk`,
  `UsageRecorded`, `GenerationCompleted`, in that order.
- A failed response (R143): over `MockSpeechModel::native(<one Say turn>).fails_responses()`, the
  response reaches the session as `SpeechResponseFailed` naming that response, then
  `GenerationCancelled` of its generation, and no `GenerationCompleted`.
- The mock scripted as `inference.duplex/authored/barge-in-cancels-response`
  (`ess/scenarios/barge-in-cancels-response.yaml:74-86`: `ObserveSpeechStarted`, `CancelResponse`,
  then `TruncateOutput` yielding `inference.duplex.OutputTruncated`) reaches the session as one
  `GenerationCancelled` naming the interrupted response's generation.
- The mock scripted as `inference.duplex/authored/input-during-output-continues-response`
  (`ess/scenarios/input-during-output-continues-response.yaml`: a second `EmitOutputTranscript` on
  the same response yields `OutputTranscriptDelta` " Berlin is mild") reaches the session with no
  `GenerationSuperseded` and no `GenerationCancelled`, and the second text arrives on the same
  generation as the first.
- Closing the session (`LiveSession::close`, `crates/inference-facade/src/live.rs:881`) delivers
  every event the binding received before the close, then `SessionClosed`.
- The in-progress gate of the two cases below (R176): a test-local model `HoldAfterCreated` wraps
  the duplex mock. Its session passes `append_audio`, `cancel_response` and `close` through, and
  once the inner session has yielded `ResponseCreated` its `next_event` returns `None` after
  `wait`, without calling the inner one, until the test opens an `Arc<AtomicBool>` gate. So the
  response stays in progress, with no output, while the test acts. Both cases run under
  `#[tokio::test(flavor = "multi_thread")]`.
- `Cancel` (R160, R176): the test pipeline opened as a run over `HoldAfterCreated`, the test
  sending utterance 1's frames, then, once the `events` egress has carried that turn's
  `GenerationStarted`, a `Cancel` control on the audio ingress and a second on the control
  ingress, then opening the gate. The egress then carries exactly one `GenerationCancelled` of
  that generation, and no `AudioChunk` or `GenerationCompleted` of it. A
  `Cancel` sent with no response in progress emits nothing, and utterance 2's reply follows in
  full.
- Drain (R160, R176): over `HoldAfterCreated` and a `Say` turn, once the `events` egress has
  carried the turn's `GenerationStarted`, the test spawns `close()` and opens the gate; `close()`
  delivers that reply's
  `GenerationCancelled` and its response `UsageRecorded` before `SessionClosed`, no
  `SpeechSessionClosed`, and nothing after `SessionClosed`.
- `stop` (R160): cancelling the run mid-reply (`LiveSession::cancel`) ends it `RunCancelled`. A
  test-local model wrapping the mock records that its session saw `close()` once and was dropped.
- The closed notification: over `MockSpeechModel::native(<one turn>).closes()`, the session delivers
  the turn's events, then `SessionEvent::SpeechSessionClosed` exactly once, unprompted by the test.
  On that event the test calls `close()`; the run completes (`RunCompleted`), `SessionClosed`
  follows, and no event follows `SessionClosed`.
- Audio after the close: the same pipeline, with the test sending ten more audio frames into the
  audio ingress after `SpeechSessionClosed` and before calling `close()`. The run ends `RunCompleted`, and
  the session delivers no `RunFailed`.
- A failed model session (R158, R170): over `MockSpeechModel::native(<one Say turn>).with_faults(Faults {
  close_after: Some(3), .. })` (`crates/inference-testkit/src/duplex/mock.rs:93-98`, `:252`), the
  test feeds utterance 0 only. Its transport closes after the turn's silence, with the response
  created and in progress (`mock.rs:619-635`), and the ledger fails that response first
  (`crates/inference-duplex/src/ledger.rs:490-497`). Read under `tokio::time::timeout(5 s, ..)`,
  the session delivers `SpeechResponseFailed`, then `SessionEvent::SpeechSessionFailed`, and no
  `SpeechSessionClosed`, with no further frame sent. Two endings, one test each:
  - The test then calls `cancel()`: the run ends `RunCancelled`.
  - The test instead sends one frame of utterance 1, which is refused: the session delivers one
    `RunFailed`, and the driver's `drive()` returns `Err(RunError::NodeFailed(..))` naming the
    `DuplexNode`.
- Tool ports: over `MockSpeechModel::native(vec![<a Reply::Call turn>])`, the node's `tool_call`
  output carries one `ToolCallUpdate` with event `Proposed` and the call's tool and arguments; the
  test sends a `ToolCallUpdate` with event `Result` for that `ToolCallId` into `tool_result`, and
  the mock receives `submit_tool_result` with the `SpeechCallId` it proposed, then streams the
  reply. An update for an unknown `ToolCallId` is dropped and the mock receives nothing.
  `call_ids().speech_call_id(<that ToolCallId>)` is `Some(SpeechCallId("call_1"))` before the
  result and still after it. A second `Result` for the same id is dropped.
- Generation and basis (R103): over the same mock, scripted with an input transcript before the
  call, the proposed `ToolCall`:
  - Its `generation_id` is the id of the generation the node started at that response's
    `ResponseCreated`, and that generation is Completed after `ResponseCompleted`.
  - Its `basis_span_id` is the span the node emitted for `InputTranscribed`, and that span is
    Committed.
  - Scripted with no transcript, the call's basis is the empty committed span emitted at
    `InputCommitted`.
- A tool outside `with_tools`: the mock proposing `"unknown"` receives an error result under its
  call id at once, and the `tool_call` port carries nothing.

Its diff lists none of `crates/inference/src/lib.rs`, `crates/inference/src/prelude.rs` and
`crates/inference/Cargo.toml` (story:sdk-crate wrote them once, decision D9 of `api-design:sdk`).

## Out of scope

Hosted-adapter live tests; tool execution inside the node (tool calls leave on the `tool_call`
port, and story:sdk-recipes wires the runtime's tool nodes to it); any edit of
`ess/domains/duplex.yaml`; ending the run from inside the node (the runtime has no such path).
