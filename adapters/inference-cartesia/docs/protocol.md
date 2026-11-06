# Cartesia text-to-speech WebSocket, as this adapter speaks it

Fetched on 2026-11-06 with `curl` from Cartesia's public documentation, without a key or a login.
No page carries a publication or revision date of its own; the fetch date is the only date there
is.

| page | what it gave |
|---|---|
| <https://docs.cartesia.ai/api-reference/tts/websocket.md> | the AsyncAPI description of `/tts/websocket`: every message both ways, the `cartesia_version` parameter, the security schemes |
| <https://docs.cartesia.ai/use-the-api/tts-websocket/contexts.md> | contexts, `continue`, the order of outputs, cancelling |
| <https://docs.cartesia.ai/use-the-api/tts-websocket/context-flushing-and-flush-i-ds.md> | flushing, not used |
| <https://docs.cartesia.ai/use-the-api/tts-websocket/buffering.md> | `max_buffer_delay_ms`, not used |
| <https://docs.cartesia.ai/build-with-cartesia/capability-guides/tts-output-audio-format.md> | `output_format`, bit depths, telephony formats |
| <https://docs.cartesia.ai/use-the-api/api-conventions.md> | base URL, `Cartesia-Version`, error envelopes |
| <https://docs.cartesia.ai/use-the-api/api-errors.md> | the WebSocket error event |
| <https://docs.cartesia.ai/get-started/authenticate-your-client-applications.md> | `X-Api-Key` or `Authorization` on a WebSocket handshake |
| <https://docs.cartesia.ai/get-started/realtime-text-to-speech-quickstart.md> | the push, end and receive pattern |
| <https://docs.cartesia.ai/build-with-cartesia/tts-models/latest.md> | Sonic 3.6, `sonic-3.6`, its dated snapshot |
| <https://docs.cartesia.ai/changelog/2026.md> | API version `2026-08-14` |

## The connection

One WebSocket per synthesis, opened when its first segment is sent, to
`{base}/tts/websocket?cartesia_version=2026-08-14`, `{base}` being `wss://api.cartesia.ai`
unless configured. The conventions page asks for the version on every request and lets a
WebSocket carry it as the `cartesia_version` query parameter; the reference marks that parameter
required and gives `2026-08-14` as its example, the version the 2026 changelog introduces.

The API key travels in the `X-API-Key` header of the handshake (the reference's `apiKey` scheme),
marked sensitive, and nowhere else. The access-token query parameter is for browsers and is not
used.

## What the adapter sends

One generation request per segment, on a context of its own:

```json
{"model_id":"sonic-3.6","transcript":"Hello there.","voice":"<voice id>",
 "output_format":{"container":"raw","encoding":"pcm_mulaw","sample_rate":8000},
 "context_id":"<per segment>","continue":false}
```

with `language` added when it is configured. `continue` false says the transcript is the
context's last, which the reference says minimizes latency. A segment whose text is blank is not
sent: it completes with no audio.

A cancel names the context of the segment being spoken: `{"context_id":"…","cancel":true}`.

## What the adapter reads

Every server message is a JSON object with a `type` and a `context_id`:

- `chunk`: `data` is the base64 of the audio in the requested format, `done` says whether it is
  the context's final chunk, `status_code` and `step_time` are not read.
- `done`: the context's generation is complete.
- `error`: `status_code`, `title` and `message` required, `error_code` and `context_id` optional.
- `flush_done`, `timestamps`, `phoneme_timestamps`: answers to options this adapter never sets.

## Output formats

`container` is `raw` on the WebSocket. `encoding` is one of `pcm_f32le`, `pcm_s16le`,
`pcm_mulaw` and `pcm_alaw`; `sample_rate` one of 8000, 16000, 22050, 24000, 44100 and 48000. The
format guide pairs `pcm_mulaw` and `pcm_alaw` with 8000 for telephony and gives the bit depths: 8
for the two G.711 laws, 16 for `pcm_s16le`, 32 (float) for `pcm_f32le`.

## Points the documentation leaves open, and what the adapter decided

- **Channels.** No page states them. The adapter takes every format as mono.
- **Byte order of `pcm_f32le`.** Little-endian, as its name says; not stated otherwise.
- **A chunk with `done` true, then `done`.** The reference defines both a chunk's `done` and a
  `done` message, and does not say whether a `done` message follows a final chunk. The adapter
  ends the context at whichever comes first and drops anything more of that context.
- **Cancel.** The contexts guide says a cancel stops only a request that has not started
  generating; one already generating goes on sending its responses to the end. So after a cancel the server may go on sending the cancelled context's chunks and its `done`. The adapter
  sends the cancel, emits nothing more of that segment and drops every later message of its
  context, on the same connection; the next segment is sent at once on a new context.
- **An error's scope.** An `error` carries `context_id` only when one is known, and the documented
  example has `done` true. The adapter fails the synthesis on any error, except one of a context it
  has already cancelled or finished, which it drops.
- **Idle connections.** No idle timeout of the connection is documented. Contexts expire one
  second after their last audio is sent; the adapter never reuses one. A connection the server has
  closed between segments fails the next segment's write or read.
- **Several contexts at once.** The reference allows them, but says only that outputs of one
  context are in order. The adapter sends a segment once the one before it is done or cancelled,
  so the chunks of two live contexts never interleave.
- **Plaintext.** The key travels in a header, so `ws://` would send it in the clear. The adapter
  takes `ws://` only to a loopback host, as a local test server is, and `wss://` to any host.
- **Usage.** No message reports billed characters. The usage record is the contract's own count
  of the characters taken and the audio emitted.
