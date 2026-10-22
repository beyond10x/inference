---
title: Talk to a hosted voice model
sidebar_position: 1
description: Hold a spoken conversation with OpenAI Realtime, GPT-Live or Amazon Nova Sonic through one conversation code path, with barge-in, a tool, usage and an optional cost estimate.
lede: The provider is configuration only - the same conversation code runs OpenAI Realtime, GPT-Live and Nova Sonic.
source: examples/realtime-voice (examples/talk.rs, src/lib.rs), adapters/inference-openai, adapters/inference-aws, crates/inference-duplex
---

# Talk to a hosted voice model

The `realtime-voice` example wires the default microphone, a hosted native speech model and the
default output into one conversation. When you speak over the model, the speaker stops at once and
the model's answer is truncated at the played position. One demo tool tells the local time.

:::warning[Requirements]

A key or credentials for the provider, a microphone and an output device. Use a headset: a laptop's
own speaker feeds back into its microphone, and the model interrupts itself. Audio leaves your
machine, and each session is billed by the provider.

:::

## Choose the provider

```sh
cargo run -q -p realtime-voice --example talk -- --help
```

```text
Talk to a realtime speech model through the default microphone and output

Usage: talk [OPTIONS] --provider <PROVIDER>

Options:
      --provider <PROVIDER>
          The provider to talk to

          Possible values:
          - openai:      The OpenAI Realtime API
          - openai-live: OpenAI GPT-Live
          - nova:        Amazon Nova Sonic on Bedrock

      --prices <FILE>
          A JSON price table, per million tokens, to estimate the cost of each response with

  -h, --help
          Print help (see a summary with '-h')
```

| `--provider` | Model | Credentials |
|---|---|---|
| `openai` | OpenAI Realtime, `gpt-realtime-2.1` unless `INFERENCE_OPENAI_REALTIME_MODEL` names another | the OS keychain, secret `openai-api-key`; or `OPENAI_API_KEY` with `INFERENCE_OPENAI_KEY_SOURCE=env` |
| `openai-live` | OpenAI GPT-Live, `gpt-live-1` | as `openai` |
| `nova` | Amazon Nova Sonic on Bedrock, `amazon.nova-2-sonic-v1:0` unless `INFERENCE_NOVA_SONIC_MODEL` names another | the AWS default credential chain; the region from `AWS_REGION` |

The `Conversation` code is the same for all three: `--provider` changes the binding, not the
application.

## Store the OpenAI key once

The keychain secret is written with `secretsctl` from [Secrets](https://beyond10x.github.io/secrets/),
which reads the value from a hidden prompt, never from the command line:

```sh
secretsctl put openai-api-key
```

To read the key from the environment instead, set `INFERENCE_OPENAI_KEY_SOURCE=env` and
`OPENAI_API_KEY`. The key is never printed.

## Talk

```sh
cargo run -p realtime-voice --example talk -- --provider openai
cargo run -p realtime-voice --example talk -- --provider openai-live
AWS_REGION=us-east-1 cargo run -p realtime-voice --example talk -- --provider nova
```

Speak, interrupt the model mid-answer, ask for the time. Both transcripts are printed. Each
response's usage is printed when it ends, with `?` for a count the provider did not report. Enter
ends the conversation and prints the session's usage totals and its duration; Ctrl-C ends it
without them.

GPT-Live decides itself when to speak and when it is interrupted, so the conversation never
cancels or truncates it; it bills the voice in seconds, which are printed with the totals.

No output is shown here: every run is a live, billed session with a microphone. The conversation
logic is tested against the duplex mock and a fake audio host in
[`examples/realtime-voice/tests`](https://github.com/beyond10x/inference/tree/main/examples/realtime-voice/tests).

## Add a cost estimate

```sh
cargo run -p realtime-voice --example talk -- --provider openai --prices prices.json
```

The price table is yours; its format is in
[Estimate cost from a price table](estimate-cost-from-a-price-table.md).

## Realtime session settings

Since 0.7.0 the OpenAI Realtime output speed can be set, as a multiple of the model's own pace
from 0.25 to 1.5 (the server's default is 1.0): `inference_openai::SessionConfig::speed` on the
client, and `OpenAiRealtime::with_output_speed` on the native speech binding. The binding clamps a
value outside that range to its nearer end, 0.25 or 1.5, and treats a value that is not a number
as unset, so the server's default applies. The client sends `SessionConfig::speed` as given and
leaves the range to the server.

## Next

- [Compare voice providers](compare-voice-providers.md) on latency and usage
- What each hosted adapter claims, and the Cargo features it needs: [Adapters](../reference/adapters.md)
