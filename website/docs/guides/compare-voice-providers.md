---
title: Compare voice providers
sidebar_position: 5
description: Send the same recorded question through each hosted native speech model and compare latency, barge-in stop time and usage, without a microphone or a speaker.
lede: One recorded question, every provider, N runs each - the median and spread of each latency, and the usage of every run.
source: examples/realtime-voice (examples/compare.rs, src/compare.rs, tests/compare.rs), adapters/inference-openai/tests/fixtures/speech.wav
---

# Compare voice providers

The `compare` example of `realtime-voice` sends the same recorded question through each provider,
N times, and prints per provider the median and spread of:

- end of the question's speech to the answer's first audio, first transcript and response end;
- barge-in to output stop, when a barge-in utterance is given;
- the usage of each run.

Every time is taken on the client, at the receipt of each duplex event. No microphone or speaker
is used.

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
session does not open, is skipped with the reason. The local cascade is not a native speech model
and is listed as skipped.

```sh
cargo run -q -p realtime-voice --example compare -- --help
```

```text
Compare native speech providers on recorded utterances

Usage: compare [OPTIONS]

Options:
      --providers <PROVIDERS>
          The providers to run, in order

          Possible values:
          - openai:      The OpenAI Realtime API
          - openai-live: OpenAI GPT-Live
          - nova:        Amazon Nova Sonic on Bedrock
          
          [default: openai,openai-live,nova]

      --runs <RUNS>
          The runs of each provider
          
          [default: 3]

      --skip <PROVIDER=REASON>
          A provider not to try, and why: `nova="no model access"`

      --frame-ms <FRAME_MS>
          The length of one input frame, in milliseconds
          
          [default: 20]

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
times in ms, taken on the client at the receipt of each duplex event: median [lowest-highest] measured/runs
from the end of the question's speech; barge-in from the barge-in's first voiced frame
provider  first audio           first transcript      response end          barge-in to stop
fast      320 [300-340] 3/3     260 [250-270] 3/3     900 [900-900] 3/3     200 [200-200] 3/3
slow      640 [600-700] 3/3     520 [500-550] 3/3     1840 [1800-1900] 3/3  100 [100-100] 3/3
nova      skipped: blocked on AWS access
usage per run, as each provider reports it
fast      run 1: responses 2: in text 20 audio 40 cached 0, out text 10 audio 80; transcription in ? out ? (0 of 0 turns reported)
fast      run 2: responses 2: in text 20 audio 40 cached 0, out text 10 audio 80; transcription in ? out ? (0 of 0 turns reported)
fast      run 3: responses 2: in text 20 audio 40 cached 0, out text 10 audio 80; transcription in ? out ? (0 of 0 turns reported)
slow      run 1: responses 2: in text 20 audio ? cached 0, out text 10 audio ?; transcription in ? out ? (0 of 0 turns reported); billed 12.5 s
slow      run 2: responses 2: in text 20 audio ? cached 0, out text 10 audio ?; transcription in ? out ? (0 of 0 turns reported); billed 12.5 s
slow      run 3: responses 2: in text 20 audio ? cached 0, out text 10 audio ?; transcription in ? out ? (0 of 0 turns reported); billed 12.5 s

.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.04s
```
