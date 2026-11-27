# The quality of this repository

This directory says what counts as the quality of `inference`, what each value is today, and
where the value comes from. The measures of each adapter crate are on
[adapters.md](adapters.md): its tests, its time bounds, its records and the commands that print
a latency. What has no command that produces it is in [not-measured.md](not-measured.md).

The documents are written in the formats of Ratchet (`beyond10x/ratchet`), against its
specification at commit `f139eccc17a548444f0f4e1dc06520120c28739f`.

- No Ratchet command works at that commit. Nothing here is run by Ratchet.
- Every field name is a proposal of that specification. A release of Ratchet can change it.
- The documents are a gate: a step of `task check` and of the required check
  `inference / task check` reads them. `task quality-records` fails when a plan file cannot be
  read, when the plan files list no assessment it writes a record for, when the declaration of
  an assessor is missing or cannot be read, when `inference-docs assess` refuses its call for an
  assessment or cannot write the record, or when a record is refused because the report of the
  tests lacks what its assessment reads. A change of a plan file or of a declaration under
  `assessors/` can fail both. Beyond that step the documents describe what `task check`,
  `task test-slow` and the workflows under `.github/workflows/` enforce.
- A measure can be defined and have no value. The latency measures on
  [adapters.md](adapters.md) are such: each names its command and its clock, and says
  `Baseline: none.`

## The files

| File | Format | Holds |
| --- | --- | --- |
| `../ratchet.toml` | local config | the specification files and the assessor directory |
| `gate.ratchet.yaml` | `ratchet/1` | the gate, the tests, the specification, turnaround, the release statuses |
| `docs-site.ratchet.yaml` | `ratchet/1` | the documentation site and the turnaround of its workflow |
| `adapters.ratchet.yaml` | `ratchet/1` | per adapter crate: tests, time bounds, records, latency; the resource profile of the local speech-to-text adapters |
| `assessors/<name>@<version>.assessor.yaml` | `ratchet-assessor/1` | what each assessor reports |
| `records/<assessment>.evidence.json` | `ratchet-evidence/1` | one baseline per assessment that has one, read from a CI run |

Three pages say the same for a reader: this one, [adapters.md](adapters.md) and
[not-measured.md](not-measured.md).

Five assessors have a program, `inference-docs assess` (`crates/inference-docs/src/assess.rs`).
It reads a file a run left behind and writes the record of one assessment. It runs no measure.
`nextest-run@1`, `package-tests@1`, `named-tests@1` and `capability-record@1` read the JUnit
report of cargo-nextest; `capability-record@1` reads its value from the record and whether the
record's test passed from the report. `printed-duration@1` reads the output that a live test
printed. The other seven declarations have no program: they name the outputs, units
and scopes that a program would report. No declaration has a `command`.

The program takes the arguments Ratchet gives an assessor and five of its own. Every call gives
two: `--from`, the file the run left, and `--source`, what the record names as its origin. A
source is an http or https URL of a forge run, its scheme in any case, without user info before
the host and without a `..` segment in its path; or the path of a file from the root, without
`..`, without a colon (`run:1.xml` reads as a URL of another scheme and is refused) and not the
root or a directory. `--started-at` and `--ended-at` give the times of a printed output, which
holds none, and `--exit-status` the exit status of the command that printed it. Ratchet's
specification says it passes an assessor no argument beside its own and the declared inputs, so
a call through Ratchet as the specification stands cannot pass these five: the declarations
would have to name them as `inputs`, or a wrapper would have to add them.

The program does not know which nextest profile wrote a report. The caller gives it the report of
the profile the plan names as the input `profile`, and the record says that profile.

A report is refused when its root counts other tests, failures, errors or skipped tests than the
test cases it holds, or when it holds a test case outside a `<testsuite>` directly under the root.
A named test is held only by the test case whose name is the module path of its file, the inline
modules around its function in that file (`mod live {`, `mod tests {`), and the function. The
program reads those modules from the file under `--root`, so the root is the tree of the run; a
file it cannot read or that defines no function of the name is refused. A capability record
whose test failed or was skipped is refused.

A printed measure is read from the one line that holds its text, after the colour codes of the
output are taken out. Of the plans of `printed-duration@1`, `compare` and `compare-cascade` write
their values to `runs.jsonl`, the example of `elevenlabs-live-example` prints its text once per
output format, and the tests of `cascade-local-adapters` print it once per turn and per test: the
program reads none of the four, and a story of its own would add a second scope, per format or
per turn, to read them. The output of a live test says whether the run held. A passed run needs
the closing line of the run, a `test result: ok.` line with a test passed or a `Summary` line of
cargo-nextest with a test passed and nothing failed, and is written as one passed run; the `ok`
of one test is not the pass of the run, and an output without a closing line is refused as a run
that did not end. `cargo test` closes the tests of each binary with a `test result:` line of their
own: a `running <n> tests` line without one after it is refused as a run that did not end, and the
pass is read from the `test result:` line of the tests that printed the measures. A failed test, a `test result: FAILED` line, cargo's `error: test failed` line,
a panic of a test the output names (`thread '<name>' (<id>) panicked at`, as the pinned rustc
prints it), a failing status line of cargo-nextest (`FAIL`, `TMT`, also after `TRY <n>`) or a
count of its `Summary` line such as `failed` or `timed out` is refused. `cascade-talk`, an
example, is not read: its run ends by Ctrl-C, and the example exits 0 also when its run ended
Failed, so neither its output nor its exit status tells a completed run from a failed one. The
program exits 2 for it. A line the example printed only for a run that completed would make it
readable. The call for the record of `workspace-tests`:

```sh
cargo run -q -p inference-docs -- assess --project inference --assessment workspace-tests \
  --root . --class ci --out workspace-tests.evidence.json --revision <commit> \
  --from target/nextest/default/junit.xml --source <URL of the check run>
```

It exits 0 with the record at `--out`. It exits 1 and writes no record when the file does not
hold what the assessment reads, with one line on stderr for each thing it lacks. It exits 2 for
a wrong call, and 3 when it cannot read a plan file or a declaration, or cannot write the
record. `task test-run` keeps the report of its workspace part at
`target/nextest/default/junit.xml`: the stack test after it runs under the nextest profile
`stack`.

The records under `records/` are still the ones written by hand from the CI runs named below.

## The records of a check run

