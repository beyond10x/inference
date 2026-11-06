# Changelog

## [Unreleased]

### Breaking

- Every envelope of a frame of audio, synthesized or captured, has kind
  `inference.audio.AudioChunk`, the event the specification declares (was
  `inference.audio.AudioFrame`, the type). `inference-audio-device`: `node::AUDIO_FRAME` is
  renamed `node::AUDIO_CHUNK`.
- `inference-facade`: `SessionEvent` has the new variant `GenerationRefused` and `HistoryKind`
  the new variant `AssistantRefusal` (`HistoryKind::ALL` has four entries).
- `inference-core`: `DeclareAdapter` has a required `descriptor: AdapterDescriptor` field, and
  `Adapter::descriptor()` and the `inference.model.Adapters` view carry it; the descriptor's
  `voices`, `codecs` and `rate_limits` are required on the wire, as the specification declares
  them. The adapter, not the capability record, carries the descriptor.

### Added

- `inference-facade`: `SessionEvent::GenerationRefused` delivers a model's refusal (its
  generation and text) to the application, and the session records it in its history as
  `assistant_refusal`. An interrupt cuts it with its generation. `inference-testkit` forwards it
  instead of dropping it, and `MockGenerator::refusing` scripts a refusing model.

### Fixed

- `inference-facade`: an event of an interrupted generation that arrives after the generation's
  end, such as a refusal kept across a cancel or supersede, is no longer delivered; the session
  remembers the last 256 cut generations past their end.

## [0.22.0] — 2026-11-06

### Breaking

- `inference-elevenlabs`: `ElevenLabsSynthesiser` speaks an utterance on one
  `multi-stream-input` connection, one context per segment, replacing the per-segment
  `stream-input` binding. `protocol::init_message` and `END_OF_INPUT` are removed;
  `context_message`, `close_context_message`, `CLOSE_SOCKET` and `FINAL_SPELLINGS` replace them,
  `text_message` takes the context id, and `BACKEND` is `elevenlabs-multi-stream-input`.

### Added

- `inference-deepslate`: a new adapter crate, `DeepslateOpal` and `DeepslateOpalSession`, a duplex
  binding over Deepslate Opal's protobuf WebSocket protocol, with its claims record and a
  `compare` provider. A tool call is answered as unavailable and the session goes on.
- `inference-phonic`: a new adapter crate, `PhonicSpeech` and `PhonicSession`, a duplex binding
  over Phonic's speech-to-speech WebSocket, with its claims record and a `compare` provider. A
  named agent's own tools are answered with an error and noted in the session's dropped events.
- `inference-cartesia`: a new adapter crate, a streaming TTS synthesiser over Cartesia Sonic's
  WebSocket, one context per segment, with its claims record; `compare --cascade-voice cartesia`
  speaks the cascade arm with it.
- `inference-openai`: feature `speech`, `speech::OpenAiSpeech`, a streaming TTS synthesiser over
  the speech endpoint: PCM resampled to the requested rate, G.711 where asked, the character
  limit as a typed error, and a cancel that keeps the usage known so far.

### Fixed

- `inference-runtime` tests: a race where the stop was sent after the run had ended, which
  failed `main`'s CI once.

## [0.21.0] — 2026-11-06

### Added

- `realtime-voice` example: the `compare` example measures a cascade (STT, LLM, TTS) beside
  native speech models in one report, with the measures they share named the same (first audio,
  response end, barge-in to stop) and the cascade's own steps (final transcript, first token, TTS
  first audio). The cascade arm runs each step on a worker thread and paces input on the caller's
  clock; `run_cascade` (simulated clock, which moves on after `SIMULATED_STALL`, 250 ms of wall
  time without a report) and `run_cascade_live` (wall clock). With no hosted TTS configured, the
  cascade arm says so and is skipped.

## [0.20.0] — 2026-11-06

### Breaking

- `inference-core`: `GenerationCompleted` carries `stop_reason: Option<StopReason>`, and
  `Generation::complete` takes it. `inference-llm`: `GenerationOutput::complete` takes it,
  `GenerationEvent` is `#[non_exhaustive]` and gains `Refused`. `inference-testkit`:
  `StreamDomain::complete_generation` takes it.
- `inference-sherpa-onnx`: `MODEL_RATE` is removed; stream and frame rates outside
  `MIN_STREAM_RATE` (8 kHz) to `MAX_STREAM_RATE` (384 kHz) are refused with `UnsupportedRate`.

### Added

- `inference-core`: `StopReason`, the event `GenerationRefused` and `Generation::refuse`, also
  declared in `ess/domains/stream.yaml` with the command `RefuseGeneration`.
- `inference-testkit`: `StreamDomain::refuse_generation`.

### Changed

- `inference-llm`: a generation completes with the stop reason of its last turn; a model's
  refusal is delivered as `GenerationEvent::Refused` with its text, not as a text segment, and a
  refusal of a turn that ended survives a later failed turn, a supersede or a cancel.
- `inference-llm`: a failed turn's usage record keeps the counts and the final mark the provider
  reported; `GenerationHandle`'s `Debug` shows ids, counts and the failure's error code only, no
  conversation text.
- `inference-testkit`: the duplex mock fails the session when its ledger refuses a scripted step,
  also for a call scripted on a turn that ends during output, where an offered tool is now proposed
  on the response in progress.

### Fixed

- `repo-podcast` example: a refused response closes the session the turn opened and reports its
  time as billed.
- `inference-deepgram` tests: the fixture server records a refused handshake before answering it.

## [0.19.0] — 2026-11-06

### Added

- `inference-audio-device`: `PlaybackFault`, `PLAYBACK_PATIENCE` (2 s),
  `AcknowledgingSink::with_playback_patience` and `AcknowledgingHandle::fault`.
- `inference-llama-server`: `loopback` and `NotLoopback`.
- `inference-runtime`: `CHILD_DROP_BOUND` (5 s), `USAGE_STOP_GRACE` (1 s),
  `SubgraphHandle::dropped_usage`, `REFUSALS_KEPT` (256), `RunController::refusals_overflow`,
  `DEFAULT_RECORDER_BOUND` (65 536), `Recorder::with_bound`, `bound` and `dropped`,
  `Recording::new`, `cut` and `cut_at`, and `Replay::compared_until`.
- `inference-core`: `AdapterDescriptor` and `RateLimit`, the design's adapter descriptor, also
  declared in `ess/domains/model.yaml`; which declaration carries it is still open.
- `inference-stt`: `TranscriptOutput::next_offered` and `take_next`, to take offered frames one
  at a time.
- `inference-testkit`: the duplex mock's `with_late_usage`.
- `ess/domains/audio.yaml` declares the event `inference.audio.AudioChunk`.

### Changed

