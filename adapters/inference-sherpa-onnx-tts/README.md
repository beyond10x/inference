# inference-sherpa-onnx-tts

A local text-to-speech adapter of the inference framework: `SherpaOnnxSynthesiser` implements the
`inference-tts` contract over sherpa-onnx's offline text-to-speech, as native FFI in the calling
process. No hosted call is made.

It is opt-in. No contract crate, no other adapter and not the `inference` facade depends on it; a
build that does not name this crate links none of it. The reason is the licence of the phonemiser,
below.

## Licences of the code

A voice is spoken by the sherpa-onnx native library built with text-to-speech. The components
whose licence was read when this adapter was adopted:

| Component | Licence | Source |
|---|---|---|
| sherpa-onnx 1.13.8 | Apache-2.0 | https://github.com/k2-fsa/sherpa-onnx/blob/v1.13.8/LICENSE |
| onnxruntime | MIT | https://github.com/microsoft/onnxruntime |
| piper-phonemize | MIT | https://github.com/csukuangfj/piper-phonemize/blob/f3ff95afc03640bc1399e113e83361192a2fafb4/LICENSE.md |
| espeak-ng | GPL-3.0 | https://github.com/csukuangfj/espeak-ng/blob/ed530aa113046142eb5115cf2fc9157854d0ffe1/COPYING |

The table is not every component of that library. The build script of `sherpa-onnx-sys` 1.13.8
names thirteen libraries (`build.rs:16-30`). The four rows cover five of them: `sherpa-onnx-c-api`
and `sherpa-onnx-core` are sherpa-onnx itself. The licences of the other eight were not read and
have no row: `kaldi-decoder-core`, `sherpa-onnx-kaldifst-core`, `sherpa-onnx-fstfar`,
`sherpa-onnx-fst`, `kaldi-native-fbank-core`, `kissfft-float`, `ucd` and `ssentencepiece_core`.

espeak-ng turns text into phonemes for both kinds of voice this adapter opens. It is GPL-3.0, which
the repository's licence rule for local adapters does not admit. The repository's owner made one
named exception, approval-record:sherpa-tts-gpl-phonemiser, with these bounds:

- espeak-ng is admitted only inside the sherpa-onnx native library built with text-to-speech and as
  the `espeak-ng-data` directory a voice carries.
- Nothing of it is committed to or distributed by this repository: no library, no archive, no data
  file. A test of this crate walks the repository for such files.
- It is recorded as a code-licence entry of its own, here and in `CODE_LICENSES`, which fills the
  `code_license` of the model the synthesiser declares. That field names the licences read, the
  four rows above.

Who builds a program on this adapter and distributes it with that library distributes GPL-3.0 code
and decides for themselves what follows from it.

## The pinned checkpoint

`SherpaOnnxSynthesiser::open(dir)` opens Kokoro-82M v0.19 for English: 24 kHz, mono, 11 voices
(`VOICES`, chosen with `with_speaker`).