`task quality-records`, which `task check` runs after `task test-run`, writes the record of each
assessment whose record is read from the report of `task test-run`. The list comes from the plan
files: `inference-docs junit-assessments` prints one line per assessment whose assessor reads the
JUnit report, whose observation runs `task test-run` and whose input `profile`, if it names one,
is `default`: its plan, its key and the kind of its target. The slow lane is not on it: its
report is the one of `task test-slow`. The task calls `inference-docs assess` once per line with
the report at `target/nextest/default/junit.xml` and the commit, and writes each record to
`target/quality-records/<plan>/<assessment>.evidence.json`. On the forge the record names the
run, with the class `ci`; elsewhere it names the report, with the class `local`. The task exits
non-zero and names every assessment whose record the program refused.

The workflow `check` runs the task in the full gate, on a pull request and on a push to `main`.
On a push to `main` it uploads `target/quality-records/` as one artifact named
`quality-records-<commit>`, the full commit id. To get the records of a run on `main`:

```sh
gh run download <run id> -R beyond10x/inference -n quality-records-<commit>
```

The forge keeps an artifact for the retention time of the repository; the series below keeps the
records past it.

## The series of records on `main`

The workflow `quality series` (`.github/workflows/quality-series.yml`) keeps the records of every
check run on `main` as a series on the ref `refs/ratchet/records`, outside the branches of the
repository. It runs when a run of `check` for a push to `main` ended with success, one run at a
time: its concurrency group never cancels a running run, and a run that starts while another
runs waits. The forge keeps one waiting run per group: a third run replaces the waiting one,
whose records are then not appended. It has two jobs, and each trusts different code:

- `append` runs the code of the tree. It checks out the commit of the check run, the run's head
  commit and not the head of `main`, without keeping credentials, and builds `inference-docs`
  from it. It downloads the artifact `quality-records-<commit>` of the check run with
  `gh run download`, fetches the ref without a token (the repository is public) or starts the
  series as a commit with no parent when the ref does not exist, runs
  `inference-docs series-append`, which writes each record that is not in the series yet, and
  commits what it added as `github-actions[bot]`, with a message that names the check run and the
  commit. Its permissions are `contents: read` and `actions: read`; the token reaches only the
  step that downloads the artifact, never the step that runs the program built from the tree. It
  hands the commit on as job outputs: a `git bundle` of it in base64, and the commit of the ref it
  was made on. A bundle over 120000 bytes in base64 fails the job.
- `push` runs no code of the tree: no checkout, no action, no build, no task, no program or
  script of the repository, only `git` and the runner's tools. It is the one writer of the
  series and the only job of any workflow with `contents: write`. It fetches the ref without a
  token, and pushes nothing when the ref is no longer at the commit `append` worked on, when the
  bundle is not one commit on that commit, or when that commit changes or removes a file or adds
  anything but `<plan>/<assessment>/<64 hex digits>.evidence.json`. Otherwise it pushes the
  commit to the ref without force. The token is in the environment of `git push` alone.

A run whose records are all in the series already runs no `push`. A push the forge refuses, also
because another writer moved the ref in between, fails the job, and its log names the refusal;
nothing is pushed again. The records of that run reach the series only when its workflow run is
run again: its job `append` then appends them to the ref as it is at that time. The ref is
unproven on the forge: until the first run of the workflow after its merge, no series exists,
and that run shows whether the forge lets the job's token create a ref outside the branches. A
refused push is not worked around.

The series holds one file per record, `<plan>/<assessment>/<SHA-256 of its bytes>.evidence.json`,
byte for byte as `task quality-records` wrote it. A record whose bytes are in the series already
is not added again, and no file of the series is changed or removed. `inference-docs
series-append --from <dir> --series <checkout> --root .` refuses, writes nothing and names the
file and the reason on stderr for each of these:

- a file or directory of the artifact that is not `<plan>/<assessment>.evidence.json` of an
  assessment `inference-docs junit-assessments` lists, or a link;
- a record that is not JSON, or whose `format` is not `ratchet-evidence/1`, whose project is not
  the plan files' or whose assessment is not the one its name says;
- a file of the series at the path of a record that holds other bytes, or a link or a directory
  there.

It exits 0 with one line per record, `added` or `in the series already`, 1 with the refusals, 2
for a wrong call, and 3 when a plan file cannot be read or a record cannot be written; the
records that call wrote are then removed again.

To read the series:

```sh
task quality-history ASSESSMENT=workspace-tests MEASURE=slowest-test
```

The task fetches the ref from `origin` into `FETCH_HEAD` alone, so it writes no ref into the git
directory, writes the tree of `FETCH_HEAD` to `target/quality-series` and runs `inference-docs history --series target/quality-series
--root . <assessment> [<measurement>]`. Without `MEASURE` it prints every measurement of the
assessment. It prints one line per record, ordered by the id of the check run its source names,
which the forge counts up: the run, the revision, and each value as `<measurement>=<value>
<unit>`, with `[<package>]` after the name for a measurement with one value per package. An
assertion prints `holds` or `fails`. An assessment with no record in the series prints no line
and says so on stderr. The program exits 2 for an assessment or a measurement the plan files do
not declare, and 3 when a record of the series cannot be read.

## Adopting the records of a run on `main`

`inference-docs adopt-records` copies the records of one check run on `main` into `records/`,
byte for byte, after it has checked all of them. It downloads nothing: the caller saves what
the forge says of the run and downloads the artifact, and the task runs the program on both:

```sh
gh run view <run id> -R beyond10x/inference --json databaseId,headBranch,headSha,event,workflowName > run.json
gh run download <run id> -R beyond10x/inference -n quality-records-<commit> --dir <dir>
task quality-adopt RUN=<run id> DIR=<dir> RUN_JSON=run.json
```

The task calls `cargo run -q -p inference-docs -- adopt-records --run <run id> --run-json
run.json --from <dir> --root .`. `<dir>` holds `<plan>/<assessment>.evidence.json`, as
`task quality-records` wrote it. The program refuses, writes nothing and names the file and the
reason on stderr for each of these:

- `run.json` does not describe the run `--run`, or the run is not a push (`event`) to `main`
  (`headBranch`) of the workflow `check` (`workflowName`), or its `headSha` is not a full commit
  id;
- a record names another run than `--run` as its source, or another commit than `headSha` as
  its revision and candidate; a record of a document names another file than its plan, or
  another digest than the file of the tree has;
- a record of an assessment `inference-docs junit-assessments` lists is missing, or the
  directory holds any other file or directory;
- a record or `run.json` is not JSON, or one of its objects holds a key twice: the program would
  read the last value, another reader of the same bytes may read the first;
- a record is of another project than the plan files, of another assessor than its assessment,
  or of another class than `ci`, or its target is not the one `inference-docs assess` writes for
  its plan and the run: its kind, class, selection, revision, or for a document its path, digest
  and the digest form `bytes`, and no other field, so a revision names no file;
