---
format: aep.planning-md/3
id: review-result:adversary-w25-sdk-payload-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the shared payload
relations:
- reviews: story:sdk-payload
revision: 1
---
unit: story:sdk-payload at ff9e47b (base d4728b5)
verdict: CONFIRMED, notes only
cases: none added, red 0
origin: introduced 2 / pre-existing 0

1. Introduced, note: `crates/inference/src/payload.rs:24` linked `Payload::Receipt`, a variant that exists only with `audio-host`; a no-feature doc build warned of an unresolved link (and `graph.rs:1` of a redundant explicit link target). Fixed in cedf7b1.
2. Introduced, note, infeasible today: defaulted port traits are all Essential, so they match the cascades and the text pipeline but not the native-voice pipeline of `crates/inference-testkit/tests/duplex_node.rs:455-479` (ControlOnly control input, incremental Structured events, Optional tool ports). A defaulted, unexposed `tool_call` output would fail validation as an unreachable mandatory output (`validation.rs:289`). Nothing declares that pipeline with defaults.

Held: every trait impl matches `VoicePayload`, `CascadePayload` and the duplex test's payload arm by arm, `Usage` and the four duplex variants included; defaulted traits equal every explicit declaration of the cascades, the text pipeline and the facade's text tests; the crate builds without `audio-host`; `input_port`/`output_port` share the code path of `input`/`output`; the R98 test fails if usage is not carried or not mapped to `UsageRecorded`; the example's test count fell by exactly the moved handoff test.

Gap: no test covers the `Tool` arms of `carried` and `session_event`.
