---
format: aep.planning-md/3
id: review-result:elevenlabs-route-p2-acceptance-round-1
kind: review-result
status: active
title: Plan critic, acceptance, round 1, on wave P2 of epic:elevenlabs-route
relations:
- reviews: epic:elevenlabs-route
revision: 1
---
needs-revision

task:release-elevenlabs-route-p2 — the acceptance requires `git merge-base --is-ancestor` to show the "merge commits of story:elevenlabs-agent-session-overrides" are ancestors, but that story is archived and out of the wave, so it has no merge commit and the check cannot pass; name only story:async-speech-handles and the other stories that are in the wave — .engineering/planning/task/release-elevenlabs-route-p2.md:36-38 (the scope at :27 repeats the same reference)

task:release-elevenlabs-route-p2 — the acceptance names no tag name or version, so "the tag exists on `origin`" cannot be checked against release 0.29.0 in the title; state the tag (v0.29.0) and a `CHANGELOG.md` section for 0.29.0 — .engineering/planning/task/release-elevenlabs-route-p2.md:35

task:release-elevenlabs-route-p2 — the acceptance demands a `Breaking` heading "naming the async handle change", but the async story's design keeps the blocking `Iterator` and `next_event(Duration)` forms and adds a default method, so nothing breaks; state what is breaking, or drop the heading from the acceptance — .engineering/planning/task/release-elevenlabs-route-p2.md:38, .engineering/planning/story/async-speech-handles.md:123-127

story:async-speech-handles — "The backend-neutral checks of the three contracts pass through the async form" names no checks, and the three contracts have different check sets (`tts::conformance`, `stt::conformance`, `inference_duplex::conformance`), so nobody can say which run and through what; name the suites and the test that runs them through the async form — .engineering/planning/story/async-speech-handles.md:120

story:compare-elevenlabs-cascade-and-ollama — "its `first audio` is at least the fixture's injected delays and does not include a VAD quiet window" sets only a lower bound, which a run that does include the 500 ms quiet window also satisfies; state an upper bound (injected delays plus a stated margin below 500 ms) so the manual-commit claim can fail — .engineering/planning/story/compare-elevenlabs-cascade-and-ollama.md:77-79

story:compare-elevenlabs-cascade-and-ollama — the `--clips` fixture bullet joins three independent outcomes (each clip runs on each arm, the table shows p95, `--runs-out` writes one JSON line per run), so one can pass while another fails; split them into separate bullets — .engineering/planning/story/compare-elevenlabs-cascade-and-ollama.md:80-81

story:status-page-voice-adapters — the first bullet runs to several sentences covering `docs-check`, a "Since" on every row, a per-binding generator rule and a test against the introducing `CHANGELOG.md` section, so it is not one statement; split it into the `docs-check` pass, the "Since" on every row, and the test against the introducing section — .engineering/planning/story/status-page-voice-adapters.md:50-54

story:status-page-voice-adapters — "ElevenLabs realtime STT shows 'Since 0.28.0' only once that tag exists; the story waits" is a precondition and a prohibition, not an outcome a reviewer can observe; state what the pages show for ElevenLabs STT once v0.28.0 exists and what check run proves it — .engineering/planning/story/status-page-voice-adapters.md:58-59

story:realtime-voice-barge-in-stop-flake — "passes 50 runs in a row alone and 5 full-suite runs in a row" also passes if the assertion is deleted or loosened, which the scope forbids ("without loosening what it proves"); add the bound or clock the test must still enforce — .engineering/planning/story/realtime-voice-barge-in-stop-flake.md:31, :26-27

task:voice-latency-comparison-run — the acceptance asks for "the run count" per arm but not the scope's minimum of 3 clips and 10 runs per clip and arm after a warm-up, and not the "not run" listing for arms without a key, so any count or any omitted arm passes; bind those minimums and the "not run" listing — .engineering/planning/task/voice-latency-comparison-run.md:35-36, :20-31

What I read: all 6 ids (3 stories, 2 tasks plus the release task), each with `aep plan artifact show <id>`; `aep plan artifact kinds`, `aep plan artifact lifecycle story`; tree checks with `git grep` for the conformance suites, `cargo tree` for `tokio` (0 matches today), `Taskfile.yml` for `docs-check`, and the assertion at `adversary2_topology_workers.rs:494`.

What I could not establish:
- Whether `inference_duplex::conformance` exercises the speech-session handle in a way an async form can reuse.
- Whether the `cargo tree` bullet in story:async-speech-handles guards anything, since it already reads "no tokio" before the work. I left it out as a standing constraint.
- Out of my lane: story:async-speech-handles lists story:elevenlabs-agent-session-overrides, an archived story, as a dependency in its scope text (`.engineering/planning/story/async-speech-handles.md` "Planned scope", last bullet). The same sentence in the release task's scope is the other half of this; both are for the scope and design critics.
- Also out of my lane: the scope bullets of story:compare-elevenlabs-cascade-and-ollama on the `generator()` and `key()` move and the docs have no acceptance bullet. I treated that as a coverage question.

```findings
- file: .engineering/planning/task/release-elevenlabs-route-p2.md
  line: 36
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the acceptance requires the merge commits of story:elevenlabs-agent-session-overrides to be ancestors of the tag, but that story is archived and out of the wave, so the check cannot pass; name only stories that are in the wave"
- file: .engineering/planning/task/release-elevenlabs-route-p2.md
  line: 35
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance names no tag name or version, so 'the tag exists on origin' cannot be checked against release 0.29.0; state the tag and the CHANGELOG.md section"
- file: .engineering/planning/task/release-elevenlabs-route-p2.md
  line: 38
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance demands a Breaking heading naming the async handle change, but the async story keeps the blocking forms and adds a default method, so nothing named is breaking; state what is breaking or drop the heading"
- file: .engineering/planning/story/async-speech-handles.md
  line: 120
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "'The backend-neutral checks of the three contracts pass through the async form' names no checks; the three contracts have different check sets, so name the suites and the test that runs them through the async form"
- file: .engineering/planning/story/compare-elevenlabs-cascade-and-ollama.md
  line: 77
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "'its first audio is at least the fixture's injected delays and does not include a VAD quiet window' sets only a lower bound, which a run that includes the 500 ms quiet window also satisfies; state an upper bound so the manual-commit claim can fail"
- file: .engineering/planning/story/compare-elevenlabs-cascade-and-ollama.md
  line: 80
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the --clips fixture bullet joins three independent outcomes (each clip on each arm, p95 in the table, one JSON line per run in --runs-out), so one can pass while another fails; split into separate bullets"
- file: .engineering/planning/story/status-page-voice-adapters.md
  line: 50
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the first bullet holds several sentences (docs-check, a Since on every row, a per-binding generator rule, a test against the introducing CHANGELOG section), so it is not one statement; split it"
- file: .engineering/planning/story/status-page-voice-adapters.md
  line: 58
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "'shows Since 0.28.0 only once that tag exists; the story waits' is a precondition and a prohibition, not an observable outcome; state what the pages show for ElevenLabs STT once v0.28.0 exists and what check proves it"
- file: .engineering/planning/story/realtime-voice-barge-in-stop-flake.md
  line: 31
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the 50 and 5 green runs also pass if the assertion is deleted or loosened, which the scope forbids; add the bound or clock the test must still enforce"
- file: .engineering/planning/task/voice-latency-comparison-run.md
  line: 35
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance asks for 'the run count' per arm but not the scope's minimum of 3 clips and 10 runs per clip and arm after a warm-up, nor the 'not run' listing for arms without a key, so any count or omitted arm passes"
```