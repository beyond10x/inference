---
format: aep.planning-md/3
id: story:whispercpp-stt-adapter
kind: story
status: draft
title: whisper.cpp STT adapter over whisper-rs, batch-only claims
summary: 'adapters/inference-whispercpp over whisper-rs 0.16.0 (Unlicense; whisper.cpp MIT): pinned ggml-base.en checkpoint (MIT) behind INFERENCE_WHISPERCPP_STT_MODEL, no incremental_input or output_streaming claimed, the sherpa-onnx conformance list mirrored, no crates/ change.'
relations:
- decomposes: epic:backend-replacement
- serves: vision:inference
- depends_on: story:sherpa-onnx-stt-conformance
- depends_on: story:capability-record
revision: 1
---
## Context

`epic:backend-replacement` swaps STT from sherpa-onnx to whisper.cpp (the epic's `## Decisions`;
`decision-blocker:first-adapters`, cleared 2026-10-20). This story is the second adapter on its own,
built to the same checks as `story:sherpa-onnx-recogniser` and `story:sherpa-onnx-stt-conformance`;
the swap inside the cascade is `story:stt-swap-by-configuration`.

- Rust reaches whisper.cpp through the `whisper-rs` crate (0.16.0, 2026-03-12, Unlicense, repository
  https://codeberg.org/tazz4843/whisper-rs), a safe wrapper over `whisper-rs-sys` (0.15.0, Unlicense),
  whose build script compiles the whisper.cpp sources shipped inside the crate with `cmake` and
  generates bindings with `bindgen`. Nothing is downloaded at build time: the sources come from
  crates.io, checksummed in `Cargo.lock`. whisper.cpp itself is MIT
  (https://github.com/ggml-org/whisper.cpp/blob/master/LICENSE). Licence metadata read from crates.io
  and GitHub on 2026-10-20. The code licence policy (`decision-blocker:weight-licenses`) is
  Apache-2.0 or MIT, BSD counting as more permissive; the Unlicense, a public-domain dedication, is
  read here as more permissive than MIT.
- The crate name is the design's: `inference-whispercpp` (`docs/design/inference.md:207`).
- whisper.cpp is a chunked offline recogniser: one `full` call transcribes one complete buffer. It
  must not claim native incremental ASR (`docs/design/inference.md:362`; §17 line 919: chunked
  transcription is not native streaming). The STT contract decides what it can observe: frames are
  taken before the end of input only under `incremental_input`, a span reaches the stream before
  the end of input only under `output_streaming`, and batch is what remains when neither is claimed
  (`crates/inference-stt/src/lib.rs:20-37`).
- Licences and tests (`decision-blocker:weight-licenses`): weights CC BY 4.0 or more permissive,
  verified at adoption with source URL and hash; tests download no weights. Hardware
  (`decision-blocker:reference-hardware`): tests on CPU, the GPU optional.

Candidate checkpoint, a candidate and not an adoption. Hash and size were read on 2026-10-20 from the
hosting repository's file metadata, not from a downloaded file:

| field | value |
|---|---|
| file | `ggml-base.en.bin` (Whisper base, English only, ggml format) |
| source | https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin |
| revision | `5359861c739e955e79d9a303bcbc70fb988958b1` |
| sha256 | `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002` |
| size | 147964211 bytes |
| weight licence | MIT: the repository card declares `license: mit`; the upstream Whisper weights are MIT (https://github.com/openai/whisper/blob/main/LICENSE) |

## Domain relations

- Adapter to Model: one adapter references one model; the model stands on its own. Inferable,
  `ess/domains/model.yaml`, entity `inference.model.Adapter`, relation `model` (references, one, via
  `model_id`).
- Recogniser to Adapter: a recogniser carries the one adapter record it was declared with.
  Inferable, inferred from `crates/inference-stt/src/lib.rs:640-641` (`Recogniser::adapter`); no
  `ess/1` document declares a recogniser.

## Planned scope

- `adapters/inference-whispercpp` (crate `inference-whispercpp`), a workspace member through the
  `adapters/*` glob, depending on `inference-stt` and `whisper-rs = "=0.16.0"` with default features.
  It exposes no GPU feature: CI runs `--all-features`, and a forwarded `cuda` feature would build
  CUDA there. It writes no `unsafe` (workspace lint `unsafe_code = "forbid"`).
- The pinned checkpoint as constants of the crate: name, source URL, source revision, code licence
  (whisper.cpp MIT, whisper-rs Unlicense), weight licence, sha256. They fill `DeclareModel`
  (`family` `whisper`, `checkpoint`, `code_license`, `weight_license`, `checksum`, `provenance`,
  `source_revision`; audio in, text out).
- A constructor taking the checkpoint file path and a maximum concurrency (1 by default). It refuses
  with a typed error, naming the file, when the file is missing, its sha256 differs from the pin, or
  `whisper-rs` fails to load it; nothing is declared then.
- `Adapter::declare` with `ExecutionForm::NativeFfi`, `Locality::SameProcess`, backend
  `whisper.cpp` with the `whisper-rs` version and the checkpoint name, `CONTRACT_REVISION` of
  `inference-stt`, and no `IncrementalInput` and no `OutputStreaming` claim.
- `Recogniser` and `RecognitionHandle`: one whisper state per recognition on a shared context,
  admitted under the maximum concurrency. Frames are buffered as offered and taken once input has
  ended; they are decoded to f32 (every `AudioEncoding`), channels averaged to mono, and resampled to
  16 kHz in the crate by linear interpolation (whisper.cpp takes 16 kHz mono only; the linear
  resampler is a known limitation, named in the crate docs). One `full` call on the whole buffer;
  its text is emitted as a hypothesis and committed at once, in the same step, after the end of
  input. An utterance with no text emits nothing.
- Tests that need no weights, run by `task check`: the declared adapter claims neither capability;
  `take_input` before end of input is refused with `CapabilityNotClaimed`; decoding, down-mixing and
  resampling; the constructor refusals against a temporary file that is missing or has the wrong
  hash; `cargo tree -p inference-whispercpp -e normal` names no HTTP, TLS or socket crate.
- Tests that need the checkpoint: `#[ignore]` with a reason naming `INFERENCE_WHISPERCPP_STT_MODEL`, the
  path of the pinned ggml file, which each reads and fails with a message naming when run without
  it. They mirror the list of `story:sherpa-onnx-stt-conformance` with the claims inverted, on the
  same fixture (`question-16k.wav`, eSpeak NG, "What is the capital of France?", copied with its
  provenance file into this crate's `tests/fixtures`): no frame taken and no span emitted before the
  end of input, and the declared capabilities are exactly none of the two; stream order and
  sequences; timestamps; the fixture as PCM16 and f32, at 16 kHz, resampled from 8 kHz and as two
  channels commits text containing "capital" and "france" (case-insensitive); a handle dropped
  before the end of input frees its place and no text of it appears in the next recognition;
  `ConcurrencyLimitReached` past the maximum; the declared model's provenance fields are non-empty.
  The sherpa-onnx checks live in that crate's `tests/` and are not shared; a shared STT conformance
  harness would belong in `crates/inference-testkit` and is not this story's (A-05).
- The adapter row in `AGENTS.md` Layout (crate, environment variable, build needs).

## Acceptance

`task check` passes on a machine with no checkpoint, and
`INFERENCE_WHISPERCPP_STT_MODEL=<pinned ggml-base.en.bin> cargo test -p inference-whispercpp -- --ignored`
passes on the reference machine on CPU, committing the fixture's text with "capital" and "france"
after the end of input and nothing before it, while `git diff --stat main...HEAD` of the story lists
no path under `crates/` (A-05, `docs/design/inference.md:1140`).

- At adoption the checkpoint's licence, source URL, revision and sha256 are re-read from the source
  and pinned; a mismatch with the table above stops the story for a per-checkpoint decision.
- The vendored build needs `cmake`, a C++ compiler and `libclang` on every builder; if CI needs a
  step for them, that file is outside `crates/` and is named in the closing note.

## Out of scope

The capability record's resource profile, evidence, languages and formats
(`decision-blocker:capability-record-shape`, open); GPU execution; a shared conformance harness in
`crates/`; the cascade swap (`story:stt-swap-by-configuration`).
