---
format: aep.planning-md/3
id: review-result:sentence-clips-with-durations-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on the sentence clips (2026-11-27)
relations:
- reviews: story:sentence-clips-with-durations
revision: 1
---
Adversary, pass 1, on story:sentence-clips-with-durations, 2026-11-27, as the adversary returned it; `<scratch>` is the pass's scratch directory outside the repository.

The code holds: I found no defect in what `voice.rs` does. The story's own tests miss five of the 16 mutants I tried, so I added 6 cases, committed as `2799130ed`. They catch those five and pass on the current head.

```
unit: story:sentence-clips-with-durations, impl/sentence-clips at 453548c1d (cases committed on top as 2799130ed)
verdict: CONFIRMED (test gaps only, warning; no behaviour defect)
cases: executed 334→340, red 0 on the head (the 6 new cases go red only on mutants)
origin: introduced 7 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory (part 6)
needs-coordinator: none
```

**1. Diff stat** (one test file, no implementation file touched):
```
 crates/inference/tests/oneshot.rs | 260 ++++++++++++++++++++++++++++++++++++++
 1 file changed, 260 insertions(+)
```

**2. Cases added** in `crates/inference/tests/oneshot.rs`, in a block `mod adversary`. All are green on the head, so none was red when first run. Each one's red output below comes from the run of the mutant it catches.

| Case (line) | What it asserts | Mutant it catches |
|---|---|---|
| `adversary_stereo_clips_at_44100_hz_…` (:1283) | Stereo 44.1 kHz: clip durations as fixed values (10_884_353, 7_256_235 and 3_628_117 ns), equal to the files read back | M04, M05 |
| `adversary_f32_clips_are_written_as_32_bit_float_files_…` (:1324) | `pcm_f32le` files are 32-bit float and hold the same sample bits as the frames | M12 |
| `adversary_the_splitter_follows_its_rule_…` (:1376) | Splitter on non-breaking and ideographic spaces, `\r\n`, `3.14`, `e.g.x`, a mark before a quote, `...` alone, multi-byte text beside a mark | M01 |
| `adversary_a_later_name_or_a_dangling_link_…` (:1401) | An existing `clip-003.wav`, a dangling `clip-002.wav` link, or a file given as the directory is refused before any session, and the directory is unchanged | M09 (only this case) |
| `adversary_a_name_taken_during_synthesis_…` (:1441) | A thread creates `clip-002.wav` after the name check: the call fails naming it and removes the `clip-001.wav` it wrote | M10 |
| `adversary_an_empty_text_written_to_a_directory_…` (:1493) | The file form with an empty text: no file, no error, no session | M16 |

Red output on the mutants:
```
M04 (duration as the sum of per-frame durations): left: [10.884351ms, 7.256234ms, 3.628117ms]  right: [10.884353ms, 7.256235ms, 3.628117ms]
M05 (duration multiplied by channels):            left: [21.768707ms, 14.512471ms, 7.256235ms] right: [10.884353ms, 7.256235ms, 3.628117ms]
M10 (no removal of written files):  assertion failed: the call removed the file it wrote and kept the other  left: ["clip-001.wav", "clip-002.wav"] right: ["clip-002.wav"]
M12 (f32 written with an Int spec):  left: (2, 22050, 32, Int) right: (2, 22050, 32, Float)
M16 (empty-text early return removed): an empty text is no error: Layer { message: "the synthesiser sent no audio, so no WAV file is written; check its binding", source: None }
```
The 6 cases passed 10 runs in a row. The slowest took 0.018 s; none is over 5 s.

**3. Suite run** after the cases existed: `cargo nextest run -p inference --features testing,cost`, exit 100.
```
FAIL [   0.017s] ( 68/340) inference::adversary2_w2u1_one_call a_chat_whose_model_calls_its_tool_once_a_turn_answers_six_turns
Summary [   3.213s] 340 tests run: 339 passed, 1 failed, 0 skipped
```
The one failure is the case the brief excludes (it belongs to the sdk-one-call unit). The "before" count of 334 is the earlier run's 339 minus my 5 cases; the 6th case was added after that run.

**Mutant table.** Mutants ran in a scratch clone of the worktree, not in `voice.rs` itself. This departs from your brief: the adversary charter forbids editing the file under attack even briefly. The command was `cargo nextest … --lib --test oneshot` on the voice, clip and sentence tests.

