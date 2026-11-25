---
format: aep.planning-md/3
id: story:local-tts-weights-cases-run-in-a-lane
kind: story
status: draft
title: The cases of the local TTS adapter on its weights run in a lane within the budget
tags:
- from-finding
relations:
- serves: vision:inference
revision: 1
---
## Context

Found when the last correction of story:local-tts-adapter-under-licence-policy was run
(branch `impl/local-tts-adapter` at `c281a8b4c`). The example `local-voice-cascade` has 16 ignored
cases that speak through the sherpa-onnx library built with text-to-speech and the pinned
checkpoint: the binaries `adversary_w2u4_synthesis` and `security_w2u4_tts_boundary`, and two cases
of `local_adapters`. No task of `Taskfile.yml` and no workflow runs them; each run so far was
started from a script outside the repository.

Measured on the build node, all 16 passed in both runs:

- With the pool giving the run several slots: 96.2 s of wall time, 258 s summed over the tests, the
  slowest 41.0 s.
- With one slot, one test at a time: 371.7 s of wall time. The slowest was
  `a_long_text_without_a_sentence_end_is_spoken_to_its_end` at 54.6 s, then
  `a_one_sentence_segment_is_delivered_once_and_two_sentences_part_by_part` at 46.3 s and
  `a_process_that_ends_as_the_thread_of_a_dropped_synthesis_ends_exits_with_status_0` at 39.7 s.
  13 of the 16 took more than 5 s.

The runner ends a test at 60 s, so the slowest case had 5.4 s left on a loaded node. The cause of
the one-at-a-time run was not determined.

Three further ignored cases of `local_adapters` need a loopback llama-server, which no test
starts. Their answer length was lowered from 24 to 18 tokens by a sum over measured deltas
(51.7 s with every delta at the longest measured); no run has confirmed it.

## Planned scope

- One task that fetches nothing new (`task sherpa-onnx-tts` provides library and checkpoint) and
  runs the 16 cases under a nextest profile that ends a test at 60 s and prints the ten slowest.
- The cases are shortened until the lane ends within 120 s with one test at a time: shorter texts
  where the assertion does not need the length, and one synthesis shared by the cases that only
  read its audio.
- The three server cases: a decision whether a task starts the loopback server for them or they
  are removed; until then the page that names them says no gate runs them.
- Whether a workflow runs the lane is decided with the owner's ruling on how the adapter ships.

## Acceptance

- `task <the lane>` on the build node with one slot ends within 120 s, no case over 30 s, and
  prints its ten slowest (state transition: the lane is added).
- A case of the 16 that is removed or renamed without the lane following fails the lane's check of
  its own list (state transition: a case is renamed).

## Out of scope

- The adapter's behaviour and its licence entries.
- The gate's test step, which runs none of these cases.
