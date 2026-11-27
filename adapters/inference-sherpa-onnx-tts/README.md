# inference-sherpa-onnx-tts

A local text-to-speech adapter of the inference framework: `SherpaOnnxSynthesiser` implements the
`inference-tts` contract over sherpa-onnx's offline text-to-speech, as native FFI in the calling
process. No hosted call is made.

It is opt-in. No contract crate, no other adapter and not the `inference` facade depends on it; a
build that does not name this crate links none of it. The reason is the licence of the phonemiser,
below.

## Licences of the code

A voice is spoken by the sherpa-onnx native library built with text-to-speech. That archive links
29 libraries, and each has a row: the licence as it was read on 2026-11-25, the version, tag or
commit it was read at, the URL of the text, and the names it has in the library list of the build
script of `sherpa-onnx-sys` 1.13.8 (`build.rs:16-30`, thirteen names). A library without a name
there is built into one that has one, or comes from the toolchain.

| Component | Read at | Licence | Text read | Names in `build.rs` |
|---|---|---|---|---|
| sherpa-onnx | `v1.13.8` | Apache-2.0 | https://raw.githubusercontent.com/k2-fsa/sherpa-onnx/v1.13.8/LICENSE | `sherpa-onnx-c-api`, `sherpa-onnx-core` |
| kaldi-native-fbank | `v1.22.3` | Apache-2.0 | https://raw.githubusercontent.com/csukuangfj/kaldi-native-fbank/v1.22.3/LICENSE | `kaldi-native-fbank-core` |
| kissfft | `febd4caeed32e33ad8b2e0bb5ea77542c40f18ec` | BSD-3-Clause | https://raw.githubusercontent.com/mborgerding/kissfft/febd4caeed32e33ad8b2e0bb5ea77542c40f18ec/COPYING | `kissfft-float` |
| kaldi-decoder | `v0.3.0` | Apache-2.0 | https://raw.githubusercontent.com/k2-fsa/kaldi-decoder/v0.3.0/LICENSE | `kaldi-decoder-core` |
| kaldifst | `v1.8.0` | Apache-2.0 | https://raw.githubusercontent.com/k2-fsa/kaldifst/v1.8.0/LICENSE | `sherpa-onnx-kaldifst-core` |
| openfst | `v1.8.5-2026-07-09` | Apache-2.0 | https://raw.githubusercontent.com/csukuangfj/openfst/v1.8.5-2026-07-09/COPYING | `sherpa-onnx-fstfar`, `sherpa-onnx-fst` |
| Eigen | `5.0.1` | MPL-2.0 | https://gitlab.com/libeigen/eigen/-/raw/5.0.1/LICENSE |  |
| simple-sentencepiece | `v0.7` | Apache-2.0 AND BSD-2-Clause AND Zlib | https://raw.githubusercontent.com/pkufool/simple-sentencepiece/v0.7/LICENSE | `ssentencepiece_core` |
| nlohmann/json | `v3.12.0` | MIT | https://raw.githubusercontent.com/nlohmann/json/v3.12.0/LICENSE.MIT |  |
| hclust-cpp | `2026-02-25` | BSD-2-Clause | https://raw.githubusercontent.com/csukuangfj/hclust-cpp/2026-02-25/LICENSE |  |
| espeak-ng | `ed530aa113046142eb5115cf2fc9157854d0ffe1` | GPL-3.0-or-later | https://raw.githubusercontent.com/csukuangfj/espeak-ng/ed530aa113046142eb5115cf2fc9157854d0ffe1/COPYING | `espeak-ng` |
| ucd-tools | `ed530aa113046142eb5115cf2fc9157854d0ffe1` | GPL-3.0-or-later AND Unicode-DFS-2016 | https://raw.githubusercontent.com/csukuangfj/espeak-ng/ed530aa113046142eb5115cf2fc9157854d0ffe1/src/ucd-tools/COPYING | `ucd` |
| piper-phonemize | `f3ff95afc03640bc1399e113e83361192a2fafb4` | MIT | https://raw.githubusercontent.com/csukuangfj/piper-phonemize/f3ff95afc03640bc1399e113e83361192a2fafb4/LICENSE.md | `piper_phonemize` |
| uni-algo | `f3ff95afc03640bc1399e113e83361192a2fafb4` | Unlicense OR MIT | https://raw.githubusercontent.com/csukuangfj/piper-phonemize/f3ff95afc03640bc1399e113e83361192a2fafb4/licenses/uni-algo/LICENSE.md |  |
| onnxruntime | `33ca9628233dc8f002435e868d4c2e9f82766ca1` | MIT | https://raw.githubusercontent.com/csukuangfj/onnxruntime/33ca9628233dc8f002435e868d4c2e9f82766ca1/LICENSE | `onnxruntime` |
| abseil-cpp | `20250814.0` | Apache-2.0 | https://raw.githubusercontent.com/abseil/abseil-cpp/20250814.0/LICENSE |  |
| onnx | `v1.22.0` | Apache-2.0 | https://raw.githubusercontent.com/onnx/onnx/v1.22.0/LICENSE |  |
| protobuf | `v21.12` | BSD-3-Clause | https://raw.githubusercontent.com/protocolbuffers/protobuf/v21.12/LICENSE |  |
| re2 | `2024-07-02` | BSD-3-Clause | https://raw.githubusercontent.com/google/re2/2024-07-02/LICENSE |  |
| flatbuffers | `v23.5.26` | Apache-2.0 | https://raw.githubusercontent.com/google/flatbuffers/v23.5.26/LICENSE |  |
| Eigen in onnxruntime | `1d8b82b0740839c0de7f1242a3585e3390ff5f33` | MPL-2.0 | https://raw.githubusercontent.com/eigen-mirror/eigen/1d8b82b0740839c0de7f1242a3585e3390ff5f33/COPYING.MPL2 |  |
| GSL | `v4.2.1` | MIT | https://raw.githubusercontent.com/microsoft/GSL/v4.2.1/LICENSE |  |
| nlohmann/json in onnxruntime | `v3.11.3` | MIT | https://raw.githubusercontent.com/nlohmann/json/v3.11.3/LICENSE.MIT |  |
| date | `v3.0.1` | MIT | https://raw.githubusercontent.com/HowardHinnant/date/v3.0.1/LICENSE.txt |  |
| Boost.Mp11 | `boost-1.82.0` | BSL-1.0 | https://raw.githubusercontent.com/boostorg/boost/boost-1.82.0/LICENSE_1_0.txt |  |
| SafeInt | `3.0.28` | MIT | https://raw.githubusercontent.com/dcleblanc/SafeInt/3.0.28/LICENSE |  |
| cpuinfo | `4628dc060ce4e82345dc166bbac875609db4ff69` | BSD-2-Clause | https://raw.githubusercontent.com/pytorch/cpuinfo/4628dc060ce4e82345dc166bbac875609db4ff69/LICENSE |  |
| libstdc++ and libgcc | `releases/gcc-11.2.0` | GPL-3.0-or-later WITH GCC-exception-3.1 | https://raw.githubusercontent.com/gcc-mirror/gcc/releases/gcc-11.2.0/COPYING.RUNTIME |  |
| glibc | `glibc-2.17` | LGPL-2.1-or-later | https://sourceware.org/git/?p=glibc.git;a=blob_plain;f=COPYING.LIB;hb=refs/tags/glibc-2.17 |  |

