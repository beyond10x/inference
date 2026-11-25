---
format: aep.planning-md/3
id: story:release-changes-on-the-documentation-site
kind: story
status: implemented
title: The documentation site says what the next release changes
relations:
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T13:18:38Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-25T13:18:38Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-25T19:04:51Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":2,"verification":1}}}
---
## Context

The section `[Unreleased]` of `CHANGELOG.md` on `main` at
`6ca6a2509b56b41d9a8361aa12d9ea4e2cf67332` is what the next release brings. A search of
`website/docs/` at that commit for the names the section introduces found:

- `agent::AgentError::SignedUrlSpent` (breaking, `inference-elevenlabs`): on no page.
  `reference/environment.md` says that a model opens one session with a signed URL; no guide and
  no error page says what the second open returns or what a caller does then.
- `LiveClient::with_close_timeout`, `OpenAiLive::with_close_timeout` and `CLOSE_TIMEOUT`
  (`inference-openai`): on no page.
- `CLOSE_GRACE` and what a drop of a synthesis inside a Tokio runtime waits for and sends
  (`inference-elevenlabs`, `inference-cartesia`): on no page.
- `Settings::silent_turn` of the example `repo-podcast`: not on `examples/repo-podcast.md`.
- `quality/` and `ratchet.toml`: no page names the directory or the test that keeps it true.
- The bound of `task test-run` and its exit 201: on `guides/run-the-checks.md` already.
- The `compare` example with a signed URL measuring its first run only: on
  `reference/environment.md` already.

Four pages of the site changed between the tag `0.33.0` and that commit
(`guides/compare-voice-providers.md`, `guides/run-the-checks.md`,
`guides/talk-to-a-hosted-voice-model.md`, `reference/environment.md`).

## Acceptance

Given `main` with this story merged, when a reader looks for a change of the `[Unreleased]`
section that alters what a caller writes, gets or waits for, then the page of
`website/docs/` that covers the crate, the example or the task says it, in words that agree with
the source at that commit, and `task check-source` passes with the generated pages regenerated
and not edited by hand.

## Planned scope

- The breaking change of the signed URL: what spends it, the error a later open returns, and what
  the caller does (a model with a new signed URL), where a reader of the ElevenLabs agent looks:
  the error reference, the adapter reference, and the guide that opens a hosted voice model where
  it names the agent.
- The close timeout of a GPT-Live session: the default, the two setters, and the longest a close
  takes.
- A dropped synthesis of Cartesia and of ElevenLabs text to speech: how long the drop holds its
  thread inside and outside a Tokio runtime, and what the server sees.
- `Settings::silent_turn` on the page of the `repo-podcast` example.
- The quality documents: where they are, what they hold, and that a test of `task test-run`
  keeps them true, on the guide that runs the checks.
- A page the generator writes is changed through its generator or the source the generator reads.

## Out of scope

- The version numbers the pages print: the release commit changes them.
- `website/docs/status/limitations.md`: no line of it changes in this story.
- A change of a crate's behaviour, API or documentation comments beyond what a generated page
  needs to read.
- The local text-to-speech adapter and the one-call recipes: not on `main`.
- A new measure or a number of latency: the quality documents hold them.
