---
format: aep.planning-md/3
id: story:sherpa-onnx-recogniser
kind: story
status: active
title: sherpa-onnx streaming recogniser as an STT adapter
summary: 'adapters/inference-sherpa-onnx over the official sherpa-onnx 1.13.8 crate: a pinned, hash-checked checkpoint, IncrementalInput and OutputStreaming claimed, a verified native archive, an ignored checkpoint test.'
relations:
- decomposes: epic:local-stt-adapter
- serves: vision:inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T08:01:30Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-20T08:01:30Z", actor: "human:timo", revision: 4}
---
## Context

Step 1 of the first demonstrator: one self-hosted STT adapter as a standalone library
(`docs/design/inference.md:1171`). The owner chose the sherpa-onnx streaming recogniser for STT
(`decision-blocker:first-adapters`), the reference machine (`decision-blocker:reference-hardware`:
Linux x86_64, tests on CPU, the GPU optional) and the licence policy
(`decision-blocker:weight-licenses`: code Apache-2.0 or MIT, weights CC BY 4.0 or more permissive,
each checkpoint verified at adoption, tests download no weights). The STT contract is
`crates/inference-stt` (`Recogniser`, `RecognitionHandle`, `TranscriptOutput`); the scripted mock
in `crates/inference-testkit/src/stt.rs` is the reference implementation of it. sherpa-onnx is a
C++ runtime, so the execution form is native FFI (section 16.1, line 876).

Rust reaches sherpa-onnx through the official crates published from the sherpa-onnx repository:
`sherpa-onnx` 1.13.8 (safe wrapper, `OnlineRecognizer` and `OnlineStream` in `online_asr.rs`) over
`sherpa-onnx-sys` 1.13.8 (raw FFI to the C API), both Apache-2.0, repository
https://github.com/k2-fsa/sherpa-onnx (crates.io metadata read 2026-10-20). The safe wrapper
keeps the adapter inside the workspace lint `unsafe_code = "forbid"`. The build script of
`sherpa-onnx-sys` links a prebuilt static archive (`sherpa-onnx-v1.13.8-linux-x64-static-lib.tar.bz2`,
which bundles onnxruntime, MIT) and, unless `SHERPA_ONNX_ARCHIVE_DIR` or `SHERPA_ONNX_LIB_DIR` is
set, downloads it from the GitHub release at build time with no checksum check.

Candidate checkpoint: `sherpa-onnx-streaming-zipformer-en-2023-06-26`, a streaming Zipformer
transducer for English at 16 kHz. Source:
https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-2023-06-26.tar.bz2.
The Hugging Face model card of the same name declared `license: apache-2.0` at revision
`672fbf1b30579d6585301139bb363f42a0ad4a24` (read 2026-10-20), which is more permissive than
CC BY 4.0. This is a candidate, not an adoption: the weight licence, the terms of its training
data, that the release archive holds the files of that revision, and the hash of every file must
be verified when the story pins it. If it fails, a streaming checkpoint that passes is pinned
instead, or the owner decides per checkpoint.

## Domain relations

- Adapter to Model: one adapter references one model; the model stands on its own. Inferable,
  `ess/domains/model.yaml`, entity `inference.model.Adapter`, relation `model` (references, one,
  via `model_id`). The recogniser declares one `Model` for the pinned checkpoint and one `Adapter`
  referencing it.
- Recogniser to Adapter: a recogniser carries the one adapter record it was declared with.
  Inferable, inferred from `crates/inference-stt/src/lib.rs:640-641` (`Recogniser::adapter`); no
  `ess/1` document declares a recogniser.

## Planned scope

- `adapters/inference-sherpa-onnx` (crate `inference-sherpa-onnx`), a member of the workspace, with
  the recogniser in module `stt` behind feature `stt`, so the TTS and VAD adapters of the same
  toolkit can follow behind features of their own. It depends on `inference-stt` and
  `sherpa-onnx = "=1.13.8"`; no contract crate changes.
- The native archive is never fetched unverified: the version and sha256 of
  `sherpa-onnx-v1.13.8-linux-x64-static-lib.tar.bz2` are pinned in the repository, a `Taskfile.yml`
  task fetches the archive into a cache directory and checks the sha256, `task check` runs it
  first, and every build sees `SHERPA_ONNX_ARCHIVE_DIR` (a relative `[env]` entry in
  `.cargo/config.toml`), so a build without the verified archive fails instead of downloading.
  The CI workflow keeps passing through `task check`.
