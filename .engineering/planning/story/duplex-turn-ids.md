---
format: aep.planning-md/3
id: story:duplex-turn-ids
kind: story
status: active
title: Duplex input events name the turn they belong to
summary: InputCommitted and InputTranscribed carry an item or turn id, so DuplexNode pairs each transcript with its own turn
relations:
- serves: vision:inference
- informed_by: review-result:adversary-w24-sdk-duplex-node-pass-1
- decomposes: epic:defects-2026-11-10
scope:
- confidence: cited
  path: adapters/inference-aws/src/duplex.rs
- confidence: cited
  path: adapters/inference-deepgram/src/agent_duplex.rs
- confidence: cited
  path: adapters/inference-deepslate/src/opal.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/src/agent.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/agent_duplex.rs
- confidence: cited
  path: adapters/inference-openai/src/duplex.rs
- confidence: cited
  path: adapters/inference-openai/src/live_duplex.rs
- confidence: cited
  path: adapters/inference-phonic/src/duplex.rs
- confidence: cited
  path: crates/inference-duplex/src/ledger.rs
- confidence: cited
  path: crates/inference-duplex/src/lib.rs
- confidence: cited
  path: crates/inference-duplex/tests/adversary1_ledger.rs
- confidence: cited
  path: crates/inference-duplex/tests/adversary2_probe.rs
- confidence: cited
  path: crates/inference-duplex/tests/ledger.rs
- confidence: cited
  path: crates/inference-pipeline/src/nodes/duplex.rs
- confidence: cited
  path: crates/inference-testkit/src/duplex.rs
- confidence: cited
  path: crates/inference-testkit/src/duplex/mock.rs
- confidence: cited
  path: crates/inference-testkit/tests/adversary_w24_duplex_basis.rs
- confidence: inferred
  path: crates/inference-testkit/tests/duplex_node.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/duplex.rs
- confidence: cited
  path: ess/domains/duplex.yaml
- confidence: inferred
  path: ess/scenarios/input-during-output-continues-response.yaml
- confidence: cited
  path: examples/realtime-voice/tests/acceptance.rs
- confidence: cited
  path: examples/realtime-voice/tests/adversary1_gpt_live_transcript.rs
- confidence: cited
  path: examples/realtime-voice/tests/adversary1_talk_over_cut_answer.rs
- confidence: cited
  path: examples/realtime-voice/tests/compare.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:16:58Z", actor: "human:operator", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-10T00:16:58Z", actor: "human:operator", revision: 6}
---
## Outcome

A host running a native speech model through `DuplexNode` gets each tool call based on the
transcript of the turn the user spoke, also when a provider sends a transcript after a later turn
was committed or never sends one.

## Context

review-result:adversary-w24-sdk-duplex-node-pass-1 and its correction round (story:sdk-duplex-node):
`InputCommitted` and `InputTranscribed` in `inference-duplex` carry only the session id, so
`DuplexNode` (`crates/inference-pipeline/src/nodes/duplex.rs`) pairs a transcript with the last
committed turn still waiting for one. A transcript that arrives only after a later turn was
committed is taken as that later turn's; a queue of committed turns would misplace every transcript
after one failed transcription. The OpenAI adapter feeds the two events from separate server
events (`adapters/inference-openai/src/duplex.rs`), so the ordering is possible; it was inferred
from the code, not observed at runtime.

## Acceptance

- `ess/domains/duplex.yaml` first: `InputCommitted` and `InputTranscribed` carry the provider's
  item id (or a turn id the binding assigns), validated by `ess specify validate --path ess`.
- Every duplex binding fills it (openai, openai live, aws, elevenlabs agent, deepgram agent, phonic,
  deepslate, the testkit mock); a binding whose provider names no item states how it assigns one.
- `DuplexNode` pairs a transcript with the committed turn of the same id; a test with transcripts
  out of order and one missing transcript shows each call's basis is its own turn's span.

## Out of scope

Observing the ordering against a live provider.

## Scope