- **Breaking:** `inference-runtime`'s `Recording` keeps where its recorder's bound cut it, in a
  private field, so it is built with `Recording::new` instead of a struct literal. A replay compares
  a cut recording only as far as it holds, instead of reporting a divergence at the cut.
- `inference-llama-server` accepts a base URL only on a loopback host (`127.0.0.0/8`, `::1`,
  `localhost`); any other host is refused before anything opens, and the refusal names scheme,
  host and port, never a user part.
- `inference-tts`: `SynthesisEvent::name()` returns `inference.audio.AudioChunk` for a frame, the
  event the specification declares (was `inference.audio.AudioFrame`, which is a type). Envelope
  kinds elsewhere still say `inference.audio.AudioFrame`.
- `inference-stt`: `input_audio_ms` counts all audio offered on a call not marked failed, and
  only the audio taken on a failed call (was the audio offered in both cases).

### Fixed

- `inference-audio-device`: the acknowledging sink's drain fails on a stream error at once, and on
  queued audio whose played position does not move for the patience, also while a frame waits
  for room, instead of hanging on a dead speaker. A segment heard in full before a turn opened is
  acknowledged at its completion, possibly after later segments' receipts. A frame whose sample
  clock goes back below held, unplayed audio is refused.
- `inference-runtime`: dropping a subgraph node ends its child run within `CHILD_DROP_BOUND`; a
  bridge send on a full parent edge waits instead of spinning; a usage send gives way
  `USAGE_STOP_GRACE` after a stop, or at once on a drop; refusals and recordings are bounded.
- `inference-duplex`: `SessionLedger::response_usage` refuses a report once the response's usage
  record is made, as `response_usage_reported` does; the testkit mock counts that refusal as a
  drop.
- `inference-core`: `UsageTotal::complete()` stays false while it holds a partial record with no
  quantities.
- `inference-deepgram`: a recognition counts as taken only the frames it forwarded, not the frames
  after a refused one.
- `local-voice-cascade` example: each turn's first-audio measure is the first frame of its own
  answer, and a discarded segment reaches the commit gate as `Gated::SegmentDiscarded`.

## [0.18.0] — 2026-11-06

### Added

- `inference-openai`: `Deadlines` holds the WebSocket handshake and write deadlines (10 s each by
  default). `RealtimeClient::connect_with` and `LiveClient::connect_with` take one;
  `OpenAiRealtime` and `OpenAiLive` gain `with_connect_timeout` and `with_write_timeout`, and the
  senders `write_timeout()`. `Endpoint` and `connect` are unchanged.
- `inference-aws`: `NovaSonic::with_open_timeout` and `open_timeout`, default `OPEN_TIMEOUT`
  (10 s, now also exported from `nova_sonic`), bound `NovaSonic::start` as the duplex binding's
  `with_open_timeout` bounds its open.
- `inference-pipeline`: `Gated::SegmentDiscarded`; the tool executor's `with_read_ahead` (default
  64); the commit gate handle's `take_record`.
- `inference-facade`: `Sessions::inputs_sent`, `LiveSessions::inputs_sent` and
  `events_released`.

### Fixed

- `inference-openai`: a server that accepts the TCP connection and never answers the handshake,
  or stops reading, fails the session at the deadline instead of hanging it. `ws://` is refused off
  loopback, and a redirect is not followed.
- `inference-aws`: Nova Sonic reports one stall per idle limit, bounds the arrivals waiting for
  the caller, and fails the session with a provider error when a later audio block comes at
  another rate. An idle limit or wait of `Duration::MAX` no longer panics.
- `inference-facade`: a draining live session whose run fails or is cancelled closes, delivering
  the run's end and then `SessionClosed`. The event log is bounded at 4096 events, and a
  generation's bookkeeping is released once its playback ended (at most 256 kept). A refused send
  records no `InputSent`.
- `inference-pipeline`: the tool authority authorizes a confirmed call queued behind a withdrawn
  one; the tool executor bounds its read-ahead and, once cancelled, starts no call and restores a
  call whose start it was sending; the voice activity detector, turn policy and commit gate end a
  send waiting on a full edge when cancelled; the commit gate keeps at most 64 ended generations.

### Changed

- The `ess` pin moves to 0.88.0: CI's `ESS_VERSION`, the testkit's `ess-conformance` and
  `ess-primitives` tags, and `requires` in `ess/ess-inputs.yaml`.

## [0.17.0] — 2026-11-06

### Added

- `inference-deepgram`, a hosted adapter at locality `external_provider`:
  - `DeepgramRecogniser`, Deepgram streaming speech-to-text over the `/v1/listen` WebSocket, as an
    `inference-stt` `Recogniser`. It claims `incremental_input` and `output_streaming`
    (`records/claims-stt.json`).
  - Input is G.711 μ-law or A-law at 8 kHz, or 16-bit PCM at any rate. Interim results revise an
    open span; `is_final` commits it; `speech_final` or `UtteranceEnd` ends the turn.
  - Messages and frames over 4 MiB fail the recognition; at most 32 server messages wait for the
    caller. The end of a recognition is bounded by one deadline from `CloseStream`.
  - With feature `duplex`, `DeepgramAgent` is a Deepgram Voice Agent session
    (`/v1/agent/converse`) as an `inference-duplex` native speech model
    (`records/claims-voice-agent.json`). A response cut after `AgentAudioDone` completes at the
    next turn event or after a 1 s quiet bound. A think or speak failure warning ends the wait
    for that reply.
  - The key comes from an injected credential source, `DEEPGRAM_API_KEY` or the keychain, and is
    sent only in the handshake. `ws://` is accepted only for a loopback host, and a base URL with
    a user part is refused for every scheme.
- `ElevenLabsAgent` in `inference-elevenlabs`, behind feature `duplex`: a conversation with an
  ElevenLabs agent over `/v1/convai/conversation`, as an `inference-duplex` native speech model
  (`records/claims-agent.json`). Messages and frames are bounded at 1 MiB; server pings are
  answered without queueing; a cancel affects only the open in progress.
- `inference_testkit::stt::conformance`: the backend-neutral checks of a streaming STT adapter,
  generic over any `Recogniser`, for an adapter crate to run against its own recogniser.

### Fixed

- The GPT-Live duplex binding of `inference-openai` ends a response cut by the user's speech at
  the model's next words: the first output fragment after a pause in the model's output and after
  the user's speech ends begins a new response. Before, the next words stayed on the cut response,
  so a caller that dropped the cut response's audio dropped them too.

### Changed

- The `ess` pin moves to 0.87.0: CI's `ESS_VERSION`, the testkit's `ess-conformance` and
  `ess-primitives` tags, and `requires` in `ess/ess-inputs.yaml`.

## [0.16.0] — 2026-11-05

### Added