- `inference-docs assess` would refuse its source, or the source is not the run on the forge;
- a rule of a record that the test of the quality documents holds refuses it: it does not parse,
  its format, project, file name, assessor, outputs, units or coverage, a result for each measure
  of its plan and for each package of a measure with one value per package, its packages, or its
  results of the listed tests.

It exits 0 with every record written, 1 with the refusals, 2 for a wrong call, and 3 when it
cannot read a plan file, an assessor declaration or the file of the tree a document target
names, or cannot write a record. It writes each record to a temporary file first and then
renames each over its record; it removes whatever stands at the path of a temporary file and
creates the file new, so a link there is never written through. When a write fails it removes
the temporary files and puts back the records it had replaced, so `records/` holds all the
records of the run or none. The records the artifact does not hold stay as they are. The pages
name the values of the records; after an adoption they change with it, and the test below fails
until they do.

## The keyed live lane on this machine

`task quality-live` runs the live measures of OpenAI and ElevenLabs on this machine, with
`inference-docs live` (`crates/inference-docs/src/live.rs`). It is outside `task check` and
outside every workflow, and nothing schedules it.

Each run spends money, and each run needs the owner's yes to its count of hosted calls, the
first run and every later one. The lane prints the count before it starts anything. Without
`YES=1` it stops there and starts no build, no command and no `secret-tool`. The program exits
2; `task` reports that as a failed task and exits 201, and `task -x quality-live` passes the 2
through:

```sh
task quality-live          # prints the count, calls nothing
YES=1 task quality-live    # only after the owner's yes to that count
```

What one run spends, each call billed by its provider:

| Provider | Key: `username` in the Secret Service, service `b10x-secrets` | Measure | Hosted calls |
| --- | --- | --- | --- |
| OpenAI | `default/default/default/openai-api-key` | `openai-speech-live` | 1 request |
| OpenAI | the same | `openai-gpt-live` | 3 sessions, billed per second of session |
| ElevenLabs | `default/default/default/elevenlabs-api-key` | `elevenlabs-live-example` | 2 connections |
| ElevenLabs | the same | `elevenlabs-cancel-probe` | 4 connections |

A key stays on this machine, so the lane builds and runs each measure apart. The build goes to
the build pool as plain `cargo`, without either key variable. The run is the executable that
cargo's JSON names for the `--test` or `--example` of the plan's command, started here with
`slot --local`, which keeps the environment it is given and never sends the run to a build
node. The executable gets the arguments the plan's command gives after `--`, and starts where
cargo starts it: a test in its package's directory, the example in the root. From the commands
of `adapters.ratchet.yaml` the lane builds:

```sh
cargo test --no-run --message-format=json-render-diagnostics -p inference-openai --features live,speech --test speech_live
cargo test --no-run --message-format=json-render-diagnostics -p inference-openai --features live,gpt-live --test live_gpt
cargo build --message-format=json-render-diagnostics -p inference-elevenlabs --features live --example live
cargo test --no-run --message-format=json-render-diagnostics -p inference-elevenlabs --features live --test tts_cancel_live
```

and runs `slot --local <executable> --ignored --nocapture` for each of the three tests and
`slot --local <executable>` for the example.

- The lane reads each key with `secret-tool lookup service b10x-secrets username <key>`, once
  per run and only for a provider with a measure to run, into the environment of that
  provider's runs only: `OPENAI_API_KEY` and `INFERENCE_OPENAI_KEY_SOURCE=env` for OpenAI,
  `ELEVENLABS_API_KEY` for ElevenLabs. Every run starts without the other provider's variable,
  and no build gets either.
- `secret-tool` has 10 s to answer (`--key-timeout-ms` of the program names another bound);
  past it the lane ends it.
- No key is printed, passed as an argument or written to a file. A line of a run's output that
  holds its key is kept with `[key]` in its place, and the lane says so.
- A key that is missing or empty, a `secret-tool` that cannot be started and one that does not
  answer in time skip the provider's measures with a line that names the key and the measures.
  The other measures run.
- `ELEVENLABS_VOICE_ID` comes from the environment. Both ElevenLabs commands read it, so without
  it both are skipped by name, their key is not read, and the count says so.
- A build pool that builds on a node and copies the executables of a build back leaves the
  executable here. One that does not, but can copy a path back after a job, is asked to: for an
  executable under `target/` of the root that is not here, the lane runs one job
  `slot --fetch <its path from the root> true`, without a key, before the run.
- A build that fails, or that names an executable that is still not on this machine, runs
  nothing, and its measure counts as failed.
- The output of each run, standard output and error in one file, is kept at
  `.ratchet/runs/<run id>/<assessment>.out`. Its record is written from that file by
  `inference-docs assess --class local` to
  `.ratchet/records/<plan>/<assessment>/<run id>.evidence.json`. The run id is the time of the
  start in UTC, without `-` and `:`; a later run in the same second adds `-02` to `-99`, so
  that the ids sort in the order of the runs.
- `inference-docs assess` reads no record of `elevenlabs-live-example`: its example prints the
  measure once per output format (see above). The lane runs it, keeps its output and writes no
  record of it, and the count says so.
- Nothing the lane writes is committed: `.ratchet/` is in `.gitignore`. A local value is not a
  baseline, and the entries on [adapters.md](adapters.md) keep theirs.
- The program exits 0 when every measure that ran gave its record (the example: exited 0), 1
  when one did not, with the measure and why, 2 without `YES=1`, and 3 when a plan file cannot
  be read.
- The tests in `crates/inference-docs/tests/live/` hold the lane without a hosted call, without
  a build and without a key: fakes of `secret-tool`, `cargo` and `slot` first on `PATH`, and
  stubs as the executables the fake `cargo` names.

## The test that keeps the documents true

`crates/inference-docs/tests/quality/`, part of `task test-run`: `mod.rs`, for the rules about
adapters, tests and commands `adapters.rs`, for the program `assess.rs`, and for
`task quality-records`, its list and its commands, `records.rs`, and for the series
`series.rs`. It is not `ratchet validate`. It fails when:

- a file stops parsing, has another format or names another project, a name is declared in two
  files, an assessor declaration or a record is not in the file named after it, or
  `ratchet.toml` does not list the specification files and the assessor directory;
- an assessment names a subject or an assessor that is not declared, a measurement names an
  output its assessor does not declare or a boundary its assessor does not declare for that
  output, a requirement names an assessment or a measurement that is not declared, or a gate
  requires a requirement that is not declared;
- a record names an assessment that is not declared, an assessor other than the one of its
  assessment, or an output that assessor does not declare;
- the coverage of a record holds another number than 0 for a count its assessor does not
  report;
