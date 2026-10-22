---
title: Repository podcast
sidebar_position: 3
description: A coding agent distils a repository into a brief, and two native speech models discuss it over the speaker in a terminal UI with token and cost counters.
lede: Two voice models, a host and a guest, talk about a repository until the minutes you asked for have played, with running token and cost counters.
source: examples/repo-podcast (examples/podcast.rs, src/, prices/, tests/), CHANGELOG.md 0.7.0
---

# Repository podcast

Since 0.7.0, `repo-podcast` turns a directory into a short spoken show:

1. The Claude Code CLI distils the directory into a brief (`claude -p`, read-only tools, run as an
   external process in `--dir`, which defaults to this repository). `--context-file` skips this
   step with a brief saved before; `--save-context` keeps one for the next run.
2. Two native speech models, a host and a guest, discuss it over the speaker, one session per turn,
   with the brief and the conversation so far in each session's instructions: the duplex contract
   takes no text input.
3. The host closes the show once `--minutes` of audio have played.

A ratatui terminal UI shows both hosts, a level meter on the one speaking, the transcript of the
turn playing, and running counters of tokens, turns, time and estimated cost. `--plain` prints lines
instead. Enter, q, Esc or Ctrl-C ends the show early; the usage per host and in total is printed at
the end.

:::warning[Requirements]

The Claude Code CLI (or a saved brief), an OpenAI key read as in
[Talk to a hosted voice model](../guides/talk-to-a-hosted-voice-model.md), and an output device.
Every show is billed by the voice provider.

:::

## Run it

```sh
cargo run -p repo-podcast --example podcast -- --minutes 5 --provider openai
cargo run -p repo-podcast --example podcast -- --save-context brief.md
cargo run -p repo-podcast --example podcast -- --context-file brief.md --plain --provider openai-live
```

```sh
cargo run -q -p repo-podcast --example podcast -- --help
```

```text
A podcast about a repository: Claude distils it, two voice models discuss it

Usage: podcast [OPTIONS]

Options:
      --dir <PATH>
          The directory the show is about; this repository by default

      --minutes <MINUTES>
          The minutes of audio the show runs before the host closes it
          
          [default: 5]

      --context-file <PATH>
          A brief saved before; nothing is distilled

      --save-context <PATH>
          Where to save the brief for later runs

      --provider <PROVIDER>
          The voice models

          Possible values:
          - openai:      The OpenAI Realtime API
          - openai-live: OpenAI GPT-Live
          
          [default: openai]

      --prices <FILE>
          A JSON price table; the provider's shipped table by default

      --plain
          Print lines instead of the terminal UI

      --claude <PATH>
          The Claude Code CLI to distil with
          
          [default: claude]

      --title <NAME>
          The name in the title; the repository's name by default (its `origin` remote's, or its top-level directory's)

      --speed <SPEED>
          How fast the OpenAI Realtime voices speak, as a multiple of the model's own pace, 0.25 to 1.5 (GPT-Live takes no speed)
          
          [default: 0.9]

  -h, --help
          Print help (see a summary with '-h')
```

## Cost

The cost counter uses the shipped price table of the provider's default model, from
[`examples/repo-podcast/prices/`](https://github.com/beyond10x/inference/tree/main/examples/repo-podcast/prices),
or the table `--prices` names. The shipped tables are list prices with their source and the date
they were read; see [Estimate cost from a price table](../guides/estimate-cost-from-a-price-table.md#the-tables-repo-podcast-ships).

## Tested without a provider

The show, its instructions, the brief, the price tables and the terminal UI are tested with the
duplex mock, the fake audio host and ratatui's test backend:

```sh
cargo test -q -p repo-podcast
```

The result line of each test target, 54 tests in all:

```text
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 10.02s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.03s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```