- G.711 in `inference-audio`:
  - `encode_mulaw`, `decode_mulaw`, `encode_alaw` and `decode_alaw` convert between a 16-bit
    sample and its one-byte code word, by the standard's segment tables;
  - `AudioFrame::to_encoding` converts a frame between `pcm_s16le`, `pcm_mulaw` and `pcm_alaw`.
    A data length that does not match the sample count, or a `pcm_f32le` frame, is refused with a
    `ConversionError`.
- `Resampler` in `inference-audio`:
  - converts 16-bit mono PCM between 8, 16, 24 and 48 kHz (`RESAMPLER_RATES`);
  - filters through a polyphase windowed-sinc low-pass, so content above the lower rate's Nyquist
    frequency is attenuated, not folded back;
  - keeps its state across frames, and its output lags its input by `Resampler::delay`;
  - `resample_frame` refuses a frame of the wrong format, a data length mismatch or a gap in the
    sample clock with a `ResampleError`.
- `inference-elevenlabs`, the first hosted TTS adapter: ElevenLabs streaming text-to-speech over
  its WebSocket text input, as an `inference-tts` `Synthesiser` at locality
  `external_provider`.
  - It claims `output_streaming` and `incremental_input`.
  - Output formats: `ulaw_8000` and `alaw_8000` as G.711 at 8 kHz, and `pcm_16000`, `pcm_22050`,
    `pcm_24000` and `pcm_44100` as 16-bit mono PCM. MP3 and Opus are refused.
  - Each segment is spoken on its own connection. A cancel closes that connection, and no frame
    of the segment follows.
  - The key comes from an injected credential source, `ELEVENLABS_API_KEY` or the keychain, and
    is sent only in the `xi-api-key` header. A `ws://` base URL is accepted only for a loopback
    host.
  - The live check is
    `cargo run -p inference-elevenlabs --features live --example live`. It needs
    `ELEVENLABS_API_KEY` and `ELEVENLABS_VOICE_ID`.

### Fixed

- The acknowledging audio sink of `inference-audio-device` records a talk-over's interruption
  when the generation's supersede reaches it before the turn's opening. Before, it waited for a
  supersede that had already come, so it never recorded the interruption or its
  interruption-to-playback-stop measure. The speaker stopped either way.

## [0.15.0] — 2026-11-05

### Added

- `InputEndpoint::close_to_senders`, a provided method, and `ClosedToSenders` (`Empty`,
  `Holding`, `Unsupported`) in `inference_core::endpoint`. An endpoint closes its edge to senders
  and says, in the same step, whether it still holds an envelope. After that, every send into the
  edge is refused `Closed` with its envelope, and `recv` hands over what the edge held, then
  answers `None`.
  - It is called only while no `recv` future of the endpoint is alive.
  - The default answers `Unsupported` and closes nothing, so no implementer has to change.
  - `QueueReceiver` implements it. The queue of an edge between two nodes of a run answers
    `Unsupported`.

### Fixed

- A send admitted into a subgraph node's run ingress before the drain and waiting for room no
  longer answers `Ok` and is then discarded. This was the known issue of 0.14.0.
  - The ingress bridge closes the run's ingress to senders at the drain, in one step with
    reading what it holds, so such a send is received or refused `Closed` with its envelope.
  - That refusal does not mark the node gone.
  - An edge between two nodes, such as one from a node on a feedback cycle through the subgraph
    node, is not closed and behaves as before.

## [0.14.0] — 2026-11-05

### Added

- `InputEndpoint::known_contents`, a provided method, and `KnownContents` (`Empty`, `NotEmpty`,
  `Unknown`) in `inference_core::endpoint`: an endpoint may say, without waiting, whether its edge
  holds an envelope now. The default answers `Unknown`, which keeps the behaviour before it, so no
  implementer has to change; `recv`'s contract is unchanged. An endpoint that overrides it and
  answers a known value must lose nothing when a pending `recv` future of it is dropped, and is
  given a new `recv` future on each poll by a caller that relies on the answer.
  `QueueReceiver` and `SinkReceiver` implement it.

### Changed

- The ESS pin moves from 0.85.0 to 0.86.0: the CI download, the `inference-testkit`
  `ess-conformance` and `ess-primitives` tags and `ess/ess-inputs.yaml`'s `requires: ess 0.86`.
  `task check` needs `ess` 0.86.0 on `PATH`.

### Fixed

- A drained subgraph node no longer drops an envelope one of its edges holds behind a pending
  receive. The ingress bridge took every `Pending` for an empty edge; it now trusts an endpoint's
  `known_contents` answer, and polls an endpoint that answers `Unknown` and woke itself up to four
  more times per drain read. An `Unknown` endpoint whose receive holds an envelope until another
  thread wakes it still cannot be told apart from an empty one.
- A subgraph node's ingress bridge takes its parent edges in strict turns; before, with three busy
  edges, the middle one could wait behind the other two indefinitely.
- `inference-testkit`'s model node, when an output send is refused as closed while its run is
  being cancelled, ends cancelled with the cut turn's usage record as not final, instead of
  failing without it. A drain the run still awaits fails it at once, as before.

## [0.13.0] — 2026-11-04

### Changed

- The ESS pin moves from 0.84.0 to 0.85.0: the CI download, the `inference-testkit`
  `ess-conformance` and `ess-primitives` tags and `ess/ess-inputs.yaml`'s `requires: ess 0.85`.
  `task check` needs `ess` 0.85.0 on `PATH`.

### Fixed

- GPT-Live: speech over a response stops its playout. The binding reports `SpeechStarted`, just
  before the fragment's `InputTranscribed`, when an input transcript fragment starts inside the
  stretch of the session timeline the response's output transcript covers, once for each
  interruption of one response; `Conversation` with `auto_interrupt` then stops playout as it
  does for every other binding. A trailing fragment of the question the response answers, or
  speech in the response's trailing silence, reports none. That both transcripts share one
  session timeline is read from the protocol documents, not observed on the hosted service.
- A subgraph node that is drained right after its parent put an envelope on one of its edges
  no longer drops that envelope. The ingress bridge read the drain after finding its edges empty,
  so an envelope arriving between the two reads stayed on the edge; it now reads the drain first.
  Measured before the fix: 1 run in 130,800 at two levels of nesting, 1 in 163,200 at three.

### Tests

- Nova Sonic: `append_audio` and `submit_tool_result` on a closed command stream keep the usage
  that already arrived before failing as `TransportClosed` (`transport_closed_arrivals`).
- The subgraph usage-rollup cancel cases wait for the held turn's `TextSegmentCommitted` before
  cancelling, which removes a 1-in-38,839 failure of the test, not of the runtime.

## [0.12.0] — 2026-11-04

### Breaking

