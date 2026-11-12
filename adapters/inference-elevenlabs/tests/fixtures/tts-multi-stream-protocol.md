# ElevenLabs multi-context text-to-speech: the protocol the fixtures follow

Read on 2026-11-12, without a key, from
<https://elevenlabs.io/docs/api-reference/text-to-speech/v-1-text-to-speech-voice-id-multi-stream-input>
(the AsyncAPI 2.6.0 description, fetched as Markdown) and
<https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/multi-context-web-socket> (the
multi-context guide). Neither page has a publication or revision date of its own, so the date
they were read is the only date. This is a summary of those pages; the binding's reading of them,
and what it decided where they are silent, are in `src/protocol.rs`.

## Connection

- `GET /v1/text-to-speech/{voice_id}/multi-stream-input` upgraded to a WebSocket. Servers:
  `wss://api.elevenlabs.io/` (default), `wss://api.us.elevenlabs.io/`,
  `wss://api.eu.residency.elevenlabs.io/`, `wss://api.in.residency.elevenlabs.io/`,
  `wss://api.sg.residency.elevenlabs.io/`.
- Authentication: header `xi-api-key`, or `authorization` or `single_use_token` in the query, or
  `xi_api_key` or `authorization` in a context's first message.
- Query: `model_id` (default `eleven_multilingual_v2`), `language_code`, `enable_logging`
  (default true), `output_format` (described only as "Any type"), `inactivity_timeout` (default
  20, at most 180), `sync_alignment` (default false), `auto_mode` (default false),
  `apply_text_normalization`, `seed`, `enable_ssml_parsing` (default false).
- At most 5 concurrent contexts per connection (the guide).

## Client to server

- Initialize connection: `text` a single space (`enum: [' ']`) "to initiate the context", with
  `voice_settings`, `generation_config`, `pronunciation_dictionary_locators`, `context_id`
  ("If not provided, a default context will be used").
- Initialise context: `text` that "should end with a single space", the same settings,
  `context_id`; creates the context or re-initializes an existing one.
- Send text: `text`, `context_id`, `flush` (default false): "If true, flushes the audio buffer
  for the specified context. If false, the text will be appended to the buffer to be generated."
- Flush context: `context_id`, `flush`, optional `text`.
- Close context: `context_id`, `close_context` true: "If set to true, the context will close. If
  it has already been set to flush it will continue flushing. The same context id can be used
  again but will not be linked to the previous context with the same name."
- Close socket: `close_socket` true: "closes all contexts and closes the entire WebSocket
  connection. Any context that was previously set to flush will wait to flush before closing."
- Keep context alive: `text` an empty string and `context_id`; resets the context's inactivity
  timeout.

## Server to client

- Audio: `audio` (base64, required), `contextId`, `alignment` and `normalizedAlignment` (each an
  object or null).
- Final: `isFinal` true (required), `contextId`: "Indicates this is the final message for the
  context." The guide's Python and JavaScript examples read it as `is_final`.

## Interruptions (the guide)

"Seamlessly handle user barge-ins by closing an existing speech context and initiating a new
one." When a user interrupts, "close the current context and create a new one": its example sends
`close_context` for the interrupted context, then opens the next context on the same connection.
"Close unused contexts promptly." Its worked conversation streams text into a context, flushes it
at the end of a turn, and closes a context once its turn is over.

## Not stated

Whether a flush alone, without a close, brings the context's final message; whether a close of a
context that was set to flush stops the generation of what the flush released ("will continue
flushing" says it does not, while the guide closes the interrupted context to handle a barge-in);
how a sixth concurrent context is answered; any error message, close code, or usage or billing
message on the WebSocket; bit depth, byte order and channels of `pcm_*`.

The first two were measured in a billed probe (`tests/tts_cancel_live.rs`); the crate
documentation in `src/lib.rs` gives the result, and the fixture server of `tests/support/`
follows it.