| What | Value |
|---|---|
| Archive | https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-en-v0_19.tar.bz2 |
| Archive sha256 | `912804855a04745fa77a30be545b3f9a5d15c4d66db00b88cbcd4921df605ac7` |
| `model.onnx` sha256 | `10ff414106a038ce7e9e0126c6461e4dc8a86efaa89dc91d2009d69fe635e339` |
| `voices.bin` sha256 | `a372c67b056ef0b695c375d39b99630d23fb07ad4c8d87aa32a19a62fca523ad` |
| `tokens.txt` sha256 | `4f31c71282d14af4e926cd12462078fe9d20d00c589e63fe2750a8f56d6d7f7b` |
| `espeak-ng-data/` | 355 files, tree digest `71a159a95ee8e26c4110582ec69093f0e39eb453c3a33882f6e971510ff5c47a`, GPL-3.0 |
| `LICENSE` sha256 | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` (the text of the Apache License 2.0) |
| Licence of weights, voices and tokens | Apache-2.0 |
| Source of that licence | https://huggingface.co/hexgrad/kLegacy at revision `d0acca3b0748cae52e0a14a28ad03515002ff22f`, `v0.19/` |

The tree digest is the sha256 of one line `<sha256 of the file>  <path below the directory>` per
file, in byte order of the paths (`tree_sha256`). `open` checks the three files and the tree before
it calls sherpa-onnx and refuses a directory that differs, naming the file. A path that is neither
a regular file nor a directory, a FIFO for one, is refused by its metadata and never opened; what
a symbolic link points to counts.

## A caller's own voice

`SherpaOnnxSynthesiser::open_voice(dir)` opens a directory with one VITS model in the Piper format
as sherpa-onnx reads it: one `*.onnx` file, `tokens.txt` and `espeak-ng-data/`. Nothing of it is
pinned or checked against a hash. Its licence is the caller's: this adapter records none, and the
model it declares for such a voice says so.

The voice speaks at its own sample rate, for example 22.05 kHz. The synthesiser reports that rate
in `format()` and in the `codecs` of its adapter's descriptor, and resamples nothing.

A voice file that sherpa-onnx cannot load ends the process. The adapter cannot make sherpa-onnx's
own load of a file safe, and no refusal comes back from inside it:

- a model file that is not an ONNX model aborts the process (`SIGABRT`);
- a model sherpa-onnx can load that is of another family than the VITS form it reads, a Kokoro
  model for one, ends the process with exit status 255 and sherpa-onnx's message on standard error.

Before sherpa-onnx is called, `open_voice` checks only what costs a good voice nothing: the model
and `tokens.txt` are regular files that hold at least one byte (`OpenRefused::FileMissing`,
`OpenRefused::FileEmpty`), and `espeak-ng-data` is a directory. So open a voice only from a
directory whose files are trusted, and in a child process when the calling process has to outlive
a voice that does not load. The pinned checkpoint is not reached this way: `open` checks each of
its files against its sha256 before sherpa-onnx reads any.

## A synthesis and its end

A segment is generated on a thread of its own, and sherpa-onnx hands its audio over one part per
sentence of the segment's text. A segment of one sentence is one part: its frames are delivered
together once the sentence is generated. Only a segment of several sentences delivers the frames of
a sentence while the next is generated.

Discarding the segment being generated, or dropping the synthesis, tells sherpa-onnx to stop at its
next part, the end of the sentence it is generating. Neither waits for that, and dropping the
synthesis and the last synthesiser does not either: no drop joins a thread. So one thread per
generation in flight, and the native engine it holds, can outlive every handle until that sentence
is generated; the thread then releases the engine and ends. A process that returns from `main`
while such a thread still generates ends with it, with the exit status `main` gave.

## The native library

sherpa-onnx publishes its shared library in two builds under one file name,
`libsherpa-onnx-c-api.so`, with the same exported symbols: with text-to-speech and without. This
crate links against either. Which one a process runs on is the loader's choice when the process
starts. `native_tts()` reads it, and opening a synthesiser in a process that loaded the build
without text-to-speech is refused with `OpenRefused::LibraryWithoutTts`, naming the library,
before sherpa-onnx is called.

In this workspace every build, with or without this crate, links against the archive built without
text-to-speech, the one `.cargo/config.toml` names in `SHERPA_ONNX_LIB_DIR`. A linked test binary
names the library as needed and carries no run path; `cargo` and `cargo nextest` put the archive's
directory first in `LD_LIBRARY_PATH` of what they start, ahead of a value set by the caller. So the
tests that are not ignored run without espeak-ng, and a build that only recognises speech links and
loads what it did before this crate existed.

To speak in this workspace, fetch the library with text-to-speech and the checkpoint once, then
start the process on that library through a target runner:

```sh
task sherpa-onnx-tts
tts="$PWD/target/sherpa-onnx-tts"
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="env LD_LIBRARY_PATH=$tts/native/sherpa-onnx-v1.13.8-linux-x64-shared-lib/lib" \
INFERENCE_SHERPA_ONNX_TTS_DIR="$tts/checkpoint/kokoro-en-v0_19" \
cargo nextest run -p inference-sherpa-onnx-tts --all-features --run-ignored only -E 'not test(without_tts)'
```

The task checks both archives against their pinned sha256 before it unpacks them under `target/`.
No test and no other task runs it.

Outside this workspace nothing sets `SHERPA_ONNX_LIB_DIR`, so the build script of `sherpa-onnx-sys`
1.13.8 downloads sherpa-onnx's shared archive for the target, which is the build with
text-to-speech and espeak-ng, and `cargo run` and `cargo test` start on it. That download is
checked against no hash. For x86-64 Linux the archive and its pin are:

| What | Value |
|---|---|
| Archive | https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-linux-x64-shared-lib.tar.bz2 |
| Archive sha256 | `3892d184be41027e18165e67f549cd4e4cdd8dcd73ac5579e97afd55e14e30b6` |

A caller outside this workspace who wants the library checked takes one of two routes:

- Fetch the archive, compare its sha256 with the pin, and set `SHERPA_ONNX_ARCHIVE_DIR` at build
  time to the directory that holds it. The build script then takes the archive from there and
  downloads nothing; it does not check it either.
- Unpack the checked archive and set `SHERPA_ONNX_LIB_DIR` at build time to its `lib` directory.
  `task sherpa-onnx-tts` of this repository does the fetch, the check against that pin and the
  unpacking, into `target/sherpa-onnx-tts/native/`.

A program started without `cargo` finds the library through the loader's search path,
`LD_LIBRARY_PATH` or the system's.

## Tests

The tests that are not ignored need no weights: the refusals of `open` and `open_voice`, the
refusal on a library without text-to-speech, the licence entries, the capability record and the
synthesis over a scripted engine. The ignored tests speak: adapter conformance
(`tests/conformance.rs`), the pinned tree and the measurement behind the capability record. They
read the checkpoint directory from `INFERENCE_SHERPA_ONNX_TTS_DIR` and download nothing.
