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

The behaviour is written down first as an executable specification under `ess/`, one domain per
layer, and tested as conformance scenarios: the testkit's target answers all 454 scenarios of the
synthesized suite (`CHANGELOG.md`, 0.5.0).
