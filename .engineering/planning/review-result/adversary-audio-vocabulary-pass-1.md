---
format: aep.planning-md/3
id: review-result:adversary-audio-vocabulary-pass-1
kind: review-result
status: active
title: Adversary, story:audio-vocabulary, pass 1
relations:
- reviews: story:audio-vocabulary
revision: 1
---
unit: story:audio-vocabulary, worktree inference-impl-audio-vocabulary on base edf8ca7 plus the uncommitted change and my one test file
verdict: NEEDS-CHANGE
cases: executed 9→15, red 5
origin: introduced 6 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory tree (part 6)
needs-coordinator: one decision for F1. The Rust side can encode `data` as base64, which adds a dependency or a hand-written codec. Or `ess/` can declare a different type for it.

**1. `git --no-pager diff --stat`** (tracked changes are the implementor's; untracked files listed below it)
```
 Cargo.lock          | 8 ++++++++
 ess/ess-inputs.yaml | 1 +
 ess/system.yaml     | 8 +++++---
```
The only untracked file I added is `crates/inference-audio/tests/adversary1_wire_and_clock.rs`, a test file. I edited no implementation file. To show a mutant survives, I mutated a copy in scratch, not the tree.

**2. Cases added** (`crates/inference-audio/tests/adversary1_wire_and_clock.rs`). Each was run alone before the suite:

| case | asserts | now |
|---|---|---|
| `a_frame_whose_data_is_ess_bytes_deserializes` | a frame with `"data":"AQD/fw=="` deserializes | red: `invalid type: string "AQD/fw==", expected a sequence` (:29) |
| `a_frame_serializes_its_data_as_ess_bytes` | `data` serializes as `"AQD/fw=="` | red: `left: Array [Number(1), Number(0), Number(255), Number(127)] right: String("AQD/fw==")` (:43) |
| `a_position_is_audio_frame_time_whatever_the_channel_count` | position 48000 at 48 kHz is 1 s with 2 and 6 channels | green; it catches mutant M1 |
| `a_position_serializes_within_the_ess_integer_range` | a position serializes as an i64 | red: `18446744073709551615 is beyond the ESS Integer range` (:71) |
| `a_format_the_ess_admits_is_not_refused` | sample_rate 2^32 and channels 65536 deserialize | red: `{"channels":1,"encoding":"pcm_s16le","sample_rate":4294967296} was refused` (:83) |
| `a_format_with_a_field_the_ess_does_not_declare_is_refused` | an extra field `bit_depth` is refused | red: `{"bit_depth":16,...} was accepted` (:98) |

Mutant M1 changes `src/lib.rs:49` to `samples_at(self.0 / u64::from(format.channels.get()), ...)`. Against it, the unit's own file still passes (`test result: ok. 9 passed`). My guard fails: `assertion failed: 2 channels left: 500ms right: 1s`.

**3. Suite**, run after the cases existed:
- `cargo test -p inference-audio --all-features --no-fail-fast`: exit 101.
  - my file: `test result: FAILED. 1 passed; 5 failed`
  - `audio_vocabulary.rs`: `test result: ok. 9 passed; 0 failed`
- `cargo clippy -p inference-audio --all-targets --all-features -- -D warnings`: exit 0.
- `cargo fmt -p inference-audio --check`: exit 1 on my file's first draft. I ran rustfmt on my file only; the rerun exits 0.
- `ess specify validate --path ess`: exit 0, `8 file(s), 4 scenario(s), valid`.

**4. Findings** (all against the worktree above; the crate does not exist at edf8ca7, which is why every origin is `introduced`)

The reference for F1 to F4 is the schema ess 0.60.0 generates from `ess/domains/audio.yaml` (`ess generate --kind schema`). It declares:
- `data`: `{"type":"string","pattern":<padded base64>,"contentEncoding":"base64"}`
- every `Integer` field: `{"type":"integer"}`, which ESS defines as an exact i64
- every struct: `additionalProperties: false`

| # | file:line | measured | what reaches it | verdict |
|---|---|---|---|---|
| F1 | `crates/inference-audio/src/lib.rs:72` | `data: Vec<u8>` goes on the wire as a JSON number array and refuses the base64 form ESS declares. The unit's own test pins the array (`tests/audio_vocabulary.rs:151`), so that test asserts the drift. | Inside the repo: nothing yet. The STT and TTS stories build on this type. Any producer generated from the ESS schema emits base64, which this crate refuses. | NEEDS-CHANGE, blocker |
| F2 | `src/lib.rs:43` | `SampleClockPosition(u64)` can serialize values above i64::MAX, which ESS refuses, and it refuses negative positions, which ESS admits. | Nothing realistic: 2^63 samples at 48 kHz is about 6 million years. | INFEASIBLE, note |
| F3 | `src/lib.rs:35-36` | `NonZeroU32` and `NonZeroU16` refuse a sample rate of 2^32 and 65536 channels; ESS states only "at least one". | Nothing realistic. The narrowing is not stated in `ess/`. | INFEASIBLE, note |
| F4 | `src/lib.rs:33` | Unknown fields are accepted, although the schema says `additionalProperties: false`. inference-core has no `deny_unknown_fields` anywhere either, so this is a repo-wide convention. | Nothing found. | CONFIRMED, note |
| F5 | `src/lib.rs:49` | The unit's suite never converts a non-zero position with more than one channel, so mutant M1 survives. My guard case closes the gap. | The acceptance cases themselves ("position-to-time conversion at the format's sample rate"). | CONFIRMED, warning |
| F6 | `ess/domains/audio.yaml:56` | Design line 293 lists "timestamps" next to the sample clock. The domain cites line 293 but neither declares timestamps nor marks them UNMAPPED, although it does mark compressed codecs UNMAPPED. | A reader of `ess/` who expects the repository's UNMAPPED convention. | CONFIRMED, note |

Fix for F1, named but not applied: either serialize `data` as padded base64, or change the ESS type and update `tests/audio_vocabulary.rs:151` to match.

**5. Attacked, did not break**
- Suite: 229 scenarios and 3 refusals both before (base ess archived into scratch) and after; the scenario id sets are identical (added [], removed []). The implementor's claim holds.
- Names, field order and enum members and order match ESS.
- No overflow at u64::MAX positions or a u32::MAX sample rate. Rounding, ceiling, f64 and direct-multiply mutants are all caught by the existing cases.
- A channel-count mutant in `duration()` is caught by the existing stereo case.
- Zero sample rate and zero channels are refused.

**6. Paths written outside the worktree.** All are under the unit scratch's `adversary-1/`:
- `schema/`: generated schema
- `base/`: base ess, archived from edf8ca7
- `mutant-m1/`: mutated copy, including its own `target/`
- `suite-before.json`, `suite-after.json`, `syn-before.log`, `syn-after.log`
- `ids.py`, `ids-before.txt`, `ids-after.txt`, `r1`, `r2`
- `suite.log`, `clippy.log`, `schema.log`

The lease for the adversary session was taken and is now released.

```findings
- file: crates/inference-audio/src/lib.rs
  line: 72
  category: contract-drift
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: AudioFrame.data goes on the wire as a JSON number array and refuses the padded base64 string that ESS Bytes declares, and tests/audio_vocabulary.rs:151 pins the array form
- file: crates/inference-audio/src/lib.rs
  line: 43
  category: contract-drift
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: SampleClockPosition(u64) serializes values above i64::MAX, which the ESS Integer refuses, and refuses negative values it admits; no realistic stream reaches either
- file: crates/inference-audio/src/lib.rs
  line: 35
  category: contract-drift
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: NonZeroU32 sample_rate and NonZeroU16 channels refuse values the ESS Integer admits, and ess/domains/audio.yaml states only the lower bound
- file: crates/inference-audio/src/lib.rs
  line: 33
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: AudioFormat and AudioFrame accept undeclared fields although the generated schema says additionalProperties false, following the same convention in inference-core
- file: crates/inference-audio/src/lib.rs
  line: 49
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a frame_time mutant that divides the position by the channel count passes all 9 unit cases and is caught only by the adversary guard case
- file: ess/domains/audio.yaml
  line: 56
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: design line 293 names timestamps for audio frames, and the domain neither declares them nor marks them UNMAPPED as it does compressed codecs
```
