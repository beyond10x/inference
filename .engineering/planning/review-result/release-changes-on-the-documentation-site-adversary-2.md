---
format: aep.planning-md/3
id: review-result:release-changes-on-the-documentation-site-adversary-2
kind: review-result
status: active
title: Adversary pass 2, with the generator binary, over the release pages of the documentation site (2026-11-25)
relations:
- reviews: story:release-changes-on-the-documentation-site
revision: 1
---
unit: story:release-changes-on-the-documentation-site (pass 2, no build; head d67bdfe8f, generator commit 0ccc257eb)
verdict: findings
wrote-outside-worktree: <scratch>/release-docs/adversary-2/ (tree/ copy, head.tar, mut.sh, mut2.sh, mutants/*.log, patch/, added-cases.patch, gen1.log, gen2.log, check-final.log, rustfmt.log, changelog.flat)

Pass 1's findings 1 to 12 are fixed at this tree. Finding 13 is still open; it is the coordinator's follow-up. I found 1 new warning and 2 notes.

I made no edit, commit or push in the worktree. `git status --short` is empty, and at the end the copy matched the worktree byte for byte.

The binary was safe to use. It was built at 15:28:35, after the newest file under `crates/inference-docs/src/` (15:26:30). `git diff 0ccc257eb d67bdfe8f -- crates/inference-docs/src` is empty.

Two things to know about the run:
- The generator binary starts `cargo metadata --no-deps` itself (`crates.rs:36`). The cargo shim passes `metadata` straight to cargo, outside slot, and it compiles nothing. Every call of the binary did this.
- I ran `rustfmt --edition 2024 --check` once, on my patch file in scratch. It exited 0.

## Pass 1 findings at this tree

1. Fixed. `adapters.rs:299-304` now says the close waits "at most the longer of the close timeout and the endpoint's read timeout", and that with the 30 s default "a close timeout of 30 s or less changes nothing". The source agrees: `live.rs:1063` (`read_timeout.max(close_timeout)`), `config.rs:22` (30 s), `config.rs:119` (`Endpoint::with_read_timeout`).
2. Fixed in the doc and with one new check.
   - The module doc at `adapters.rs:19-28` now says what the name check holds and what it skips.
   - A new check (`defines`, line 425; used at lines 527-542) requires the last part of `a::b` to be defined.
   - Mutants M10, M12a, M13 and M14 are caught. M11, M12b, M15 and M16 pass, and the doc names each of those cases.
3. Fixed. Each constant now names the file it is defined in (`adapters.rs:274-279`), and the value is read from that file only (lines 492-500). M03 and M04 are caught. M09b, a change to the other `DEFAULT_READ_TIMEOUT` (`agent.rs:100`), rightly passes.
4. Fixed.
   - `duration_constant` (lines 372-411) reads `_` in digits and a definition wrapped after `=`: M07b and M08a pass with the right value.
   - A plain expression is refused with the file and the name in the message (M05).
   - One gap remains: finding 14.
5. Fixed in the doc, `adapters.rs:24-25`.
6. Fixed. `compare-voice-providers.md:57-61` agrees with `examples/compare.rs:22-27,661-672` and with `src/compare.rs:1595-1622` (a later failure becomes a run with the error as its note).
7. Fixed. `talk-to-a-hosted-voice-model.md:189-191` agrees with the doc comments at `live.rs:886-892` and `live_duplex.rs:403-405`. The anchor resolves.
8. Fixed. `adapters.rs:330-336` says "that the binding does not refuse for its configuration, and whose runtime was built".
9. Fixed. The front matter at `repo-podcast.md:6` is updated, and line 100 says "A model that creates every response itself, as GPT-Live does". `show.rs:387,490-491` agree.
10. Fixed.
    - `run-the-checks.md:162` says "Ratchet (`beyond10x/ratchet`)".
    - Lines 168-171 say "the YAML files and records stop agreeing … with the pages that describe them".
    - The heading "The test that keeps the documents true" exists at `quality/README.md:37`.
11. Fixed. `run-the-checks.md:162` says "one record per assessment whose baseline was read from a CI run".
12. Fixed. The pointer sentence (`errors.rs:185,212`) is rendered only when `agent_note()` (line 217) holds, and the test at line 251 holds both together. The committed `errors.md` matches generation.
13. Not fixed, as expected. `quality/README.md:657` still says "holds for 4 of 4 tests" and does not list `every_note_is_of_the_release_whose_changelog_section_brought_it`.

## New findings

14. **warning**: a constant written as the sum of two durations is read as one of them, so a false value passes.
    - Where: `crates/inference-docs/src/adapters.rs:396-410` (`duration_constant`, its `number` closure). The closure takes the first `from_secs(`/`from_millis(` call that closes the expression and ignores what stands before it.
    - Mutant M06: `adapters/inference-elevenlabs/src/synthesiser.rs:38` becomes `std::time::Duration::from_secs(1) + std::time::Duration::from_millis(5)`, which is 1005 ms. `generate --check` exits 0 and the page still says "`CLOSE_GRACE` (5 ms)".
    - Swapped (`from_millis(5) + from_secs(1)`), the check reads "1 s".
    - Why it is not covered: the module doc (line 17) says "exactly once as whole seconds or milliseconds". The `<…>` before the call in the doc of `Note::constants` (line 276) does not say that an arithmetic prefix is read through.
    - Test: `added-cases.patch` adds `a_constant_that_adds_two_durations_is_refused_not_read_as_one_of_them` to the test module of `adapters.rs`. It fails now on both cases. rustfmt reports it clean and `git apply --check` passes; it has never been compiled.
    - What I would accept: refuse a definition whose text before the call, trimmed, is not a path (only identifiers and `::`). The added test then passes with the message "gives no whole seconds or milliseconds in digits for SUM" (and SWAPPED).

15. **note**: the refusal messages say what is wrong but not where to change it.
    - Where: `adapters.rs:498,500,503,523,539`.
    - Source: M04, an honest move of `CLOSE_BOUND` from `connection.rs` to `lib.rs`. The message is `inference-docs: ElevenLabs streaming TTS: adapters/inference-elevenlabs/src/connection.rs does not define CLOSE_BOUND`. M02 gives the same kind of message: `the note does not say \`CLOSE_BOUND\` (1000 ms)`.
    - Neither message names `NOTES` in `crates/inference-docs/src/adapters.rs`. An adapter editor who has never seen the generator does not learn which file to edit.
    - What I would accept: each note error starts with "the note on <binding> in `NOTES` of crates/inference-docs/src/adapters.rs:".

16. **note**: a term not introduced on its page.
    - Where: `website/docs/guides/talk-to-a-hosted-voice-model.md:190`, "waits for `session.closed`". It is the only place this event appears on that page, and the linked adapters page does not explain it either.
    - What I would accept: "waits for the server's closing event, `session.closed`, …".

## Mutants (copy only, each restored from the worktree's file; logs in adversary-2/mutants/)

| id | mutant | result |
|---|---|---|
| M01 | `CLOSE_TIMEOUT` 15 s → 20 s (`live.rs:106`) | caught |
| M02 | elevenlabs `CLOSE_BOUND` `from_millis(1000)` (another unit) | caught, the message gives the value to write |
| M03 | cartesia `CLOSE_GRACE` defined twice in `synthesiser.rs` | caught |
| M04 | elevenlabs `CLOSE_BOUND` moved to `lib.rs` | caught (finding 15) |
| M05 | `CLOSE_GRACE` = `from_millis(2 + 3)` | caught (refused) |
| M06 | `CLOSE_GRACE` = `from_secs(1) + from_millis(5)` | **passed** (finding 14) |
| M07 | cartesia `CLOSE_BOUND` `from_millis(1_000)` | caught, message gives the value |
| M07b | `from_secs(0_1)` | passed, correct (1 s) |
| M08a | rustfmt-style wrap after `=`, same value | passed, correct |
| M08b | wrapped definition, value 50 ms | caught |
| M09 | openai `DEFAULT_READ_TIMEOUT` 30 s → 10 s | caught |
| M09b | elevenlabs `agent.rs` `DEFAULT_READ_TIMEOUT` changed (no note names that file) | passed, correct |
| M10 | variant `SignedUrlSpent` renamed; the word stays in comments | caught |
| M11 | renamed; defined only in a `#[cfg(test)]` module of `agent.rs` | passed; the doc names it ("belongs to `a`" not checked) |
| M12a | renamed; defined only in another adapter (cartesia) | caught |
| M12b | renamed; defined only in `crates/inference-duplex` | passed; the doc names it |
| M13 | both `fn close_timeout` renamed | caught |
| M14 | `fn open_error` renamed | caught |
| M15 | `LiveClient::with_close_timeout` removed; `OpenAiLive::` one kept | passed; the doc names it |
| M16 | `"session.closed"` → `"session.ended"` in `live.rs` (a skipped span) | passed; the doc names the skip |
| M17 | `CHANGELOG.md` GPT-Live phrase reworded | generation passes, as the doc says. By reading, the status test fails with "no section says" (needs a build) |

Generator mutants, argued by reading (each needs a build):
- A note on an unlisted binding is refused at line 461, and two notes on one binding at line 484. The test at lines 1380-1383 covers both.
- With the agent note removed, `agent_note()` is false: the committed `errors.md` goes stale, and the `errors` test's `assert!(agent_note())` fails.
- With `SignedUrlSpent` renamed in the note, generation refuses at `defines` (the same path as M10).
- With a note's status set to `Since`, the status test fails. All three note phrases occur only under `[Unreleased]` (character positions 614, 1243 and 4743, before the first release heading at 5769).

Determinism:
- `generate` run twice on the copy gives no diff between the runs.
- All 6 generated files are byte-equal to the worktree's.
- `generate --check` on the unchanged copy exits 0 (`check-final.log`).

Rules of brief item 6 hold:
- The unit's diff (`676dd869a..d67bdfe8f`) touches only `crates/inference-docs/src/{adapters,errors,main}.rs`, the six website pages and the planning store commit `5ffb10180`.
- No file under `adapters/*/src`, `quality/`, `ux/`, `CHANGELOG.md` or `limitations.md` changed, and no version changed.
- I found no person's name, private path or duration in the diff.

```findings
- file: crates/inference-docs/src/adapters.rs
  line: 396
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: duration_constant reads a sum such as from_secs(1) + from_millis(5) as 5 ms, or as 1 s when swapped, so a false CLOSE_GRACE value passes generation; the module doc does not name this
- file: crates/inference-docs/src/adapters.rs
  line: 498
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the refusals of a note (a moved constant, a changed unit) do not name NOTES in crates/inference-docs/src/adapters.rs as the place to change
- file: website/docs/guides/talk-to-a-hosted-voice-model.md
  line: 190
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the guide uses session.closed without introducing it on the page
```

## Summary line and what is not checked

- **No test suite ran in this pass.** The last suite run anyone made is the implementor's, at 0ccc257eb before the merge: `Summary [  11.475s] 80 tests run: 80 passed, 0 skipped`. Nothing has run the tests at d67bdfe8f. The merge brought `quality/adapters.md`, `quality/not-measured.md` and adapter tests, which the `workflows` binary's quality tests read.
- **Needs a build:**
  - the added test failing now, and passing after a fix for 14;
  - the status test against M17;
  - the generator mutants above;
  - the full suite at the merge head.
- **Not checked:**
  - the site build or Docusaurus rendering (I checked anchors by heading only);
  - every row of `quality/README.md`;
  - the count of "54 tests" on `repo-podcast.md`;
  - `status.mdx`;
  - block comments or raw strings that hold a `pub const` line (they would count as a definition).
