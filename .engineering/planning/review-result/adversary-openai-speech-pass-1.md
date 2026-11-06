---
format: aep.planning-md/3
id: review-result:adversary-openai-speech-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the OpenAI speech TTS binding
relations:
- reviews: story:openai-speech-tts-adapter
revision: 1
---
unit: ADP-openai-speech, story:openai-speech-tts-adapter, unit commit 4de6e53; my tests are commit 0195f03 on impl/openai-speech-tts-adapter
verdict: nothing found (no red case; one pre-existing clippy failure confirmed, CONFIRMED / pre-existing)
cases: executed 23→34, red 0
origin: introduced 0 / pre-existing 1 / undecided 0
wrote-outside-worktree: 10 files under the wave scratch dir `adv1-openai-speech/` (listed in part 6)
needs-coordinator: whether to add the one-line `required-features = ["duplex"]` fix for `adversary1_usage_openai` (a manifest fix, not mine to apply)

1. `git --no-pager diff --stat 4de6e53 HEAD`: `adapters/inference-openai/Cargo.toml | 8 +` (two `[[test]]` entries, `required-features = ["speech"]`), `tests/adversary_rf1_speech_audio.rs | 224 +`, `tests/adversary_rf1_speech_transport.rs | 323 +`. All three are test paths or test entries. No implementation file was touched.

2. Cases added. Each runs inside `within_deadline` (25 s). All were green when first run, so there is no red output:
- `adversary_rf1_speech_transport` (7 passed):
  - `https://` sends a TLS handshake record first (0x16) and fails cleanly as Connect or Stalled.
  - A chunked body cut before its last chunk fails as `Transport`, and so does a content-length body cut short. Neither segment ends Synthesized.
  - A server that never answers fails as `Stalled` within 4 s at a 1 s read timeout, and usage still comes last.
  - A 307 gives `Status(307)` and the redirect target sees nothing.
  - The key and the word "Bearer" appear in no Debug or Display text: not the synthesiser's, not the handle's, not the failure's.
  - With `HTTP(S)_PROXY` and `ALL_PROXY` set in a child run of the test binary, the proxy gets no connection.
- `adversary_rf1_speech_audio` (4 passed):
  - Bodies cut into ragged 1–9-byte chunks, so sample bytes are split, give byte-identical frames to a body sent as one chunk. Checked for μ-law and A-law at 8 kHz and PCM at 16 and 48 kHz, with the exact sizes 1600 / 6400 / 19200 bytes.
  - Three segments each equal one segment spoken alone, on a contiguous clock: no sample lost or repeated at a segment boundary.
  - Exactly 4096 ASCII characters and 4096 astral code points (8192 UTF-16 units) are both sent.
  - On a current-thread runtime's own thread, iterating, a mid-segment discard and dropping the handle work: no panic, and the connection closes.
- Commands: `cargo-run.sh test -p inference-openai --features speech --test <each>`, then the proxy case alone.

3. Suite, run after the cases existed: `cargo-run.sh test -p inference-openai --features speech` with targets speech_conformance, formats, cancel, request, claims_record and both adversary targets. Result: 6+5+3+8+1+7+4 = 34 passed, 0 failed, `rc=0 secs=4`, EXIT=0.
   Clippy: `cargo-run.sh clippy -p inference-openai --features speech --all-targets --keep-going -- -D warnings` gives rc=101. The only target that fails is `test "adversary1_usage_openai"` (E0432: `inference_openai::duplex`, `inference_duplex`). My targets are clippy-clean. `cargo fmt --check` is clean.

4. Judgement findings:
- `adapters/inference-openai/tests/adversary1_usage_openai.rs:12`, CONFIRMED, pre-existing. The file is present at c76ee3f and has no `[[test]]` entry there, so `--all-targets` without `duplex` fails with or without this unit. The `--keep-going` run shows it is the only failing target. Adding `[[test]] name = "adversary1_usage_openai"` with `required-features = ["duplex"]` would fix it; the file imports only duplex items.

5. Attacked and could not break:
- **Request body vs the docs:** checked against the reference and guide re-fetched today. Model, input, voice name or `{id}`, `pcm`, `stream_format` `audio`, the instructions and speed bounds all match.
- **Character limit:** the reference's `maxLength: 4096` counts code points, which `chars()` also counts. Exactly 4096 is accepted, and the existing test covers 4097 being refused.
- **Resampler edges:** dropping the delay and the zero-padded flush give ceil(N·L/M) samples per segment at every rate. No click and no duplicate across segments, because each segment gets a fresh filter.
- **Half-sample carry:** byte-identical output under ragged chunks.
- **Cancel:** the existing tests cover no frame after a cancel, usage counting only the emitted audio, and a dropped handle.
- **Read deadline:** the response head and each chunk are bounded. A server trickling bytes faster than the read timeout is never cut off, but that is how the deadline is documented.
- **Plaintext rule:** the loopback check is shared with the realtime client's (`client::is_loopback`).
- **Redirects and proxies:** both off.
- **Key exposure:** the key is in no Debug or error text.
- **Note only:** the fetched reference says `gpt-4o-mini-tts`, the default model, shuts down on 2027-01-06. `protocol.rs` already records this.

6. Paths written outside the worktree, all in the wave scratch dir: `adv1-openai-speech/docs/create.md`, `adv1-openai-speech/docs/guide.md`, `adv1-openai-speech/{transport,audio,proxy,suite,clippy,clippy2,fmt}.log` and `adv1-openai-speech/msg.2cXRHr`. `worktree discard-cache` has run on the tree (655 MiB freed).

Cargo time was about 0.4 min and wall time about 35 min. I briefly started an until-loop to wait on a log, which the invariants forbid, and killed it by PID within a minute.

```findings
[{"file":"adapters/inference-openai/tests/adversary1_usage_openai.rs","line":12,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"No red test: `clippy -p inference-openai --features speech --all-targets` fails only on this target, which imports duplex items and has no [[test]] entry; adding required-features = [\"duplex\"] fixes it."}]
```
