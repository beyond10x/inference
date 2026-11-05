---
format: aep.planning-md/3
id: review-result:adversary-topology-latency-pass-1
kind: review-result
status: active
title: 'Adversary pass on the topology latency report: the cascade arm stops sending input while a step blocks'
summary: One blocker escalated, the unit leaves the wave; three findings fixed by the one correction
relations:
- reviews: story:voice-topology-latency-per-step
revision: 1
---
## Pass

This is the adversary pass on the topology latency report at `7664ed9`. The cases are in
`970dd59`, in `adversary1_topology_busy_step.rs`.

verdict: NEEDS-CHANGE
cases: executed 77→79, red 2
origin: introduced 4 / pre-existing 0 / undecided 0

## Cases

- **Red, blocker: barge-in to stop shrinks while the LLM is busy.**
  - The LLM takes 200 ms to give its second segment, and the arm sends no input meanwhile.
  - The barge-in's frames go out late, in one burst, so barge-in to stop reads 10 ms where 30 ms
    is right.
- **Red: a cascade with no run is labelled native.** `Comparison::ran` inferred the topology from
  the first run.
- **Judgement: no column measures playout.** `ClientPlayout` plays nothing, so "playout start"
  equals TTS first audio, and the cascade's barge-in to stop is a constant of the detector.
- **Docs:** the environment table did not name `compare` among the readers of the local model
  variables.
- **Held:**
  - the step-to-step tolerance is exact, against a mutant that shifts one step's delay into the
    next and one that flattens every step;
  - `n/a` against `- 0/N` and `measured/runs`;
  - every skip path, with no key in the output and no keychain prompt;
  - `--help` equals the guide byte for byte.

## Routing

The unit's one correction (`859e1b3`, `2ff98aa`) settles the second case, the judgement and the
docs finding:

- the topology is given explicitly;
- the cascade uses the native client playout model, and the "playout start" column is dropped;
- the environment table names `compare`.

The blocker is escalated. Its fix runs the bindings on worker threads and needs a test-side change
to the adversary's case, which advances the shared fake clock from inside the LLM. The unit leaves
the 0.16.0 wave with that design recorded on the story.

```findings
[{"file":"examples/realtime-voice/src/compare.rs","line":1034,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"while a cascade step blocks in its binding the arm sends no input, so the barge-in's frames go out late in one burst and barge-in to stop reads 10 ms instead of 30 ms"},{"file":"examples/realtime-voice/src/compare.rs","line":1592,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"Comparison::ran infers the topology from the first run, so a cascade with no run is labelled native"},{"file":"examples/realtime-voice/examples/compare.rs","line":262,"category":"judgement","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"with ClientPlayout the example's playout start equals TTS first audio and the cascade's barge-in to stop is a detector constant"},{"file":"website/docs/reference/environment.md","line":1,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"the Local models table names only talk, while compare with feature local reads the same variables"}]
```
