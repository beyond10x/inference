---
format: aep.planning-md/3
id: story:final-message-commit
kind: story
status: implemented
title: Every final assistant message of a model turn is committed once
summary: A turn that streams one message and returns another commits both, each once; a model delta commits only its own segment, in time linear in the deltas
relations:
- serves: vision:inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-23T02:18:23Z", actor: "human:operator", revision: 2, decided_on: {"recorded":{"test_result":1}}, executor: "agent:implementor-u3-final-messages"}
- {from: "proposed", to: "active", at: "2026-11-23T02:18:23Z", actor: "human:operator", revision: 3, decided_on: {"recorded":{"test_result":1}}, executor: "agent:implementor-u3-final-messages"}
- {from: "active", to: "implemented", at: "2026-11-23T02:18:23Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1}}, executor: "agent:implementor-u3-final-messages"}
---
## Context

A review of tag 0.31.0 drove `inference-llm`'s `GenerationHandle` with a recorded model whose
turn streamed the delta "First." and returned two assistant messages, "First." and "Second.",
the second with no delta shown for it. The conversation held both messages; the graph committed
only "First.", so speech never received "Second.". A turn that only streamed, and a turn that
streamed nothing, each committed all of their text. The wrapper kept one flag per turn, whether
any delta was shown, and when it was set it took no text from the outcome at all.

`llm`'s contract (`llm_core::StreamEvent`, tag 0.20.0) says each assistant message of a
successful turn carries exactly the deltas shown for it, and a message no delta was shown for
carries the text the server sent. So the wrapper can tell, per message, whether its text already
reached the caller.

Two more findings of the same review sit on the same commit path:

- a segment the caller emitted into the generation through `GenerationHandle::emit`, documented
  to stay Tentative, was committed by the next model delta, so speech could take a draft the
  caller had not committed;
- every model delta committed by scanning every segment the generation ever emitted, so a
  generation of N deltas made N(N+1)/2 segment visits.

## Planned scope

- `crates/inference-llm/src/lib.rs`: at the end of a turn, match the deltas shown in that turn to
  the outcome's assistant messages in order, and commit the text of every message no delta was
  shown for; a model delta commits the segment it emitted and no other; the generation tracks
  which segments are still Tentative, so committing visits only those.
- `crates/inference-llm/tests/model_turns.rs`: the recorded fixture and its controls.
- `CHANGELOG.md`.

## Acceptance

- A streaming adapter whose turn streams "First." and returns the messages "First." and
  "Second." delivers the committed segments "First." and "Second.", each once.
- Controls: a turn that streams and returns only "First." commits "First." once; a turn that
  streams nothing and returns "First." and "Second." commits "First.Second." once.
- An adapter that does not stream, over the same mixed turn, delivers one segment
  "First.Second." at completion.
- A segment the caller emitted through `GenerationHandle::emit` before a model turn stays
  Tentative after the turn's delta commits; only the model's segment is committed.
- The commits of N streamed deltas of one generation look at N segments, one each, not at every
  earlier one (N(N+1)/2), counted in test builds; `commit_tentative` looks only at the segments
  still Tentative. A wall-clock ratio test was tried and dropped: on the shared host the time of
  8N deltas against N varied from 5 to 85 times on linear code.
- `cargo test -p inference-llm`, `cargo clippy -p inference-llm --all-targets -- -D warnings` and
  `cargo fmt --all --check` pass.