- The pinned checkpoint as constants of the crate: name, source URL, source revision, code licence
  (sherpa-onnx Apache-2.0, onnxruntime MIT), weight licence, and the sha256 of each file the
  recogniser loads (encoder, decoder, joiner, tokens). They fill `DeclareModel` (`family`,
  `checkpoint`, `code_license`, `weight_license`, `checksum`, `provenance`, `source_revision`; audio
  in, text out).
- A constructor taking the checkpoint directory and a maximum concurrency (1 by default). It
  refuses with a typed error, naming the file, when a file is missing or its sha256 differs from
  the pin, and when `OnlineRecognizer::create` returns `None`; nothing is declared then.
- `Adapter::declare` with `ExecutionForm::NativeFfi`, `Locality::SameProcess`, the backend
  `sherpa-onnx 1.13.8` and the checkpoint name, `CONTRACT_REVISION` of `inference-stt`, and the
  claims `IncrementalInput` and `OutputStreaming`: the online recogniser accepts waveform before
  the utterance ends and yields partial results (line 362).
- `Recogniser` and `RecognitionHandle`: one `OnlineStream` per recognition on a shared
  `OnlineRecognizer`, admitted under the maximum concurrency as the mock admits; frames taken
  through `TranscriptOutput::take_input`, decoded to f32 (every `AudioEncoding`), channels averaged
  to mono, passed with their sample rate to `accept_waveform`, decoded while `is_ready`. The first
  non-empty partial result emits a hypothesis, a changed result revises it, an endpoint
  (`is_endpoint`, endpoint rules on) or the end of input commits it and resets the stream; an
  utterance with no text emits nothing.
- Tests that need no weights, run by `task check`: PCM decoding and down-mixing, the constructor
  refusals against a temporary directory of wrong or missing files, the declared model and adapter.
- A test that needs the checkpoint: `#[ignore]` with a reason naming
  `INFERENCE_SHERPA_ONNX_STT_DIR`, the directory of the unpacked pinned checkpoint, which it reads
  and fails with a message naming when run without it. It streams a committed fixture in 20 ms
  frames: `tests/fixtures/question-16k.wav` with its provenance file, copied from
  `adapters/inference-aws/tests/fixtures` (eSpeak NG, "What is the capital of France?"). No test
  and no build step downloads weights.
- The adapter row in `AGENTS.md` Layout (crate, feature, environment variable, archive task).

## Acceptance

`task check` passes on a machine with no checkpoint, with the native library verified by its pinned
sha256 and no espeak-ng or piper-phonemize linked, and `INFERENCE_SHERPA_ONNX_STT_DIR=<pinned
checkpoint> cargo test -p inference-sherpa-onnx --all-features -- --ignored` passes, recognising the
checkpoint's own LibriSpeech clip `test_wavs/0.wav`, streamed in 20 ms frames, into a committed span
that contains its distinctive words.

Amended on 2026-10-20 by the coordinating session. The first live run showed the pinned
LibriSpeech-only checkpoint turns the eSpeak NG fixture "What is the capital of France?" into
"capitals"; a NeMo streaming checkpoint did no better. The pipeline itself transcribes real speech
word for word. Synthetic eSpeak speech stays as a second ignored test that only runs to end of
input, and the limitation is written in the crate docs.

## Out of scope

Conformance against the claims, cancellation, stale output, concurrency and the A-01 example
(`story:sherpa-onnx-stt-conformance`); the capability record with resource profile, evidence,
languages and formats (`decision-blocker:capability-record-shape`); GPU execution; VAD and
endpointing as graph nodes; the whisper.cpp swap (backend-replacement).

## Licence

Added 2026-10-20 by the coordinating session. sherpa-onnx's default build turns TTS on and links
espeak-ng, which is GPL-3.0 (`decision-blocker:sherpa-tts-gpl-phonemiser`); the code licence policy
in `AGENTS.md` admits Apache-2.0, MIT or more permissive. The STT adapter links no espeak-ng and no
piper-phonemize:

- Before pinning the static archive, list its libraries. If it carries espeak-ng or
  piper-phonemize, the crate links against a sherpa-onnx built from source with
  `SHERPA_ONNX_ENABLE_TTS=OFF` (its source tarball pinned by sha256 like the archive), or against a
  shared-library archive without them; whichever needs no change outside `adapters/`, `Taskfile.yml`
  and `.cargo/config.toml`.
- A test run by `task check` fails when the linked test binary contains an espeak symbol (for
  example `espeak_ng_Initialize`), so a later version bump cannot bring it back unseen.
- If neither route builds, stop and report; do not link the GPL code.
