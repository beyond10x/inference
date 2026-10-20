---
format: aep.planning-md/3
id: story:session-history
kind: story
status: implemented
title: 'Session history: committed user turns, assistant segments and tool results kept across runs'
summary: Declares HistoryEntry in session.yaml and records it from what the session commits and delivers; history outlives the run that produced it.
relations:
- decomposes: epic:session-facade
- serves: vision:inference
- depends_on: story:session-live-text
scope:
- confidence: cited
  path: crates/inference-facade/src/history.rs
- confidence: cited
  path: crates/inference-facade/src/lib.rs
- confidence: cited
  path: crates/inference-facade/src/live.rs
- confidence: cited
  path: crates/inference-facade/src/session.rs
- confidence: cited
  path: crates/inference-facade/tests/history.rs
- confidence: cited
  path: crates/inference-testkit/src/session.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/session.rs
- confidence: cited
  path: ess/domains/session.yaml
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T22:56:33Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-19T22:56:33Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-20T00:06:55Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

The design gives the session "history" (`docs/design/inference.md:105`) and keeps conversation
state (committed messages, tool results, memory projections, turn history) apart from model and
media state (735-741); it is "reconstructible committed event history" owned by the facade or the
application (990), and recovery may start a new generation from committed history (1008).
`ess/domains/session.yaml:47-52` leaves history `UNMAPPED`: "the design does not say whether
history is a list of committed spans, of events or of messages". The topology switch
(`story:session-topology-switch`) is what makes it matter: a session outlives its runs (105), so
what was said in it has to outlive them too.

Decisions taken on silence:

- What history holds: an ordered list of entries, each one committed fact the session delivered
  or accepted. A user turn is the committed input text of one turn; an assistant segment the text
  of one committed output segment; a tool result the call id and its outcome. Never a hypothesis, a
  tentative segment or a delta, and never audio (media state, 739): a voice turn enters as its
  committed transcript text. Each entry names the run it came from and its position in the session.
- When: a user turn at `commit_input` for the text the application sent, or when the session
  delivers the committed transcript of a voice turn; an assistant segment when the session delivers
  a committed segment; a tool result when it delivers a tool result event. Entries are recorded
  while the session is Opened, Ready or Draining (a drain completes what is in flight) and refused
  once it is Closed.
- History is held in memory by the facade and read through a view. It is not persisted, not fed
  into a new run's model context (no LLM context input is wired by this epic), and memory
  projections stay with the application (open question 13, 1238).
- History outlives the run that produced it and lives as long as the session row does.

## Planned scope

- Spec first, `ess/domains/session.yaml`: entity `inference.session.HistoryEntry`, command
  `RecordHistory` (accepting outcome `recorded`; refused for an unknown session and a Closed one),
  its event, view `inference.session.History`, `read from:` 105, 735-741, 990, 1008. The marker at
  47 loses its history part and keeps logical identity and interaction policy. Then
  `ess specify validate` and `ess verify conform synthesize` as AGENTS.md lists.
- `crates/inference-facade`: `src/history.rs` (new), recording from `src/live.rs`, the command in
  `src/session.rs`, `src/lib.rs`.
- Testkit: the session store and its target answer the new command and view (`src/session.rs`,
  `tests/target/session.rs`).
- Test `crates/inference-facade/tests/history.rs` (new); the tool-result entry is covered there by
  a payload carrying a tool result event on the egress.

## Domain relations

- Session owns its HistoryEntries, many: declared by this story, by the decision above; an entry
  exists only within its session.
- HistoryEntry references Run, one: declared by this story; each entry names the run the session
  held when it was recorded.
- UNMAPPED, left so: an entry's link to the row it records (a Turn or Span, an OutputSegment, a
  ToolCall). It would point at one of three entities depending on the entry's kind, the design
  does not say whether history references or copies them, and no `relations:` entry is guessed;
  the entry carries the text.
- Session references Run, zero or one: `ess/domains/session.yaml`, `inference.session.Session`,
  relation `run`, unchanged.

## Acceptance

The testkit conform run answers every `inference.session` scenario with none unsupported,
`inference.session.RecordHistory/outcome/recorded` and its refusal of a Closed session among them,
and `crates/inference-facade/tests/history.rs` shows that after two committed exchanges over the
text pipeline the `inference.session.History` view holds user turn, assistant segment, user turn,
assistant segment in that order, each naming the run, with no entry for a delta, and still holds
them after the run is Completed and the session Closed.

## Out of scope

Persisting history; replaying it into a model's context; memory projections; the history of a
voice turn end to end (`story:session-topology-switch` exercises it).
