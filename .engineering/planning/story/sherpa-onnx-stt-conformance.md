---
format: aep.planning-md/3
id: story:sherpa-onnx-stt-conformance
kind: story
status: draft
title: sherpa-onnx STT adapter passes adapter conformance and stands alone (A-01)
summary: Section 23.2 checks against the recogniser (claims observed, order, timestamps, cancellation, concurrency, provenance, no network crates) and an example depending on inference-stt and the adapter only.
relations:
- decomposes: epic:local-stt-adapter
- serves: vision:inference
- depends_on: story:sherpa-onnx-recogniser
- depends_on: story:capability-record
revision: 1
---
## Context

`story:sherpa-onnx-recogniser` puts the sherpa-onnx streaming recogniser behind the STT contract
and shows it transcribing a fixture. This story checks it against the adapter conformance of
section 23.2 (`docs/design/inference.md:1114-1120`) and the exit of `epic:local-stt-adapter`:
declared capabilities match observed behaviour (1114); errors, cancellation and finalization
propagate (1115); a recognition ended early leaks nothing and revives no stale output (1116); audio
timestamps and format transformations are valid (1117); concurrency limits are respected (1118);
provenance and licence metadata are available (1119); the local-only profile prohibits external
networking (1120); and A-01 (line 1137): a user depends on `inference-stt` and this adapter only.
An adapter may claim `IncrementalInput` only if the backend takes audio before the utterance ends
(line 362); the contract leaves catching a false claim to the conformance checks of a real adapter
(`crates/inference-stt/src/lib.rs:31-34`).

In process there is no connection to lose: the equivalent of a backend disconnect is a
recognition whose handle is dropped mid-utterance. The STT contract carries no deadline and no
error event on a running recognition (`crates/inference-stt/src/lib.rs:636-688`), so deadline
propagation is not tested here; errors are the typed refusals of construction and start.

Checkpoint-dependent tests follow the pattern of the live provider tests
(`adapters/inference-aws/tests/nova_sonic_live.rs`): `#[ignore]` with a reason, here naming
`INFERENCE_SHERPA_ONNX_STT_DIR`; they read the checkpoint directory from it and download nothing.

## Domain relations

- Adapter to Model: one adapter references one model; the model stands on its own. Inferable,
  `ess/domains/model.yaml`, entity `inference.model.Adapter`, relation `model`. The provenance and
  licence check reads the model the adapter references.
- Recogniser to Adapter: a recogniser carries the one adapter record it was declared with.
  Inferable, inferred from `crates/inference-stt/src/lib.rs:640-641`; no `ess/1` document declares
  a recogniser.

## Planned scope

- Conformance tests in `adapters/inference-sherpa-onnx/tests/`, ignored without
  `INFERENCE_SHERPA_ONNX_STT_DIR`, each against the committed fixture streamed in 20 ms frames:
  - claims observed: frames are taken before the end of input and a hypothesis reaches the stream
    before the end of input, so `IncrementalInput` and `OutputStreaming` are observed, and the
    declared capabilities are exactly those two;
  - stream order: sequences start at the request first sequence and run without gaps, every
    revision names an open hypothesis, a committed span is never revised again, and every span is
    committed once input has ended;
  - timestamps: envelope timestamps on `AudioFrameTime` never decrease and never pass the samples
    offered; the fixture as PCM16 and as f32, at 16 kHz and resampled from 8 kHz, and as two
    channels, yields committed text containing "capital" and "france";
  - cancellation: a handle dropped mid-utterance frees its place, so a recogniser at maximum
    concurrency 1 starts the next recognition, whose events carry only its own request and none of
    the text of the dropped one;
  - concurrency: at maximum concurrency 2 two interleaved recognitions of different clips each
    commit their own text, and a third start is refused with `ConcurrencyLimitReached`;
  - provenance: the declared model carries the pinned checkpoint, source URL, revision, code and
    weight licence and checksum, none empty.
- A test that needs no weights: `cargo tree -p inference-sherpa-onnx -e normal --features stt`
  names no HTTP, TLS or socket crate (reqwest, hyper, ureq, rustls, tokio-tungstenite or the like),
  and the adapter declares `Locality::SameProcess`; the archive download lives only in the build
  dependencies of `sherpa-onnx-sys`, outside the normal tree.
- `examples/stt-sherpa-onnx`, an example package (no binary target, as `examples/stt-only`) that
  depends on `inference-stt` and `inference-sherpa-onnx` with feature `stt` only, and an example
  that reads the checkpoint directory from `INFERENCE_SHERPA_ONNX_STT_DIR` and a WAV path from its
  arguments and prints each committed span; a tree test like `examples/stt-only/tests/tree.rs`
  asserting that its normal dependencies name the STT contract and this adapter and none of
  `inference-tts`, `inference-llm`, `inference-runtime`, `inference-pipeline`, `inference-testkit`.
- Any defect these tests find in the recogniser is fixed here.

## Acceptance

`task check` passes, including the A-01 tree test of `examples/stt-sherpa-onnx` and the
network-free dependency check, and `INFERENCE_SHERPA_ONNX_STT_DIR=<pinned checkpoint> cargo test
-p inference-sherpa-onnx --all-features -- --ignored` passes every conformance test listed in
Planned scope on the reference machine on CPU.

## Out of scope

The capability record with resource profile, evidence, languages and formats
(`decision-blocker:capability-record-shape`); deadlines on a recognition, which the STT contract
does not carry; a pipeline node wrapping the recogniser and the cascade (local-voice-cascade);
the whisper.cpp swap (backend-replacement).
