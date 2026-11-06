# Phonic speech-to-speech WebSocket: the documentation the binding was taken from

Fetched on 2026-11-06 with `curl`, no key and no login, from Phonic's public documentation (the
`.md` form the site offers for every page). The pages are Phonic's text and are not copied here;
this file is a summary in our words.

| page | URL |
|---|---|
| Conversations | <https://docs.phonic.co/api-reference/conversations/conversations.md>, the AsyncAPI 2.6.0 reference of `GET /v1/sts/ws` |
| Via WebSockets | <https://docs.phonic.co/docs/build/with-web-sockets/via-web-sockets.md> |
| WebSocket tools | <https://docs.phonic.co/docs/build/tools/websocket_tools.md> |
| Authentication | <https://docs.phonic.co/docs/platform/authentication/overview.md> |

`src/protocol.rs` says which of it the binding speaks, and `src/duplex.rs` how a conversation
maps onto the `inference-duplex` contract. What the documentation does not say is listed below
and left out of the code, or decided by the binding where a session cannot go on without a
decision; each such decision is named in `src/protocol.rs` as inferred.

## What the documentation states

- **Endpoint.** `wss://api.phonic.ai/v1/sts/ws`. A server-side client sends
  `Authorization: Bearer <API key>` in the handshake; a session token or JWT goes in a query
  parameter instead, for browsers.
- **Client messages**, each a JSON text message with a `type`: `config` (the first),
  `audio_chunk`, `tool_call_output`, `update_system_prompt` (deprecated), `add_system_message`,
  `update_tools_subset`, `set_external_id`, `mute` and `unmute` (push to talk only),
  `generate_reply`, `say`, `reset`.
- **`config`**: all fields but `type` optional. Among them `agent`, `project`, `model`
  (`phonic-0.5`, `phonic-1` or another string; omitted, the current default), `system_prompt`,
  `voice_id`, `input_format` and `output_format` (each one of `pcm_44100`, `pcm_24000`,
  `pcm_16000`, `pcm_8000`, `mulaw_8000`; default `pcm_44100`), `tools`, `websocket_timeout_sec`
  (1 to 300, default 60), VAD fields, `turn_taking`, `push_to_talk`.
- **Inline tools**: `{"type": "custom_websocket", "tool_schema": {"type": "function",
  "function": {"name", "description", "parameters", "strict": true}}, "execution_mode": "sync"}`.
  A `tool_call` is answered with `tool_call_output` carrying its `tool_call_id` and an `output`
  of any JSON value, within `tool_call_output_timeout_ms` (default 5000). An output whose JSON
  text exceeds 16,000 characters is truncated.
- **`audio_chunk`** (client): `audio`, base64 of Int16 PCM or 8-bit μ-law. Each chunk holds at
  most 40 ms of audio, longer chunks are rejected with an error; about 20 ms is recommended.
- **Server messages**: `ready_to_start_conversation`, `conversation_created`
  (`conversation_id`), `input_text` (`language`, `text`), `input_cancelled`, `audio_chunk`
  (`audio` base64, `text`, optional `timings`), `user_started_speaking`,
  `user_finished_speaking`, `assistant_started_speaking`, `assistant_finished_speaking` (not
  sent when the user's speech interrupted the assistant), `interrupted_response` (`text`, what
  was spoken before the interruption), `dtmf`, `tool_call` (`tool_call_id`, `tool_name`,
  `parameters`), `tool_call_output_processed`, `tool_call_interrupted`,
  `assistant_chose_not_to_respond`, `assistant_ended_conversation`, `error` (`error.message`,
  optional `error.code`, `param_errors`), `warning` (non-fatal; `warning.message`, optional
  `warning.code`).
- **Close codes**: 4505 when `max_conversation_duration_sec` is reached; 4800 when a reconnect
  finds the conversation expired; 1006, 1012 and 1001 with reason `restarting` are the
  disconnects the vendor SDKs reconnect after.
- **Usage**: no server message carries usage or billing.

## What it leaves open

1. Whether `ready_to_start_conversation` comes before or after `config`. The binding sends
   `config` at once and sends no audio until it has read `ready_to_start_conversation`.
2. Whether `conversation_created` precedes `ready_to_start_conversation`. Either order is read.
3. The byte order of PCM (`Int16Array` is platform order, little-endian everywhere the SDKs
   run), and the encoding of the server's `audio`: taken to be the `output_format` as the
   client's is the `input_format`, 16-bit little-endian mono for `pcm_*`.
4. Whether `error` ends the conversation. Only `warning` is called non-fatal; the binding fails
   the session on `error`.
5. Whether `user_started_speaking` itself cuts the assistant, or only an interruption that
   `interrupted_response` then reports. The binding cuts the response at
   `interrupted_response`, and drops audio after it until the next `assistant_started_speaking`.
6. Whether a response without `assistant_started_speaking` (a welcome message) exists. Audio
   while no response is in progress opens one.
7. What follows `assistant_ended_conversation`: the binding completes the response in progress
   and expects the server to close.
8. Any server-side keepalive. None is documented; the binding sends a WebSocket ping after a
   quiet interval and takes a connection that answers nothing as stalled.
9. Message size bounds. The binding reads messages of at most 1 MiB.
10. Whether `strict: true` requires a schema in OpenAI's strict form. The tool's schema is sent
    as it was declared.
11. Whether `reconnect_conv_id` is a documented query parameter outside the SDKs. The binding
    does not reconnect.
