---
format: aep.planning-md/3
id: approval-record:wave-0-9-0
kind: approval-record
status: draft
title: 'Wave approved: ess 0.77.0 pin, device and voice acceptance without hardware, release 0.9.0'
relations:
- decides: story:audio-device-adapter
- decides: story:realtime-voice-demo
- decides: story:ess-defect-fixes
revision: 1
---
## Decision

The operator's coordinator approved this wave on 2026-10-30: "Approved ... U1-U3 with the commits you listed, release 0.9.0."

## Units

| unit | story | surface |
|---|---|---|
| U1 | story:ess-defect-fixes (record only) | the `ess` pin moves to 0.77.0: `crates/inference-testkit/Cargo.toml`, `Cargo.lock`, `.github/workflows/check.yml`, `.github/workflows/pages.yml`, `ess/ess-inputs.yaml`, `AGENTS.md` |
| U2 | story:audio-device-adapter | `adapters/inference-audio-device`: the loopback of `mic-loopback` as a library function over `AudioHost`, and the hardware test's flow against the fake host |
| U3 | story:realtime-voice-demo | `examples/realtime-voice/tests/acceptance.rs`: one scripted conversation per provider configuration against the mock model and the fake host |

Selected with `aep plan artifact waves --kind story --status active` (aep 0.78.0): wave 1
story:audio-device-adapter, story:ess-defect-fixes; wave 2 story:realtime-voice-demo (inferred);
0 collisions, 0 unassessed.

## Out of the wave

- The device parts of both device stories' acceptance (a microphone and a speaker, a headset per
  provider): run by the operator on a laptop.
- The one synthesis refusal and the one unsupported interpreted scenario of story:ess-defect-fixes:
  unchanged under ess 0.77.0 and owned by ESS.
- Draft stories: not accepted.

## Commits authorised

One commit per unit, their merges into `wave/0.9.0`, one store commit, the 0.9.0 release commit,
one pull request to `main` through the bot, the tag `0.9.0` and its GitHub release.
