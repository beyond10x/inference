---
format: aep.planning-md/3
id: review-result:adversary-audio-vocabulary-pass-2
kind: review-result
status: active
title: Adversary, story:audio-vocabulary, pass 2
relations:
- reviews: story:audio-vocabulary
revision: 1
---
unit: story:audio-vocabulary, uncommitted working tree on base edf8ca7 (worktree inference-impl-audio-vocabulary)
verdict: INFEASIBLE (one red case: the code is stricter than the declared `Bytes` wire, and I found no producer that sends what it refuses)
cases: executed 19→21, red 1
origin: introduced 1 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory (the unit scratch's adversary-2/, listed in part 6)
needs-coordinator: decide whether ESS `Bytes` means canonical base64 only. If yes, re-pin my red case to assert refusal and add one line to the `ess_bytes` doc comment. If no, change the decoder (fix below).

**1. `git --no-pager diff --stat`**
```
 Cargo.lock          | 15 +++++++++++++++
 Cargo.toml          |  1 +
 ess/ess-inputs.yaml |  1 +
 ess/system.yaml     |  8 +++++---
 4 files changed, 22 insertions(+), 3 deletions(-)
```
All four lines are the implementor's. I wrote one untracked file inside the unit's untracked `crates/inference-audio/`, so the stat does not show it: `crates/inference-audio/tests/adversary2_bytes_and_narrowing.rs`. I changed no non-test path.

**2. Cases added** (`crates/inference-audio/tests/adversary2_bytes_and_narrowing.rs`)

| case | asserts | now |
|---|---|---|
| `frame_data_the_ess_bytes_pattern_admits_deserializes` (:21) | a frame whose `data` is `"AQD/fx=="`, `"AB=="` or `"AAB="` deserializes to `[01 00 ff 7f]`, `[00]` and `[00 00]`. All three match the `Bytes` pattern `ess generate --kind schema` emits for `AudioFrame.data` today; only their unused low bits are non-zero | red |
| `a_format_at_the_top_of_the_declared_u32_rate_and_u16_channel_narrowing_deserializes` (:38) | `sample_rate` u32::MAX and `channels` u16::MAX deserialize and round-trip. This guards the re-pinned pass-1 case, whose `is_err()` alone would still pass if the format refused everything | green (guard) |

Red output of that file run alone (`cargo test -p inference-audio --all-features --test adversary2_bytes_and_narrowing`, exit 101):
```
test a_format_at_the_top_of_the_declared_u32_rate_and_u16_channel_narrowing_deserializes ... ok
test frame_data_the_ess_bytes_pattern_admits_deserializes ... FAILED
thread 'frame_data_the_ess_bytes_pattern_admits_deserializes' (732324) panicked at crates/inference-audio/tests/adversary2_bytes_and_narrowing.rs:29:13:
{"data":"AQD/fx==","format":{"channels":1,"encoding":"pcm_s16le","sample_rate":16000},"position":0,"sample_count":2} was refused (Invalid last symbol 0x78 ('x') at offset 5, decoded as 0b00110001.): "AQD/fx==" matches the pattern ESS emits for Bytes, padded standard base64, and only its unused low bits are not zero
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

**3. Gate, run after the cases existed**

| command | exit | summary |
|---|---|---|
| `cargo test -p inference-audio --all-features --no-fail-fast` | 101 | adversary1 `ok. 6 passed`; adversary2 `FAILED. 1 passed; 1 failed`; audio_vocabulary `ok. 13 passed` |
| `cargo clippy -p inference-audio --all-targets --all-features -- -D warnings` | 0 | `Finished` |
| `cargo fmt -p inference-audio --check` | 0 | no output |
| `ess specify validate --path ess` | 0 | `inference v1 — 8 file(s), 4 scenario(s), valid` |
| `ess verify conform synthesize` (tree, and base from `git archive edf8ca7 ess`) | 0 / 0 | `229 scenario(s) (4 authored), 3 refusal(s)` both. Appeared [], vanished [], scenario bodies byte-identical (`cmp` exit 0) |

The `before` count of 19 comes from the implementor's `test-green-r1.log` (6 + 13).

**4. Findings**
- **F-A, `crates/inference-audio/src/lib.rs:134`**: `STANDARD.decode` keeps base64 0.23.1's defaults (`decode_allow_trailing_bits: false`, `RequireCanonical`; registry source, `general_purpose/mod.rs:341-342`). So it refuses strings the declared `Bytes` schema pattern admits.
  - The doc comment at :125-126 lists the refusals and leaves this one out.
  - What was measured: the red case above.
  - What reaches it: nothing I found. Standard encoders zero the pad bits, RFC 4648 §3.5 lets decoders reject non-zero ones, and ESS's own `retained.rs:21-24` also requires canonical input. No synthesized scenario carries an audio type. Verdict INFEASIBLE, severity note.
  - Fix, either one:
    - Build the engine with `GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true)`.
    - Keep the strict decoder, re-pin the case to assert refusal, and add "non-zero trailing bits" to the doc.
- **J-1, judgement, no case: `crates/inference-audio/src/lib.rs:105-106`, `ess/domains/audio.yaml` (AudioFrame)**: nothing ties `data.len()` to `sample_count × channels × sample width`. `duration()` reads only `sample_count`. The unit's own tests build frames like that (`audio_vocabulary.rs:228`: `sample_count` u32::MAX with `data: ""`).
  - ESS `types:` cannot carry an invariant, and I don't know if pass 1 raised this (F5 is not described in scratch). Origin introduced, severity note, verdict INFEASIBLE.

**5. Attacked and not broken**
- F1: encode is padded standard. Missing pad, surplus pad, URL-safe alphabet and garbage are all refused. Mutants using NO_PAD, URL_SAFE or Indifferent padding are caught by the existing cases.
- F2: `SampleClockPosition` can only be built through `new` or deserialize. The `<=`→`<` mutant is caught. -1, i64::MAX+1 and u64::MAX are refused on the wire.
- F3/YAML: ESS 0.60 has no integer bounds (`Primitive` in ess-domain `types.rs:45`), so the narrowing can only be written as YAML prose. The prose for rate, channels, sample_count and position matches the Rust types. Field names, field order and enum members match the generated schema.
- F4: `additionalProperties: false` in the schema matches `deny_unknown_fields`. A field added inside the nested format is refused.
- F6: the UNMAPPED marker for frame timestamps is present, and so is the one for compressed frames.
- Re-pinned adversary1 file: compared against `scratch/adversary1_wire_and_clock.rs.orig`. The position case gained assertions. The format case flipped from accept to refuse as decided. It never asserted that the upper bounds are accepted, so my guard case now covers that.
- Suite: 229 scenarios, 3 refusals, ids identical to base. Pass 1's `ids-*.txt` files are empty (0 bytes), so I checked against my own base re-synthesis.
- Hygiene: the unit's files contain no host path, session id or private name. Every design citation lands on the line named. base64 MSRV is 1.71 and the workspace's is 1.88.

**6. Paths written outside the worktree**
- The unit scratch's `adversary-2/` holds `validate.log`, `syn-now.log`, `syn-base.log`, `suite-now.json`, `suite-base.json`, `base/ess/` (an archive of the base), `schema/` and `schema.log`, plus the logs `red.log`, `suite.log`, `clippy.log` and `fmt.log`.
- Inside the worktree, cargo built into its own `target/`.

```findings
- file: crates/inference-audio/src/lib.rs
  line: 134
  category: contract-drift
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: AudioFrame.data decoding refuses base64 with non-zero trailing bits that the ESS Bytes schema pattern admits, and the ess_bytes doc comment does not list that refusal.
- file: crates/inference-audio/src/lib.rs
  line: 105
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: nothing relates data length to sample_count, channels and encoding width, so duration() can disagree with the buffer it describes, and the unit's own cases build such frames.
```
