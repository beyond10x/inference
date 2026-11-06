---
format: aep.planning-md/3
id: story:facade-session-refusal-event
kind: story
status: implemented
title: The facade's session events carry a model's refusal
summary: A refused generation reaches a facade application as a session event with its text.
relations:
- serves: vision:inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T19:11:13Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-06T19:11:13Z", actor: "human:operator", revision: 4}
- {from: "active", to: "implemented", at: "2026-11-06T21:20:41Z", actor: "human:operator", revision: 5, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
# Story: the facade's session events carry a model's refusal

## Outcome

An application on `inference-facade` sees a model's refusal as one session event naming the
generation and carrying the refusal text, instead of losing it.

## Context

0.20.0 gives the stream domain a `GenerationRefused` event and `inference-llm` a
`GenerationEvent::Refused` (story:llm-wrapper-outcomes-and-debug, finding N27). The facade's session
vocabulary (`ess/domains/session.yaml`, `inference-facade`'s `SessionEvent`) has no refusal, so the
testkit's session mapping drops `GenerationRefused` and an application on the facade never sees a
model's refusal or its text.

## Planned scope

- `session.yaml`: a session event for a refused generation, carrying the generation id and the
  refusal text.
- `inference-facade`: the matching `SessionEvent` variant, emitted when the generation node reports
  `Refused`.
- `inference-testkit`: the session mapping forwards it instead of dropping it.

## Acceptance

- A scripted model that refuses yields, through the facade, one session event naming the generation
  and carrying the refusal text, and no text segment for it.
- The synthesized conformance suite covers the new event.
- `CHANGELOG.md` `## [Unreleased]` has an `Added` line (and a `Breaking` line if `SessionEvent` is
  not `#[non_exhaustive]`).
