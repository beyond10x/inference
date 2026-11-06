---
format: aep.planning-md/3
id: story:audio-envelope-kind-is-the-declared-event
kind: story
status: draft
title: Audio envelopes carry the declared event name inference.audio.AudioChunk
summary: SynthesisEvent::name() now returns inference.audio.AudioChunk for a frame (0.19.0, N14), while about 25 sites (the TTS testkit node, inference-audio-device AUDIO_FRAME, runtime, examples, tests) still stamp the envelope kind inference.audio.AudioFrame; move them together, keeping the old constant as a deprecated alias
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
- serves: vision:inference
revision: 1
---
<!-- Starting point for a `story` artifact, seeded by `aep artifact new story <name>`.
     No frontmatter here on purpose: the `---` block is written by the CLI from the id, kind, status
     and relations you gave it, and a second copy in this file would be the one that went stale.
     Delete the italic guidance as you fill each section. -->

# Story: <name>

## Outcome

*What is true for whom once this has shipped, in one sentence. If it names a component rather than a
person, it is a task — say what changes for someone.*

## Context

*Why this is worth doing now, and what it depends on. Link the epic or specification it comes from
rather than restating it; the `derived_from` relation already carries the edge.*

## Acceptance

*The conditions under which this is done, each one something a person or a check can observe. "Works
correctly" is not one of them.*

## Out of Scope

*What a reasonable reader would expect to be included and is not — the boundary that stops this
story quietly becoming an epic.*

## Ambiguities

*Each gap this story found and did not close, classified. `inferable` — the answer is already
written down, so give the `path:line` or the artifact id that settles it.
`requires-stakeholder-input` — nobody here can decide it, so name who does, and raise that entry as
a `decision-blocker` with a `blocks` edge to this story, or it is a sentence somebody improvises
later.*

## Open Questions

*Anything still undecided belongs in `## Ambiguities` above, classified and with its citation or its
decider. Keep this section for a question that is neither — an unowned question is a story that
stalls without anybody noticing.*
