---
format: aep.planning-md/3
id: story:duplex-turn-ids
kind: story
status: draft
title: Duplex input events name the turn they belong to
summary: InputCommitted and InputTranscribed carry an item or turn id, so DuplexNode pairs each transcript with its own turn
relations:
- serves: vision:inference
- informed_by: review-result:adversary-w24-sdk-duplex-node-pass-1
revision: 1
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