Derived 2026-11-10 by `story-scoper` from main at af93222a. Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-duplex` (event types and `SessionLedger`) and `crates/inference-pipeline/src/nodes/duplex.rs` (`DuplexNode` pairing) — cited, story Context and Acceptance
- **Files, specification:** `ess/domains/duplex.yaml`: entities `inference.duplex.InputCommitted` (:1220) and `inference.duplex.InputTranscribed` (:1241) — cited; commands `CommitInput` (:578, payload :598-601) and `TranscribeInput` (:671, payload :693-697), each gaining an id input — inferred
- **Files, contract:** `crates/inference-duplex/src/lib.rs`: `InputCommitted` (:373), `InputTranscribed` (:393), each gaining the id — cited; a new id type beside them — inferred
- **Files, ledger:** `crates/inference-duplex/src/ledger.rs` — cited: the only two constructions in the library (:1129 `input_transcribed`, :1708 `record_commit`); signatures `input_committed` :1071, `commit_input` :734, `input_transcribed` :1123; the `ProviderFrame` delegation at :154 and :165
- **Untouched in the contract crate:** `contract.rs:315,318` (enum variants only), `conformance.rs:271` (reads `heard.text`) — cited; both compile unchanged — inferred
- **Files, DuplexNode:** `crates/inference-pipeline/src/nodes/duplex.rs` — cited: state `transcribed` :421-422, `awaiting: Option<Turn>` :423-424; `open_turn` reset :631-632; `InputCommitted` arm :831-840, `InputTranscribed` arm :842-853; module doc :34-42, which states the defect
- **Bindings** (each one's call sites change):
  - OpenAI Realtime, `adapters/inference-openai/src/duplex.rs`: provider id — `AudioCommitted.item_id` (`event.rs:73-76`), `InputTranscript.item_id: Option<String>` (`event.rs:57-58`), dropped by `..` at `duplex.rs:959`, `:894`; manual-commit gap: `commit_input` (:1183-1186) records `InputCommitted` before the server's `AudioCommitted`, which `caller_commits` (:960-961) swallows — cited
  - GPT-Live, `adapters/inference-openai/src/live_duplex.rs`: no id (`live.rs:356-360`); transcript at :1086; never sends `InputCommitted` (`manual_commit: false` :211) — cited; must assign a turn id to each fragment — inferred
  - AWS Nova Sonic, `adapters/inference-aws/src/duplex.rs`: provider id, the user TEXT block's `contentId` (`nova_sonic/output.rs:67-68`); commit at :1000, block stored at :1018, transcript at :1045 looked up at :1037 — cited
  - ElevenLabs agent, `adapters/inference-elevenlabs/src/agent_duplex.rs`: no id read (`agent.rs:378`, :457-458); commit and transcript sent together at :1111-1113 — cited; whether the wire event carries an id — inferred, unknown
  - Deepgram agent, `adapters/inference-deepgram/src/agent_duplex.rs`: no id (`agent.rs:421-424`); sent together at :1210-1212 — cited
  - Phonic, `adapters/inference-phonic/src/duplex.rs`: no id (`protocol.rs:446`); sent together at :1037-1039 — cited
  - Deepslate, `adapters/inference-deepslate/src/opal.rs`: `UserTranscription { turn_id: u32, .. }` (`protocol.rs:659-663`) dropped by `..` at `opal.rs:1146`; commit from `VadStateEvent` (no turn id) at :1140 — cited; mapping Deepslate's `turn_id` onto the commit id — inferred
- **Files, testkit:** `crates/inference-testkit/src/duplex/mock.rs` (commit :779, :788; transcript :605), `crates/inference-testkit/src/duplex.rs` (:253, :307), `crates/inference-testkit/tests/target/duplex.rs` (:245-285, :990-1001) — cited
- **Files, examples:** `examples/realtime-voice/tests/acceptance.rs:302` — cited
- **Test callers of the changed ledger methods:** `crates/inference-duplex/tests/ledger.rs`, `adversary2_probe.rs`, `adversary1_ledger.rs`; `examples/realtime-voice/tests/adversary1_gpt_live_transcript.rs`, `adversary1_talk_over_cut_answer.rs`, `compare.rs` — cited
- **Not touched:** `crates/inference-facade` (its `InputCommitted` is `inference.session`'s), `crates/inference`, `examples/realtime-voice/src/lib.rs:488`, `src/compare.rs:754` — cited
- **ElevenLabs hunks shared with story:signed-url-token-copies:** only the `UserTranscript` arm at `agent_duplex.rs:1109-1114` and the module doc at :49 — cited
- **Also likely:** `ess/scenarios/input-during-output-continues-response.yaml:77-78`; the doc lines of each binding — inferred
- **Documents:** `CHANGELOG.md` `## [Unreleased]`, a `Breaking` entry — inferred; no page under `website/docs/` names either event — cited
- **Tests:** red then green, a new case in `crates/inference-testkit/tests/duplex_node.rs` — inferred; regression `crates/inference-testkit/tests/adversary_w24_duplex_basis.rs` — cited; commands: `cargo test -p inference-testkit --features run,duplex --test duplex_node`, the testkit `conform` target, `inference-duplex --test ledger`, and each binding's duplex test target, `realtime-voice --test acceptance`, `ess specify validate --path ess` — cited
- **Confidence:** high
- **Would collide with:** any unit on `crates/inference-duplex/src/{lib,ledger}.rs`, `crates/inference-pipeline/src/nodes/duplex.rs`, the provider-event handling of any of the seven duplex bindings, the testkit duplex mock and target, the `inference.duplex` block of `ess/domains/duplex.yaml`; on ElevenLabs, a unit editing `agent_duplex.rs` near :1109-1114

Open design questions the scoper could not settle: whether ElevenLabs' `user_transcription_event` carries an id; whether Deepslate's `turn_id` lines up with its VAD commits; how GPT-Live fragments group into a turn; for OpenAI's manual commit, whether to hold `InputCommitted` until `AudioCommitted` or assign an id up front; whether synthesis needs a new authored scenario.

Defect confirmation on main `af93222a` (by reading):
1. `DuplexNode` keeps one slot, `awaiting: Option<Turn>` (`crates/inference-pipeline/src/nodes/duplex.rs:423-424`); each `InputCommitted` with no transcript overwrites it with the newest turn (:836-837).
2. `InputTranscribed` takes whatever is in that slot (`self.awaiting.take()`, :843), because the event carries only `session_id` and `text` (`crates/inference-duplex/src/lib.rs:393-396`); a late transcript of turn N becomes the span and basis of turn N+1 (:850-851).
3. The module doc says so at :41-42; the OpenAI binding drops the item ids with `..` at `adapters/inference-openai/src/duplex.rs:894` and `:959`.