- `llm` moves from 0.8.3 to 0.20.0, so the `llm_core` types in the public API (re-exported by
  `inference-llm`, `Generator::new`'s model, `inference-facade`'s `cost` feature) are 0.20.0's.
  A crate that also depends on `llm` moves to 0.20.0, where the packages have lost their
  `b10x-` prefix (`llm-core`, not `b10x-llm-core`). See the `llm` changelog for its own changes,
  among them new variants of `Item`, `StreamEvent`, `Protocol` and `Advisory` and new fields of
  `Error`, `Controls` and `Hints`.

### Changed

- A model's refusal text, which `llm` 0.20.0 carries apart from its answer
  (`StreamEvent::RefusalDelta`, `Item::AssistantRefusal`), adds no text segment to a generation;
  it stays in the turn's items.

## [0.11.0] — 2026-11-03

### Breaking

- `inference_openai::Usage` has a new public field `cached_input_audio_tokens`; a literal that
  lists every field adds it. `QuantityKind` has a new variant `CachedInputAudioTokens` and
  `QuantityKind::ALL` twelve kinds; a `match` that names every kind adds it.
- `inference_tools::DeclareTool` has a new public field `description`, the text a model reads to
  choose the tool, kept by `Tool::description`; a literal adds it (`String::new()` declares a
  tool without one), and a serialized `DeclareTool` without `description` no longer
  deserializes. `inference_testkit::tools::ToolsRow` carries it too.
- `inference_openai::ConversationItem` has a new variant `SystemText`; a `match` that names every
  variant adds it.

### Added

- `RealtimeClient::into_split` and `LiveClient::into_split` split an open session into a sender
  (`RealtimeSender`, `LiveSender`: every send method) and a receiver (`RealtimeReceiver`,
  `LiveReceiver`: `next_event`, `session` and the events kept during setup) that two tasks own,
  so server events are read while a write the peer does not drain is pending.
  `RealtimeClient::reunite` and `LiveClient::reunite` put the halves back together for `close`,
  and refuse halves of two sessions with `ReuniteError`, which hands both back. The whole
  clients keep their API.
- The `cached_input_audio_tokens` quantity kind: audio input tokens the provider served from its
  cache, counted inside both `cached_input_tokens` and `input_audio_tokens`, never added to
  either. The Realtime client reads it from `response.done`'s
  `usage.input_token_details.cached_tokens_details.audio_tokens` into
  `Usage::cached_input_audio_tokens`, `None` when the server leaves it out or sends it in
  another shape, and `Usage`'s `Display` shows it. The Realtime duplex binding records it,
  provider-reported when sent and unknown when not.
- `ConversationItem::SystemText`, a `message` item with `role: "system"` and one `input_text`
  part, for `create_item` and `ResponseOptions::input`: an out-of-band response
  (`conversation: "none"`) can carry instructions in context without adding them to the
  conversation.

### Changed

- Every duplex response usage record carries a `cached_input_audio_tokens` quantity, unknown
  unless the binding's provider reports it, as `cached_input_tokens` is carried today.
- A price binding that charges `cached_input_audio_tokens` gets a `price_unknown` line:
  `llm-cost` has no unit for it.
- A model price charges as cached input only the cached text tokens: `cached_input_tokens`
  less a known `cached_input_audio_tokens`, stopping at 0, so a Realtime record whose cached
  input is mostly audio is priced instead of refused as invalid usage; with the audio share
  unknown, the whole cached input is charged as before.
- The Nova Sonic duplex binding describes a tool by its declared description, and
  `inference_llm::tools::tool_spec` by `name: description`; a tool declared without one is still
  described by its name.

### Fixed

- Both OpenAI duplex bindings, Realtime and GPT-Live, send each tool's declared description in
  its function tool instead of an empty one.
- The Realtime and GPT-Live read timeout holds across reads: `next_event` on `RealtimeClient`,
  `LiveClient` and their receivers reports `RealtimeError::Stalled` once the read timeout has
  passed since the last message read (text or binary; pings and pongs do not count), starting at
  the end of the handshake, however many calls that spans. Before, each call restarted the
  bound, so a caller that dropped its read (a `select!` or a shorter timeout around
  `next_event`) and called again never saw `Stalled`. A message already received is still
  returned after the bound passed, and after a stall the bound runs again from the stall.
- `RealtimeClient::close` and `LiveClient::close` return `RealtimeError::Stalled` within their
  documented bound (the read timeout; the longer of the read timeout and `CLOSE_TIMEOUT`) when
  the peer stops draining the socket. Before, they waited for the close-frame or
  `session.close` write forever, outside that bound.
- A read timeout too large to add to the current instant (such as `Duration::MAX`) no longer
  panics: reads and `close` on the Realtime and GPT-Live clients wait without a deadline. A wait
  that large passed to either duplex binding's `next_event` no longer panics either.

## [0.10.0] — 2026-11-03

### Breaking

- `inference_openai::SessionConfig` has four new public fields; a literal that lists every field
  adds `noise_reduction`, `transcription_language`, `transcription_prompt` and `tool_choice`
  (`None` keeps the 0.9.0 `session.update`).
- `ServerEvent::FunctionCall::name` is `Option<String>` and `ServerEvent::AudioCommitted` has a
  new field `previous_item_id`.

### Added

- `SessionConfig` sends input noise reduction (`NoiseReduction`), the input transcription's
  language and prompt, and a session `tool_choice` (`ToolChoice`).
- `RealtimeClient::create_item` adds a user, assistant or function-output `ConversationItem`,
  with an optional `event_id` and a `previous_item_id` placement (`ItemOptions`).
- `RealtimeClient::create_response_with` sends `response.create` with per-response
  `ResponseOptions`: instructions, tool choice, conversation `none` for an out-of-band response,
  input items, tools and an `event_id`.
- `RealtimeClient::ping` and `LiveClient::ping` write one WebSocket ping frame, for a keepalive
  the caller schedules.
- `ServerEvent::AudioCommitted` carries the server's `previous_item_id`.

### Changed

- A `response.function_call_arguments.done` without `name`, or with an empty one, is read instead
  of refused; the client fills the name from the `function_call` output item it saw for the same
  item id before the call, and the duplex binding drops a call with no name as a tool not offered,
  even when a tool is declared with the empty name.
- CI, `ess` and the testkit's `ess-conformance` and `ess-primitives` move from 0.77.0 to 0.84.0.

## [0.9.0] — 2026-10-30

No breaking changes.

### Added

- `inference_audio_device::loopback::run` plays a microphone back through a speaker with a fixed
  delay on any `AudioHost`; `mic-loopback` uses it, and the hardware test's flow also runs against
  the fake host on every test run.

### Changed

- CI, `ess` and the testkit's `ess-conformance` and `ess-primitives` move from 0.76.0 to 0.77.0.

## [0.8.1] — 2026-10-29

