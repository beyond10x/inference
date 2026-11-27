---
format: aep.planning-md/3
id: approval-record:wave-2026-11-27-one-call-ordered
kind: approval-record
status: draft
title: The one-call SDK, the tool loop's bound per input and the sentence clips go out in one pull request (2026-11-27)
relations:
- decides: story:sdk-one-call
- decides: story:tool-loop-bound-counts-rounds-per-input
- decides: story:sentence-clips-with-durations
revision: 1
---
## Decision

On 2026-11-27 the owner allowed the amendment of `ess/domains/graph.yaml` that the tool loop's
bound waited for since 2026-11-25: a permission check had refused that edit, and only the owner
lifts such a refusal (decision D22, option A). The amendment is its own commit, before the code,
as approval-record:tool-loop-bound-ordered-2026-11-25 asks.

The same morning the operator decided how story:sentence-clips-with-durations goes on after its
scope read found that no sentence segmenter exists and that its first acceptance needed the frames
of one synthesis told apart by segment (decision D64, option A): the story is revised with the
scope read's defaults and built in the same pull request as story:sdk-one-call and
story:tool-loop-bound-counts-rounds-per-input. Option B was to leave it out.

## What this covers

- story:sdk-one-call: merged with `main`, and delivered together with the bound round.
- story:tool-loop-bound-counts-rounds-per-input: one implementor round, one adversary pass, one
  correction round on the adversary's warning.
- story:sentence-clips-with-durations: the revision, one implementor round, one adversary pass,
  one correction round on its notes.

## What this does not authorise

No tag and no release. story:speak-text-file-to-audio-file stays held: it also depends on
story:local-tts-adapter-under-licence-policy, which waits on its own decision.
