---
format: aep.planning-md/3
id: review-result:adversary-status-page-pass-1
kind: review-result
status: active
title: Adversary pass 1 on story:status-page-voice-adapters
relations:
- reviews: story:status-page-voice-adapters
revision: 1
---
unit: story:status-page-voice-adapters, commit 7c652bf0 plus merge 27a3e5f0 on impl/status-page-voice-adapters (tree at 27a3e5f0)
verdict: CONFIRMED (one judgement finding, no red case)
cases: executed 28→28, red 0
origin: introduced 1 / pre-existing 0 / undecided 0
wrote-outside-worktree: 11 paths
needs-coordinator: none

**Result:** I wrote no failing case. Every check in the brief held. One judgement finding remains, a warning: an adapter that is on main but in no release yet can no longer get a row in the adapters page without turning the test red.

1. `git --no-pager diff --stat` of the worktree: empty. No file changed, nothing committed. The branch `adv/status-page-1` was never created and the worktree stayed on impl/status-page-voice-adapters. `target/` was deleted as the brief asked (I created it with my build) and my lease is released.

2. Cases added: none. Every attack below came back green, so there was no red case to write.

3. Suite run (no cases of mine exist, so the count is 28 both before and after):
```
$ cargo +1.100.0 run -q -p inference-docs -- generate --check
website/data/status.json: current … website/docs/status.mdx: current
website/docs: front matter, admonition titles, code fences and dependency lines checked
exit 0
$ cargo +1.100.0 test -p inference-docs
test result: ok. 27 passed; 0 failed; 0 ignored
test result: ok. 1 passed; 0 failed; 0 ignored
exit 0
```

4. Judgement finding, covering tree 27a3e5f0:
   - **Where:** `crates/inference-docs/src/adapters.rs:50` (`since: &'static str`) and `crates/inference-docs/src/adapters.rs:736` / `:781`.
   - **What:** a binding's status can only be "Since <released version>". The test skips the `[Unreleased]` section and refuses a binding whose phrase is not in a released section. There is no "On main, in the next release" form for a binding, although `status.rs` has one (`next()`). `AGENTS.md:35` says a page status is "Since 0.x.0", "On main, in the next release" or planned.
   - **Effect:** before this unit, a new binding's row appeared on main straight away. Now the story that adds one must either leave the row out until the release commit, or make the test red.
   - **What reaches it:** the draft `.engineering/planning/story/more-hosted-voice-adapters.md` ("one story per binding") and `eleven-realtime-duplex.md`. The suggested fix, not applied: a "next" form of `since`, mirroring `status::next`, and a test that accepts it only when the phrase is in `[Unreleased]`.
   - **Verdict:** CONFIRMED, introduced, warning.

5. Attacked and not broken:
   - **Every "Since" against git tags:** for each binding I found the first tag whose tree contains its type or crate. All 16 match the claimed release, including tools-on llama-server at 0.6.0 and ElevenLabs STT at 0.28.0.
   - **Rewritten changelog sections:** the sections for 0.5.0, 0.6.0, 0.16.0, 0.17.0, 0.22.0 and 0.28.0 are byte-identical at HEAD and at their tags, so no phrase was reworded later.
   - **Binding without `since`:** does not compile.
   - **`since` newer than the workspace version:** the test goes red, because no released section has the phrase. The release commits for 0.26.0 to 0.28.0 bump the version and add the changelog section in one commit, so a released section can't run ahead of the version.
   - **Mutants of `releases()`** (record kind always true, always false, empty): the status test kills all three.
   - **Status tests run in `task check`:** none of the 9 tests is ignored. Three are behind `stt` or `duplex`, but `task check` runs `cargo test --workspace --all-features` and CI runs `task check`.
   - **Deleting a claims record:** the test panics on `read_to_string`. With `claims-stt.json` removed from a scratch copy, the docs check exits 1 ("reading …claims-stt.json: No such file").
   - **Hand-edited generated files** (in a scratch copy): the adapters.md Since cell, the status.json detail, the old Nova Sonic label and the status.mdx count each make `generate --check` exit 1 "stale or edited". Restored, it exits 0.
   - **Nova Sonic relabel:** no site code, landing config, link or test selects the old label. The remaining "Amazon Nova Sonic" mentions are prose, doc comments, planning files and brand SVGs.
   - **Hosted bindings not in the list:** every hosted binding type and claims record is in `BINDINGS`.
   - **Origin tag:** `0.28.0` on origin resolves to 38f9066b, the same as locally.

6. Paths written outside the worktree, all under `<scratch>/p2/wave2/status-adv/`:
   - `since.sh`, `since-tags.sh`, `sections.sh`, `probe-docs-check.sh`, `suite.sh`
   - `build.log`, `suite.log`
   - `orig-adapters.md`, `orig-status.json`, `orig-status.mdx`
   - `probe-root/` was a 23 MB copy of HEAD made with `git archive`; it is deleted.

7.
```findings
- file: crates/inference-docs/src/adapters.rs
  line: 50
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a binding's since can only name a released changelog section, so a binding on main in no release cannot get an adapters row without turning the test red, against AGENTS.md:35's "On main, in the next release" status
```