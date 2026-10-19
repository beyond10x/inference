---
format: aep.planning-md/3
id: story:session-live-text
kind: story
status: active
title: 'A live session over the text-only pipeline: open, send, receive, commit_input and a draining close'
summary: inference-facade binds a session to a run through the pipeline's exposures and delivers one session event vocabulary; send and commit_input are declared in session.yaml with their Ready guard.
relations:
- decomposes: epic:session-facade
- serves: vision:inference
- depends_on: story:facade-crate
- depends_on: story:pipeline-exposed-ports
scope:
- confidence: inferred
  path: Cargo.lock
- confidence: cited
  path: crates/inference-facade/Cargo.toml
- confidence: cited
  path: crates/inference-facade/src/events.rs
- confidence: cited
  path: crates/inference-facade/src/lib.rs
- confidence: cited
  path: crates/inference-facade/src/live.rs
- confidence: cited
  path: crates/inference-facade/src/session.rs
- confidence: cited
  path: crates/inference-facade/tests/text_session.rs
- confidence: cited
  path: crates/inference-testkit/src/session.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/session.rs
- confidence: cited
  path: ess/domains/session.yaml
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T22:56:33Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-19T22:56:33Z", actor: "human:timo", revision: 5}
---
## Context

§13.1 (`docs/design/inference.md:711-719`) gives the session seven operations;
`ess/domains/session.yaml` declares open, attach, ready and close and leaves send, receive,
commit_input, interrupt and describe `UNMAPPED` (`session.yaml:158-160`). This story makes a session
live over the text-only topology A (749-757): the application opens it over a planned pipeline, the
facade creates and drives the run, and the application sends input, commits it, receives one typed
event stream and closes by draining. The facade hides topology (107): it reaches the pipeline only
through its exposed ingress and egress (`story:pipeline-exposed-ports`). Its state machine is the
one `story:facade-crate` moved into `inference-facade`. In-process calls are the first transport
(725).

Decisions taken on silence:

- Open question 13 (1238), which features belong in the facade: the seven operations of 713-719 and
  the session history (`story:session-history`). Memory projections (737) and interaction policy
  beyond commit and interrupt stay with the application; a voice pipeline's turn policy is a node
  of that pipeline, not the session's.
- Readiness (`session.yaml:272-276` names no condition for `SessionReady`): the facade marks the
  session ready when the run it attached is Active, and not before.
- The event vocabulary is one enum, `SessionEvent`, in `inference-facade`, grouping the §6.4
  families (330-345): transcript events, text generation events, synthesis audio, tool events,
  session lifecycle (`SessionOpened`, `RunAttached`, `SessionReady`, `SessionDraining`,
  `SessionClosed`) and run events. Every variant is an event `ess/` already declares or a §6.4
  name, so no ESS entity is added for it. The run's payload type converts into it through a trait
  the facade declares, and a topology emits the subset it produces. A payload event with no variant
  is not delivered; namespaced extensions (328) are later.
- `send` and `commit_input` are declared in `session.yaml` as `inference.session.SendInput` and
  `inference.session.CommitInput`, refused unless the session is Ready (`wrong-state`,
  `SessionStateConflict`), which is how "admits no new interaction" (the `drain` summary) holds for
  input. Their accepting outcomes, `sent` and `committed`, move nothing and emit an event naming the
  session; if ess 0.60.0 admits no accepting outcome that moves nothing, each is a self-transition
  from Ready to Ready, and the story body records which. Below the session, `send` forwards the
  application's envelope to the ingress stamped with the session id (310) and the current run id,
  and `commit_input` sends a control event of kind `commit`
  (`inference_core::envelope::ControlKind::Commit`) into the ingress.
- `receive` and `describe` are reads, not commands, and `session.yaml` does not declare them;
  describe is `story:session-topology-switch`'s.
- Closing drains: `CloseSession` (Draining), then `stop` into every ingress, then
  `CompleteSessionClose` once the run is Completed. The API is async and names no executor (665).

## Planned scope

- Spec first, `ess/domains/session.yaml`: `SendInput` and `CommitInput` with their outcomes,
  refusals and events, `read from:` 714, 716, 339; the marker at 158 shrinks to interrupt, receive
  and describe. Then `ess specify validate` and `ess verify conform synthesize` as AGENTS.md lists.
- `crates/inference-facade`: the two commands in `src/session.rs`; `src/live.rs` (new) binds a
  session to a `RunController` over the exposures; `src/events.rs` (new) holds the vocabulary and
  the payload trait; `src/lib.rs`; `Cargo.toml` (the runtime, and the testkit as dev-dependency).
- Testkit: the session store and its target answer the new commands (`src/session.rs`,
  `tests/target/session.rs`), and the testkit text payload implements the facade trait in
  `src/session.rs`.
- Test `crates/inference-facade/tests/text_session.rs` (new) over the A-02 text pipeline declared
  with exposures.

## Domain relations

- Session references Pipeline, one, and Run, zero or one: `ess/domains/session.yaml`,
  `inference.session.Session`, relations `pipeline`, `run`.
- Run references Pipeline, one: `ess/domains/graph.yaml`, `inference.graph.Run`, relation
  `pipeline`.
- A Pipeline's exposures: declared by `story:pipeline-exposed-ports`.
- No relation from Session to Turn, Span or Generation is added: the session reaches them through
  its current run, and `ess/domains/stream.yaml` relates each to a Run (`inference.stream.Turn`,
  `Span`, `Generation`, relation `run`).

## Acceptance

The testkit conform run answers every `inference.session` scenario with none unsupported,
`inference.session.SendInput/outcome/sent`, `inference.session.SendInput/outcome/wrong-state` and
`inference.session.CommitInput/outcome/committed` among them, and
`crates/inference-facade/tests/text_session.rs` opens a session over the text pipeline, receives
`SessionOpened`, `RunAttached` and `SessionReady` before anything else, sends text and commits it,
receives the mock model's reply as text events of `SessionEvent`, has a send refused once the
session is Draining, and after close receives `SessionDraining` and `SessionClosed` with the run
Completed and every queue dropped.

## Out of scope

Interrupt and the cancel branch of close (`story:session-interrupt-and-cancel`); history
(`story:session-history`); the voice cascade, the switch and describe
(`story:session-topology-switch`); transports other than in-process calls (726-729).

## Implementation note

`SendInput` and `CommitInput` are declared as self-transitions from Ready to Ready
(`inference.session.Session` transitions `send` and `commit`), not as accepting outcomes that move
nothing. ess 0.60.0 validates the no-move form (`updates: inference.session.Session` with a
`when_subject: {predicate: state != Ready}` refusal), but synthesis then refuses both accepting
outcomes with ESS-SYNTH-003 (no arrangement of a Ready session), and `wrong_state: true` beside
an `updates` outcome is refused with ESS-COMMAND-012. The self-transition form synthesizes
`SendInput/outcome/sent`, `CommitInput/outcome/committed`, their two `transition` scenarios and
the `Session/state/{Opened,Draining,Closed}/refuses/...` scenarios that witness `wrong-state`.

The acceptance names `inference.session.SendInput/outcome/wrong-state`; ESS witnesses a
`wrong_state` outcome as `inference.session.Session/state/<State>/refuses/inference.session.SendInput`
(as for `CloseSession`), and no scenario carries the `/outcome/wrong-state` id.
