# Status and roadmap

**Released: 0.7.0** (2026-10-21). The releases before it, 0.1.0 to 0.6.0, are in `CHANGELOG.md`.
The plan lives in the repository's planning store, `.engineering/planning/`; each line below is
one epic.

| Epic | Where it stands | Status |
|---|---|---|
| Contracts and testkit (P0) | STT, LLM, TTS, tools contracts with mocks; the conformance target answers all 454 synthesized scenarios | <span class="st st-rel">0.5.0</span> |
| Graph minimum (P1) | declare, validate, plan; bounded edges; run lifecycle and controller | <span class="st st-rel">0.5.0</span> |
| Tools and turns (P3) | tool lifecycle, authority, executor, safe retries, bounded feedback loop, VAD and turn policy | <span class="st st-rel">0.5.0</span> |
| Session facade and reusable subgraphs (P4) | facade, history, topology switch, subgraphs, declarative definitions in 0.5.0; record and replay in 0.6.0 | <span class="st st-rel">0.5.0</span> <span class="st st-rel">0.6.0</span> |
| Native and hybrid voice (P5) | duplex contract, OpenAI Realtime and Nova Sonic in 0.5.0; OpenAI GPT-Live in 0.6.0; a self-hosted native model planned | <span class="st st-rel">0.5.0</span> <span class="st st-rel">0.6.0</span> <span class="st st-plan">planned</span> |
| Local STT adapter | sherpa-onnx streaming and whisper.cpp batch recognisers | <span class="st st-rel">0.6.0</span> |
| Local LLM adapter | `llama-server`: output streaming, cancellation, tool proposals | <span class="st st-rel">0.6.0</span> |
| Local TTS adapter | backend-neutral TTS checks exist; the adapter waits on a licence decision | <span class="st st-plan">planned</span> |
| Local voice cascade, offline (P2) | commit gates, barge-in, `offline_strict` and egress watch, the `talk` example; TTS is a mock until a local one exists | <span class="st st-rel">0.6.0</span> |
| Backend replacement from configuration | swap sherpa-onnx and whisper.cpp in the cascade by configuration (`INFERENCE_CASCADE_STT`), same tests, no contract change | <span class="st st-rel">0.7.0</span> |
| Usage and cost | usage records, one per LLM call, the realtime example's usage and provider comparison in 0.6.0; per-call records for STT, TTS and speech models in 0.7.0; session totals, pricing, usage on cancel and the subgraph roll-up on main for the next release | <span class="st st-rel">0.6.0</span> <span class="st st-rel">0.7.0</span> <span class="st st-plan">planned</span> |
| Duplex research (P6) | orchestrated and native duplex text, reported as different capabilities; trace export with consent | <span class="st st-plan">planned</span> |
| Hosted providers and scale-out (P7) | worker process protocol, quotas, egress audit | <span class="st st-plan">planned</span> |

## Known limits today

- No local TTS: the offline cascade speaks silence.
- Nova Sonic's live acceptance is still open.
- The egress watch is Linux-only and a tripwire, not a sandbox.
- Audio resampling is linear, with no anti-aliasing filter; there is no echo cancellation or
  noise suppression.
