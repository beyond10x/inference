---
title: Limitations
sidebar_position: 1
description: What does not exist yet, what each adapter cannot do, and where the egress watch stops.
lede: The gaps that shape a product built on inference today, each with its reason.
source: adapters/*/src (lib.rs, stt.rs, live_duplex.rs, convert.rs), adapters/inference-sherpa-onnx-tts/README.md, crates/inference-runtime/src/egress.rs, crates/inference-audio, AGENTS.md (licence policy)
---

# Limitations

What has shipped and what is planned is on [Status](../status.mdx). This page lists what a product
built on inference today has to work around.

## No self-hosted native speech model

The three native speech adapters are hosted services. A self-hosted native speech model is planned.

## Audio

- There is no echo cancellation or noise suppression. Use a headset: a laptop's own speaker feeds
  back into its microphone, and the model interrupts itself.
- The device adapter resamples linearly, with no anti-aliasing filter: downsampling folds content
  above the new Nyquist frequency back into the band. For speech from a headset or laptop
  microphone that content is small. The band-limited `Resampler` of `inference-audio` (since 0.16.0)
  does not fold, but the device adapter does not use it yet.
- `inference-audio` resamples only between 8, 16, 24 and 48 kHz, 16-bit mono, and converts only
  between `pcm_s16le`, `pcm_mulaw` and `pcm_alaw` (since 0.16.0).

## Hosted adapters

- Every hosted adapter runs at locality `external_provider`, which only the `hybrid_opt_in`
  [deployment profile](../concepts/deployment-profiles.md) admits. Audio leaves the machine.
- Only GPT-Live claims `native_duplex`; OpenAI Realtime and Nova Sonic have not given the
  conformance procedure that evidence.
- GPT-Live decides itself when to speak. It has no responses, commits, cancellation or truncation
  on its voice side, so a caller's commit, response creation, cancel and truncation are refused, and
  it does not claim `interruptible`. The API marks neither the end of a spoken answer nor an
  interruption, so a response the listener cut short ends as completed. The binding reports
  speech over an answer when an input transcript fragment starts inside the stretch the answer's
  output transcript covers; that both transcripts share one timeline is read from the protocol
  documents, not observed on the hosted service.
- OpenAI keys are injected, sent once in the handshake and never logged by the adapter. The
  WebSocket library below it, `tungstenite`, logs the raw handshake, the key included, at trace
  level. Keep `tungstenite` targets at debug or lower, for example `RUST_LOG=tungstenite=debug`.

## Local adapters

- **sherpa-onnx speech recognition** links the release archive built without TTS, so no
  GPL-licensed phonemiser is linked. The checkpoint is trained on read audiobook speech and does
  not recognise synthetic speech reliably. The native archive is for Linux x86_64.
- **sherpa-onnx text-to-speech** (`inference-sherpa-onnx-tts`, on `main` and in no release yet)
  speaks only in a process started on the sherpa-onnx library built with TTS, which carries the
  phonemiser espeak-ng, GPL-3.0; the pinned checkpoint carries espeak-ng's phoneme data. The
  repository commits and distributes neither: a task fetches them to your machine, and whoever
  distributes a program with that library distributes GPL-3.0 code. Opening the synthesiser on the
  library without TTS is refused, naming the library. The pinned checkpoint speaks English at
  24 kHz, and the archive is for Linux x86_64.
  - It streams output part by part: one part per sentence, and a further part within a sentence
    each time its phonemes have filled one, about 510 tokens (sherpa-onnx's `max_token_len`, the
    first number of the model's `style_dim` metadata, 511 for the pinned checkpoint; no field of
    its configuration). Measured: one sentence of 139 characters arrived as one part, one of 679
    characters in 2 parts, 4.35 s and 5.74 s apart in two runs. So a segment of one short sentence
    is delivered whole, after it is generated; frames arrive while more is generated only for a
    segment of several sentences or of one sentence long enough to be split. Every segment of
    the local cascade is a segment of one part.
  - Each segment is spoken on its own, and the local cascade makes a segment of each delta of the
    language model, so an answer is spoken delta by delta, without the prosody of a sentence and
    longer than the sentence is. Measured: a 12-word sentence offered as 13 word deltas came to
    7133 ms of audio, against 3186 ms offered as one segment.
  - A discard or a drop stops a generation at the end of the part being generated. No drop
    waits for that: one thread and the native engine can outlive the synthesiser until the
    part is generated.
  - In the local cascade a superseded answer is not cut at the synthesiser. The TTS node takes the
    next event on its text input only once the segment it offered last is generated, so every
    segment committed before the supersede is generated to its end. Measured twice with an answer of
    three one-sentence segments, superseded within 3 ms of their commit: the speaker stopped at
    once and dropped the 399 frames that still came, the last of them 2275 ms and 5671 ms after
    the supersede on a shared machine, and the first frame of the next answer followed 760 ms and
    1072 ms later.
  - A voice of your own in the Piper format is opened unpinned, its licence yours, at its own
    sample rate. A voice file that sherpa-onnx cannot load ends the process inside the native
    load: an abort for a file that is not a model, exit status 255 for a model of another family.
    The adapter checks beforehand only that the model and the tokens are regular files that the
    process may open and that are not empty, and that the phoneme data is a directory it may
    list. The pinned checkpoint is not reached this way: its files are checked against their
    sha256 before sherpa-onnx reads any.
- **whisper.cpp** is a batch recogniser: it holds a whole utterance in memory until input ends,
  decodes at most 30 s per window, and claims no streaming capability. It runs on the CPU.
- **llama-server**: cancelling closes the connection, and the adapter does not claim
  `interruptible`, because whether the server stopped computing is visible only on the server. Tool
  calling needs the server started with `--jinja`.

Measured memory, load time and real-time factor per adapter are in
[Adapters](../reference/adapters.md).

## The egress watch is not a sandbox

Under `offline_strict`, the egress watch fails a run on a connection that is not loopback. It is a
tripwire and a test instrument:

- it polls, so a socket opened and closed between two polls is missed;
- it prevents nothing: bytes of a connection it sees may already have left;
- it reads every socket of the process, so it cannot tell which node opened one;
- it does not read packet sockets, SCTP, or another process's sockets;
- it exists on Linux only.

If you need isolation, run the process in a network namespace or behind a firewall as well.

## Licence policy for local adapters

- Backend code is Apache-2.0 or MIT, or more permissive. One exception is named: espeak-ng,
  GPL-3.0, is admitted for the sherpa-onnx text-to-speech adapter alone, inside the sherpa-onnx
  native library built with TTS and the `espeak-ng-data` a voice carries. The adapter is a crate
  of its own that only the offline cascade example depends on, behind its feature `local`.
  espeak-ng is a licence entry of its own in three places: the `code_license` of the `Model` the
  adapter declares, the README of the crate, and the adapter's line on
  [Adapters](../reference/adapters.md). The capability record file has no licence field. The
  entries are the four components whose licence was read at adoption; the README names the eight
  further libraries of the native build whose licence was not read.
- Pinned weights are CC BY 4.0, or more permissive.
- Each checkpoint's licence, source URL and hash go into the adapter's capability record when it is
  adopted. Any other licence needs an explicit decision for that checkpoint.
- Checkpoints are pinned by sha256; an adapter refuses a file that differs, naming it.
- Tests download no weights and start no model server: a test that needs one reads its path or URL
  from an environment variable and is ignored without it.

The licences of each shipped checkpoint are in [Adapters](../reference/adapters.md).
