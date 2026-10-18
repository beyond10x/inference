---
format: aep.planning-md/3
id: architecture-design:inference
kind: architecture-design
status: draft
title: 'inference: Rust inference contracts, adapters, pipelines and runtime (design v0.2)'
summary: The design document docs/design/inference.md and its review, recorded as the artifact every epic derives from.
relations:
- designs: product-requirements:inference
revision: 2
---
## The design

`docs/design/inference.md`, version 0.2, dated 2026-10-16, 1276 lines, 29 sections. Its review is
`docs/design/review-2026-10-18.md`. This artifact is the record the epics derive from; the two files
carry the content.

## Decisions the design records

`docs/design/inference.md:1186-1198`.

| id | decision |
|---|---|
| ADR-001 | one `inference` Cargo workspace with separate public crates |
| ADR-002 | contracts model externally observable behaviour, not neural architecture |
| ADR-003 | typed multiport nodes form graphs |
| ADR-004 | pipelines are declarations; the runtime executes validated plans |
| ADR-005 | sessions are optional, above graph executions |
| ADR-006 | input, output and duplex streaming are distinct features |
| ADR-007 | provisional and committed events are first-class |
| ADR-008 | tools live outside model adapters |
| ADR-009 | self-hosted, open-source integrations are the reference baseline |
| ADR-010 | hosted providers are opt-in adapter implementations |
| ADR-011 | cross-language plugins use versioned process protocols |
| ADR-012 | training is a separate workflow using runtime traces |
| ADR-013 | explicit converters and capability checks |

## Crate split

`docs/design/inference.md:178-227` and the dependency table at lines 231-248. Contract crates:
`inference-core`, `inference-text`, `inference-audio`, `inference-stt`, `inference-llm`,
`inference-tts`, `inference-duplex`, `inference-tools`, `inference-pipeline`, `inference-runtime`,
`inference-facade`, `inference-testkit`, `inference`. Adapters live under `adapters/` and depend on
contracts, never the reverse (line 248). §5.3 (line 273) starts with nine crates and two or three
adapters; `duplex`, `tools` and `facade` split out when their contracts are stable.

## What the review found and what the owner decided

| finding | decision (2026-10-18) |
|---|---|
| F1: five text-LLM adapter crates duplicate the `llm` repository | kept as the design says; `llm` treated as absent, an adapter to it possible later |
| F2: tool authorization and execution duplicate Loom | kept as the design says; Loom treated as absent |
| F3: 13 nouns without a typed home | the ESS domain is drafted first: `executable-system-specification:inference` |
| F4: Q9, Q10, Q11, Q15 gate P2, P5 and P6 | four `decision-blocker` artifacts with defaults |
| F5: roadmap by stage, acceptance by prose | epics follow §24.1's six demonstrator steps; acceptance is named scenarios |
| F6: nine adapters against the design's own two or three | two for P2, one for the swap, the rest from P5 |
| F7: root `tests/`, `LICENSES/`, Python workers, no metrics consumer named | per-crate tests; one `LICENSE`; a non-Rust worker is an external process; consumer unchecked |

## Open questions carried as blockers

| question (`docs/design/inference.md:1222-1241`) | blocker |
|---|---|
| Q9 reference hardware for the first self-hosted suite | `decision-blocker:reference-hardware` |
| Q10 weight licences permitted by the reference profile | `decision-blocker:weight-licenses` |
| Q11 the first two adapters that prove replacement is cheap | `decision-blocker:first-adapters` |
| Q15 conformance evidence required before claiming `native_duplex` | `decision-blocker:native-duplex-evidence` |

Q1 to Q3 are answered by the specification (see its body). Q4 to Q8 and Q12 to Q14 do not gate
P0 to P3 and stay in §27.

## Specification

`ess/` declares the nouns of the design in six domains; `executable-system-specification:inference`
records it, its validation and the decisions taken while drafting.
