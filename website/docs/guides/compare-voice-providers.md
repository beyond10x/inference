---
title: Compare voice providers
sidebar_position: 5
description: Send the same recorded question through each hosted native speech model and compare latency, barge-in stop time and usage, without a microphone or a speaker.
lede: One recorded question or a directory of them, every provider, N runs each - the median, p95 and spread of each latency, and the usage of every run.
source: examples/realtime-voice (examples/compare.rs, src/compare.rs, src/elevenlabs.rs, tests/compare.rs, tests/compare_elevenlabs.rs), adapters/inference-openai/tests/fixtures/speech.wav
---

# Compare voice providers

The `compare` example of `realtime-voice` sends the same recorded question, or each clip of a
directory, through each provider, N times, and prints per provider, native speech model or
cascade of STT, LLM and TTS, the median, the 95th percentile and the spread of:

- end of the question's speech to the answer's first audio, first transcript and response end,
  the turn latency;
- barge-in to output stop, when a barge-in utterance is given;
- for the cascade, end of speech to the recogniser's final transcript, the LLM's first token and
  the TTS's first audio;
- for the cascade, the synthesiser's own time to first audio of the answer's first segment, from
  its offer to its first frame, as the TTS contract measures it (`tts offer to audio`);
- the usage of each run.

A step a topology does not have shows `n/a`. Every time is taken on the client, on one clock, at
the receipt of each event. No microphone or speaker is used.

Since no device plays the answer, both topologies play it on the client's model of playout: each
frame from its receipt or from the end of the frame before it, whichever is later, cut where the
client stops playout. Barge-in to output stop runs to the end of the answer's audio as the client
plays it. For the cascade, playout therefore starts at the TTS's first audio, so the cascade's
first audio is its TTS first audio and the table has no playout start column. The cascade's
client stops playout once the local cascade's detector has heard the barge-in; an answer that ends
before then stops where it ends.

:::warning[Requirements]

The keys or credentials each provider needs, read as in
[Talk to a hosted voice model](talk-to-a-hosted-voice-model.md). Every run is billed.

:::

## Run it

```sh
cargo run -p realtime-voice --example compare -- --providers openai,openai-live --runs 2
INFERENCE_COMPARE_BARGE_IN_WAV=<a WAV file of speech> cargo run -p realtime-voice --example compare -- --runs 2
cargo run -p realtime-voice --example compare -- --skip nova="no model access"
```

The question is the OpenAI adapter's speech fixture, "What is the capital of France?". The
barge-in utterance is the 16-bit PCM WAV file `INFERENCE_COMPARE_BARGE_IN_WAV` names; without it no
barge-in is sent and that column stays empty. A provider whose model cannot be made, or whose first
session does not open, is skipped with the reason.

The `cascade` provider speaks with ElevenLabs when its key resolves, from `ELEVENLABS_API_KEY` or,
when that is unset, the OS keychain's `elevenlabs-api-key` (waiting up to 8 s, and saying so on
standard error), and `ELEVENLABS_VOICE_ID` names a voice; otherwise it is skipped as "no hosted
TTS configured". It
connects to the host `ELEVENLABS_RESIDENCY` names (`global`, `us`, `eu`, `in` or `sg`, global when
unset), as a key of a data-residency stack needs; a value it does not take skips the arm. Its
recogniser and LLM are the local voice cascade's, bound with feature `local` and configured as in
the local cascade's guide; a missing one skips it, naming which:

```sh
cargo run -p realtime-voice --features local --example compare -- --providers openai,cascade
```

With `--cascade-stt elevenlabs` the cascade arm listens with ElevenLabs realtime speech to text
instead, in `pcm_16000` at the same host, its key `ELEVENLABS_API_KEY` or the OS keychain's
`elevenlabs-api-key`. Its LLM is the server `INFERENCE_LLAMA_SERVER_URL` and
`INFERENCE_LLAMA_SERVER_MODEL` name, on loopback, a `llama-server` or Ollama
([Run the voice cascade offline](run-the-local-voice-cascade-offline.md)); the local recogniser's
variables are not read. The arm still needs feature `local`, which binds that LLM:

```sh
INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:11434/v1 INFERENCE_LLAMA_SERVER_MODEL=qwen2.5:1.5b \
    cargo run -p realtime-voice --features local --example compare -- \
    --providers openai,elevenlabs,cascade --cascade-stt elevenlabs --runs 5
```

The ElevenLabs recognition commits manually: it is opened with `commit_strategy=manual`, and the
arm's own detector, the local cascade's (600 ms of silence after speech), ends each turn and so
sends the commit, as it ends the local recogniser's input. The endpoint's voice activity detection
is not used, since it would add its quiet window of 500 ms after the commit's answer to the
cascade's first audio. A failed recognition is a note of the arm's run, with the endpoint's reason,
also when the run's limit passes first: the run then waits for the recognition at most 3 s, the
plan's drain, and says so when it had still not ended.

