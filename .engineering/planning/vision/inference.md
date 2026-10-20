---
format: aep.planning-md/3
id: vision:inference
kind: vision
status: draft
title: 'inference: a reusable Rust inference ecosystem'
summary: Six objectives O1 to O6 the epics and stories serve, from the design's final recommendation and goals.
revision: 3
---
## Vision

A reusable Rust inference ecosystem in which voice agents are one kind of graph assembled from
independent, interoperable building blocks (`docs/design/inference.md:1275`). Contracts first,
adapters below them, pipelines of typed nodes, a runtime that owns execution, an optional session
facade above. Self-hosted and open-source first.

## Objectives

Each objective names the goals of `product-requirements:inference` it carries. A story or epic
`serves` this vision and names the objective in its body.

| id | objective | goals |
|---|---|---|
| O1 | Interchangeable, independently usable inference contracts: an application embeds one STT, LLM or TTS adapter without the rest | G-01, G-02, G-05, G-06 |
| O2 | Honest streaming: output streaming, incremental input, input during output and native duplex are declared, negotiated and tested apart | G-04, §1.2 |
| O3 | Safe commitment: provisional input, tentative output and irreversible effects are separated, and nothing side-effecting rests on a hypothesis | G-07, G-11 |
| O4 | A bounded, cancellable, observable runtime: finite buffers, explicit failure and cancellation, replayable control decisions, resource admission | G-08 |
| O5 | Composable graphs and an optional session above them, so one topology can replace another without changing the application's vocabulary | G-03, G-09 |
| O6 | Research without bundling training: time-aligned traces leave the runtime under consent; a new model arrives as an adapter | G-10 |

## Boundaries

The library ships no binary, server or CLI; its core carries no concrete implementation. Other
repositories of the organization are dependencies only where `AGENTS.md` names them: `inference-llm`
wraps the `llm` repository's model contract at a release tag (owner decision, 2026-10-20), and
adapters may use the `secrets` library for local credential lookup (owner decision, 2026-10-19).
Every other repository is treated as absent until an adapter to it is planned (owner, 2026-10-18).
