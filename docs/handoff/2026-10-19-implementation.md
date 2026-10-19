# Implementation handoff, 2026-10-19

State when the implementation session stopped, after release 0.4.0. Everything below is in the
repository or the planning store.

## Done

| item | state |
|---|---|
| release | `0.4.0` tagged on `main` and published as a GitHub release; `CHANGELOG.md` carries the entry |
| gate | `task check` exit 0 on `main`, 961 tests |
| conformance | the testkit target answers 296 of 317 synthesized scenarios; the 21 left are `inference.session`, listed in `ess/SKIPPED.md` |
| stories | 22 implemented, 5 draft, 1 active (`story:ess-defect-fixes`, shown `blocked: dependency`; not touched by the implementation waves) |
| crates | `inference-core`, `-audio`, `-text`, `-llm`, `-stt`, `-tts`, `-pipeline`, `-runtime`, `-tools`, `-testkit`; example `stt-only` |

Each story was implemented in a wave: one implementor per story in its own managed worktree, then
one or two adversary passes. Every adversary pass is a `review-result:adversary-<story>-pass-<n>`
artifact, and its findings are `review_outcome` evidence on the story (`fixed`, `no-op`).

## Left, in order

1. `aep plan artifact waves --kind story --status draft` gives the order:
   `story:endpoint-controller` and `story:tool-safe-retries`, then `story:tool-feedback-loop`, then
   `story:tool-call-interruption`, then `story:cascade-read-only-tool`.
2. `story:tool-call-interruption` carries `## Gaps left by wave 14`:
   - the check-then-send window on cancel in the tool nodes;
   - a run that waits for a cancel when a consumer node fails.
3. `epic:session-facade` (P4) after those: it has no stories yet. Its stories wait for the
   UNMAPPED Session to Run relation to be settled in `ess/`. They are what answers the 21
   unsupported session scenarios.
4. Stop at the decision-blocked epics:
   - `decision-blocker:first-adapters` holds the local LLM, STT and TTS adapters and backend
     replacement;
   - `decision-blocker:native-duplex-evidence` holds duplex research and native voice;
   - `decision-blocker:reference-hardware` and `decision-blocker:weight-licenses` stay open;
   - hosted and scale-out (P7) is not started.

## Facts a next session needs

- The end of input is a control event of kind `stop` (`inference_core::envelope::ControlKind`).
  `ess/` declares no control type; a control event is told from content by its kind.
- `RunController::drive` drains nodes upstream first; every output sender refuses a repeated or
  decreasing sequence among a stream's content events.
- The bot route admits `merge --no-ff` and a plain merge but not `--ff-only`. The 0.4.0 release
  therefore sits on a merge commit, which carries a git note.
- `b10x-gates scan-text` and `b10x-gates gh` need the trusted policy file. Where the default
  location has none, pass the policy repository's copy with `--policy`.