No breaking changes.

### Changed

- CI, `ess` and the testkit's `ess-conformance` and `ess-primitives` move from 0.68.0 to 0.76.0.

### Fixed

- `Speaker::push` no longer queues a frame on the new clock when `Speaker::stop` lands between
  its stop check and the queuing: the check now runs under the queue's lock, and the push
  answers `PushError::Stopped`.

## [0.8.0] — 2026-10-23

### Added

- `ess/domains/duplex.yaml` models what a session receives from its provider:
  `ReceiveSessionEvent` and `DeliverSessionEvent` over the entity `ReceivedSessionEvent` and its
  view `ReceivedSessionEvents`. `CloseSpeechSession` and `FailSpeechSession` deliver every event
  still held before their end event, and nothing is received after it. `inference-duplex` carries
  the record. `SessionLedger` keeps one for every provider event it holds
  (`received_events`, `receive_event`, `deliver_event`, `DeliverSessionEventError`), delivers it
  when the caller takes the event and stamps every one still held at a close or failure. The
  testkit `DuplexDomain` holds each session in a `SessionLedger` (`SessionLedger::open_record`),
  so the conformance suite runs the bindings' ledger. Two authored scenarios
  (`close-delivers-received-usage`, `fail-delivers-received-usage`) stand for the `closed` and
  `failed` outcomes, which ess 0.60.0 synthesis refuses (ESS-SYNTH-001). Each holds two events at
  the end and one the caller already had. The close's scenario also has the caller take one event
  out of order before the close, and a second open session holding one of its own, which the
  close leaves held.
- `SessionLedger` counts the frames a binding could not read (`unreadable_frame`,
  `unreadable_frames`, `last_unreadable`) and ends a response the provider ended unreadably
  (`response_ended_unread`). A binding records a frame it may read only in part through
  `SessionLedger::provider_frame`, one frame at a time: the `ProviderFrame` borrows the ledger,
  so overlapping frames do not compile. The frame records through methods of its own that
  forward to the ledger's, and hands out no `&mut SessionLedger`, so the ledger under it cannot
  be replaced. `ProviderFrame::read_in_part` counts the frame after what it recorded and takes
  every hearing of the frame out of the liveness bound; dropped, the frame was readable.
- `SessionLedger::usage_awaited` keeps the next response from settling a response whose final
  usage the provider reports in a frame of its own; `SessionLedger::response_usage_unknown`
  settles it unknown when that frame was lost. `SessionLedger::dropped` counts every refusal a
  binding drops (`dropped_count`), and `dropped_events` keeps the most recent
  `SessionLedger::DROPPED_EVENTS_KEPT` (64) of them; the Realtime, GPT-Live and Nova Sonic
  bindings hand every one to it.
- OpenAI Realtime `RealtimeClient::unparsed_type` gives the top-level `type` of the last message
  the client could not take as an event, when that message has one.
- `SessionLedger::session_usage_at_end` takes the total of a session billed as a whole and records
  it once, just before the session's end event. Given after the end, it records nothing.
- `inference-facade`: `SessionEvent::UsageRecorded`, and usage totals per run and per session
  (`LiveSession::usage`, `UsageTotal` in `inference-core`), where an unknown count is never added as
  0; `LiveSession::record_inputs` records a session's ingress inputs for replay.
- `inference-facade`, feature `cost`: a usage record priced against a caller's price document
  (`CostRecord`), with `price_unknown` lines instead of zero and a session cost total through
  `LiveSession::price_with`; the synthesized suite grows from 460 to 462 scenarios, all answered.
- A subgraph node's `usage` output port also carries the usage of its child run, counted once under
  the parent run.
- The documentation site moves from mdBook under `site/` to Docusaurus under `website/`, with
  pages generated from the source by the `inference-docs` crate and checked by `task check`.
- `ess/domains/usage.yaml` declares the view `UsageRecords`, every usage record with its
  quantities, answered by the testkit's `UsageDomain::usage_records_view`; synthesis now obliges a
  scenario for the `Quantity` invariants instead of refusing them (ESS-SYNTH-013). The authored
  scenario `unknown-quantity-stays-unknown` records a quantity of source `unknown` and checks that
  the view publishes it with no value, never as zero. The suite grows from 469 to 471 scenarios,
  9 of them authored, all answered.

### Changed

- **Breaking:** `SpeechSessionHandle::close` delivers every event the binding received before the
  close, usage included, after the close and before `SpeechSessionClosed`, and nothing the provider
  sends after it; a caller that counts usage reads `next_event` until `SpeechSessionClosed`. Nova
  Sonic, OpenAI Realtime and GPT-Live now record what already arrived before closing.
- GPT-Live records the billed session seconds after the end events of its responses and just
  before `SpeechSessionClosed` or `SpeechSessionFailed`, not as they arrive.
- **Breaking:** provider events are decoded field by field, so a field an event may lack is an
  `Option`: OpenAI Realtime `ServerEvent::ResponseDone` (`response_id`, `status`, `usage`) and
  `ServerEvent::InputTranscript` (`item_id`, `transcript`), both of which also gain
  `unreadable_usage`, saying why a `usage` in another shape could not be read; GPT-Live `BackendEvent::Completed` and
  `BackendEvent::Ended` (`response_id`), both of which also gain `unreadable_usage`,
  `BackendUsage` (`input_tokens`, `output_tokens`, `total_tokens`), `LiveEvent::SessionClosed` and `LiveClosed` (`reason`,
  `seconds`); Nova Sonic `OutputEvent::CompletionEnd` (`stop_reason`), `OutputContentEnd`
  (`content_id`, `kind`) and `OutputEvent::Interrupted` (`content_id`).
- `LiveClient::close` waits for `session.closed` for `CLOSE_TIMEOUT` in all, or the read timeout
  if longer, not per read. A GPT-Live session whose close reaches that bound still closes, with
  the usage the server sent meanwhile recorded, unconfirmed.
- A stall now reads "the provider sent nothing readable for … ms".
- **Breaking:** `SessionLedger::response_usage_reported` for a response whose usage is already
  recorded changes nothing: it no longer records `ResponseUsageRecorded`. OpenAI Realtime
  `ServerEvent` gains the variant `UnreadableResponseDone`.
- `inference-llm`: a cancelled or superseded generation keeps the usage its turn reported, as a
  final or partial record, instead of dropping it.
- `inference-llama-server` and `inference-llm` move to `llm` 0.8.3; the adapter uses its
  `AnonymousResolver`, and a server response without a usage chunk reports its tokens from the
  server's timings.
- CI, `ess` and the testkit's `ess-conformance` and `ess-primitives` move from 0.66.0 to 0.67.0.
- CI, `ess` and the testkit's `ess-conformance` and `ess-primitives` move from 0.67.0 to 0.68.0.

