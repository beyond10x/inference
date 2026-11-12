---
title: Talk to a hosted voice model
sidebar_position: 1
description: Hold a spoken conversation with OpenAI Realtime, GPT-Live or Amazon Nova Sonic through one conversation code path, with barge-in, a tool, usage and an optional cost estimate, or with a cascade of ElevenLabs speech to text, a local LLM and ElevenLabs text to speech.
lede: The provider is configuration only - the same conversation code runs OpenAI Realtime, GPT-Live and Nova Sonic.
source: examples/realtime-voice (examples/talk.rs, src/lib.rs, src/talk.rs, src/elevenlabs.rs), adapters/inference-openai, adapters/inference-aws, adapters/inference-elevenlabs, crates/inference-duplex
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
          - openai:             The OpenAI Realtime API
          - openai-live:        OpenAI GPT-Live
          - nova:               Amazon Nova Sonic on Bedrock
          - elevenlabs-cascade: ElevenLabs speech to text, the local LLM (feature local), ElevenLabs text to speech

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
| `elevenlabs-cascade` | a cascade, built with feature `local`: ElevenLabs speech to text (`scribe_v2_realtime`), the `llama-server` that `INFERENCE_LLAMA_SERVER_URL` and `INFERENCE_LLAMA_SERVER_MODEL` name, ElevenLabs text to speech in the voice `ELEVENLABS_VOICE_ID` names | `ELEVENLABS_API_KEY`, or the OS keychain, secret `elevenlabs-api-key`, when it is unset; the host from `ELEVENLABS_RESIDENCY` |

The `Conversation` code is the same for the three native models: `--provider` changes the
binding, not the application. `elevenlabs-cascade` is a cascade of three models instead; see
[Talk to the ElevenLabs cascade](#talk-to-the-elevenlabs-cascade).

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
cancels or truncates it; it bills the voice in seconds, which are printed with the totals. When you
speak over its answer, the binding reports the speech start from where your transcript falls on
the session timeline, and the answer's queued audio stops playing.

No output is shown here: every run is a live, billed session with a microphone. The conversation
logic is tested against the duplex mock and a fake audio host in
[`examples/realtime-voice/tests`](https://github.com/beyond10x/inference/tree/main/examples/realtime-voice/tests).

## Talk to the ElevenLabs cascade

`elevenlabs-cascade` hears you with ElevenLabs realtime speech to text, answers with a local
`llama-server`, and speaks with ElevenLabs text to speech. Start the server on loopback as in
[Run the local voice cascade offline](run-the-local-voice-cascade-offline.md), store the ElevenLabs key once with
`secretsctl put elevenlabs-api-key` (or set `ELEVENLABS_API_KEY`), and run:

```sh
INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:8080/v1 \
INFERENCE_LLAMA_SERVER_MODEL=qwen2.5-1.5b-instruct \
ELEVENLABS_VOICE_ID=<voice id> ELEVENLABS_RESIDENCY=eu \
    cargo run -p realtime-voice --features local --example talk -- --provider elevenlabs-cascade
```

| Variable | Effect | When it is unset, or set to a value not taken |
|---|---|---|
| `INFERENCE_LLAMA_SERVER_URL` | the base URL of the `llama-server` API, on loopback | `talk` exits with status 2, naming it; also for a host that is not loopback |
| `INFERENCE_LLAMA_SERVER_MODEL` | the model the server answers to, its `--alias` | `talk` exits with status 2, naming it; also when the adapter takes no binding of the URL and model |
| `ELEVENLABS_VOICE_ID` | the voice the answer is spoken in, `pcm_24000` | `talk` exits with status 2, naming it; also for a value that is not a voice id |
| `ELEVENLABS_API_KEY` | the ElevenLabs key, never printed | the OS keychain, secret `elevenlabs-api-key`, after a line saying `talk` waits for it; exit status 2 when the keychain does not open, gives no key, or gives no answer within 8 s, as when an unlock prompt is not answered |
| `ELEVENLABS_RESIDENCY` | the data-residency host of the key, `global`, `us`, `eu`, `in` or `sg`, for both ElevenLabs endpoints; a key of a residency stack is refused by every other host | `global`; any other value exits with status 2, naming the variable, never the value |

The variables are checked in the order of the table, before anything opens or is sent. Without feature
`local` the provider exits with status 2, naming the feature. With none of them set:

```sh
cargo run -q -p realtime-voice --features local --example talk -- --provider elevenlabs-cascade
```

```text
error: INFERENCE_LLAMA_SERVER_URL is not set
```

The microphone is sent in `pcm_16000`, and the endpoint's voice activity detection commits each
turn when you pause. Each committed turn is asked of the server on its own, without the turns
before it, and each committed sentence of the answer is sent to ElevenLabs as it arrives, one
after another. When you speak over the answer for 30 ms, or a new turn is committed while it
still plays, its playback stops and no further sentence of it is sent. ElevenLabs still generates
and bills the sentence it was speaking, whose audio is dropped as it arrives: a barge-in saves
playback and the sentences not yet sent, not that one. When the recognition fails, as when the
host refuses the key, the conversation ends and says why. The cascade has no tool and prints no
usage. Enter ends the conversation without waiting for the rest of the answer.

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

Since 0.10.0 `SessionConfig` also sends input noise reduction (`NoiseReduction`), the input
transcription's language and prompt beside its model, and a session `tool_choice`
(`ToolChoice`); each unset field is left out of `session.update`. On the open client,
`RealtimeClient::create_item` adds a user, assistant or function-output item with an optional
`event_id` and a `previous_item_id` placement, `RealtimeClient::create_response_with` creates a
response with its own instructions, tool choice, tools and input, or out of band with
conversation `none`, and `RealtimeClient::ping` and `LiveClient::ping` send a WebSocket ping
for a keepalive the caller schedules.

Since 0.11.0 `RealtimeClient::into_split` and `LiveClient::into_split` split an
open session into a sender (`RealtimeSender`, `LiveSender`), which holds every send method, and a
receiver (`RealtimeReceiver`, `LiveReceiver`), which holds `next_event`, `session` and the events
kept during setup. Two tasks own them, so events keep arriving while an audio write the peer does
not drain is pending. `RealtimeClient::reunite` and `LiveClient::reunite` put the halves back
together for `close`; halves of two sessions are refused with `ReuniteError`, which hands both
back. The read timeout holds across `next_event` calls: a session that sends no message for that
long reports `RealtimeError::Stalled`, even when the caller drops and repeats its read.
`ConversationItem::SystemText` adds a system-role item, so an out-of-band response can carry
instructions in context without adding them to the conversation.

## Next

- [Compare voice providers](compare-voice-providers.md) on latency and usage
- What each hosted adapter claims, and the Cargo features it needs: [Adapters](../reference/adapters.md)
