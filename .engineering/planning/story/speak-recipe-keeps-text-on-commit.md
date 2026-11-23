---
format: aep.planning-md/3
id: story:speak-recipe-keeps-text-on-commit
kind: story
status: implemented
title: The speak recipe keeps the text an agent commits and synthesises it
summary: Agent::commit sends buffered words to the recipe's own text ingress and keeps them when the send fails
relations:
- decomposes: epic:sdk
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-23T12:35:47Z", actor: "human:operator", revision: 2, decided_on: {"recorded":{"test_result":1}}}
- {from: "proposed", to: "active", at: "2026-11-23T12:35:48Z", actor: "human:operator", revision: 3, decided_on: {"recorded":{"test_result":1}}}
- {from: "active", to: "implemented", at: "2026-11-23T12:35:48Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Context

Review of 0.32.0, finding 3: the `speak` recipe accepts text and loses it on commit. Over
`Agent::from_recipe(Recipe::speak())`, `send_text()` succeeded; `commit()` took the buffered
words out of the buffer and sent them to an ingress named `user_input`, while `speak` exposes its
text ingress as `text`. The first `commit()` returned the session's missing-ingress error, and a
retry returned success with the buffer already empty, so the words never reached the
synthesiser (`crates/inference/src/agent.rs`, `Agent::commit`; `crates/inference/src/recipe.rs`,
`Recipe::speak`; `crates/inference/src/recipe/topology.rs`, `speak`).

## Planned scope

- `Agent::commit` sends the buffered words to the recipe's own text ingress: its exposed text
  input, `user_input` when the recipe has one. Into a text ingress whose node is role-bound to
  `tts`, as `speak`'s, the words go as one committed segment of a generation of their own
  (`GenerationStarted`, `TextDelta`, `TextSegmentCommitted`), which a TTS node synthesises; into
  any other, as one `TranscriptCommitted`, as before.
- The words leave the buffer only once the ingress took them. A recipe with no text ingress, or a
  refused send, fails the commit and keeps them, so a retry fails the same way instead of
  succeeding with nothing sent.

## Acceptance

- `send_text("hello there")` and `commit()` on an agent over `speak` and the mock synthesiser
  succeed, and the session's usage record of the synthesis counts the 11 input characters of
  that text.
- On an agent over `transcribe`, which exposes no text ingress, `send_text` followed by two
  `commit()` calls fails both commits: the words stay buffered after the first failure.
