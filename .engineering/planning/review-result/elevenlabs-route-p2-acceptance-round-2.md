---
format: aep.planning-md/3
id: review-result:elevenlabs-route-p2-acceptance-round-2
kind: review-result
status: active
title: Plan critic, acceptance, round 2, on wave P2 of epic:elevenlabs-route
relations:
- reviews: epic:elevenlabs-route
revision: 1
---
needs-revision

story:async-speech-handles — the first bullet's check (20 sessions on a current-thread runtime, "without `spawn_blocking`", "every session delivers all its events") also passes for a default form that blocks the runtime thread inside each call, because serial blocking still delivers every event, so a thread-per-call implementation is not told apart from a woken one; add a condition that fails then, such as a ticker task that must keep running while the sessions wait — .engineering/planning/story/async-speech-handles.md:42-44
story:async-speech-handles — the first bullet says "One test" drives the sessions "against their fixture servers", while the design section puts that test in `crates/inference-testkit/tests/` against the testkit mocks plus one per ElevenLabs handle under `adapters/inference-elevenlabs/tests/`, so the number and place of tests the bullet names disagrees with the section below it; state which tests exist and where in the bullet — .engineering/planning/story/async-speech-handles.md:42-44, :59-61
story:status-page-voice-adapters — the fourth bullet joins three outcomes in two sentences (the detail text carries the release, no new field is added, each item is held by an existing non-ignored test), so one can pass while another fails; split it into separate bullets — .engineering/planning/story/status-page-voice-adapters.md:39-41

Round 1 findings of my lane that are resolved:
- **`task:release-elevenlabs-route-p2`:** the archived story is gone from the acceptance. The tag is `0.29.0`, which matches the tags in the tree (`0.9.0`, `0.8.1` and so on). The `## [0.29.0]` section is required. `### Breaking` is conditional on a public type changing, which is checkable.
- **`story:async-speech-handles`:** it names `tts::conformance`, `stt::conformance` and `inference_duplex::conformance`, and all three exist in the tree.
- **`story:compare-elevenlabs-cascade-and-ollama`:** the upper bound of delays plus 250 ms is stated, and the `--clips`, p95 and `--runs-out` bullets are separate.
- **`story:status-page-voice-adapters`:** the first bullet is split, and the 0.28.0 precondition is now an observable outcome.
- **`story:realtime-voice-barge-in-stop-flake`:** it now requires a run with the stop delayed past the 30 ms bound, recorded in the change.
- **`task:voice-latency-comparison-run`:** it binds at least 3 clips, 10 runs per clip after a warm-up, and "not run: no key".

What I read: all 6 ids (4 stories, 2 tasks) in full with `aep plan artifact show`, plus `review-result:elevenlabs-route-p2-acceptance-round-1`, `aep plan artifact lifecycle task`, and `git tag`, `grep` and `git grep` and a `cargo tree | grep -c tokio` count in the plan checkout.

What I could not establish:
- Whether the `cargo tree` bullet in `story:async-speech-handles` guards anything, since it already shows no `tokio` today (count 0). I left it as a standing constraint, as in round 1.
- Out of my lane: scope bullets of `story:compare-elevenlabs-cascade-and-ollama` with no acceptance bullet, namely the `generator()` and `key()` move, the guide naming Ollama and the commit-mode statement. That is a coverage question for the scope critic.

```findings
- file: .engineering/planning/story/async-speech-handles.md
  line: 42
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the first bullet's check (20 sessions on a current-thread runtime without spawn_blocking, every session delivers all events) also passes for a default form that blocks the runtime thread inside each call, so a thread-per-call implementation is not told apart from a woken one; add a condition that fails then, such as a ticker task that must keep running while the sessions wait"
- file: .engineering/planning/story/async-speech-handles.md
  line: 42
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the first bullet says 'One test' against the ElevenLabs fixture servers while the design section places the test in crates/inference-testkit/tests against the testkit mocks plus one per ElevenLabs handle, so the bullet and the section disagree on which tests exist and where; state it in the bullet"
- file: .engineering/planning/story/status-page-voice-adapters.md
  line: 39
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the fourth bullet joins three outcomes in two sentences (detail text carries the release, no new field, each item held by an existing non-ignored test), so one can pass while another fails; split it into separate bullets"
```