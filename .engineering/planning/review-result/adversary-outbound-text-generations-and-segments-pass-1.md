---
format: aep.planning-md/3
id: review-result:adversary-outbound-text-generations-and-segments-pass-1
kind: review-result
status: active
title: Adversary, story:outbound-text-generations-and-segments, pass 1
relations:
- reviews: story:outbound-text-generations-and-segments
revision: 1
---
```
unit: story:outbound-text-generations-and-segments, working tree on base 18f4624 (uncommitted diff plus untracked files), with my 3 untracked test files added
verdict: CONFIRMED
cases: executed 48→78, red 1
origin: introduced 4 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory (scratch adversary-1/, holding a 1.1G mutant build)
needs-coordinator: whether consistency tokens should be one per target instead of one per domain (also affects the wave-3 model module and the coming graph module)
```

**1. `git --no-pager diff --stat`**: the same 7 implementor files as before I started (1644+/24-). I added three untracked files and changed nothing else:
- `crates/inference-testkit/tests/adversary2_stream_deferred.rs`
- `crates/inference-testkit/tests/adversary2_stream_store.rs`
- `crates/inference-testkit/tests/adversary2_stream_tokens.rs`

No file outside the tests was touched. rustfmt left `tests/target/*` byte-identical to the scratch copy.

**2. Cases added (each written, then run alone)**

| file | what it asserts | now |
|---|---|---|
| adversary2_stream_tokens.rs:61 | A stream view read with the token from `inference.model.DeclareModel` (which the runner sends from the last command, whatever its domain) gets an answer | **red** |
| adversary2_stream_deferred.rs:243 | The 58 generation and segment scenarios held back for want of a Run all pass against the stream module. A stub inside the test answers the policy/graph setup commands and `StartGeneration`; everything else goes to the shipped module | green (guard) |
| adversary2_stream_store.rs:62 | A refused supersede, cancel or complete on a terminal generation that holds segments moves none of them | green (guard) |
| adversary2_stream_store.rs:126 | A random walk of all commands (5 fixed seeds, 600 steps each) keeps the declared lifecycle: only declared state moves, a Played segment never changes, the cascade leaves only Played or Discarded, one event per accepted command | green (guard) |

The red output, captured when the token case first ran alone:
```
panicked at crates/inference-testkit/tests/adversary2_stream_tokens.rs:84:13:
`inference.stream.Generations` at the token of inference.model.DeclareModel (ConsistencyToken(seq:1)): reading `inference.stream.Generations` failed: the read demands ConsistencyToken(seq:1), and the store has reached ConsistencyToken(seq:0)
```

The 78 executed cases include 25 existing module tests that run a second time because `mod target;` and `#[path = "target/stream.rs"]` pull them into my binaries. I wrote 5.

**3. Suite run, after the cases existed**
- `cargo test -p inference-testkit --all-features --no-fail-fast`: TEST_EXIT=101.
  - lib 14, adversary1_store 3, adversary1_target 21, conform 10: all pass.
  - adversary2_stream_deferred 18 and adversary2_stream_store 2: pass.
  - adversary2_stream_tokens: `9 passed; 1 failed`.
- `cargo clippy -p inference-testkit --all-targets --all-features -- -D warnings`: CLIPPY_EXIT=0. My deferred file needs `#![allow(clippy::duplicate_mod)]` because it loads `target/stream.rs` twice.
- `cargo fmt -p inference-testkit --check`: FMT_EXIT=0.
- The `48` is the same run minus my 3 binaries. I did not run the suite before writing the cases.

**4. Findings**

| # | file:line | verdict / origin | measured | what reaches it |
|---|---|---|---|---|
| F1 | tests/target/stream.rs:266 | INFEASIBLE / introduced | The token is `seq:<stream events>`. A token from another domain's command is compared against it and refused (red case above) | The runner reads views at the last command's token, whatever the domain (ess 0.60.0 `runner.rs` `query_view`, :990-1003). No current scenario reads a stream view right after another domain's command. The one cross-domain read, `cancel-run-discards-unplayed-segments` (graph `Runs` after `AcknowledgePlayback`, 16 stream events), runs into the same check once the graph module copies this pattern. Fix: one token counter for the whole target |
| F2 | src/stream.rs:746 | NEEDS-CHANGE / introduced | The unit's refused-move test seeds no segments. Mutant A (the discard runs before the generation-state check, `mutantA.diff`) passes lib 14, conform 10 and the earlier adversary tests; my store cases fail on it (:88, :252) | The cascade on the refusal path. Now guarded by store.rs:62 |
| F3 | tests/target/stream.rs:113, :342 | NEEDS-CHANGE / introduced | The target's success paths for `CancelGeneration` and `DiscardSegment` are never exercised, and the event mapping is only partly checked. Mutant B (`SegmentDiscarded` reported as `SegmentQueued`) and mutant C (`cancelled` answered as `superseded`) both pass the unit's suite. The deferred case fails on them: 2 and 7 of 58 scenarios | Every one of those 54 scenarios once `story:run-lifecycle` lands. Now guarded |
| F4 | tests/conform.rs:30 | CONFIRMED / introduced | The unit edited `conform.rs` to list the 8 ids by name. The brief marks that file as not the unit's | Brief scope only. The change itself is sound |

**5. Attacked and not broken**
- State sets, their order and wire names: they match `stream.yaml` for both entities.
- Transition sources: checked across the whole state-by-transition grid, plus the random walk.
- Event payloads, error fields and view fields: they match the declarations, including `stale-generation` and the unknown-identity wrong-state with no `current` (matches ess `execute.rs:1847`).
- The cascade skips Played and already-Discarded segments, matching ess `set_effects.rs:1308-1320`.
- `SKIPPED.md`: 54 stream ids left, all naming run-lifecycle.
- `SegmentId` refuses the urn, braced, unhyphenated and padded spellings.
- Counter ids do not advance on a refusal.
- `inference-core` and `inference-text`: no mutant found that survives their tests, so I added no files there.

**6. Paths written outside the worktree** (all under `<unit scratch>/adversary-1/`):
- `mutant/`: a copy of the worktree plus its own `target/` (1.1G). I left it in place, since build dirs are not deleted by hand.
- `suite.json`, `arrange.jq`, `crossread.jq`
- `mutantA.diff`, `mutantA.log`, `mutantB.diff`, `mutantB.log`, `mutantC.diff`, `mutantC.log`
- `suite-testkit.log`, `clippy.log`, `fmt.log`

The `adversary2-stream-*` directories under the worktree's `target/tmp` are written by the test itself.

My lease (f8bfe626-wave4-adv) is released.

```findings
- file: crates/inference-testkit/tests/target/stream.rs
  line: 266
  category: contract-drift
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: the stream module's consistency token counts only stream events, so a read_your_writes view read at another domain's token is refused although that command wrote nothing the store must wait for
- file: crates/inference-testkit/src/stream.rs
  line: 746
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the refused-move test seeds no segments, so running the discard cascade before the generation-state check survives the unit's suite
- file: crates/inference-testkit/tests/target/stream.rs
  line: 113
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the target's CancelGeneration and DiscardSegment success branches and the event-name mapping are unasserted, so a wrong outcome name or event name survives the unit's suite
- file: crates/inference-testkit/tests/conform.rs
  line: 30
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the unit edited conform.rs, which its brief lists as not the unit's file
```