- an assessment names packages as its input, and its record has `tests_run` results for
  another set of packages, or its coverage counts another number of executed tests than the
  `tests_run` results sum to, or of passed tests than the `tests_passed` results sum to;
- the unit of a measurement, of the condition of a requirement or of a result in a record
  differs from the unit the assessor declares for the output;
- a command calls a task `Taskfile.yml` does not have;
- a command calls no task, stands in no `cmds` list of `Taskfile.yml` and names no test or
  example of a package, unless it is the one command below, or that list names a command that
  is not such a one;
- a `cargo test` command that names a package with `-p` and a test with `--test`, or a
  `cargo run` command that names a package and an example with `--example`, in the YAML or in a
  code span of one of the three pages, names a package that is no member of the workspace, a
  test or example whose file does not exist, or a feature the `Cargo.toml` of the package does
  not declare, or it leaves out a feature the target requires;
- a measure has no entry on this page or on [adapters.md](adapters.md), an entry has no measure
  or stands twice, or an entry has no `Baseline:` line that names a run of the table below or a
  file of the tree;
- an entry names a run, and `records/` has no record of that run for its assessment with a value
  for the measure;
- a baseline or a table differs from the values in the record, or a bound differs from the
  number in the requirement;
- an entry says `Baseline: none.` in other words than `none. No run of this command is kept.`,
  or a record holds a value for its measure, or a requirement bounds its measure;
- anything under the heading of such an entry, up to the next heading, holds a digit: a line of
  the entry other than `Command:` and `Needs:`, a bullet without a name, a paragraph, a row of
  a table;
- a directory under `adapters/` with a `Cargo.toml` has no section on
  [adapters.md](adapters.md), a section names no such directory or stands twice, or the sections
  are not in the order of the directories;
- a section does not name a file committed under `records/` or `capability/` of its adapter;
- the table of a section lacks a row for a measure with one value per package, or a row differs
  from the record: the value, the unit, the test, or the run it was read from;
- one of the three pages names a test, by its function and its file, and the file does not
  exist or has no line that starts a function of that name: `fn`, the name and `(`, with
  nothing before them but `pub`, `async`, `const` or `unsafe`. The name in a comment or in a
  string does not count;
- an assessment lists tests as its input, and one is not in its file, the entry of the measure
  names another set of tests, or the record has another set of results;
- an entry on [adapters.md](adapters.md) does not give the command of its assessment, whole, in
  its `Command:` line;
- `inference-docs assess`, run on the fixtures under
  `crates/inference-docs/tests/fixtures/assess/`, writes another record than the expected file,
  writes a record that the rules above refuse in the place of a committed one, reads a printed
  measure by a text that no `println!`, `eprintln!`, `print!`, `format!`, `write!` or
  `writeln!` of the source of its command writes in one string literal, with the text its value
  follows after it and a `{...}` placeholder right after that text, past spaces only, or writes
  a record from a file that lacks what the assessment reads. That check reads the first string
  literal of each such macro outside comments; it does not read a literal built by `concat!` or
  a format string held in a variable, and it does not know that the macro runs or what the
  placeholder prints;
- a record written as `task quality-records` writes it, with the class `ci` and the URL of a
  forge run or the class `local` and the path of the report, and the commit of another run,
  breaks a rule above that holds a record by itself or against its plan, in the place of the
  committed record: it does not parse, its format, project, file name, assessor, outputs, units
  or coverage, a result for each measure of its plan and for each package of a measure with one
  value per package, its packages, or its results of the listed tests. The baselines, tables and
  runs the pages state are not held against such a record: a new run changes them;
- `inference-docs adopt-records`, run on the fixture artifact under
  `crates/inference-docs/tests/fixtures/adopt/` (the committed records of the listed
  assessments with another run and commit, and its `run.json`), does not write each record of
  it byte for byte and leave the others, writes records that the rules of a record above refuse,
  writes a record on any refusal of the section above, leaves a record changed when a write
  fails, writes through a link at the path of a temporary file, or does not refuse, in the words
  of this test, a record in which one rule of a record above is broken (two plants for a rule a
  copy could narrow: a missing value besides a wrong one, a second count, the other sum);
- the commands of `task quality-records`, run with `sh` and stand-ins of `cargo` and `git`, do
  not call the program for the list and then once per listed assessment from the report of
  `task test-run`, ask `git` for anything but `rev-parse HEAD`, give `--revision` to a target
  that is no revision, give the class and source
  of the forge run without its variables or the report's with them, keep a record of an earlier
  run, or exit 0 with a refused record or an empty list;
- `inference-docs junit-assessments` lists an assessment of another profile or of an assessor
  that reads no report, or leaves out one of a reader of the report that names no profile;
- `inference-docs series-append`, run on the fixture artifact under
  `crates/inference-docs/tests/fixtures/adopt/` and a series in a directory, does not write each
  record byte for byte under the digest of its bytes, adds a record a second time, changes a file
  of the series, adds more than the record whose bytes changed, or writes anything on a refusal
  of the section "The series of records on `main`" (nine plants in the artifact and a link there
  to a record of its own assessment; a file of other bytes, a link at the path of a record and a
  link where a directory of records stands in the series), or leaves a record of a call whose
  record could not be written;
- `inference-docs history` does not list the values of a measurement oldest run first, records
  of one run by their start and a record whose source names no forge run last, does not
  name the package of each value of a measurement with one value per package, prints a line for
  an assessment with no record, or does not exit 2 for an assessment or a measurement the plan
  files do not declare.

The workflow `quality series` is held by `crates/inference-docs/tests/workflows.rs`, which fails
when it is triggered otherwise than by a completed run of `check` for a push to `main` that
succeeded; when it has other jobs than `append` and then `push`, when `append` has other
permissions than `contents: read` and `actions: read`, `push` others than `contents: write`, or
any other job of any workflow grants `contents: write`, or a workflow sets no permissions; when
a step of `append` other than the download, or the env of a job or of the workflow, holds the
token; when the checkout of `append` names another commit than the head of the check run or
keeps credentials; when a step of `push` uses an action, runs in a directory of the tree, or
runs a build, a task, the program built from the tree or a script; when `push` does not need
`append` or runs on another condition than its output `appended`; when its concurrency cancels
a run; when it takes an action that `check.yml` does not pin by its commit; when it names the
ref other than once; or when its steps, run with `sh` against a bare repository in a directory
and stand-ins of `gh` and `git`, do not start the series as a commit with no parent, push a
commit for records the series holds, add more than one file for one changed record, fail
without pushing again after another writer moved the ref, push a commit that changes, removes
or adds anything but a record, exit 0 after a refused push or a refusal of `series-append`,
print the token, or hand the token to the program built from the tree.