| Mutant | Caught by |
|---|---|
| M01 split after a mark whatever follows | `voice::tests::a_sentence_ends…`, `a_text_with_no_sentence_end…`, adversary splitter case |
| M02 inner `next_if` loop removed | none (gives the same result, see finding 6) |
| M03 empty parts kept | 8 cases, among them both splitter unit tests |
| M04 duration per frame, not per run | adversary stereo case only |
| M05 duration × channels | adversary stereo case only |
| M06 `{n:02}` names | `a_missing_directory…`, `clips_written…`, 2 adversary cases |
| M07 no directory check | `a_missing_directory…`, adversary later-name case |
| M08 no name check | `a_missing_directory…`, adversary later-name case |
| M09 name check with `exists()` (follows links) | adversary later-name case only |
| M10 no removal of written files | adversary name-taken case only |
| M11 f32 read big-endian | none (the mock's f32 frames are all zero bytes) |
| M12 f32 spec as Int | adversary f32 case only |
| M13 i16 read big-endian | `clips_written…` |
| M14 mu-law/A-law written as Int 16 | `clips_in_mu_law_or_a_law…` |
| M15 no `remove_file` inside `write_wav` | none (no test can make hound fail halfway) |
| M16 empty text to file form returns an error | adversary empty-text case only |

**4. Findings** (all on `voice.rs` at 453548c1d; adversary cases in 2799130ed)

| # | file:line | Finding | Verdict | Origin | Severity | What reaches it |
|---|---|---|---|---|---|---|
| 1 | voice.rs:82 | No story test checks stereo or a rate where 160 samples is not a whole number of nanoseconds, so M04 and M05 survived | CONFIRMED | introduced | warning | Any 44.1 kHz or stereo synthesiser |
| 2 | voice.rs:348 | No story test covers the removal of already-written files when a later write fails (M10) | CONFIRMED | introduced | warning | Another process creating a clip file during synthesis |
| 3 | voice.rs:149 | The float WAV path had no test at all (M12) | CONFIRMED | introduced | warning | Any `pcm_f32le` synthesiser |
| 4 | voice.rs:340 | No story test covers an empty text in the file form (M16) | CONFIRMED | introduced | warning | `synthesise_clips_to("")` |
| 5 | voice.rs:185 | The f32 sample mapping cannot be observed through the public API with the mock, whose f32 frames are silent (M11). Closing it needs a unit test of `write_wav` with non-zero data inside `voice.rs` (the implementor's file) | INFEASIBLE | introduced | note | Real f32 synthesisers |
| 6 | voice.rs:57 | The inner `next_if` loop changes no result: each mark already checks the character after it, so M02 gives the same output | CONFIRMED | introduced | note | Nothing; redundant code |
| 7 | voice.rs:330 | From the 1000th clip the name is `clip-1000.wav`: four digits, not the story's three, and it sorts between `clip-100` and `clip-101`. The story does not say what happens past 999 | INFEASIBLE | introduced | note | A text of more than 999 sentences; not tested (1000 sessions) |

The likely fix for finding 5 is a test-only unit test in `voice.rs`'s `mod tests`; that is for the implementor.

**5. Attacked and could not break**
- Splitter: byte slicing next to multi-byte characters (no panic); non-breaking space, ideographic space and `\r\n` count as whitespace; `3.14`, `e.g.x` and a mark before a quote follow the story's literal rule.
- Check order: directory, then names (`symlink_metadata`, so dangling links count), then synthesis, then encoding, then `create_new`. Nothing is written when a check fails.
- A file given as the directory is refused. Nothing is written outside the directory.
- Mu-law and A-law are refused by name with nothing written. Integer samples are written in the right order; only s16le is tested, in mono and stereo.
- Durations equal the length of the files read back.
- An empty text opens no session in either form.

**6. Paths written outside the worktree** — 1 directory, `<scratch>/adversary-1/` (28 MB), holding:
- `mut/`, the mutant clone; its 605 MB build output is already deleted;
- `module.rs`, `mutants.sh`, `mutants.log`, `mutant-M01…M16-*.log`, `suite.log`, `commit-msg.txt`.

**Refused call** (unedited): `PreToolUse:Bash hook error: [if command -v b10x >/dev/null 2>&1 && b10x guard --help >/dev/null 2>&1; then b10x guard bash; fi]: b10x guard (heredoc_stdin): a heredoc into a stdin reader has hung a session for 22 h: write the script with the Write tool and run it by path`. I then wrote the test module with the Write tool, as the hook's message says to, and appended it with `cat`.

My lease `inference-clips-adv` is ended. The worktree is clean at `2799130ed` and nothing is pushed.

**7.**
```findings
- file: crates/inference/src/voice.rs
  line: 82
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: duration per frame instead of per run, or multiplied by channels, passed every story test because they use only mono 16 kHz, where 160 samples are exactly 10 ms
- file: crates/inference/src/voice.rs
  line: 348
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: dropping the removal of already-written clip files after a later write fails passed every story test
- file: crates/inference/src/voice.rs
  line: 149
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the pcm_f32le WAV path had no test, so writing float audio with an Int spec passed every story test
- file: crates/inference/src/voice.rs
  line: 340
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the file form of an empty text had no test, so turning it into an error passed every story test
- file: crates/inference/src/voice.rs
  line: 185
  category: mutant
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: f32 byte order cannot be observed through the public API because the mock's f32 frames are silent; it needs a unit test of write_wav with non-zero data
- file: crates/inference/src/voice.rs
  line: 57
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the inner next_if loop over a run of marks changes no result and is redundant code
- file: crates/inference/src/voice.rs
  line: 330
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: from the 1000th clip the name has four digits, not the story's three, and it sorts out of order; the story does not say what happens past 999
```
