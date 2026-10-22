---
title: Limitations
sidebar_position: 1
description: What does not exist yet, what each adapter cannot do, and where the egress watch stops.
lede: The gaps that shape a product built on inference today, each with its reason.
source: adapters/*/src (lib.rs, stt.rs, live_duplex.rs, convert.rs), crates/inference-runtime/src/egress.rs, crates/inference-audio, AGENTS.md (licence policy)
---

# Limitations

What has shipped and what is planned is on [Status](../status.mdx). This page lists what a product
built on inference today has to work around.

## No local text-to-speech

The offline cascade binds the mock synthesiser: the answer is printed, and the speaker plays 10 ms
of silence per word. A local TTS adapter is planned and waits on the licence policy below:
sherpa-onnx's TTS build links espeak-ng, which is GPL-3.0, and the policy admits Apache-2.0, MIT or
more permissive backend code. The backend-neutral TTS checks exist, so an adapter that fits the
policy is held to the same contract.

## No self-hosted native speech model

The three native speech adapters are hosted services. A self-hosted native speech model is planned.

## Audio

- There is no echo cancellation or noise suppression. Use a headset: a laptop's own speaker feeds
  back into its microphone, and the model interrupts itself.
- `inference-audio` carries no codec or resampler. The device adapter resamples linearly, with no
  anti-aliasing filter: downsampling folds content above the new Nyquist frequency back into the
  band. For speech from a headset or laptop microphone that content is small.

## Hosted adapters

- Every hosted adapter runs at locality `external_provider`, which only the `hybrid_opt_in`
  [deployment profile](../concepts/deployment-profiles.md) admits. Audio leaves the machine.
- Only GPT-Live claims `native_duplex`; OpenAI Realtime and Nova Sonic have not given the
  conformance procedure that evidence.
- GPT-Live decides itself when to speak. It has no responses, commits, cancellation or truncation
  on its voice side, so a caller's commit, response creation, cancel and truncation are refused, and
  it does not claim `interruptible`. The API marks neither the end of a spoken answer nor an
  interruption, so a response the listener cut short ends as completed.
- OpenAI keys are injected, sent once in the handshake and never logged by the adapter. The
  WebSocket library below it, `tungstenite`, logs the raw handshake, the key included, at trace
  level. Keep `tungstenite` targets at debug or lower, for example `RUST_LOG=tungstenite=debug`.

## Local adapters

- **sherpa-onnx** links the release archive built without TTS, so no GPL-licensed phonemiser is
  linked. The checkpoint is trained on read audiobook speech and does not recognise synthetic
  speech reliably. The native archive is for Linux x86_64.
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

- Backend code is Apache-2.0 or MIT, or more permissive.
- Pinned weights are CC BY 4.0, or more permissive.
- Each checkpoint's licence, source URL and hash go into the adapter's capability record when it is
  adopted. Any other licence needs an explicit decision for that checkpoint.
- Checkpoints are pinned by sha256; an adapter refuses a file that differs, naming it.
- Tests download no weights and start no model server: a test that needs one reads its path or URL
  from an environment variable and is ignored without it.

The licences of each shipped checkpoint are in [Adapters](../reference/adapters.md).