One command stands in no `cmds` list of `Taskfile.yml` and names no test or example of a
package, so the test holds it against nothing. A change of the workflow step that runs it goes
unnoticed here:

- `npx playwright test --workers=2`

The test reads no CI run. That a record equals its run was checked by hand. A baseline without
a record is held only to name a file that exists: the test does not read the number in that
file. A bound without a number ("must hold", "recorded, no bound") is not held against
anything. Of the other lines of an entry, the test holds the tests and the commands they name,
and nothing else: not the sentence for a user, not the value of a constant, not what a line
says a test compares, not the clock of a latency, not whether a command is billed. That a named
test is ignored, or runs in `task test-run`, is not held either.

More that the test does not hold:

- The search for a named test takes any function of the name: one without `#[test]`, one with
  `#[ignore]`, and one in a file behind a `#![cfg(feature = "...")]` that the gate does not
  turn on. A line of a block comment, or of a string over several lines, that starts like the
  function is taken for it.
- A number written in words under an entry that says `Baseline: none.`. The test finds digits
  only.
- The coverage of a record beyond the two rules above: not that a count is the count of the
  run.

## The runs the baselines were read from

All ran on 2026-11-24. The first three ran on `main` at
`59b8f4a858ab9c4eef94bc21de8e7bb85f90ddad`.

