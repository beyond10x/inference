---
format: aep.planning-md/3
id: story:duplex-ledger-correctness
kind: story
status: implemented
title: Duplex ledger keeps the provider's finality, checks frame length and delivers without rescans
summary: A failed session's running usage is not final, a short PCM frame is refused, and next_event finds its received event directly
relations:
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-23T11:47:39Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":1}}}
- {from: "proposed", to: "active", at: "2026-11-23T11:47:40Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1}}}
- {from: "active", to: "implemented", at: "2026-11-23T11:47:40Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Context

A review of tag 0.31.0 found three defects in `inference-duplex`'s `SessionLedger`
(`crates/inference-duplex/src/ledger.rs`):

- Finding 10: a failed speech session can mark an interim provider count as final. A fixture
  reported seven running output tokens (`UsageTotal::Running`, as Nova Sonic reports) and ended the
  session with an error; the response's usage record carried 7 with `final: true`, the same as a
  control whose seven tokens the provider confirmed final. The session's end settles every
  response, and the record took that local settlement for the provider's word.
- Finding 12: duplex accepts PCM audio whose declared length exceeds its bytes. Two mono PCM16
  samples need four bytes; a two-byte frame declaring two samples was accepted as input, emitted
  as output, and truncation at sample 2 succeeded. The TTS contract already refuses such a frame
  (`FrameLengthMismatch`), and `inference-audio` checks the same length for conversion and
  resampling.
- Finding 14: receiving audio rescans the whole session history. Each event built from the
  provider's traffic is appended to the received events, then looked up from the start by its id
  when `next_event` delivers it: N frames read back one by one make N(N+1)/2 visits (1,000, 2,000
  and 4,000 frames took 10.7, 52.6 and 124.1 ms in the review's unoptimised build).

## Planned scope

- `crates/inference-duplex/src/ledger.rs`: a response's usage record is final only when the
  provider confirmed its counts (a `UsageTotal::Final` report) or the provider's next response
  settled its running total; settling at the session's end or by a lost final frame records
  `final: false`. Input and output audio are refused with the frame-length check before they are
  accepted or advance the response's emitted samples. `next_event` delivers the received event
  it carries by its place, without a search.
- `crates/inference-duplex/src/contract.rs`: `SessionError::FrameLength`.
- `crates/inference-audio/src/lib.rs`: `FrameLengthMismatch` and `AudioFrame::check_length`, the
  one length check; `to_encoding` and `Resampler` use it.
- `crates/inference-tts/src/lib.rs`: uses that check and type instead of its own.
- `CHANGELOG.md`, `website/docs/concepts/usage.md`.

## Acceptance

- Finding 10: a response whose provider reported seven output tokens as a running total, ended by
  a failure of the session, is recorded with seven output tokens and `final: false`; a close of the
  session records it `final: false` as well. The control, seven tokens reported
  `UsageTotal::Final`, is recorded `final: true`. A running total settled by the provider's next
  response is recorded `final: true`; one settled by `response_usage_unknown` is `final: false`.
- Finding 12: a mono `pcm_s16le` frame declaring two samples with two bytes of data is refused by
  `accept_input` and by `output_audio` with `SessionError::FrameLength` (expected 4, actual 2);
  nothing is recorded and the response's emitted samples do not move, so truncation at sample 2
  is refused with `BeyondOutput`. The four-byte control is accepted and emitted.
- Finding 12: the duplex ledger, the TTS contract, `AudioFrame::to_encoding` and `Resampler` all
  check a frame's length through `AudioFrame::check_length`; no second copy of the check is left.
- Finding 14: 4,000 output audio frames, each read back by `next_event` right after it was
  recorded, visit 4,000 received events in delivery, one each, counted in test builds; not
  N(N+1)/2.
- `cargo test`, `cargo clippy --all-targets --all-features -- -D warnings` for `inference-audio`,
  `inference-tts` and `inference-duplex`, and `cargo fmt --all --check` pass.