### Fixed

- A speech session whose transport closed under a command (Nova Sonic `append_audio`, `send` and
  `submit_tool_result`; a failed send on OpenAI Realtime and GPT-Live) records what had already
  arrived first, so its usage counts, and fails as the first stream end, client failure or service
  error among it, else as `TransportClosed`.
- A frame or message a binding cannot decode no longer ends a speech session (Nova Sonic, OpenAI
  Realtime, GPT-Live, and `LiveClient::close`). The binding drops it and reads on, so usage the
  provider sends after it is still recorded by `next_event` and by the close.
- Such a frame is no longer dropped silently: the session counts it and keeps the last one's
  type or the start of its text, and it does not refresh the liveness bound. A later failure of
  the session names that count and the last frame. Stalled turns into ProviderError only when an
  unreadable frame arrived after the last readable frame. A readable frame clears that state. The
  detail still names the total unreadable count. Every other failure keeps the kind the transport
  or the provider stated, so a GPT-Live `connection_lost` without its seconds is still
  `TransportClosed`.
- An event that lacks a field the binding does not need keeps its usage and its end. A Realtime
  `response.done` without `status` ends its response, which fails as a provider error, and still
  counts its usage. One without its id ends the oldest response still awaiting its
  `response.done`, never a newer one, so it no longer blocks the next. A `usage` in a shape the
  binding cannot read counts as unknown, and its frame is counted as unreadable. A transcription
  without its transcript still counts its usage. A Realtime `response.created` while a response is
  still in progress ends that one first, as a provider error naming the unreadable frames, and the
  new response is accepted. Its usage, like a cancelled response's, is recorded once by its own
  `response.done` whenever it arrives, else with every count unknown when a later response's
  `response.done` carrying its id ends it (Realtime ends responses in creation order) or at the
  session's end, before `SpeechSessionClosed` or `SpeechSessionFailed`. A message that is not
  JSON but whose `type` reads `response.done` (`ServerEvent::UnreadableResponseDone`) ends the
  oldest response awaiting its end, its usage unknown. An id-less `response.done` skips a
  response only when a message without a readable top-level `type` may have been its
  `response.done`: one that arrived after its creation and before the next `response.created`,
  which ended it or followed the caller's cancel of it, the message before the cancel or after
  it. A message
  whose `type` reads as another event, even one the binding cannot decode, does not make it skip
  one. A frame read in part, such as a transcription without its transcript, does not either, and it
  leaves the session as an unreadable frame does, so a stall after it is a provider error. A Nova
  Sonic `append_audio` or `submit_tool_result` after the session ended returns `NotOpen` and
  drops nothing. A GPT-Live backend usage is read field by field: each count it carries counts,
  and one it lacks or sends in another shape leaves that count of the response unknown; the frame
  counts once as read in part. A backend usage that is not an object, and a backend response end
  whose `response` is missing or not an object, count no usage and their frame once as
  unreadable. A GPT-Live
  backend end without its id, and a `session.closed` without its reason or seconds, keep their
  meaning. A Nova Sonic `completionEnd` without `stopReason` ends the response in progress, and a
  `contentEnd` or interruption notice without `contentId` applies to it.
- A frame read only in part no longer refreshes the liveness bound, as the bindings document: a
  GPT-Live backend response end whose `response` or `usage` was read in part or not at all, a
  GPT-Live `session.closed` lacking a field, and an OpenAI Realtime `response.done` with its
  `status` but an unreadable `usage`, or a transcription without its transcript or with an
  unreadable `usage`. What such a frame recorded stays recorded. A Realtime `response.done`
  without `status` still starts the bound anew, since the caller then awaits a new response. The
  bound runs from the latest readable frame and its limit never changes. Frames are recorded one
  at a time, and a frame read in part is taken out whole however often it heard the provider: a
  readable frame before it stays, also at the same instant, a request, answer or check while it
  is open stays, a check inside it starts a bound that waited for that check, and one read in
  part at the instant last heard leaves a bound that waited for the next check waiting.
- A token sum that would pass `u64::MAX` no longer wraps or panics. In a usage record it is
  unknown (GPT-Live backend usage sums and the total of input and output, Nova Sonic usage sums);
  the contract's `ResponseUsage`, which has no unknown, stops at `u64::MAX`, as does an OpenAI
  Realtime total the server left out. A Realtime transcription total the server left out is
  unknown when its input and output tokens sum past `u64::MAX`.
- A token count above `i64::MAX` is recorded as unknown in a duplex usage record
  (`inference_duplex::reported`), not clamped to `i64::MAX`, and so are TTS input
  characters and output audio milliseconds and STT input audio milliseconds in their usage
  records.
- The TTS conformance kit (`inference_testkit::tts::conformance::usage_recorded`) expects
  characters or audio milliseconds above `i64::MAX` as unknown, as `inference-tts` records them,
  not clamped to `i64::MAX`.
- `inference-openai`: the GPT-Live binding no longer claims `interruptible`. A caller can neither
  cancel nor interrupt its responses, so nothing stops its work within a declared latency.
- A nested subgraph drain no longer loses input sent right before stop.
- An output port of a subgraph node with no edge no longer fails the run; the child's output on it
  is read and dropped.
- A subgraph node's egress bridges end with the child run, so a child that keeps its endpoint past
  stop no longer hangs the drain, and a drain fails when every parent edge of a port refused.

## [0.7.0] — 2026-10-21

### Added

- `inference-duplex`: the session ledger records one usage record per native speech response once
  its usage is settled, one per reported transcription, and holds usage no response owns yet as a
  session record before the session closes or fails, so billed usage is not lost at the end.
  OpenAI Realtime, GPT-Live (billed session seconds) and Nova Sonic report through it; a count the
  provider leaves out is unknown, never 0.
- `inference-stt`: a recognition ends with `TranscriptEvent::Usage`, after its last committed span:
  input audio counted exactly from the frames offered, one request, and the backend's local compute
  where the adapter measures it (sherpa-onnx and whisper.cpp do); a failed call is recorded with
  `final: false`. The testkit STT node sends it on a `usage` port.
- `inference-tts`: a synthesis ends with `SynthesisEvent::Usage`: characters taken, output audio,
  one request and local compute where measured; the backend-neutral TTS checks check it, and the
  testkit TTS node sends it on a `usage` port.
- `inference-openai`: `SessionConfig::speed` sets the Realtime output speed (0.25 to 1.5).
- `examples/repo-podcast`: Claude distils a repository, two voice models discuss it over the
  speaker in a terminal UI with running token and cost counters.
- `examples/local-voice-cascade`: `INFERENCE_CASCADE_STT` (or `talk --stt`) selects the sherpa-onnx
  or the whisper.cpp recogniser by configuration; the same cascade tests pass under both.