The archive holds three objects. The rows from sherpa-onnx to uni-algo are
`libsherpa-onnx-c-api.so`, beside which `libsherpa-onnx-cxx-api.so` is sherpa-onnx's own C++
wrapper; the rows from onnxruntime to cpuinfo are `libonnxruntime.so`; the last two rows are pieces
of the compiler's runtime, in all three objects, and of the C library, in the first. `CODE_LICENSES`
is the same list, and a test of this crate compares it with the build script of the pinned
`sherpa-onnx-sys`, so a pin that moves is red until the list follows.

What a row's identifier does not say:

- One file of espeak-ng is offered under BSD-2-Clause as well. Its `COPYING` is the text of the GPL
  version 3; "or later" is read from its `README.md` ("released under the GPL version 3 or later")
  and from the headers of its sources ("either version 3 of the License, or (at your option) any
  later version", `src/libespeak-ng/speech.c`).
- `GPL-3.0-or-later AND Unicode-DFS-2016` for ucd-tools is this repository's reading of the texts,
  not an expression a file states. ucd-tools has licence files of its own in espeak-ng's source
  tree. The files read: `src/ucd-tools/COPYING`, the text of the GPL version 3; the compiled
  `src/ucd-tools/src/case.c` and `src/ucd-tools/src/categories.c`, whose headers say "either
  version 3 of the License, or (at your option) any later version"; and
  `src/ucd-tools/COPYING.UCD`, the licence of the Unicode data files from which the compiled tables
  are generated, in the form SPDX names Unicode-DFS-2016.
- `Apache-2.0 AND BSD-2-Clause AND Zlib` for simple-sentencepiece is this repository's reading of
  the texts as well. The files read: `LICENSE`, the Apache License 2.0;
  `ssentencepiece/csrc/darts.h`, the bundled darts-clone, whose header names "The BSD 2-clause
  license"; and `ssentencepiece/csrc/threadpool.h`, the bundled thread pool, whose header carries
  the wording SPDX names Zlib ("This software is provided 'as-is'") and no name for it. Both
  bundled files are in the object.
- `Unlicense OR MIT` for uni-algo is read from `licenses/uni-algo/LICENSE.md` in piper-phonemize's
  tree: a public-domain dedication in the wording SPDX names Unlicense, then the MIT text. The
  header of the bundled `src/uni_algo.h` says "License: Public Domain or MIT - choose whatever you
  want."
- onnxruntime is not built by sherpa-onnx: the object is the one of the prebuilt
  `onnxruntime-linux-x64-glibc2_17-Release-1.28.2.zip` that sherpa-onnx 1.13.8 pins
  (`cmake/onnxruntime-linux-x86_64.cmake`), sha256
  `c4f8994d56191d9d2c92a961b39fe790459f2c5d155f912b239506ea31359534`, version 1.28.2, built from the
  commit of its row. The text was read at a fork that holds that commit and is byte for byte the
  `LICENSE` of the zip; the upstream repository did not answer that day.
- Boost.Mp11 has no licence file in its own repository at the tag; its header names the Boost
  Software License 1.0, read in the Boost super-project at the same tag.
- The glibc files carry, beside the LGPL, a permission that has no SPDX identifier: "unlimited
  permission to link the compiled version of this file with other programs, and to distribute those
  programs without any restriction coming from the use of this file" (header of `stdlib/atexit.c`
  at the tag).

