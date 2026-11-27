---
format: aep.planning-md/3
id: review-result:quality-live-lane-on-the-workstation-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on the local keyed lane (2026-11-27)
relations:
- reviews: story:quality-live-lane-on-the-workstation
revision: 1
---
Adversary, pass 1, on story:quality-live-lane-on-the-workstation, 2026-11-27, as the adversary returned it; `<scratch>` is the pass's scratch directory and `<worktree>` the unit's worktree, both outside the repository.

**Verdict: breaks.** On this machine, an agent session that runs `YES=1 task quality-live` gets no key into any of the four measure commands. The lane's own code holds on the key and on the spend gate. I added six test cases: all pass on head, and each fails on a mutant the existing cases missed. No case fails on head. Findings cover head `85e478f92`: the implementation at `587bf64e1` plus my test commit.

```
unit: story:quality-live-lane-on-the-workstation, branch impl/quality-live, findings cover 85e478f92 (587bf64e1 + adversary tests)
verdict: NEEDS-CHANGE
cases: executed 161→167, red 0 on head (6 added; red on mutants M1–M7)
origin: introduced 6 / pre-existing 0 / undecided 0
wrote-outside-worktree: 8 paths (part 6)
needs-coordinator: how F1 is fixed (one command off the pool shim, or build with --no-run and run the test binary here); naming the keys in SLOT_ENV would send them to the build node, so that choice is the owner's
```

### 1. Diff stat (587bf64e1..85e478f92)
```
 crates/inference-docs/tests/live/mod.rs | 232 ++++++++++++++++++++++++++++++++
 1 file changed, 232 insertions(+)
```
Only a test file changed.

### 2. Cases added (`crates/inference-docs/tests/live/mod.rs`, all pass on head now)
- `a_command_that_fails_gives_no_record_and_the_lane_exits_1`: a command exits 1 and the example exits 3. Expects no record for either, "2 failed" and exit 1.
- `two_runs_keep_two_run_directories_and_neither_overwrites_the_other`: two runs give two run directories and two records per recorded measure.
- `a_key_given_with_a_line_end_reaches_its_command_exactly`: `secret-tool` prints `not a=key 0004\n`. The command must see exactly the stored text, and no output may hold it.
- `a_yes_that_is_not_exactly_1_calls_nothing`: `YES` set to `10`, ` 1`, `1 `, `01`, `true`, `Y` or `1\n` gives exit 2 and starts nothing.
- `without_secret_tool_every_measure_is_skipped_by_name`: with an empty `PATH`, both providers are skipped by name and no stub starts.
- `no_task_a_workflow_or_check_reaches_starts_the_lane`: walks from `check` and every `task <name>` a workflow runs, through `cmds` and `deps`. It also catches the program form `inference-docs … live`.

The first run of these alone had one failure, and it was my mistake, not the lane's. `20261127T105031Z-2.evidence.json` sorts before `…Z.evidence.json`, so my list comparison failed. I sorted both sides; that case now passes. The naming quirk itself is F4.

### 3. Suite run after the cases existed
`cd <worktree> && cargo nextest run -p inference-docs --all-features` gave `Summary [0.885s] 167 tests run: 167 passed, 0 skipped`, exit 0. The "before" count of 161 comes from running my six cases alone (6 run, 161 skipped).

Mutants, run in a scratch clone (head cases = the existing ones, with my six excluded):

| Mutant | Head cases | My cases |
|---|---|---|
| M1 `Ok(u8::from(failed > 0))` becomes `Ok(0)` | **survive** | killed |
| M2 a failed example counts as kept | **survive** | killed |
| M3 the run directory is reused (`create_dir_all`) | **survive** | killed |
| M4 no line-end strip of the key | **survive** | killed |
| M5 `YES` is trimmed before the check | **survive** | killed |
| M6 Taskfile `test-slow` gets `- task: quality-live` | **survive** | killed |
| M7 Taskfile `check-source` gets `- target/debug/inference-docs live --root .` | **survive** | killed |
| M8 the key goes to every command | killed | survive |
| M9 the count is off by one | killed | survive |
| M10 the `YES` check is inverted | killed | killed |
| M11 records go under `quality/records/` | killed | killed |

### 4. Findings