### Changed

- Breaking: `TranscriptEvent::span_id` and `SynthesisEvent::segment_id` return an `Option`, since
  the usage event names no span or segment; after a recognition's or synthesis's usage record,
  further moves are refused (`RecognitionEnded`, `SynthesisEnded`), and the record is refused while
  a span is uncommitted or a segment is not synthesized, unless the call failed.
- The local cascade's `stt` node requires no capability, so a batch recogniser binds to it;
  `STREAMING_STT_REQUIRES` keeps the stricter declaration.
- CI, `ess` and the testkit's `ess-conformance` and `ess-primitives` move from 0.60.0 to 0.66.0; the
  specification is unchanged and the synthesized suite stays at 460 scenarios, all answered; the
  testkit target reads the inputs 0.66.0 synthesizes with optional fields left out.

## [0.6.0] — 2026-10-20

### Added

- `adapters/inference-sherpa-onnx`: the sherpa-onnx streaming recogniser as a local STT adapter
  (incremental input, output streaming), linked against the shared library built without TTS so
  no GPL phonemiser is linked; checkpoint and native archive pinned by sha256; conformance tests
  and a capability record for the reference machine.
- `adapters/inference-whispercpp`: whisper.cpp as a local batch STT adapter (no streaming claims),
  CPU-only; silence and non-speech annotations commit nothing.
- `adapters/inference-llama-server`: a local llama.cpp `llama-server` as an output-streaming LLM
  through `llm`'s Chat Completions client: cancellation, failures and deadlines, tool proposals as
  structured events, and one capability record per configuration.
- `adapters/inference-openai`: OpenAI GPT-Live (`v1/live/sessions`) behind feature `gpt-live`, as
  an `inference-duplex` native speech model that the probe observes as native duplex.
- `inference.model.CapabilityRecord`: an adapter's claims, the tests that evidence them, languages,
  formats and a resource profile per hardware target; the testkit checks a record file against
  the adapter's declaration.
- `inference.usage`: one usage record per model call (who, when, quantities, each reported,
  measured or unknown); `inference-llm` emits one per `llm` turn on a pipeline's `usage` egress.
- `inference-pipeline`: the commit-gate node; planning refuses an adapter whose locality the
  deployment profile does not admit, inside subgraphs too.
- `inference-runtime`: the egress watch that fails an `offline_strict` run on a connection or
  unconnected datagram socket that is not loopback; a recorder and replay driver that compare a
  run's control decisions; a best-effort telemetry sink that never delays a node.
- `inference-audio-device`: a sink that acknowledges playback and drops frames of superseded
  generations after a barge-in.
- `examples/local-voice-cascade`: STT, LLM and TTS through commit gates with barge-in, on the
  testkit mocks and, behind feature `local`, on sherpa-onnx and `llama-server` (TTS still a mock);
  `examples/stt-sherpa-onnx`; `realtime-voice` shows usage per response and session totals,
  selects `openai`, `openai-live` or `nova` by `--provider`, and its `compare` example measures
  latency and usage across providers.
- `inference-testkit`: backend-neutral TTS adapter checks.
- The documentation site under `site/`, deployed to GitHub Pages on each release tag.

### Changed

- The specification adds the capability record and the usage domain (`ess verify diff` against
  0.5.0: 18 changes, all compatible); the synthesized suite grows from 454 to 460 scenarios, all
  answered.
- `AGENTS.md` states the licence policy for local adapters and that tests download no weights and
  start no model server.
- CI installs `libclang-dev` and `cmake` and gives the check job 45 minutes.

## [0.5.0] — 2026-10-20

### Added

- `inference-facade`: the session facade. An application opens one session, sends, receives,
  commits input, interrupts, describes and closes it, and sees one event vocabulary whether the
  session runs the text-only pipeline or the voice cascade; the topology switches by
  configuration; history and a live session over a running graph.
- `inference-duplex`: the native speech session contract (one multiport node for a model that
  listens and speaks), its session ledger, a mock, and a conformance probe that reports whether a
  model's native duplex claim was observed.
- `inference-pipeline`: exposed ports, declarative graph definitions validated against runtime
  node descriptors, and subgraphs that keep cancellation and tracing boundaries; the endpoint
  controller, voice activity and turn policy nodes; tool retries by side-effect class with an
  at-most-once start; a bounded tool feedback loop through the LLM node; tool calls under
  interruption.
- `adapters/inference-openai`: OpenAI Realtime as an `inference-duplex` native speech model, with
  offline tests and live tests gated on a credential (environment, or the local keychain behind a
  feature).
- `adapters/inference-aws`: Amazon Nova Sonic as an `inference-duplex` native speech model, with
  offline and live tests; service errors are shown with ARNs and account ids redacted.
- `adapters/inference-audio-device`: microphone capture and speaker playout over cpal, with a
  lock-free ring and G.711 conversion.
- `examples/realtime-voice`: talk to a realtime model from a laptop
  (`cargo run -p realtime-voice --example talk -- --provider openai|nova`).
- `inference-testkit`: the session to run link and the session store; the conformance target
  answers all 454 scenarios of the synthesized suite.

### Changed

- `inference-llm` is a thin wrapper over the `llm` repository's model contract
  (`llm_core::Model`, tag 0.8.0). A `Generator` refuses an adapter claim its model does not
  declare; a turn that showed no delta gives the outcome's text as one segment. The workspace's
  `rust-version` moves from 1.88 to 1.98, which that tag requires.
- The workspace includes `adapters/*`.
- The specification adds the session, duplex and audio-encoding changes above (`ess verify diff`
  against 0.4.0: 146 changes; 3 breaking for readers, the new `AudioEncoding` variants `pcm_mulaw`
  and `pcm_alaw` and the new `GraphDefect` variant `duplicate_exposure_name`; 23 unknown, 120
  compatible). The synthesized suite grows from 317 to 454 scenarios.

## [0.4.0] — 2026-10-19

### Added

- `inference-stt`: the STT contract with no concrete model, the scripted mock recogniser and the
  `stt-only` example, whose dependency tree carries no runtime.
- `inference-tts`: the TTS contract, the mock synthesiser and a sink double; playback is
  acknowledged with a `PlaybackReceipt` built from the synthesized audio.
- `inference-pipeline`: pipelines declared through commands or the typed builder, checked against
  a catalog of declared models and adapters, validated with the structural checks of design 10.2
  and planned from the weakest node's capabilities; the node-instance trait; the tool authority
  node, which validates arguments against the tool's schema, binds the tool's
  `requires_confirmation` through a confirmation hook that confirms or denies, and decides each
  call once; the tool executor node, generic over the executor contract, which runs each call
  once under its deadline.
