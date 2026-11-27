---
format: aep.planning-md/3
id: story:quality-live-lane-on-the-workstation
kind: story
status: implemented
title: A local lane runs the keyed live measures on the workstation, says its count first and keeps its records local
relations:
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-27T10:16:47Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-27T10:16:47Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-27T11:37:21Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Context

Sixteen measures of `quality/` need a hosted call; none has a value, because no lane runs them
(`quality/not-measured.md`). The owner decided on 2026-11-27 (D43 A) that keyed runs execute on
this workstation: keys come from the local keychain, are never printed and never reach a commit
or a log. Two keys are in the keychain today, for OpenAI and for ElevenLabs. What a run spends
is the owner's word each time (money): the lane says its count before it calls anything and does
nothing without an explicit flag.

## Planned scope

- `task quality-live`, outside `task check` and outside every workflow. It runs the four live
  measures the plan names for OpenAI and ElevenLabs: `openai-speech-live` (1 request),
  `openai-gpt-live` (3 sessions), `elevenlabs-live-example` (2 connections),
  `elevenlabs-cancel-probe` (4 connections). Check each name and count against
  `quality/adapters.ratchet.yaml` and the commands' sources; the plan wins where they differ.
- Before it starts anything it prints the hosted calls it will make, per provider. Without
  `YES=1` it stops there with exit 2 and calls nothing.
- Keys are read from the Secret Service by their attributes (`secret-tool lookup …`), only into
  the environment of the one command that needs it; a missing key skips that provider's
  measures with a line that names the key's label and the measure, and the lane goes on. No key
  is printed, logged, written to a file or passed in an argument. `ELEVENLABS_VOICE_ID` comes from
  the environment; without it `elevenlabs-live-example` is skipped by name.
- The records of the run are written with `inference-docs assess` (class `local`) to
  `.ratchet/records/<plan>/<assessment>/<run id>.evidence.json`, a git-ignored directory. Nothing
  of a local run is committed.
- Tests without a hosted call: the count and the refusal without `YES=1`; key reading against a
  fake `secret-tool` on `PATH` (the key reaches the child's environment and no output); a missing
  key skips by name; records written from captured outputs of the four commands (fixtures).
- `quality/README.md`: how to run the lane and what it spends; `.gitignore`: `.ratchet/`.

## Acceptance

- `task quality-live` without `YES=1` prints the count per provider and calls nothing (state
  transition: none).
- With a fake `secret-tool` and stub commands, `YES=1` runs the four measures, each command sees
  only its own key, no output holds a key, and one record per measure is written under
  `.ratchet/records/` (state transition: the local records directory gains one file per
  measure).
- A missing key or voice id skips the measures that need it by name and the others run (state
  transition: as above, for the others).
- The first live run happens only after the owner's yes to its count, and its records stay
  local (state transition: none in the repository).

## Out of scope

`compare` and `compare-cascade`; Cartesia, Deepgram, Phonic, Deepslate and AWS; keyed runs on the
forge; committing a local value as a baseline; scheduling the lane.
