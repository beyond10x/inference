---
format: aep.planning-md/3
id: review-result:adversary-elevenlabs-tts-pass-1
kind: review-result
status: active
title: 'Adversary pass on the ElevenLabs streaming TTS adapter: a refused remedy, a stalled error, cleartext to a remote host'
summary: Three introduced red cases, fixed by the one correction
relations:
- reviews: story:elevenlabs-streaming-tts-adapter
revision: 1
---
## Pass

This is the adversary pass on the ElevenLabs streaming TTS adapter at `46a5289`. Its cases are
in `741806c`, in one test file, `adapters/inference-elevenlabs/tests/adversary1_attack.rs`.

verdict: NEEDS-CHANGE
cases: executed 39→52, red 3
origin: introduced 3 / pre-existing 0 / undecided 0

## Cases

- **Red: the runtime guard refused its own remedy.**
  - On a `spawn_blocking` thread a runtime is current, so the `InsideRuntime` guard refused the
    synthesis.
  - The error's own text names that thread as the remedy.
  - An async caller had no route that worked.
- **Red: an error with null audio stalled.**
  - `{"audio": null, "error": ...}` was read as an empty audio message.
  - The synthesis stalled until the read timeout, and the server's reason was lost.
- **Red: `ws://` to a remote host was accepted.**
  - The `xi-api-key` header would cross the network unencrypted.
- **Green:**
  - PCM samples split across one-byte, odd-length, empty and null chunks.
  - A-law counts one sample per byte, and positions continue across segments.
  - Cancel is bounded against a server that never reads the close, and no frame of the cancelled
    segment follows. Cancel after `isFinal` and a second cancel are refused.
  - A server that only pings stalls at the read timeout. A 20 MiB message ends as a transport
    error.
  - The key appears in no rendered error, `Debug` output, path, query or message. A key with CR or
    LF in it is refused as an invalid credential.
  - The concurrency limit holds. A handle dropped mid-segment gives back its slot.
  - An error message is cut at the 256-character cap on a character boundary.
- **Suite:** the adapter's tests with all features gave 49 passed and 3 failed. Clippy and fmt
  are clean.

## Routing

The unit's one correction, `3e86e7a`, makes all three green without an edit to the adversary's
file:

- **The runtime guard.** On a thread where a runtime is current, the handle's own runtime now
  runs on a scoped thread while the caller waits, so no thread is refused. The crate doc says that
  an async worker is blocked meanwhile and advises `spawn_blocking`. The `InsideRuntime` variant
  is gone. The unit's own case now asserts that a synthesis inside a runtime runs. On `46a5289`
  that case would fail with `InsideRuntime`.
- **Null audio.** A message is audio only when `audio` is a non-empty string. Otherwise an
  `error`, `message` or `detail` field makes it the server's error.
- **`ws://`.** It is accepted only for loopback hosts. A `user@host` form is refused.

After the correction, the adapter's suite with all features passes on three runs.

```findings
[{"file":"adapters/inference-elevenlabs/src/synthesiser.rs","line":363,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the InsideRuntime guard refuses spawn_blocking threads, the remedy its own error names, so an async caller cannot run a synthesis on that route"},{"file":"adapters/inference-elevenlabs/src/protocol.rs","line":185,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"an error object with audio null is read as nothing, so the synthesis stalls instead of failing with the server's error"},{"file":"adapters/inference-elevenlabs/src/protocol.rs","line":100,"category":"judgement","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"a ws:// base URL to a non-loopback host is accepted, so the xi-api-key header would cross the network unencrypted"}]
```
