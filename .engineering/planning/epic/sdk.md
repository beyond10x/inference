---
format: aep.planning-md/3
id: epic:sdk
kind: epic
status: draft
title: An SDK that makes a voice agent ten lines and any stack a configuration change
summary: 'One inference crate: one-call functions, handles, recipes, stacks from config, tool macro, scripted testing, usage and cost'
relations:
- serves: vision:inference
revision: 8
---
## Context

The `inference` crate is reserved by the design as "Optional ergonomic re-exports"
(`docs/design/inference.md:245`) and does not exist. Applications assemble the contracts, the
runtime and the session themselves: 188 lines of payload enum, 176 lines of port declarations, a
352-line event loop and 721 lines of pricing in the examples, and no example uses the session facade
(`docs/design/sdk.md` §1). Four gaps sit under that: the model nodes live in the testkit (G1), no
node runs a native speech model (G2), no example uses the facade (G3), and no shared payload exists
(G4).

Serves O1 and O5 of `vision:inference`: adapters usable alone, and one application vocabulary over
interchangeable topologies.

## Outcome

One dependency and one import give an application five layers (one call, handles, recipes, graph,
contracts), each one method away from the next, with the adapter for each role chosen by a stack
file or environment, usage and cost on every handle, and an offline test path through the same code.
Design: `docs/design/sdk.md`. Specification: `ess/domains/sdk.yaml` (`inference.sdk`).

## Exit

The targets of `docs/design/sdk.md` §2, each checked by the stories that name it:

| target | story |
|---|---|
| T1 voice agent with barge-in and its usage printed (with the cost when a price document is named), in at most 10 lines without a tool, at most 15 with one, counting the non-blank, non-comment lines of the whole example file, `#[inference::main]` and the tool function included | story:sdk-examples (the count), story:sdk-tool-macro (`#[inference::main]`), story:sdk-agent (`Agent::voice_with`), story:sdk-usage-cost (`Usage: Display`) |
| T2 cascade or native model by configuration alone | story:sdk-examples |
| T3 one `inference*` key and no contract crate under `[dependencies]` per SDK example (`[dev-dependencies]` may list contract crates for tests) | story:sdk-examples (`sdk-voice` and `realtime-voice`; `local-voice-cascade` keeps three adapter crates, named there); story:sdk-crate (no adapter crate without a feature) |
| T4 offline tests through the same code path | story:sdk-testing (scripted stack, `FakeClock`, `FakeHost`), story:sdk-examples (the T1 example driven through the `run` that `#[inference::main]` generates) |
| T5 every layer one method away | story:sdk-agent (`session`, `pipeline`, `describe`, `Event::raw`), story:sdk-one-call (the same on `Chat`, `Transcriber`, `Voice`), story:sdk-recipes (`declaration`), story:sdk-stack (`Stack::adapter(Role)`) |
| T6 usage and cost on every handle | story:sdk-usage-cost |
| T7 every error names its fix | story:sdk-crate (`Display` of every variant), story:sdk-stack (`AdapterNotCompiled` raised by a real resolve) |

## Ordering

The `inference` crate's `lib.rs`, `prelude.rs` and `Cargo.toml` are written once, by
story:sdk-crate, with every module, re-export, feature and dependency the later stories need; each
later story edits only the files its scope names. story:sdk-duplex-node follows story:sdk-model-nodes
because both edit `crates/inference-pipeline/src/nodes/mod.rs` and `crates/inference-pipeline/Cargo.toml`.
`Cargo.lock` is in no story's scope: a merge conflict on it is resolved by regenerating the lockfile with the build after the merge, never by ordering stories.

## Open

Q5 of `docs/design/sdk.md` §12: the facade's `cost` feature prices audio tokens as `price_unknown`
(`crates/inference-facade/src/cost.rs:24-27`), so `cost()` of a native speech session cannot equal
what `examples/realtime-voice/src/usage.rs` computes for audio. The default taken: audio lines stay
`price_unknown`, never zero, and `realtime-voice` keeps its own tally until `llm-cost` prices audio.

## Out of scope

A server, CLI or network transport; bindings for other languages; provider fallback, retries or
routing by price; structured output; publishing to crates.io (`docs/design/sdk.md` §11).


## Review

Four plan critics (scope, acceptance, design, parallel safety) approved this epic, its twelve
stories and `api-design:sdk` in round 24; the one remaining warning was fixed in round 25.
Operator instruction (2026-10-22): the design lands on `main` through a bot pull request after
every check is green, and the stories stay draft until a wave is proposed for them.
