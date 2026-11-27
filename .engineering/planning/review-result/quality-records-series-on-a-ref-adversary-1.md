---
format: aep.planning-md/3
id: review-result:quality-records-series-on-a-ref-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on the series of quality records on a ref (2026-11-27)
relations:
- reviews: story:quality-records-series-on-a-ref
revision: 1
---
Adversary, pass 1, on story:quality-records-series-on-a-ref, 2026-11-27, as the adversary returned it; `<scratch>` is the pass's scratch directory and `<worktree>` the unit's worktree, both outside the repository.

unit: story:quality-records-series-on-a-ref, worktree `<worktree>`. The findings cover implementation commit `259074e0c`; my test commit is `4fbf74fa8`.
verdict: breaks (NEEDS-CHANGE). The workflow hands the token that can write to the repository to the `inference-docs` binary it builds from the checked-out tree, and only the push needs that token.
cases: executed 174→182, red 1
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 8 paths, listed in part 6
needs-coordinator: whether to split the job (F2 changes the story's "one job" shape) and whether the lost waiting run (F3) is acceptable

### 1. Diff (`git --no-pager diff --stat 259074e0c HEAD`)
```
 crates/inference-docs/tests/quality/series.rs | 149 ++++++++++++++++++++++
 crates/inference-docs/tests/workflows.rs      | 172 ++++++++++++++++++++++++++
 2 files changed, 321 insertions(+)
```
Only test files changed. Commit `4fbf74fa8`, made with `b10x-gates bot`: it has a body, no attribution and no `/home` path.

### 2. Cases added (each was written first, then run alone)
**Red, `tests/workflows.rs:1515` `the_binary_built_from_the_checked_out_tree_runs_without_the_write_token`.** A stand-in for `$DOCS` records every environment variable that holds the token, raw or base64, then runs the real binary. Run alone:
```
FAIL [   0.105s] (1/1) inference-docs::workflows the_binary_built_from_the_checked_out_tree_runs_without_the_write_token
panicked at crates/inference-docs/tests/workflows.rs:1538:5:
assertion `left == right` failed: the process of the tree's binary holds the write token in these variables
  left: ["TOKEN"]
 right: []
```

**Green now.** Each of these catches a mutant that the suite of `259074e0c` lets pass. All 7 passed when run alone.
- `workflows.rs` `a_record_series_append_refuses_fails_the_job_and_pushes_nothing`: catches W5, `|| true` after `series-append`, which made the job exit 0 with the message "holds every record already".
- `workflows.rs` `the_job_never_prints_the_token`: catches W4, `set -eux`.
- `workflows.rs` `the_checkout_of_the_series_job_persists_no_credentials`: catches W3.
- `series.rs` `a_link_in_the_series_where_a_directory_of_records_stands_is_refused`: catches S6. Without the check, the record is written through `gate` (or `gate/workspace-tests`) when it is a link to a directory elsewhere.
- `series.rs` `a_link_in_the_artifact_to_a_record_of_its_own_assessment_is_refused`: catches S7. The existing plant that uses a link points at a record of another assessment, so the assessment check refused it, not the link check.
- `series.rs` `a_record_that_cannot_be_written_leaves_no_record_of_the_call`: catches S8, the rollback. It uses a mode-0555 directory `gate/workspace-tests`, which is written last after 21 records.
- `series.rs` `history_orders_records_of_one_run_by_their_start_and_puts_records_of_no_run_last`: catches S4 and S5.

### 3. Suite (run after the cases existed)
`cd <worktree> && cargo nextest run -p inference-docs --all-features --no-fail-fast`
```
Starting 182 tests across 2 binaries
FAIL [   0.157s] (166/182) inference-docs::workflows the_binary_built_from_the_checked_out_tree_runs_without_the_write_token
  left: ["TOKEN"]
 right: []
Summary [   0.930s] 182 tests run: 181 passed, 1 failed, 0 skipped
exit 100
```
The before count comes from a second run with my 8 cases deselected: `174 tests run: 174 passed, 8 skipped`. The slowest case took 0.77 s. `cargo fmt --check` and `clippy -D warnings --tests` are clean.

**Mutants.** They ran in the scratch clone only, against the `binary(workflows)` tests (137 cases before my additions, 145 after).

| mutant | suite of 259074e0c | with my cases |
|---|---|---|
| S1 skip check removed | caught | caught |
| S2 refusal after writing | caught | caught |
| S3 order reversed | caught | caught |
| S4 record with no run not put last | **survives** | caught |
| S5 tie-break on start time dropped | **survives** | caught |
| S6 check for a link where a directory stands removed | **survives** | caught |
| S7 artifact `is_file` filter removed | **survives** | caught |
| S8 rollback removed | **survives** | caught |
| S9 project check removed | caught | caught |
| W1 `contents: write` at top level | caught | caught |
| W2 `cancel-in-progress: true` | caught | caught |
| W3 `persist-credentials` removed | **survives** | caught |
| W4 `set -eux` | **survives** | caught |
| W5 `series-append ... \|\| true` | **survives** | caught |
| W6 `push --force` | caught | caught |
| W7 checkout `ref: main` instead of `head_sha` | survives | **survives** |
| W8 `branches: [main]` removed | caught (only by the test's precondition) | caught |
| W9 `::add-mask::` removed | survives | **survives** |
| W10 second refusal exits 0 | caught | caught |
| W11 fix probe: `env -u TOKEN "$DOCS" series-append` | 137/137 green | **145/145 green**: this one-line fix turns the red case green |

### 4. Findings
| # | file:line | severity | verdict | origin | finding | what reaches it |
|---|---|---|---|---|---|---|
| F1 | `.github/workflows/quality-series.yml:66` (and `:103`) | defect | NEEDS-CHANGE | introduced | The step env exports `TOKEN` (`contents: write`). `"$DOCS" series-append` is the tree's binary plus all its locked dependencies, and it runs with that token in its environment. Only `forge push` needs it. Fix: `env -u TOKEN "$DOCS" ...` (W11), or a step without the token. | Every run of the job. Exploiting it needs hostile code on main or in a locked dependency; that code already runs in `check`, but there it has no write token. |
| F2 | `.github/workflows/quality-series.yml:49` | warning | CONFIRMED | introduced | `cargo build` runs the build scripts and proc macros of the checked-out tree inside the job that holds `contents: write`. They can write `$GITHUB_ENV` or `$GITHUB_PATH` and so steer later steps that hold `GH_TOKEN` and `TOKEN` (a fake `gh` or `git` on the path). F1's fix does not close this. Closing it needs the build and `series-append` in a job with read-only permissions, plus a separate job that only runs `git` to push. Neither the workflow nor `quality/README.md` says which code the job trusts. Not tested: it needs a runner. | The same runs as F1. The change departs from the story's "one job" shape, so it is the coordinator's or owner's call. |
| F3 | `.github/workflows/quality-series.yml:14` | note | INFEASIBLE | introduced | GitHub keeps one waiting run per concurrency group, so a third run replaces it and the replaced run's records never reach the series. The README documents this. The story says the job "queues (never cancels)". It cannot be tested off the forge. | Three pushes to `main` within one job's runtime. |
| F4 | `Taskfile.yml:117` | note | CONFIRMED | introduced | `task quality-history` also writes the local ref `refs/ratchet/records` into the shared git directory of every worktree, not only `target/`. The README says so; the story says "into a directory under `target/`". Fix: fetch without a destination ref and run `git archive FETCH_HEAD`. | Every call of the task. |
| F5 | `crates/inference-docs/tests/workflows.rs` | note | CONFIRMED | introduced | Mutants W7 (the checkout no longer pinned to `head_sha`) and W9 (mask removed) survive every case. No case is written for them: W9 only matters together with a print, which W4's case now catches. | none found |

### 5. Attacked, could not break
- Trigger and condition: the job does not run for a pull request, a fork, another workflow, another branch, or a failed or cancelled run (`workflow_run` filter plus `if:` on event, conclusion and branch).
- Script injection: run id, head SHA and artifact name reach `run:` only through `env` and quoted.
- Every action is pinned by full SHA.
- Non-fast-forward push: the job fetches once, appends once more, then fails. A refused push of another kind, and the first run with no ref, are handled. The job never force-pushes or deletes.
- `series-append` writes nothing when a plant is refused. It never changes an existing file, including through a link at the file. Plants tried: `..` in a name (read_dir cannot produce one), a directory where a record should be, a file that is not JSON, an empty artifact (`if-no-files-found: error` in `check.yml`). Equal bytes under two assessments cannot happen: the bytes name their assessment.
- `history`: a measure missing from a record prints `=none`; an assessment with no record prints no line.

### 6. Paths written outside the worktree
Under `<scratch>/`:
- `clone/` (its `target/` is deleted)
- `mutants.sh`
- `mutants-new/`
- `mutants-old/`
- `red1.log`
- `suite.log`
- `commit-message.txt`
- `workflows.diff`

### 7. Findings block
```findings
- file: .github/workflows/quality-series.yml
  line: 66
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the append step exports the contents-write GITHUB_TOKEN as TOKEN into the environment of inference-docs built from the checked-out tree, though only the push needs it
- file: .github/workflows/quality-series.yml
  line: 49
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: build scripts and proc macros of the tree run in the job holding contents write and can steer later token-holding steps through GITHUB_ENV or GITHUB_PATH; the workflow names no trusted code
- file: .github/workflows/quality-series.yml
  line: 14
  category: concurrency
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: a third queued run replaces the waiting one, whose records never reach the series, against the story's never cancels
- file: Taskfile.yml
  line: 117
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: task quality-history writes the shared local ref refs/ratchet/records besides target/quality-series
- file: crates/inference-docs/tests/workflows.rs
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: checkout ref changed from head_sha to main and removal of the add-mask line survive every case
```