What the reading did not determine: the versions are those the build files of sherpa-onnx 1.13.8
and of the onnxruntime commit name, read from those files and not observed in a build, so which
Eigen and openfst went into the sherpa-onnx object, and whether the onnxruntime object was built
from exactly the versions its `cmake/deps.txt` names, is open. The pieces of the compiler's runtime
and of the C library were read at the upstream releases nearest to what the objects say, GCC 11.2.0
for a compiler string of 11.2.1 and glibc 2.17 by the name of the build container, not in the
sources of the container's packages. Files with a licence of their own inside the trees were not
audited one by one, the revision of the project that sherpa-onnx's tashkeel tokenizer was adapted
from is not pinned, and what the sherpa-onnx build fetches but the archive does not link was not
read.

espeak-ng turns text into phonemes for both kinds of voice this adapter opens. It is under the GPL,
version 3 or later (GPL-3.0-or-later), which the repository's licence rule for local adapters does
not admit. The repository's owner made one named exception,
approval-record:sherpa-tts-gpl-phonemiser, with these bounds:

- espeak-ng is admitted only inside the sherpa-onnx native library built with text-to-speech and as
  the `espeak-ng-data` directory a voice carries.
- Nothing of it is committed to or distributed by this repository: no library, no archive, no data
  file. A test of this crate walks the repository for such files.
- It is recorded as a code-licence entry of its own, here and in `CODE_LICENSES`, which fills the
  `code_license` of the model the synthesiser declares. That field names every row above, each as
  `<licence> (<component>)`.

Five more rows are outside that rule: Eigen and Eigen in onnxruntime (MPL-2.0), ucd-tools,
libstdc++ and libgcc, and glibc. The owner's decision of 2026-11-27 admits them only as built into
the upstream archives the build fetches, never committed or distributed by this repository:
ucd-tools as part of the espeak-ng exception, under the bounds above, and the compiler's runtime
and the C library under their runtime and linking exceptions. Every other row is inside the rule.