- `inference-runtime`: bounded edges with backpressure and the overload policies; the `Run`
  state machine and its nine commands; the run controller, which drives node instances through
  prepare, start, drain and stop; `RunController::drive`, which ends a run when every source has
  sent end of input or on a drain or cancel request, draining nodes upstream first; a guard that
  refuses a repeated or decreasing sequence among the content events a node emits on a stream.
- `inference-tools`: the `ToolCall` lifecycle (propose, authorize, reject, start, complete, fail),
  the executor contract and the provisional-input barrier.
- `inference-core`: the control event kinds (`ControlKind`; end of input is `stop`) and
  `SendError::SequenceNotIncreasing`.
- `inference-testkit`: the run stores behind an optional `run` feature; mock source, sink and
  model nodes of a pipeline; the scripted mock tool executor with its invocation log. The
  conformance target answers 296 of 317 scenarios, every `inference.graph`, `inference.stream`,
  `inference.policy` and `inference.tools` scenario among them.

### Changed

- `OutputSegment` transitions take the segment by value, so a segment cannot be committed twice;
  `Generator::start` returns a `Result`, and the LLM mock refuses a second concurrent generation.
- The specification adds the node-instance and tool-call lifecycles, their events and errors, and
  `AddPort.pipeline_id` (`ess verify diff` against 0.3.0: 67 changes; 12 breaking, all new state
  variants for readers and the new required `AddPort` input for callers; 27 unknown, 28
  compatible). The synthesized suite grows from 229 to 317 scenarios.

### Fixed

- The defects of the 0.3.0 review in the runtime queue (every discard counted, coalesce keeps the
  newest, stale-epoch sends wake), the pipeline (forced outcomes, ports under their pipeline) and
  the STT and TTS contracts.

### Known issues

- The 21 `inference.session` scenarios are unsupported until the session facade lands; they are
  named in `ess/SKIPPED.md`.
- A run waits for a cancel when a node that consumes events fails, and a cancel arriving between a
  tool node's check and its send can let one event through; both are recorded on the tool-call
  interruption story.
- `ess verify conform synthesize` still refuses the three `affects` cascades (ESS-SYNTH-001); the
  authored scenarios execute them.

## [0.3.0] — 2026-10-19

### Added

- `inference-core` follows the 0.2.0 model vocabulary (`orchestrated_duplex`, a model's
  `provenance` and `source_revision`, `ModelNotFound`) and gains `Model::declare_as` and
  `Adapter::declare_as` for stores that assign identifiers, `Generation` with its lifecycle, and the
  event envelope (`EventEnvelope`, `Timebase`, `EventId`).
- `inference-audio`: audio frames, formats and sample clock positions, declared first as the
  `inference.audio` domain in `ess/domains/audio.yaml`; frame data is canonical base64 on the wire.
- `inference-text`: output segments (tentative, committed, queued, synthesized, played, discarded),
  turns and spans; a committed span is immutable.
- `inference-llm`: the LLM contract, with no concrete model. Deltas stream only where the adapter
  claims `output_streaming`, and input is taken during output only where it claims
  `concurrent_input_output`; admitted input is bound to the generation that admitted it.
- `inference-testkit`: in-memory stores per domain, the event log, a conformance target driven by
  the `ess-conformance` runner whose results `ess verify conform report` admits, and the mock
  generator behind the `llm` feature. `ess/SKIPPED.md` names every scenario the target does not
  answer yet and the story that will.
- `task check` builds and tests with `--all-features`.

### Changed

- The specification adds the `inference.audio` domain and the event envelope types to
  `inference.stream` (`ess verify diff` against 0.2.0: 8 changes, all compatible); the synthesized
  suite is unchanged at 229 scenarios.

### Known issues

- The testkit target answers 15 of the 229 scenarios; the other 214, most of which arrange a run,
  are named in `ess/SKIPPED.md` with the story that will answer them.
- `ess verify conform synthesize` still refuses the three `affects` cascades (ESS-SYNTH-001); the
  authored scenarios execute them against the interpreted target.
- The conformance results the testkit writes carry a fixed `completed_at`, not the run's own time.

## [0.2.0] — 2026-10-19

### Added

- The Cargo workspace under `crates/` and its first crate, `inference-core`: the `inference.model`
  vocabulary of `ess/domains/model.yaml` (models, adapters, capabilities, modalities, locality,
  execution form, the concurrency refusal) and the shared identifiers of runs, nodes, streams,
  generations and sessions, read only in the canonical Uuid spelling.
- `task check` runs `cargo fmt`, `cargo clippy -D warnings` and `cargo test` after the
  specification and planning checks.
- `ess/scenarios/`: four authored scenarios for what ess 0.60.0 synthesis does not reach: the
  discard cascades of supersede, cancel and run cancellation (ESS-SYNTH-001), and a Draining run
  that fails.

### Changed

- The specification fixes 66 findings of a hardening pass and five reviews (13 P1): a superseded
  or cancelled generation emits, commits, queues and plays nothing more; the tool barrier reads
  the tool's class and the span's state instead of the caller's claim; profiles are checked
  against the design's table; runs, sessions and graph edits refuse the wrong pipeline or run
  state; refusals carry typed diagnostics. Breaking against 0.1.0 (`ess verify diff`, exit 4):
  new enum members, `FailRun.failure: FailureReason` replacing `reason`, new required inputs on
  `DeclareModel`, `DeclareTool` and `ProposeToolCall`, the new `DeclareProfile` refusal.

### Known issues

- `ess verify conform synthesize` yields 229 scenarios with 3 ESS-SYNTH-001 refusals, the
  `affects` cascades of `CancelRun`, `CancelGeneration` and `SupersedeGeneration`; the authored
  scenarios execute them and the interpreted run passes 229 of 229.

## [0.1.0] — 2026-10-19

The design, its specification and its plan. No code yet.

### Added

- `docs/design/inference.md`, the design (v0.2): vocabulary, contracts, streaming semantics, the
  graph, the runtime, the roadmap and the open questions; `docs/design/review-2026-10-18.md`, its
  review, with the owner's decisions on each finding.
- `ess/`, the ESS specification: `system.yaml`, `ess-inputs.yaml` and six domain documents
  (model, policy, graph, stream, tools, session) with 17 entities and 36 commands, every
  declaration citing the design line it was read from; `ess verify conform synthesize` yields
  153 scenarios with 0 refusals.
- `.engineering/planning`, the AEP planning store: a vision, product requirements, an
  architecture design, the ESS record, 12 epics, 22 stories with typed scope in 18 waves, 4
  decision blockers and 24 review results from two critic rounds.
- `Taskfile.yml` with `task check`; `.github/workflows/check.yml` runs it with pinned `ess`
  0.60.0 and `aep` 0.74.0 releases; `.github/workflows/shared-gates.yml` calls the common source
  gates at an exact commit.