| # | file:line | Finding | Verdict | Origin | Severity |
|---|---|---|---|---|---|
| F1 | `crates/inference-docs/src/live.rs:465` | The measure commands start `cargo` from `PATH`. In agent shells that is the pool shim, and this repository's builds go to the cloud node `the build node`. The pool client holds back every variable with `_KEY` in its name, so the command never gets the key. | NEEDS-CHANGE | introduced | blocker |
| F2 | `crates/inference-docs/src/live.rs:212` | No case covered a command that fails, run-id uniqueness, the line-end strip or a near-1 `YES`. M1–M5 survived; cases added. | CONFIRMED | introduced | warning |
| F3 | `crates/inference-docs/tests/live/mod.rs:565` | The gate test only follows `check` and only matches `-- live`. It misses `test-slow` (run by `slow.yml`), `deps`, and the task's own form `target/debug/inference-docs live`. M6 and M7 survived; case added. | CONFIRMED | introduced | warning |
| F4 | `crates/inference-docs/src/live.rs:304` | A second run in the same second gets id `<base>-2`, which sorts before `<base>` in record file names (`-10` also sorts before `-2`). I found no reader that orders records by name. | INFEASIBLE | introduced | note |
| F5 | `crates/inference-docs/src/live.rs:324` | `secret-tool lookup` has no time limit, so a hung Secret Service or an unlock prompt blocks the lane with no end. | CONFIRMED | introduced | note |
| F6 | `Taskfile.yml:113` | The task description says it "exits 2" without `YES=1`. Measured: `task quality-live` exits 201 (the lane inside exits 2). | CONFIRMED | introduced | note |

**F1, what I measured:**
- `<slot config>/remote-node.env` exists. `remote-node-map` sends this repository to `the build node` by default, and `inference` is in neither deny list.
- My own `task quality-live` run printed `slot: synced to the build node … 146082 bytes sent` for its `cargo build`.
- In the pool's code, `toolchain.rs:147` (`credential()`, which matches `_KEY`), `client.rs:543-545` and `env-names.sh` hold back `OPENAI_API_KEY`, `ELEVENLABS_API_KEY` and `INFERENCE_OPENAI_KEY_SOURCE` from the node.
- Without those variables, the live tests fall back to the node's own keychain (`speech_live.rs:43`, `live_gpt.rs:71`, `tts_cancel_live.rs:77`, `examples/live.rs:42`). So the run either fails, or calls the providers from the node with whatever key the node holds. I did not check which.

**F1, what reaches it:** any Claude Code session. `~/.zshenv:1-2` puts the shim first on `PATH` whenever `CLAUDECODE` is set. From your own fish shell, `cargo` is the real one and the lane works as written. The stub tests (`--stubs`) never take this path.

### 5. Attacked and could not break
- The key never reaches stdout, stderr, a record, `.ratchet/`, an argument or a `Debug` print. A command that prints its key gets `[key]` in the kept output.
- The other provider's command and `assess` never get the key. The ElevenLabs commands do get any `INFERENCE_OPENAI_KEY_SOURCE` already set in your shell, which is not a key.
- The `YES` gate is exact: `None`, empty, `0`, `yes` and the near values above all start nothing. The Taskfile passes `YES` only through `{{.YES}}`, and the Taskfile has no `dotenv`.
- The printed count matches the sources: speech 1 request, gpt 3 ignored tests with one session each, the example 2 formats, the probe 4 `open()` calls. Neither adapter retries or reconnects.
- A run that failed is refused: `assess` refuses any exit status other than 0. Records are only written under `.ratchet/records/`, which `.gitignore` covers.
- No workflow and no task reached from `check` runs the lane today.

### 6. Paths written outside the tracked tree
Under `<scratch>/`:
- `task-quality-live.log`
- `new-cases.log`
- `suite.log`
- `mutants.sh`
- `mutants-run.log`
- `mutants/` (the table, plus a diff and logs per mutant)
- `clone/` (source only; I deleted its 381 MB `target/`)
- `commit-msg.txt`

Inside the worktree, git-ignored and left in place by rule: `target/debug/inference-docs`, which slot copied back from my `task quality-live` run. Slot also keeps a mirror of the tree on `the build node`, which I cannot see.

### 7. Findings block
```findings
- file: crates/inference-docs/src/live.rs
  line: 465
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: from an agent shell the lane's cargo commands go through the pool shim to the build node, which never gets OPENAI_API_KEY, ELEVENLABS_API_KEY or INFERENCE_OPENAI_KEY_SOURCE, so the keyed measures cannot run on this workstation
- file: crates/inference-docs/src/live.rs
  line: 212
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: exit code on failure, a failed example, run-id uniqueness, the key line-end strip and an exact YES had no case; mutants M1-M5 survived the existing cases
- file: crates/inference-docs/tests/live/mod.rs
  line: 565
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the gate test follows only check and matches only "-- live", so test-slow, deps and the binary form target/debug/inference-docs live pass unnoticed
- file: crates/inference-docs/src/live.rs
  line: 304
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: a same-second second run id <base>-2 sorts before <base> in record file names, and -10 before -2; no reader found that orders by name
- file: crates/inference-docs/src/live.rs
  line: 324
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: secret-tool lookup has no time limit, so a hung Secret Service or an unlock prompt blocks the lane with no end
- file: Taskfile.yml
  line: 113
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the task description says it exits 2 without YES=1, but task quality-live exits 201 (the lane exits 2)
```
