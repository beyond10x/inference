---
format: aep.planning-md/3
id: review-result:adversary-outbound-text-generations-and-segments-pass-2
kind: review-result
status: active
title: Adversary, story:outbound-text-generations-and-segments, pass 2
relations:
- reviews: story:outbound-text-generations-and-segments
revision: 1
---
```
unit: story:outbound-text-generations-and-segments, working tree on base 18f4624 after correction round 1 (uncommitted diff, untracked files, my 2 new test files)
verdict: INFEASIBLE
cases: executed 88→110, red 1
origin: introduced 2 / pre-existing 0 / undecided 2
wrote-outside-worktree: 1 directory (scratch adversary-2/, which holds a 1.4G mutant copy and build)
needs-coordinator: should observed-event positions (`ObservedEvent::at`) be one target-wide order, like tokens now are? Also delete adversary-2/mutant/ when you are done with it.
```

**1. `git --no-pager diff --stat`**: the same 11 implementor paths as before I started (1816+/76-). I changed no tracked file. I added two untracked test files and nothing else:
- `crates/inference-testkit/tests/adversary3_target_sequence.rs`
- `crates/inference-text/tests/adversary3_identifier_macro.rs`

rustfmt touched only these two files. The md5 of `tests/target/*.rs` and `git status` were the same before and after.

**2. Cases added (each written first, then run alone)**

| file:line | what it asserts | now |
|---|---|---|
| adversary3_target_sequence.rs:209 | A scripted run across both domains: each accepted command moves the token by exactly 1 (seq:1, 2, 4, 5, 6, 7). Refusals (unknown-model, wrong-state, unknown-generation, stale-generation, a second supersede) leave it where it was, and so do the two `TargetError` inputs. All 4 views answer at every token returned and at seq:0. seq:8 and a foreign token `v:1` both get `Unavailable` | green (guard) |
| adversary3_target_sequence.rs:325 | A fixed-seed walk (5 seeds, 400 steps each) across both domains. After each command the token = previous token + events published. An accepted command publishes exactly 1 event and a refusal 0. A random view answers at a random earlier token and refuses at token+1 | green (guard) |
| adversary3_target_sequence.rs:411 | ModelDeclared, then GenerationStarted, then TextDelta carry positions 1, 2, 3 in "the target's own publication order" | **red** |
| adversary3_identifier_macro.rs:59 | `SegmentId` and all 7 core ids (ModelId, AdapterId, RunId, NodeId, StreamId, GenerationId, SessionId) give identical results over 19 spellings: same accept set, same written form, same error text. `SegmentId` accepts only canonical spellings (either case, plus nil and max) and writes them in lower case. Display matches the wire form | green (guard) |
| adversary3_identifier_macro.rs:108 | The `From` conversions, `as_uuid`, Display and `Ord` of `SegmentId` all match the Uuid it wraps. `generate()` gives a version 4 id | green (guard) |

Red output, from the first run of the file alone (`--test adversary3_target_sequence`, EXIT=101; the line was :395 before rustfmt, :438 now):
```
assertion `left == right` failed: three events published in this order at tokens seq:1, seq:2 and seq:3
  left: [("inference.model.ModelDeclared", Some(1)), ("inference.stream.GenerationStarted", Some(1)), ("inference.stream.TextDelta", Some(2))]
 right: [("inference.model.ModelDeclared", Some(1)), ("inference.stream.GenerationStarted", Some(2)), ("inference.stream.TextDelta", Some(3))]
```
The macro file passed when run alone (EXIT=0): it is a guard. The testkit binary also re-runs 17 module tests, because I load `target/model.rs` and `target/stream.rs` into it the same way pass 1 did.

**3. Suite runs, after the cases existed**

| command | exit | result |
|---|---|---|
| `cargo test -p inference-testkit --all-features --no-fail-fast` | 101 | adversary3_target_sequence: `19 passed; 1 failed`; every other binary passes (14, 3, 22, 19, 2, 11, conform 11) |
| `cargo test -p inference-text --all-features --no-fail-fast` | 0 | 2 + 6 |
| `cargo test -p inference-core --all-features --no-fail-fast` | 0 | 34 (unchanged) |
| `cargo clippy -p inference-testkit` / `-p inference-text --all-targets --all-features -- -D warnings` | 0 / 0 | |
| `cargo fmt -p inference-testkit` / `-p inference-text --check` | 0 / 0 | after rustfmt on my two files only |

