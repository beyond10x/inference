---
title: Concepts
sidebar_label: Overview
sidebar_position: 1
description: One vocabulary, from a single model call to a full voice agent, and where each part is specified.
lede: Each box is a node with typed ports, each arrow a bounded edge with a declared overload rule, and every rule is specified before it is coded.
source: crates/inference-pipeline, crates/inference-runtime, ess/system.yaml, ess/domains/, CHANGELOG.md 0.6.0
---

# Concepts

One vocabulary, from a single model call to a full voice agent.

```text
 audio ─▶ speech detection ─▶ speech-to-text ─▶ commit ─▶ language model ─▶ commit ─▶ text-to-speech ─▶ audio
                                                              ▲      │
                                                              │      ▼
                                                        tool result  tool call ─▶ authority ─▶ executor

 or, with a native speech model:

 audio ⇄ native speech model ⇄ audio      (one node; transcripts and tool calls as side ports)
```

Each box is a node with typed ports. Each arrow is a bounded edge with a declared overload rule. A
loop, like the tool round trip, declares how many times it may turn.

| Page | In one line |
|---|---|
| [Contracts](contracts.md) | What a model kind must do, with no model inside. |
| [Streams and commit rules](streams.md) | What is provisional, what is committed, what was actually played. |
| [Graphs](graphs.md) | Nodes, typed ports, bounded edges; validated and planned before running. |
| [Runtime](runtime.md) | Runs a planned graph: lifecycle, backpressure, cancellation, drain, replay. |
| [Session facade](session.md) | One interface for the application, whatever topology answers. |
| [Deployment profiles](deployment-profiles.md) | Where a model may run, checked at planning and watched at run time. |
| [Usage and cost](usage.md) | One usage record per model call; unknown is never zero. |

## Specified first

The behaviour is written down first as an executable specification in
[ESS](https://beyond10x.github.io/ess/), under `ess/`, one domain per layer: `audio`, `duplex`,
`graph`, `model`, `policy`, `session`, `stream`, `tools` and `usage`. Each error a domain declares
is listed in [Errors](../reference/errors.md). The specification's scenarios run as a conformance
suite in `task check`. At 0.8.0 the suite has 471 scenarios, 9 of them authored, and the testkit's target answers
all of them (`CHANGELOG.md`, 0.8.0).
