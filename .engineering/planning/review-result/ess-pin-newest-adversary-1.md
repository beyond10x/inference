---
format: aep.planning-md/3
id: review-result:ess-pin-newest-adversary-1
kind: review-result
status: active
title: aep:adversary on story:ess-pin-newest, pass 1
relations:
- reviews: story:ess-pin-newest
revision: 1
---
needs-change

aep:adversary, pass 1, on the unit commit a512761 (ess pin 0.66.0 to 0.67.0), base f9f2ebb.

**What held:** `ESS_SHA256` is the sha256 of `ess-0.67.0-x86_64-unknown-linux-gnu.tar.gz`, downloaded
independently, and matches the release's `SHA256SUMS` line; both workflows download that asset name.
`Cargo.lock` changes only the eight `ess-*` packages, all to the commit the 0.67.0 tag points at. No
other file pins ess 0.66 or requires 0.60. No synthesized suite is committed, so the suite cannot drift.

**Case added:** `crates/inference-docs/tests/ess_pin.rs`
`the_run_the_checks_guide_names_the_ess_version_ci_installs`, red on a512761 (exit 101).

**Resolution:** a3df986 rewrites the guide sample from an ess 0.67.0 run, adopts the case, and restores
the released 0.7.0 CHANGELOG entry.

```findings
- file: website/docs/guides/run-the-checks.md
  line: 60
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the guide still says CI installs ess 0.66.0 and shows the requires-newer warning the unit removed, and the docs drift check does not catch it
- file: website/docs/guides/run-the-checks.md
  line: 72
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: the sample validation output says 10 files and 6 scenarios where ess prints 11 files and 8 scenarios
- file: CHANGELOG.md
  line: 187
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the unit rewords an entry in the released 0.7.0 section, which the commit message does not mention, and the new line is over 100 characters
```
