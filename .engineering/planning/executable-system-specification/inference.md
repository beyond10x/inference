---
format: aep.planning-md/3
id: executable-system-specification:inference
kind: executable-system-specification
status: validated
title: ESS specification of the inference nouns (ess/)
summary: Seven domains, 18 entities, 54 commands; validates on ess 0.60.0; 330 synthesized scenarios (4 authored), 3 refusals.
relations:
- specifies: product-requirements:inference
- derived_from: architecture-design:inference
revision: 11
transitions:
- {from: "draft", to: "validated", at: "2026-10-18T21:12:53Z", actor: "human:timo", revision: 2}
---
## Files

| path | what |
|---|---|
| `ess/system.yaml` | `format: ess/25`, `system: inference`, `version: v1`, seven domains |
| `ess/ess-inputs.yaml` | `ess-inputs/2`, `requires: ess 0.60` |
| `ess/domains/model.yaml` | `inference.model`: Model, Adapter; Capability, Modality, Locality, ExecutionForm |
| `ess/domains/policy.yaml` | `inference.policy`: DeploymentProfile; ProfileKind, Egress |
| `ess/domains/graph.yaml` | `inference.graph`: Pipeline, Node, Port, Edge, Binding, Run, NodeInstance; the run lifecycle |
| `ess/domains/stream.yaml` | `inference.stream`: Turn, Span, Generation, OutputSegment; the provisional/committed/irreversible state machine; the event envelope (EventEnvelope, Timebase, EventId) |
| `ess/domains/tools.yaml` | `inference.tools`: Tool, ToolCall; the provisional-input barrier |
| `ess/domains/session.yaml` | `inference.session`: Session |
| `ess/domains/audio.yaml` | `inference.audio`: types only; AudioEncoding, AudioFormat, SampleClockPosition, AudioFrame |

Every declaration carries a `read from: docs/design/inference.md:<line>` comment naming the design
line it was read from (282 such lines across the seven documents, `grep -c 'read from:'`).

## What it declares

From `ess specify compile --path ess --format json` on 2026-10-19 after wave 16: 18 entities, 54
commands, 55 events, 41 errors, 18 views, 65 types. Since the count before it (17 entities, 41
commands, 40 events, 34 errors, 17 views, 60 types, after story:inbound-text-spans-and-envelope),
among the additions, story:endpoint-controller declared
`inference.stream.SpeechStarted` and `SpeechEnded`, story:tool-feedback-loop declared
`inference.graph.FeedbackLoop` (a feedback edge of a pipeline with `max_iterations > 0`), its
command `DeclareFeedbackLoop`, event, two errors and view, and story:session-run-link gave
`inference.session.Session` its current run (`run_id`, relation to `inference.graph.Run`) and the
command `AttachRun`.

The event envelope types (`inference.stream.EventId`, `Timebase`, `EventEnvelope`) are those of
story:inbound-text-spans-and-envelope.

## Evidence

| command | output | date |
|---|---|---|
| `ess specify validate --path ess` | `inference v1 — 7 file(s), valid`, exit 0 | 2026-10-18 |
| `ess verify conform synthesize --path ess --out .engineering/drafts/suite.json` | `153 scenario(s) (0 authored), 0 refusal(s)`, 0 notes | 2026-10-18 |
| `ess specify validate --path ess`, after `inference.audio` (story:audio-vocabulary) | `inference v1 — 8 file(s), 4 scenario(s), valid`, exit 0 (before: `7 file(s), 4 scenario(s), valid`) | 2026-10-19 |
| `ess verify conform synthesize --path ess --scenarios ess/scenarios --out .engineering/drafts/suite.json`, after `inference.audio` (story:audio-vocabulary) | `229 scenario(s) (4 authored), 3 refusal(s)`, exit 0; before the domain the same `229 scenario(s) (4 authored), 3 refusal(s)`, the 229 scenario ids identical and the three refusals the ESS-SYNTH-001 ones on `inference.graph.CancelRun/cancelled`, `inference.stream.CancelGeneration/cancelled` and `inference.stream.SupersedeGeneration/superseded`; the domain declares `types:` only | 2026-10-19 |
| `ess specify validate --path ess`, after the event envelope (story:inbound-text-spans-and-envelope) | `inference v1 — 8 file(s), 4 scenario(s), valid`, exit 0 (before: the same line) | 2026-10-19 |
| `ess verify conform synthesize --path ess --scenarios ess/scenarios --out .engineering/drafts/suite.json`, after the event envelope (story:inbound-text-spans-and-envelope) | `229 scenario(s) (4 authored), 3 refusal(s)`, exit 0; before the declaration the same; the 229 scenario ids and their bodies identical, inference.stream 83; the change declares `types:` only | 2026-10-19 |


