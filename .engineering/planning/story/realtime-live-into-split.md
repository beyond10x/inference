---
format: aep.planning-md/3
id: story:realtime-live-into-split
kind: story
status: draft
title: Realtime and GPT-Live clients split into a sender and a receiver
summary: An optional into_split so one task reads events while another writes.
tags:
- downstream-adopter
- openai-realtime
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 1
---
## Context

A downstream adopter asked for an optional `into_split` on `RealtimeClient` and `LiveClient`: a
sending half and a receiving half that two tasks can own, so one task reads events while another
writes audio. On 0.9.0 every method takes `&mut self` on one socket
(`adapters/inference-openai/src/client.rs`, `adapters/inference-openai/src/live.rs`).

## Why it is not in the 0.10.0 wave

Not small: each client's whole send surface moves to a sender type, the receiver keeps the
pending queue, `unparsed_type` and the read timeout, and `close` needs both halves back
(`reunite`), for two clients. Filed as a draft for a later plan.

## Planned scope

- `adapters/inference-openai/src/client.rs`, `adapters/inference-openai/src/live.rs`: `into_split`
  returning a sender and a receiver over `futures_util::StreamExt::split`, and a way to close.

## Acceptance

To be written when the story is accepted.
