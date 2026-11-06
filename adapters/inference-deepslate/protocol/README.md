# The Opal protocol, as this binding reads it

Our summary of Deepslate's public documentation, fetched on 2026-11-06 without a key or a login.
No page of that documentation is copied here; `src/protocol.rs` holds the field numbers the
binding uses.

## Sources

| fetched | URL | what it says, in our words |
|---|---|---|
| 2026-11-06 | <https://docs.deepslate.eu/websocket.md> | The server-side WebSocket guide: the endpoint path with vendor and organization ids, an optional model id appended to pin a model, a bearer key in the handshake, `InitializeSessionRequest` first, audio as `UserInput` with a trigger mode, the server's messages and what each means, barge-in through `PlaybackClearBuffer`, tool calls that must each be answered. |
| 2026-11-06 | <https://docs.deepslate.eu/api-reference/realtime.md> | The message reference: every client and server message, its fields and their meaning; the three stages of a turn's end (`InferenceComplete`, `ResponseEnd`, a final `TurnSnapshot`); audio output only when a speech provider is configured. |
| 2026-11-06 | <https://raw.githubusercontent.com/deepslate-labs/deepslate-docs/refs/heads/main/api-reference/realtime.proto> | The protocol buffers definition the reference links, package `eu.deepslate.realtime.speeq`: the field numbers, that the service assumes 16-bit PCM, that a client chooses packet ids freely, and that a client should ping every 10 seconds since a silent session times out after about 30. Fetched at docs repository commit `421718c18d5f`, sha256 `1afc5fc57235365f6de4db9d12cf8a3e4e099b60636c1a216fdb4b7e8e5d223c`. |
| 2026-11-06 | <https://docs.deepslate.eu/opal.md> | The model page: Opal is speech to speech end to end, `opal-v2.1` is the platform default and `opal-v3.0-preview` needs access. |

## Open questions

The documentation does not say these, so the binding does not rely on them:

- Which sample rates the audio lines take. The examples use 16 kHz and 48 kHz and an error example
  names 8 to 48 kHz; the binding offers 8, 16 and 48 kHz mono only.
- Whether `VadConfiguration` may be left out, and its defaults. The binding always sends the values
  every example sends, with the session's silence and prefix padding in their place.
- How a turn sensitivity maps onto `confidence_threshold` and `min_volume`. The binding refuses a
  sensitivity.
- Whether `ModelTextFragment` of a turn can arrive before its `ResponseBegin`. The binding holds
  such text for the turn.
- Whether a turn's audio can arrive after its `ResponseEnd` or after `PlaybackClearBuffer`. The
  binding drops and counts it.
- Whether a tool call's answer continues the same `turn_id` or opens a new one. The binding begins
  a new response when the call's turn has ended.
- Whether a `PlaybackPositionReport` sent after the server's own interruption still truncates the
  context. The binding sends none and claims no truncation.
- Whether `ResponseBegin` of a new turn can come while an earlier turn's response has had no
  `ResponseEnd`. The binding completes the earlier one first.
- Which usage, if any, a session reports. No message carries any; usage is recorded unknown.
