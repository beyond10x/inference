---
format: aep.planning-md/3
id: approval-record:wave-2026-11-05-inference
kind: approval-record
status: draft
title: 'Wave approved: voice agents for phone calls (G.711, long talk-over, ElevenLabs TTS, topology latency), release 0.16.0'
relations:
- decides: story:g711-codec-and-anti-aliased-resampler
- decides: story:long-talk-over-does-not-resume-the-cut-answer
- decides: story:elevenlabs-streaming-tts-adapter
- decides: story:voice-topology-latency-per-step
revision: 2
---
## Decision

The coordinator approved this wave on 2026-11-05 under the orchestrator's grant for waves,
commits, pull requests and releases through the bot route, on the operator's words in chat that
day: "i think this is something to ingest for you for b10x/inference". The input is a downstream
adopter's wish for low-latency voice agents with good barge-in and the best voices, first for
phone calls.

Ten drafts were filed from that input. The coordinator's acceptances:

- **this wave:**
  - story:g711-codec-and-anti-aliased-resampler;
  - story:long-talk-over-does-not-resume-the-cut-answer;
  - story:elevenlabs-streaming-tts-adapter;
  - story:voice-topology-latency-per-step;
- **next wave:**
  - story:deepgram-voice-agent-adapter;
  - story:elevenlabs-realtime-agent-adapter;
  - story:deepgram-streaming-stt-adapter;
- **later:** story:blind-voice-preference-record;
- **left in draft:** story:deepslate-realtime-adapter and story:more-hosted-voice-adapters.

The four stories of this wave move from draft to active on the coordinator's word. Those for the
next wave and later move to proposed.

## Units

Units run one at a time, U1 to U4, then one staged full gate and release 0.16.0.

| unit | story | surface | peak `target/` | cargo minutes |
|---|---|---|---|---|
| U1 | story:g711-codec-and-anti-aliased-resampler | `crates/inference-audio` | 0.4G | 3 |
| U2 | story:long-talk-over-does-not-resume-the-cut-answer | tests in `inference-testkit` and `inference-facade`; facade code only if a case is red | 1.8G | 8 |
| U3 | story:elevenlabs-streaming-tts-adapter | new `adapters/inference-elevenlabs`, the workspace member list, `Cargo.lock` | 2.0G | 12 |
| U4 | story:voice-topology-latency-per-step | `examples/realtime-voice`; `ess/` only if the report is a new noun | 4.0G | 15 |

- Each unit tells at 1.5 times its estimate and stops at twice it, never over 8G, 10G, or 20 and 30
  cargo minutes.
- Review: one adversary pass per unit, then at most one correction.
- Before U3, public ElevenLabs API documentation is fetched into scratch.
- Tests are fixture-only. Live runs need keys, which only the operator holds.

## Gate terms

| stage | content | peak | cargo minutes |
|---|---|---|---|
| s1 | sherpa archive, `ess specify validate`, `aep plan artifact validate`, docs `generate --check`, fmt | 0.2G | 0.1 |
| s2 | clippy, workspace, all targets and features | 1.4G | 1.5 |
| s3a | tests of the core crates, the ESS conformance suite among them | 5.0G | 2.5 |
| s3b | tests of the examples and the facade | 3.6G | 2.5 |
| s3c | tests of the adapters | 5.6G | 3.5 |

- Before each stage, one pre-check: at least 45G free, plus the orchestrator's trees budget.
- Two build slots; `target/` is discarded after each stage.
- On green: one pull request through the bot, merged, tag `0.16.0`, release, release check.

## Constraints

- Nothing from the adopter's discussion or survey is committed verbatim, and no company, customer,
  product, host, channel, person or ticket name appears. Every committed text is scanned.
- story:waiting-sends-land-in-the-order-they-started-waiting and
  story:withdrawn-ingress-send-gives-back-its-sequence stay drafts for a later runtime wave.

## Talk-over finding

The coordinator decided on 2026-11-05 how the talk-over unit's finding is handled:

- story:long-talk-over-does-not-resume-the-cut-answer ships as it is. Its four acceptance cases
  are green.
- Audio of the cut answer, when a provider keeps streaming it after speech started, plays today
  during the talk-over. That behaviour is pinned as today's behaviour.
- story:cut-response-audio-is-told-from-the-next-words is filed. The coordinator accepts it for
  the next inference wave, beside story:deepgram-voice-agent-adapter and
  story:elevenlabs-realtime-agent-adapter.
- Its change is additive: either the binding marks frames by response item or turn, or the
  `inference-duplex` contract is extended additively. The pinned tests flip in that story.
- The cascade's 6 s case must not enter the gate while it is still flaky. It failed once in eight
  runs, waiting for the interruption. Before the gate, its cause is either explained and fixed in
  the test or harness, or the case is made deterministic. No case is newly ignored without the
  coordinator's word.