The `88` before my cases is the same run with my two files left out (testkit 82, text 6).

**4. Findings**

| # | file:line | verdict / origin | what was measured | what reaches it |
|---|---|---|---|---|
| F1 | tests/target/stream.rs:292, model.rs:147 | INFEASIBLE / introduced | Observed-event positions still count per domain, so ModelDeclared and GenerationStarted are both at position 1 (red case above). Tokens are now target-wide (seq:3 at the same point) | Nothing found: no ESS 0.60.0 code reads `ObservedEvent.sequence` (I grepped the whole checkout). It becomes possible with this unit, because stream is the first second domain that records events. Fix: number positions from one counter across the stores, as `Stores::sequence` does |
| F2 | tests/target/mod.rs:62 | CONFIRMED / introduced | Mutant M1 (`sequence` counts stream events only, `mutantM1.diff`) passes every earlier binary and conform 11. `one_sequence` arranges no model event, so it cannot catch it. My cases fail on it (:216, :379) | Doc promise only: under M1 the runner's reads still pass. Now guarded |
| F3 | tests/target/mod.rs:98 | CONFIRMED / undecided | Mutant M3 (a foreign token reads as `0`, `mutantM3.diff`) passes every earlier binary. My case fails on it (:304) | No runner sends a foreign token. Base `model.rs` had the same function, and the base tests only check seq:5. Not run against base. Now guarded |
| F4 | crates/inference-core/src/id.rs:64 | CONFIRMED / undecided | Mutant M4 (the macro trims trailing whitespace before parsing, `mutantM4.diff`) passes all core tests (34) and `segment_lifecycle`, which tests a leading space only. My macro case fails on it (:66) | Every id type reads through this macro. Now guarded |

**5. Attacked and not broken**
- A refusal or a `TargetError` never moves the token, in either domain (scripted run plus 2000 walk steps).
- Cross-domain reads work both ways at every token; reads demanding seq:N+1 are refused.
- The supersede cascade moves the token by its one generation event only.
- The macro: `$crate::__private` paths, the hand-written Serialize matching the old `transparent` JSON, the error text, Display, Ord and version 4. All are identical for all 8 id types.
- Earlier guards: the committed adversary files are unchanged against base. The pass-1 files still hold their cases at the reported lines (tokens :84, store :62/:88/:126, deferred :243/:277). Test names in the implementor's logs before and after the correction differ by additions only.

**6. Paths written outside the worktree** (all under `<unit scratch>/adversary-2/`):
- `mutant/`: a copy of the worktree (source restored) plus its own `target/`, 1.4G. I left it in place because build dirs are not deleted by hand.
- `mutantM1.diff`, `mutantM3.diff`, `mutantM4.diff`, `mutantM1.log`, `mutantM3.log`, `mutantM4.log`, `mod.rs.orig`, `id.rs.orig`
- `alone-sequence.log`, `alone-macro.log`, `after-fmt-sequence.log`, `suite-testkit.log`, `suite-text.log`, `suite-core.log`, `clippy-inference-testkit.log`, `clippy-inference-text.log`, `fmt-inference-testkit.log`, `fmt-inference-text.log`
- `g-*.txt`, `r1-*.txt`, `status-before-fmt.txt`, `target-md5.txt`

I deleted `adversary-1/mutant/` (1.1G) as instructed. My lease f8bfe626-wave4-adv2 is released.

```findings
- file: crates/inference-testkit/tests/target/stream.rs
  line: 292
  category: contract-drift
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: observed-event positions are counted per domain, so a model event and a stream event both sit at position 1 of what ess documents as the target's one publication order
- file: crates/inference-testkit/tests/target/mod.rs
  line: 62
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: dropping model events from Stores::sequence survives the unit's suite and every earlier adversary case because one_sequence arranges no model event
- file: crates/inference-testkit/tests/target/mod.rs
  line: 98
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: undecided
  message: reading a foreign token as seq 0 instead of beyond any survives every case written before this pass
- file: crates/inference-core/src/id.rs
  line: 64
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: undecided
  message: a uuid_newtype deserializer that trims trailing whitespace survives all inference-core and inference-text tests, for every core identifier and SegmentId
```