| Name here | Workflow | Run |
| --- | --- | --- |
| check run | `check` | [45474241015](https://github.com/beyond10x/inference/actions/runs/45474241015) |
| slow run | `slow` | [45474240955](https://github.com/beyond10x/inference/actions/runs/45474240955) |
| gates run | `Shared source gates` | [45474241551](https://github.com/beyond10x/inference/actions/runs/45474241551) |
| docs run | `Docs UX` | [45467764733](https://github.com/beyond10x/inference/actions/runs/45467764733) |

`Docs UX` does not run on `main`. The docs run is of a pull request with the head
`adf479ee2707962b93a2c8ce6509bb53775024bb`. It checked out the merge of that head into `main`,
the commit `1ed5587d63ab46006e85cc0164e7e0ecd70ec18c`. The tree of that commit is the tree of
`59b8f4a8`: `17ea6f2eb4d9601ed4d3bd6c5e33c966dfe225f0`. The records of the docs run name the
head as the candidate and the merge commit as the revision, with the selection `merge_result`.

The values per adapter crate on [adapters.md](adapters.md) were counted in the log of the step
`task test-run` of the check run. That log has one line per test, with the result, the time in
brackets and the name of the package. This counts the tests of one package, here
`inference-aws`, in a log saved as the file `$log`:

```sh
sed -E 's/(\x1b|\^\[)\[[0-9;]*m//g' "$log" |
  grep -cE '\( *[0-9]+/3059\) +inference-aws(::[^ ]+)? '
```

The same lines that say `PASS` are the tests that passed, and the longest time on them is the
slowest test.

## How to read an entry

The heading is the measure: `<assessment>/<measurement>`, as the YAML names it. "Holds" means
the measure is an assertion: it has no unit, and its direction is that it must hold. "Recorded,
no bound" means nothing fails when the value gets worse. The baseline ends with its source: a
run of the table above, or a file of the tree.

A test is named by the word "test", its function in a code span, the word "of", and its file
in a code span, as a path from the root of the repository. An entry that says
`Baseline: none. No run of this command is kept.` defines a measure that has no value yet.
## Source

### `formatting/clean`

- For a user: the code reads the same everywhere, so a patch shows only what it changes.
- Command: `cargo fmt --all --check`, a step of `task check-source`.
- Unit and direction: none; holds.
- Bound: must hold. `task check-source` fails otherwise.
- Baseline: holds. Check run.

### `lint/clean`

- For a user: no code that Clippy warns about ships in a release.
- Command: `cargo clippy --workspace --all-targets --all-features -- -D warnings`, a step of
  `task check-source`.
- Unit and direction: none; holds.
- Bound: must hold. `-D warnings` makes every warning an error.
- Baseline: holds. Check run.

### `doc-build/clean`

- For a user: the API documentation builds and none of its links is broken.
- Command: `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps`, a step of
  `task check-source`.
- Unit and direction: none; holds.
- Bound: must hold. `-D warnings` makes every warning an error.
- Baseline: holds. Check run.

### `generated-docs/current`

- For a user: the reference pages of the site say what the code of the same commit does.
- Command: `task docs-check`.
- Unit and direction: none; holds, once per generated file.
- Bound: must hold for every file. `task check-source` fails otherwise.
- Baseline: holds for 6 of 6 files. Check run.

### `planning-store/valid`

- For a user: the stories and decisions behind a release can be read and are consistent.
- Command: `aep plan artifact validate`, a step of `task check-source`.
- Unit and direction: none; holds.
- Bound: must hold. `task check-source` fails otherwise.
- Baseline: holds. Check run.

### `source-scan/passed`

- For a user: no secret and no private detail is in the history of the public repository.
- Command: the workflow `.github/workflows/shared-gates.yml`, job `common / Security and
  privacy`. It calls a workflow of `beyond10x/gates`. No task of this repository runs it.
- Unit and direction: none; holds.
- Bound: must hold. The job fails otherwise.
- Baseline: holds. Gates run.

## Specification and conformance

### `ess-specification/valid`

- For a user: the behaviour the library promises is written down in a form a tool can check.
- Command: `ess specify validate --path ess`, a step of `task check-source`.
- Unit and direction: none; holds.
- Bound: must hold. `task check-source` fails otherwise.
- Baseline: holds, over 11 files. Check run.

### `ess-specification/scenarios`

- For a user: how many scenarios were written by hand beside those derived from the rules.
- Command: `ess specify validate --path ess`, which prints the count.
- Unit and direction: scenarios; higher is better.
- Bound: recorded, no bound.
- Baseline: 10 scenarios. Check run.

### `conformance/passing-scenarios`

- For a user: each promised behaviour has a scenario, and the library passes it.
- Command: `cargo test -p inference-testkit --features run --test conform`, part of
  `task test-run`. It writes `report.json` under `target/tmp/conform-<run>/`.
- Unit and direction: none, it is a set of scenario ids; no id may leave the set.
- Bound: every id that `crates/inference-testkit/tests/conform.rs` names must pass, and none may
  fail. The test fails otherwise.
- Baseline: the 472 ids named in `crates/inference-testkit/tests/conform.rs`, with no skipped id
  in `ess/SKIPPED.md`.

The test that holds those ids passed in the check run. The total the suite executed is not in
that run: see [not-measured.md](not-measured.md). There is no record for this assessment, so the
baseline names the two files and no run.

### `native-archive/checksum-matches`

- For a user: the native library a local adapter links is the one that was reviewed.
- Command: `task sherpa-onnx-archive`.
- Unit and direction: none; holds.
- Bound: must hold. The task fails when the sha256 of the download differs from the pin in
  `Taskfile.yml`.
- Baseline: holds, against the pin
  `bf2d998c8b07012cd5098f3b92673bc1333fd9b927767d7cb664be8190d8bc0b`. Check run.

## Tests

### `workspace-tests/slowest-test`

- For a user: the tests are quick enough that every change runs all of them.
- Command: `task test-run`, the part that runs `cargo nextest run` with the default profile.
- Unit and direction: seconds; lower is better.
- Bound: under 60 s. `.config/nextest.toml` ends a test at 60 s and counts it as failed.
- Baseline: 8.011 s. Check run.

The same run gives the coverage of this assessment, which the record holds: 3059 tests run,
3059 passed. Recorded, no bound. The `Summary` line of the run's log also says 72 tests skipped.
No record holds that number: cargo-nextest writes no skipped test into its JUnit report, so
`nextest-run@1` does not report `skipped`, and its record has 0 there, as Ratchet's format says
for a count an assessor does not report. `task test-run` has two more parts that this
assessment does not cover: the stack test (23 tests run, 23 passed) and the doc tests (38
passed). Check run.

The tests run and passed and the slowest test of each adapter crate, from the same part of the
same run, are on [adapters.md](adapters.md).

### `slow-lane/slowest-test`

- For a user: a wrong use of a macro of the library gives the compile error the tests pin.
- Command: `task test-slow`.
- Unit and direction: seconds; lower is better.
- Bound: under 600 s. The `slow` profile of `.config/nextest.toml` ends a test after ten periods
  of 60 s.
- Baseline: 26.703 s, with 1 test run and 1 passed. Slow run.

The slow lane does not run for every change. On a workstation `task test-slow` runs only when
one of the nine `sources` of the task in `Taskfile.yml` changed since its last run. The workflow
`.github/workflows/slow.yml` runs it for a change under the same nine paths, and for a change of
`Taskfile.yml` or of the workflow file itself. `task check` does not run it. The gate `slow` of
the YAML carries the list of the workflow and qualifies runs in CI only. The list of the task is
not in the YAML: it is compared with the last local run, not with the change.

## Turnaround

### `test-run-step/run-time`

- For a user: a fix reaches a release sooner when the test step is short.
- Command: `task test-run`.
- Unit and direction: seconds; lower is better.
- Bound: under 120 s. `Taskfile.yml` ends the step at 120 s and fails it.
- Baseline: 101 s. Check run, step `task test-run`.

### `check-source-step/run-time`

- For a user: none directly; it is part of the time a change waits for its verdict.
- Command: `task check-source`.
- Unit and direction: seconds; lower is better.
- Bound: recorded, no bound.
- Baseline: 224 s. Check run, step `task check-source`.

### `test-build-step/run-time`

- For a user: none directly; it is part of the time a change waits for its verdict.
- Command: `task test-build`.
- Unit and direction: seconds; lower is better.
- Bound: recorded, no bound.
- Baseline: 233 s. Check run, step `task test-build`.

### `check-workflow/run-time`

- For a user: none directly; it is the time a change waits for its verdict once a runner has it.
- Command: the workflow `.github/workflows/check.yml`, job `inference / task check`.
- Unit and direction: seconds; lower is better.
- Bound: under 2700 s. The job has `timeout-minutes: 45`.
- Baseline: 589 s. Check run.

### `check-workflow/queue-wait`

- For a user: none directly; it is the time a change waits for a runner.
- Command: the workflow `.github/workflows/check.yml`, job `inference / task check`.
- Unit and direction: seconds; lower is better.
- Bound: recorded, no bound.
- Baseline: 284 s, from the creation of the run to the start of the job. Check run.

### `slow-lane-step/run-time`

- For a user: none directly; it is part of the time a change of a macro waits for its verdict.
- Command: `task test-slow`.
- Unit and direction: seconds; lower is better.
- Bound: recorded, no bound of its own. Neither the task nor its step in
  `.github/workflows/slow.yml` has a limit. What ends a long run is the bound of each test and
  the timeout of the job.
- Baseline: 76 s. Slow run, step `task test-slow`.

### `slow-workflow/run-time`

- For a user: none directly; it is the time a change of a macro waits for the slow lane once a
  runner has it.
- Command: the workflow `.github/workflows/slow.yml`, job `inference / task test-slow`.
- Unit and direction: seconds; lower is better.
- Bound: under 1800 s. The job has `timeout-minutes: 30`.
- Baseline: 105 s. Slow run.

## Documentation site

Twelve measures of `docs-ux` say what a reader of the site meets. They share what follows.

- Command: `npx playwright test --workers=2` under `ux/`, then `node tools/ratchet.ts`, which
  compares with `ux/baseline.json`. No task runs them; the workflow
  `.github/workflows/docs-ux.yml` does, on a pull request that touches `website/`, `docs/` or
  `ux/`.
- Direction: lower is better.
- Bound: no value may be worse than its value in `ux/baseline.json`, and a better value must be
  written to that file in the same change. `node tools/ratchet.ts` fails otherwise.
- Baseline: each value below is the value in `ux/baseline.json`. The docs run measured the same:
  45 tests passed, 80 values, 0 problems.
- A view is a scenario at a viewport, written `<scenario>/<viewport>`. The viewports are
  1440x900 and 390x844.

The first five are measured over every page and theme of the built site, scenario `sitemap`.

### `docs-ux/horizontal-scroll`

- For a reader: no page scrolls sideways.
- Unit: pages.
- Baseline: 0 at both viewports. Docs run.

### `docs-ux/overflowing-elements`

- For a reader: nothing is cut off at the edge of the screen.
- Unit: elements.
- Baseline: 0 at both viewports. Docs run.

### `docs-ux/always-visible-misses`

- For a reader: what must always be on screen is on screen.
- Unit: misses.
- Baseline: 0 at both viewports. Docs run.

### `docs-ux/accessibility-violations`

- For a reader: a screen reader meets no serious or critical violation.
- Unit: nodes.
- Baseline: 0 at both viewports. Docs run.

### `docs-ux/keyboard-unreachable`

- For a reader: everything that can be clicked can be reached with the keyboard.
- Unit: elements.
- Baseline: 0 at both viewports. Docs run.

The other seven are measured for five tasks a reader has: find the dependency lines, the error
kinds, the limitations, the provider comparison, the variable of the realtime model.

### `docs-ux/completion-misses`

- For a reader: the task ends on the page that answers it.
- Unit: misses.
- Baseline: 0 in all ten views. Docs run.

### `docs-ux/steers`

- For a reader: the reader never has to go back a page.
- Unit: steers.
- Baseline: 0 in all ten views. Docs run.

### `docs-ux/actions`

- For a reader: how many clicks the task takes, with opening the menu and going back.
- Unit: actions.
- Baseline: the table. Docs run.

| Scenario | 1440x900 | 390x844 |
| --- | --- | --- |
| find-dependency-lines | 1 | 2 |
| find-error-kinds | 2 | 4 |
| find-limitations | 2 | 4 |
| find-provider-comparison | 1 | 2 |
| find-realtime-model-variable | 1 | 2 |

### `docs-ux/looks`

- For a reader: how often the next link is off screen and has to be scrolled to.
- Unit: looks.
- Baseline: the table. Docs run.

| Scenario | 1440x900 | 390x844 |
| --- | --- | --- |
| find-dependency-lines | 0 | 0 |
| find-error-kinds | 1 | 1 |
| find-limitations | 1 | 1 |
| find-provider-comparison | 0 | 1 |
| find-realtime-model-variable | 1 | 1 |

### `docs-ux/steps-to-act`

- For a reader: how many links the reader follows to the page that answers the task.
- Unit: steps. A value within 5 percent of the baseline counts as equal.
- Baseline: the table. Docs run.

| Scenario | 1440x900 | 390x844 |
| --- | --- | --- |
| find-dependency-lines | 1 | 1 |
| find-error-kinds | 2 | 2 |
| find-limitations | 2 | 2 |
| find-provider-comparison | 1 | 1 |
| find-realtime-model-variable | 1 | 1 |

### `docs-ux/glance-cost-median`

- For a reader: how hard the next link is to find, at the median step of the task.
- Unit: level. 1 is a link in the main navigation on screen, 2 a link elsewhere on screen, 3 a
  link off screen.
- Baseline: the table. Docs run.

| Scenario | 1440x900 | 390x844 |
| --- | --- | --- |
| find-dependency-lines | 1 | 1 |
| find-error-kinds | 1 | 1 |
| find-limitations | 1 | 1 |
| find-provider-comparison | 1 | 1 |
| find-realtime-model-variable | 3 | 1 |

### `docs-ux/glance-cost-p95`

- For a reader: the same, at the hardest step of the task.
- Unit: level. 1 is a link in the main navigation on screen, 2 a link elsewhere on screen, 3 a
  link off screen.
- Baseline: the table. Docs run.

| Scenario | 1440x900 | 390x844 |
| --- | --- | --- |
| find-dependency-lines | 1 | 1 |
| find-error-kinds | 3 | 3 |
| find-limitations | 3 | 3 |
| find-provider-comparison | 1 | 3 |
| find-realtime-model-variable | 3 | 3 |

## Documentation site: turnaround

The four measures below are the times of the same tests. Each is from the one docs run.

### `docs-ux/slowest-test`

- For a reader: none directly; the tests of the site are quick enough to run for every change
  of it.
- Command: `npx playwright test --workers=2` under `ux/`, which prints its slowest tests.
- Unit and direction: seconds; lower is better.
- Bound: under 60 s. `ux/playwright.config.ts` ends a test at 60 s (`timeout: 60_000`).
- Baseline: 4.3 s. Docs run.

### `docs-ux/run-time`

- For a reader: none directly; it is part of the time a change of the site waits for its
  verdict.
- Command: `npx playwright test --workers=2` under `ux/`, which prints the wall time of the run.
- Unit and direction: seconds; lower is better.
- Bound: under 120 s. `ux/playwright.config.ts` ends the run at 120 s (`globalTimeout`). With
  `UX_ROUND` set the limit of that file is 900 s; the workflow does not set it.
- Baseline: 48.0 s. Docs run.

### `docs-ux-step/run-time`

- For a reader: none directly; it is part of the time a change of the site waits for its
  verdict.
- Command: the step of `.github/workflows/docs-ux.yml` that runs
  `npx playwright test --workers=2`.
- Unit and direction: seconds; lower is better.
- Bound: under 120 s. The step has `timeout-minutes: 2`.
- Baseline: 49 s. Docs run.

### `docs-ux-workflow/run-time`

- For a reader: none directly; it is the time a change of the site waits for its verdict once a
  runner has it.
- Command: the workflow `.github/workflows/docs-ux.yml`, job `Docs UX gates`.
- Unit and direction: seconds; lower is better.
- Bound: under 1200 s. The job has `timeout-minutes: 20`.
- Baseline: 133 s. Docs run.

## Local speech-to-text adapters

Six values are not from a CI run. Each is the result of an ignored test that a person runs on
the reference machine and copies into the capability record of the adapter. The record names the
machine. The test repeats the measurement three times and prints the median. There is no
evidence record for these six: see [not-measured.md](not-measured.md).

The two commands, each with the release profile:

- sherpa-onnx: `INFERENCE_SHERPA_ONNX_STT_DIR=<dir> cargo test --release -p
  inference-sherpa-onnx --all-features --test stt_resources -- --ignored --nocapture`. It needs
  the pinned checkpoint unpacked in that directory.
- whisper.cpp: `INFERENCE_WHISPERCPP_STT_MODEL=<ggml-base.en.bin>
  INFERENCE_WHISPERCPP_STT_CLIP=<0.wav> cargo test --release -p inference-whispercpp --test
  stt_resources -- --ignored --nocapture`. It needs the pinned model file and the speech clip.

The conditions of the two plans are the ones the capability records state: the reference
machine, execution on the CPU, one onnxruntime thread for sherpa-onnx, and whisper.cpp 1.8.3 on
4 threads.

The real-time factor has a second measure per adapter, `<adapter>-stt-record/real-time-factor`.
It is the value as committed, and every `task test-run` holds its bound.

### `sherpa-onnx-stt-resources/real-time-factor`

- For a user: recognition on a CPU keeps up with speech.
- Unit and direction: thousandths of real time; lower is better.
- Bound: under 1000. The ignored test asserts it when it is run. Between two runs of it, the
  bound is held on the committed value: `sherpa-onnx-stt-record/real-time-factor`.
- Baseline: 131. `adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json`.

### `sherpa-onnx-stt-record/real-time-factor`

- For a user: the real-time factor the repository states for the adapter is one at which
  recognition keeps up with speech.
- Command: `task test-run`, the test `the_resource_profile_is_a_measured_cpu_profile` of
  `adapters/inference-sherpa-onnx/tests/stt_record.rs`. It reads the committed record and
  measures nothing.
- Unit and direction: thousandths of real time; lower is better.
- Bound: under 1000 and at least 1. The test fails otherwise, in every `task test-run`.
- Baseline: 131. Check run, which read
  `adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json`.

### `sherpa-onnx-stt-resources/load-time`

- For a user: how long the first call waits for the model.
- Unit and direction: milliseconds; lower is better.
- Bound: recorded, no bound.
- Baseline: 4937 ms. `adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json`.

### `sherpa-onnx-stt-resources/peak-memory`

- For a user: how much memory the adapter needs.
- Unit and direction: MiB; lower is better.
- Bound: recorded, no bound.
- Baseline: 363 MiB. `adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json`.

### `whispercpp-stt-resources/real-time-factor`

- For a user: recognition on a CPU keeps up with speech.
- Unit and direction: thousandths of real time; lower is better.
- Bound: under 1000. The ignored test asserts it when it is run. Between two runs of it, the
  bound is held on the committed value: `whispercpp-stt-record/real-time-factor`.
- Baseline: 202. `adapters/inference-whispercpp/records/stt-i9-10900k-cpu.json`.

### `whispercpp-stt-record/real-time-factor`

- For a user: the real-time factor the repository states for the adapter is one at which
  recognition keeps up with speech.
- Command: `task test-run`, the test `the_resource_profile_is_a_measured_cpu_profile` of
  `adapters/inference-whispercpp/tests/stt_record.rs`. It reads the committed record and
  measures nothing.
- Unit and direction: thousandths of real time; lower is better.
- Bound: under 1000 and at least 1. The test fails otherwise, in every `task test-run`.
- Baseline: 202. Check run, which read
  `adapters/inference-whispercpp/records/stt-i9-10900k-cpu.json`.

### `whispercpp-stt-resources/load-time`

- For a user: how long the first call waits for the model.
- Unit and direction: milliseconds; lower is better.
- Bound: recorded, no bound.
- Baseline: 630 ms. `adapters/inference-whispercpp/records/stt-i9-10900k-cpu.json`.

### `whispercpp-stt-resources/peak-memory`

- For a user: how much memory the adapter needs.
- Unit and direction: MiB; lower is better.
- Bound: recorded, no bound.
- Baseline: 337 MiB. `adapters/inference-whispercpp/records/stt-i9-10900k-cpu.json`.

## Adapters

The measures of each adapter crate are on [adapters.md](adapters.md), one section per crate:
tests run and passed, the slowest test, the time bounds its tests hold, the records its tests
hold, and the commands that print a latency. The latency measures are defined and have no
baseline. The measures of the two local speech-to-text adapters stay on this page, in the
section above.

## A release

A release is a version tag on `main`. The commit the tag points at holds the changelog entry of
the version, the version in `Cargo.toml`, and that version as the `adapter_revision` of every
record under `adapters/`. `AGENTS.md` has the rule.

No release has a gate of its own in the YAML. A gate of the formats cannot say that it judges
the commit a tag points at, so a gate `release` is left out: see
[not-measured.md](not-measured.md). What a release has to hold is what every commit on `main`
has to hold:

- everything the gate `check` requires, and, for the paths they name, the gates `slow` and
  `docs-ux`;
- the records of each adapter fit the code and carry the version of the workspace: the eight
  `<adapter>-records/held` measures on [adapters.md](adapters.md), the tests the sections of
  the two local speech-to-text adapters name there, and `generated-docs/current`, whose command
  refuses a record of another version;
- the statuses of the documentation agree with the changelog: the measure below.

The gate `check` requires the last two already. Of the workflows, only
`.github/workflows/shared-gates.yml` runs for a tag. Every other check ran on the commit before
it was tagged.

### `release-statuses/held`

- For a user: a page that says "Since" a version names the release that shipped the thing, and
  a page that says "On main" names something no release has yet.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 4 of 4 tests. Check run.
- Bindings against the changelog: the test
  `every_binding_is_since_the_release_whose_changelog_section_introduced_it` of
  `crates/inference-docs/src/adapters.rs` holds that each binding of the adapters page names
  the release whose section of `CHANGELOG.md` first has its introducing phrase.
- On main: the test `a_binding_is_on_main_only_while_its_entry_is_unreleased` of
  `crates/inference-docs/src/adapters.rs` holds, on a changelog of its own, that the rule
  refuses a binding marked for the next release whose phrase is in a released section.
- Status items: the test
  `every_hosted_binding_is_a_shipped_item_since_the_release_of_its_adapter_row` of
  `crates/inference-docs/src/status.rs` holds that each hosted binding since a release has a
  shipped item of the status page that names that release.
- Next items: the test
  `a_hosted_binding_on_main_is_a_next_item_and_one_since_a_release_a_shipped_item` of
  `crates/inference-docs/src/status.rs` holds the same rule on items of its own, in both
  directions.

## What Ratchet's first working release could judge

That release does not gate sampled measurements and does not read conditions such as the runner,
the tool versions or the host load. It has no time windows, presets, exceptions or trade-offs.

It could judge the requirements on exact measurements:

- `formatted`, `no-lint-warnings`, `no-doc-warnings`, `generated-docs-current`,
  `planning-store-valid`, `specification-valid`, `native-archive-checksum`,
  `source-scan-passes`;
- `conformance-keeps-passing`;
- the twelve requirements of the documentation site, `docs-...-not-worse`;
- the four on the committed capability records: `sherpa-onnx-stt-record-keeps-up`,
  `sherpa-onnx-stt-record-is-measured`, `whispercpp-stt-record-keeps-up`,
  `whispercpp-stt-record-is-measured`;
- the nine on the time bounds of the adapters, `<adapter>-time-bounds-held`, and the eight on
  their records, `<adapter>-records-held`;
- `release-statuses-held`.

It would have nothing to judge for the latency measures of [adapters.md](adapters.md): they are
sampled, and no requirement names them.

It would only record the requirements on sampled measurements, every one of them a duration or
a ratio of durations:

- `test-budget`, `slow-test-budget`, `test-step-budget`, `check-job-within-timeout`,
  `slow-job-within-timeout`;
- `docs-test-budget`, `docs-run-budget`, `docs-step-within-limit`, `docs-job-within-timeout`;
- `sherpa-onnx-stt-keeps-up`, `whispercpp-stt-keeps-up`.

The bounds of those eleven stay enforced where they are today: in `.config/nextest.toml`,
`Taskfile.yml`, `ux/playwright.config.ts`, the three workflows and the two ignored tests.
