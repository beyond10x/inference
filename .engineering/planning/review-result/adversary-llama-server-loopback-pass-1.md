---
format: aep.planning-md/3
id: review-result:adversary-llama-server-loopback-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the llama-server loopback and local cascade fixes
relations:
- reviews: story:llama-server-loopback-only
revision: 1
---
unit: 0.19.0 RF-local story:llama-server-loopback-only, unit head 36f4ab3; the adversary's cases are commit b6826ac (two test files)
verdict: CONFIRMED (one blocker, introduced)
cases: executed 19→22, red 1
origin: introduced 2 / pre-existing 0 / undecided 0

**Cases**:
- red: `examples/local-voice-cascade/tests/adversary_rf1_first_audio.rs`. Two turns, the first answer superseded before any of its audio plays; the second turn's measure is `None` where its own answer's first frame was expected. `first_audio` pairs turns with per-segment first frames by position.
- green: `adapters/inference-llama-server/tests/adversary_rf1_loopback.rs`, 14 URLs the WHATWG parser reads as non-loopback, all refused; userinfo cut by `?` or `#` withheld from the refusal. It kills the mutant that drops `?` and `#` from `AUTHORITY_END`, which no unit test covered.

**Suite**: llama-server loopback 5, adversary_rf1_loopback 2, offline 9; local-voice-cascade lib 3, barge_in 1, gates 1, adversary_rf1_first_audio 0/1; fmt and clippy clean.

**Attacked, not broken**: IPv4 shorthand, `localhost.`, `*.localhost`, percent-encoding and zone ids are refused; `llm`'s client refuses control bytes, uses no proxy and follows no redirect; the API change is additive.

Cargo 0.8 min, wall about 30 min, peak `target/` 890M.

```findings
[{"file":"examples/local-voice-cascade/src/lib.rs","line":742,"category":"acceptance","severity":"blocker","verdict":"CONFIRMED","origin":"introduced","message":"first_audio pairs turns with per-segment first frames by position, so a turn superseded before any audio takes the next turn's first frame and the next turn's measure becomes None (adversary_rf1_first_audio red); pair through segment_id to generation turn_id instead."},
{"file":"adapters/inference-llama-server/src/lib.rs","line":235,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"no unit test drives a URL with '?' or '#' before '@', so dropping them from AUTHORITY_END leaves the unit's suite green while loopback() accepts http://example.com?@127.0.0.1; adversary_rf1_loopback now covers it."}]
```
