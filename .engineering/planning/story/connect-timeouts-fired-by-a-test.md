---
format: aep.planning-md/3
id: story:connect-timeouts-fired-by-a-test
kind: story
status: draft
title: A test fires the connect timeout of Cartesia, Phonic and the ElevenLabs agent
tags:
- from-finding
relations:
- serves: vision:inference
revision: 1
---
## Context

Found while the time bounds of each adapter were written down
(story:quality-per-adapter-and-latency-defined). No test of the gate fires the connect timeout
`DEFAULT_CONNECT_TIMEOUT` of three bindings: Cartesia, Phonic and the ElevenLabs agent
(`quality/not-measured.md`, the list of bounds no test fires). A connect that never ends is the
case the constant exists for, and a change that drops the bound leaves the gate green. For
Deepgram transcription and ElevenLabs text to speech it was not determined whether a test fires
the bound.

## Planned scope

- One test per binding against a local listener that accepts the connection and never answers
  the handshake: the open ends with the binding's timeout error within a bound the test sets
  through the binding's configuration, well under a second.
- Deepgram transcription and ElevenLabs text to speech: read first whether such a test exists;
  add it where it does not.
- `quality/adapters.md` names each test under the binding's time bounds, and the lines leave
  `quality/not-measured.md`.

## Acceptance

- With the connect timeout taken out of a binding's open, that binding's new test fails; each
  such planted change is shown red once.
- No new test runs over 5 s, and none uses the network.

## Out of scope

- The default values themselves (story:time-bound-defaults-and-close-bounds-held).