`--clips <dir>` asks each 16-bit PCM WAV file of a directory (`.wav`, in the order of the file
names) in place of the built-in question: each provider runs `--runs` runs of each clip, every run
of the first clip first, and the table pools the runs of all clips. `--runs-out <file>` writes
every run as one JSON line: `provider`, `topology`, `clip` (the file name without `.wav`, or
`question` for the built-in one), `run` (from 1 for each clip), each measure the provider's
topology has as `<measure>_ms`, null when the run did not measure it, the cascade's
`tts_segment_first_audio_ms`, `seconds` billed, and `notes`. The file is written after the table
is printed; when it does not write, `compare` says why and exits with status 1, the table already
out:

```sh
cargo run -p realtime-voice --example compare -- --providers openai --clips questions/ --runs 10 --runs-out runs.jsonl
```

Each cell of the table is `median [lowest-highest] measured/runs`, followed by `p95` and the 95th
percentile, by nearest rank, when more than one run measured it.

```sh
cargo run -q -p realtime-voice --example compare -- --help
```

```text
Compare native speech providers and a cascade on recorded utterances

Usage: compare [OPTIONS]

Options:
      --providers <PROVIDERS>
          The providers to run, in order

          Possible values:
          - openai:      The OpenAI Realtime API
          - openai-live: OpenAI GPT-Live
          - nova:        Amazon Nova Sonic on Bedrock
          - elevenlabs:  An ElevenLabs agent conversation
          - deepgram:    A Deepgram Voice Agent session
          - phonic:      A Phonic speech-to-speech conversation
          - deepslate:   A Deepslate Opal session
          - cascade:     A recogniser (--cascade-stt), the local LLM and a hosted voice (--cascade-voice)
          
          [default: openai,openai-live,nova,elevenlabs,deepgram,phonic,deepslate,cascade]

      --runs <RUNS>
          The runs of each provider
          
          [default: 3]

      --skip <PROVIDER=REASON>
          A provider not to try, and why: `nova="no model access"`

      --frame-ms <FRAME_MS>
          The length of one input frame, in milliseconds
          
          [default: 20]

      --cascade-voice <CASCADE_VOICE>
          The hosted voice the cascade arm speaks with

          Possible values:
          - elevenlabs: ElevenLabs streaming TTS
          - cartesia:   Cartesia Sonic streaming TTS
          
          [default: elevenlabs]

      --cascade-stt <CASCADE_STT>
          The recogniser the cascade arm listens with

          Possible values:
          - local:      The local voice cascade's recogniser, sherpa-onnx or whisper.cpp
          - elevenlabs: ElevenLabs realtime speech-to-text, committed when the arm's detector ends the turn
          
          [default: local]

      --clips <DIR>
          A directory of 16-bit PCM WAV question clips, each asked in turn in place of the built-in question

      --runs-out <FILE>
          A file to write every run's times to, one JSON line per run

  -h, --help
          Print help (see a summary with '-h')
```

## What the table looks like

A live comparison is billed, so no live output is shown. The test below runs the same comparison
with two scripted providers on a fake clock, `fast` and `slow`, and a skipped `nova`, and prints
the table the example prints:

```sh
cargo test -q -p realtime-voice --test compare two_scripted_providers_give_their_figures_their_order_and_the_table -- --nocapture
```

```text

running 1 test
times in ms, taken on the client at the receipt of each event: median [lowest-highest] measured/runs, then p95 when more than one run measured it; n/a: a step the topology does not have
from the end of the question's speech; barge-in from the barge-in's first voiced frame; response end is the turn latency
tts offer to audio: the synthesis's own, from the answer's first segment's offer to its first frame
provider  topology  first audio                     first transcript                response end                    barge-in to stop                final transcript                first token                     tts first audio                 tts offer to audio
fast      native    320 [300-340] 3/3 p95 340       260 [250-270] 3/3 p95 270       900 [900-900] 3/3 p95 900       200 [200-200] 3/3 p95 200       n/a                             n/a                             n/a                             n/a
slow      native    640 [600-700] 3/3 p95 700       520 [500-550] 3/3 p95 550       1840 [1800-1900] 3/3 p95 1900   100 [100-100] 3/3 p95 100       n/a                             n/a                             n/a                             n/a
nova      native    skipped: blocked on AWS access
usage per run, as each provider reports it
fast      run 1: responses 2: in text 20 audio 40 cached 0, out text 10 audio 80; transcription in ? out ? (0 of 0 turns reported)
fast      run 2: responses 2: in text 20 audio 40 cached 0, out text 10 audio 80; transcription in ? out ? (0 of 0 turns reported)
fast      run 3: responses 2: in text 20 audio 40 cached 0, out text 10 audio 80; transcription in ? out ? (0 of 0 turns reported)
slow      run 1: responses 2: in text 20 audio ? cached 0, out text 10 audio ?; transcription in ? out ? (0 of 0 turns reported); billed 12.5 s
slow      run 2: responses 2: in text 20 audio ? cached 0, out text 10 audio ?; transcription in ? out ? (0 of 0 turns reported); billed 12.5 s
slow      run 3: responses 2: in text 20 audio ? cached 0, out text 10 audio ?; transcription in ? out ? (0 of 0 turns reported); billed 12.5 s

.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.05s

```
