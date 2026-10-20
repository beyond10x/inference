---
format: aep.planning-md/3
id: epic:contracts-and-testkit
kind: epic
status: implemented
title: Contracts and testkit (P0)
summary: Core, text, audio, STT, LLM and TTS contracts plus mock nodes; the testkit answers the model and stream scenarios.
relations:
- derived_from: architecture-design:inference
- serves: vision:inference
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T07:56:15Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":2}}}
- {from: "proposed", to: "active", at: "2026-10-20T07:56:16Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1,"review_outcome":2}}}
- {from: "active", to: "implemented", at: "2026-10-20T07:56:18Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1,"review_outcome":2}}}
---
## Context

Stage P0 of the roadmap, "vocabulary and contracts" (`docs/design/inference.md:1158`): the crates
`inference-core`, `inference-text`, `inference-audio`, `inference-stt`, `inference-llm`,
`inference-tts` and `inference-testkit` (lines 233-244; §5.3 at line 273 says to begin with these).
Contracts model externally observable behaviour, not neural architecture (ADR-002, line 1187), and
a persistent connection is not proof of a streaming model (§1.2, lines 43-54). The core and the
domain contract crates carry no concrete implementation (owner, 2026-10-18).

The nouns are declared in `ess/domains/model.yaml` and `ess/domains/stream.yaml`
(`executable-system-specification:inference`). Two declarations arrive with this epic, spec first:
the event envelope and its timebase enum in `ess/domains/stream.yaml`
(`story:inbound-text-spans-and-envelope`) and the audio nouns in `ess/domains/audio.yaml`
(`story:audio-vocabulary`).

## Outcome

A consumer depends on one domain contract crate and the testkit, writes a mock node against it,
and the §23.1 contract tests pass; the testkit is the conformance target that answers the
`inference.model` and `inference.stream` scenarios.

## Scope

Each item names the story that claims it; two items are narrowed, with the reason stated.

- `inference-core`: identifiers (`story:workspace-skeleton`); the capability enum (§8.1, lines
  419-427), modality, locality and execution form (`inference.model`; `story:workspace-skeleton`);
  the event envelope (§6.2, lines 303-318) with the timebase enum it names (§11.5, lines 643-645)
  and the `kind` that keeps a control event apart from content (line 297;
  `story:inbound-text-spans-and-envelope`); `Generation`
  (`story:outbound-text-generations-and-segments`); typed errors, each domain's `errors:` landing
  with the crate that carries its entities (`inference.model.NonPositiveConcurrency` with
  `story:workspace-skeleton`, the six `inference.stream` errors with the two stream stories).
  Control events are narrowed to what `ess/` declares: the start, stop, commit, revise, cancel and
  error events of §6.1 (line 297) are the `inference.stream` lifecycle events and typed errors
  `ess/domains/stream.yaml` already declares, and the envelope's `kind` is how a consumer tells
  them from content (`TranscriptHypothesis`, `TextDelta`); no separate control-event type is
  declared in `ess/` and none is built in this epic.
- `inference-text`: spans and output segments with their lifecycles (`inference.stream`,
  §11.1-11.2; `story:inbound-text-spans-and-envelope`,
  `story:outbound-text-generations-and-segments`).
- `inference-audio`: frames, formats and sample clocks in audio frame time (§6.1 line 293, §6.3
  lines 324-325, §11.5 lines 643-645; `story:audio-vocabulary`). Narrowed from "timebases": the
  timebase enum lives in `inference-core` beside the envelope that names it, because
  `inference-audio` depends on `core` and not the reverse (line 235); `inference-audio` carries the
  audio-frame-time reading, the sample clock position, and none of the other three clocks.
- `inference-stt`, `inference-llm`, `inference-tts`: the domain traits of §7.1-7.3, each with output
  streaming, incremental input and input-during-output declared separately (lines 358, 372 and
  385-386; `story:audio-and-stt-contract`, `story:llm-contract`, `story:tts-contract`).
- `inference-testkit`: the target that answers `ExecuteCommand` and `QueryView` for the two domains
  above, one module per domain, and the runner route of the Exit
  (`story:testkit-conformance-target`; the `inference.stream` module is filled by the two stream
  stories); the cargo features `stt`, `llm` and `tts` the mocks sit behind
  (`story:testkit-conformance-target`); mock STT, LLM and TTS nodes
  (`story:audio-and-stt-contract`, `story:llm-contract`, `story:tts-contract`).

## Exit

- The synthesized suite (`ess verify conform synthesize --path ess --target ir`) is executed by the
  testkit's own Rust runner, which writes one `ess-conformance-results/1` document, and
  `ess verify conform report --suite <suite.json> --results <results.json> --report-out <report.json>`
  turns it into the `ess-conformance-report/2` that `aep plan artifact evidence --from` records
  (`ess verify conform run` carries no Rust target on ess 0.60.0, so `run` is not the route;
  `story:testkit-conformance-target`). That report passes every `inference.model` and
  `inference.stream` scenario (3 and 60 synthesized on 2026-10-18), among them
  `inference.stream.ReviseSpan/outcome/immutable`,
  `inference.stream.Span/state/Committed/refuses/inference.stream.ReviseSpan`,
  `inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.QueueForSynthesis`,
  `inference.stream.OutputSegment/state/Played/refuses/inference.stream.DiscardSegment` and
  `inference.stream.Generation/transition/supersede/by/inference.stream.SupersedeGeneration/superseded`.
- §23.1 tests 1, 2, 3 and 6 (lines 1105-1107, 1110) exist as Rust tests and pass
  (`story:audio-and-stt-contract`, `story:llm-contract`, `story:tts-contract`,
  `story:inbound-text-spans-and-envelope`).
- A-01 (line 1137): an example depends on `inference-stt` and a mock adapter only; `cargo tree`
  shows no `inference-runtime` and no `inference-tts` (`story:audio-and-stt-contract`).

## Out of scope

Nodes, ports, edges, pipelines and the runtime (epic graph-minimum); `inference-duplex`,
`inference-tools`, `inference-facade` (split out later per §5.3); any real backend.