Who builds a program on this adapter and distributes it with that library distributes code under
the GPL, version 3 or later, and decides for themselves what follows from it.

## The pinned checkpoint

`SherpaOnnxSynthesiser::open(dir)` opens Kokoro-82M v0.19 for English: 24 kHz, mono, 11 voices
(`VOICES`, chosen with `with_speaker`).

| What | Value |
|---|---|
| Archive | https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-en-v0_19.tar.bz2 |
| Archive sha256 | `912804855a04745fa77a30be545b3f9a5d15c4d66db00b88cbcd4921df605ac7` |
| `model.onnx` | 345555491 bytes, sha256 `10ff414106a038ce7e9e0126c6461e4dc8a86efaa89dc91d2009d69fe635e339` |
| `voices.bin` | 5755904 bytes, sha256 `a372c67b056ef0b695c375d39b99630d23fb07ad4c8d87aa32a19a62fca523ad` |
| `tokens.txt` | 1078 bytes, sha256 `4f31c71282d14af4e926cd12462078fe9d20d00c589e63fe2750a8f56d6d7f7b` |
| `espeak-ng-data/` | 355 files, tree digest `71a159a95ee8e26c4110582ec69093f0e39eb453c3a33882f6e971510ff5c47a`, GPL-3.0 |
| `LICENSE` sha256 | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` (the text of the Apache License 2.0) |
| Licence of weights, voices and tokens | Apache-2.0 |
| Source of that licence | https://huggingface.co/hexgrad/kLegacy at revision `d0acca3b0748cae52e0a14a28ad03515002ff22f`, `v0.19/` |

The tree digest is the sha256 of one line `<sha256 of the file>  <path below the directory>` per
file, in byte order of the paths (`tree_sha256`). `open` checks the three files and the tree before
it calls sherpa-onnx and refuses a directory that differs, naming the file. A path that is neither
a regular file nor a directory, a FIFO for one, is refused by its metadata and never opened; what
a symbolic link points to counts. A file the process may not open is refused with the kind of that
error.

What is read is bounded by the pin. A file whose length is not the pinned one is refused without
being read (`OpenRefused::LengthMismatch`), and a file of the pinned length is read to that length.
The walk of `espeak-ng-data` ends at the first file over the pinned count of 355, before any file
of the tree is read (`OpenRefused::TooManyFiles`). The length of a file inside that tree is not
pinned: a tree of at most 355 files is read whole for its digest.

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
and `tokens.txt` are regular files that the process may open and that hold at least one byte
(`OpenRefused::FileMissing`, `OpenRefused::FileEmpty`), and `espeak-ng-data` is a directory the
process may list. Each of the two files is opened for that, so one of mode 000 is refused with
`FileMissing` and the kind permission denied; the files inside `espeak-ng-data` are not opened. So
open a voice only from a directory whose files are trusted, and in a child process when the
calling process has to outlive a voice that does not load. The pinned checkpoint is not reached this way: `open` checks each of
its files against its sha256 before sherpa-onnx reads any.

## A synthesis and its end

A segment is generated on a thread of its own, and sherpa-onnx hands its audio over part by part. A
part is what it generates in one run of the model. It makes one part per sentence, as espeak-ng
divides the text, and starts a further part within a sentence each time its phonemes have filled
one: about 510 tokens. That bound is sherpa-onnx's `max_token_len` for a Kokoro model, no field of
its configuration: it is the first number of the model's `style_dim` metadata, 511 for the pinned
checkpoint. Measured on a CPU: one sentence of 139 characters arrived as one part; one sentence of
679 characters arrived in 2 parts, 4.35 s apart in one run and 5.74 s in another.

So the frames of a segment of one short sentence are delivered together once the sentence is
generated. Frames arrive while more is generated only for a segment of several sentences, or of one
sentence long enough to be split.

Discarding the segment being generated, or dropping the synthesis, tells sherpa-onnx to stop at its
next part, the end of the part it is generating. Neither waits for that, and dropping the
synthesis and the last synthesiser does not either: no drop joins a thread. So one thread per
generation in flight, and the native engine it holds, can outlive every handle until that part
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