| `ess specify validate --path ess`, after the feedback loop and the session to run link (story:tool-feedback-loop, story:session-run-link) | `inference v1 — 8 file(s), 4 scenario(s), valid`, exit 0 | 2026-10-19 |
| `ess verify conform synthesize --path ess --scenarios ess/scenarios --out .engineering/drafts/suite.json`, after the same two stories | `330 scenario(s) (4 authored), 3 refusal(s)`, exit 0; before them `317 scenario(s) (4 authored), 3 refusal(s)`; the same three ESS-SYNTH-001 refusals; no scenario id removed; six added in inference.graph (`DeclareFeedbackLoop` outcomes and the FeedbackLoop invariant) and seven in inference.session (`AttachRun` and the attach transition) | 2026-10-19 |
| `cargo test -p inference-testkit --all-features --test conform`, after the same two stories | every scenario answered, none unsupported; `ess/SKIPPED.md` lists no id | 2026-10-19 |

Scenarios per domain, 2026-10-19 after wave 16 (330 with `--scenarios ess/scenarios`): graph 158, stream 83, tools 53, session 28, model 4, policy 4.

## How the specification answers the design's open questions 1 to 3

- Q1 (which types live in core versus text, audio and domain crates): an entity's crate follows its
  domain. `inference.model` and `inference.policy` are `inference-core`; `inference.stream` is
  `inference-text` (spans, segments) and `inference-core` (generation); `inference.graph` is
  `inference-pipeline` (declaration) and `inference-runtime` (Run, NodeInstance);
  `inference.tools` is `inference-tools`; `inference.session` is `inference-facade`.
- Q2 (async traits, node descriptors, or two layers): two layers. The ESS entity is the node
  descriptor; the Rust trait is a projection of it, generated or held to it by conformance.
- Q3 (minimum event vocabulary): the 36 events here, which are the §6.4 families the first
  scenarios need; the vocabulary grows per scenario, never ahead of one.

## Decisions taken while drafting

The owner asked for decisions, not questions (2026-10-18). Each is reversible in the file named.

| # | decision | why | file |
|---|---|---|---|
| D1 | Span `references` Turn, not owned | a committed span is durable history (line 990); the turn is a boundary (line 606) | stream.yaml |
| D2 | Generation `owns` OutputSegment | cancellation discards the generation's unplayed segments (line 628) | stream.yaml |
| D3 | Binding is an entity owned by Pipeline | adapters bind at composition time (lines 248, 519); avoids an optional reference | graph.yaml |
| D4 | effective capabilities are an `Optional<List<Capability>>` on Pipeline, cleared at declaration, supplied by the implementation at planning | §10.1 names the plan as the product of planning (line 519) and nothing else reads it apart from the pipeline | graph.yaml |
| D5 | OverloadPolicy variants pause, drop, coalesce, fail, drop_stale_epochs | names for the behaviours of the §12.3 table (lines 679-689); the design names no enum | graph.yaml |
| D6 | Run has the design's six states plus Failed and Cancelled; NodeInstance has one state | lines 671-673; the node-instance lifecycle comes with graph-minimum | graph.yaml |
| D7 | ToolCall has one state (Proposed); the §15.3 barrier is declared now | lines 849-850, 866; authorization, execution and result come with tools-and-turns | tools.yaml |
| D8 | Session has one state (Opened) | §6.4's four session events come with session-facade | session.yaml |
| D9 | Edge invariant `capacity > 0` | every edge specifies finite buffering (line 679) | graph.yaml |
| D10 | run failure is a command (`FailRun`) the runtime issues | a failed node surfaces a typed error to the run controller (line 1006) | graph.yaml |
| D11 | DeploymentProfile holds admitted localities and a default egress | the profile table (lines 1046-1049) | policy.yaml |

## UNMAPPED

1. Session to Run: a session "potentially spans multiple graph runs" (line 105) and the envelope's
   `session_id` is optional (line 310), so a run may exist without a session. Whether a session owns
   its runs or references them, and on which side the link is held, the design does not say. Marker
   in `ess/domains/session.yaml` on `inference.session.Session`. The session-facade epic settles it
   before its first story.

## Not expressed in ESS, checked elsewhere

- §8.3's effective-capability computation (the weakest node or edge decides) is a graph algorithm.
  The scenario `inference.graph.PlanPipeline/outcome/mismatch` checks the refusal; the computation
  is a Rust property test in epic graph-minimum.
- §10.2's ten validation checks: `inference.graph.ValidatePipeline/outcome/rejected` is an external
  refusal; the checks themselves are Rust tests in graph-minimum.
- §11.4 across rows (superseding a generation discards its unplayed segments) is a set effect the
  code targets refuse; local-voice-cascade covers it with an authored scenario.
