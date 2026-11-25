# Not measured

What this repository does not measure today. Each entry says what is missing and what would
produce the value. No entry has a baseline. A number stands here only where a file of the tree
holds it.

The measures that have a command are in [README.md](README.md).

## No command produces it

### Latency and audio timing of a session

No benchmark and no test reports the time to the first audio of a reply, the time from the end
of speech to the reply, or the time a barge-in takes to stop the output. The workspace has no
bench target. A benchmark that plays recorded provider streams through a recipe on a named
machine and reports percentiles would produce it.

### Throughput

Nothing measures how many sessions one process or one core carries. A load test that opens
sessions against stubbed providers until a latency bound breaks would produce it.

### Line and branch coverage of the tests

No coverage tool is configured. A coverage run of the workspace's tests, such as
`cargo llvm-cov`, would produce it.

### `unsafe` code in the dependencies

The workspace forbids `unsafe` in its own crates: `Cargo.toml` sets `unsafe_code = "forbid"` and
every crate under `crates/`, `adapters/` and `examples/` inherits it. The build fails on a use,
so this is part of `lint/clean`; nothing counts it. The `unsafe` blocks of the dependencies are
not counted. A tool such as `cargo geiger` over `Cargo.lock` would produce the count.

### Dependency audit: advisories and licences

No step checks `Cargo.lock` against an advisory database or the licences of the dependencies
against a list. `cargo deny check` with a `deny.toml` would produce both as sets of findings.

### The licence of each library a native archive links

`task sherpa-onnx-archive` pins the archive by its sha256 and checks nothing inside it. No file
lists the shared libraries of the archive and their licences. A manifest of the archive, written
when the pin changes and compared in the gate, would produce it.

### Every model call counted once in usage, per recipe

Two tests hold parts of it. `every_model_node_of_a_built_in_recipe_sends_its_usage_to_an_egress`
in `crates/inference/tests/recipes.rs` holds the wiring for the built-in recipes.
`a_completed_cascade_turn_reports_the_usage_of_each_model_call` counts the calls of one cascade
turn. No command reports, per recipe, the number of model calls beside the number of usage
records. One conformance scenario per recipe that compares the two would produce it.

### Breaking changes named per release

`CHANGELOG.md` has a `Breaking` section per release, written by hand. No command compares the
public API of a release with the one before. `cargo semver-checks` against the last tag would
produce the set of breaking changes to compare with the section.

### Flaky tests under host load

`.config/nextest.toml` configures no retry, so a test that fails once fails the step and is not
counted as flaky. No run records the load of its host. Retries in nextest, with the flaky count
read from `junit.xml` and the host load recorded beside it, would produce it.

### The resource profile of the llama-server adapter

`adapters/inference-llama-server/capability/reference-machine-cpu.json` holds a peak resident
memory of 1839 MiB and a load time of 13911 ms. No test measures them: the tests of the adapter
read the record and measure neither. An ignored test like `stt_resources` of the speech-to-text
adapters would produce them.

### Results of the live tests against hosted providers

The tests that call a hosted provider are ignored and billed. A person runs them; no workflow
does, and no file of the tree holds their latency, their cost or the day they last passed. A
scheduled run that writes one record per binding would produce it.

### Findings of the security and privacy scan

The job reports pass or fail. The run holds no count of findings and no list of what was
scanned. A report file of the scan, kept as an artifact of the run, would produce it.

### Warnings that are not denied

`-D warnings` covers the lints of rustc, Clippy and rustdoc. Cargo's own warnings about a
manifest, the warnings of `ess specify validate` and the notes of `aep plan artifact validate`
are printed and neither denied nor counted. A step that counts those lines, or the tools' own
flags to deny them, would produce it.

### The time of each part of `task check-source`

The run gives one time for the whole step. The times of Clippy, of the documentation build and
of the other parts are not separate. One workflow step per part, or a timing line per command
in the Taskfile, would produce them.

### The time of the gate on a workstation

No run of `task check` outside CI is recorded. A record written by the local gate at its end
would produce it.

### Turnaround over time

Each turnaround baseline is one run. No percentile of the queue wait or of the run time over a
day or a week exists. A record per run of the `check` workflow on `main`, kept as a series,
would produce it.

## The command exists, the value is not kept

### The totals of the conformance suite

`cargo test -p inference-testkit --features run --test conform` writes `report.json` with the
counts of scenarios passed, failed, in error and unsupported, and prints the counts. In CI the
file is not kept, and nextest hides the output of a passing test. Keeping `report.json` as an
artifact of the `check` workflow would give the totals a baseline.

### The number of compile-fail cases of the slow lane

`task test-slow` runs one test that compiles every case under `crates/inference/tests/ui/` and
`crates/inference/tests/compile_fail/`. It prints one line per case, and nextest hides the
output of a passing test. Printing the output of that test, or counting the case files in a
check, would produce it.

### The documentation site on `main`

The `Docs UX` workflow runs on pull requests only. The baseline in the README is from the pull
request head whose tree is the tree of `main`. A run of the workflow on `main` would give a
baseline at the commit itself.

## Measured, but not expressible in the formats at the pin

Each of these is checked or recorded by this repository today. The formats at commit
`f139eccc17a548444f0f4e1dc06520120c28739f` cannot say it, so it is not in the YAML or has no
record. No field was invented for it.

### The digest of the native archive against its pinned value

`task sherpa-onnx-archive` compares the sha256 of the download with the pin in `Taskfile.yml`. A
result of the value type `digest` compares only with a generated reference, a committed file
that a generator reproduces. A downloaded archive is not generated from the tree. The YAML
therefore holds the assertion `native-archive/checksum-matches` and not the digest.

### Tests run, passed and skipped as a value that must not get worse

The record of `workspace-tests` holds the three counts as coverage. Coverage has fixed bounds
only: a least number executed, a most number skipped. It has no direction and no accepted value,
so "never fewer tests than before" cannot be required.

### The gate that is skipped for a change of planning and prose files only

`.github/workflows/check.yml` skips the build and the tests when every changed file is a
planning or prose file. An entry of a gate can say `when: {changed: [paths]}`: it applies when
a listed path is touched. It cannot say "unless every changed path is in this list". The gate
`check` in the YAML is therefore written without the skip.

### The capability records as evidence

The values of `sherpa-onnx-stt-resources` and `whispercpp-stt-resources` are in a committed file
that a person writes after a run on the reference machine. An evidence document names where an
imported run came from with one of three kinds: a forge run, a job log of the build pool, a
JUnit file. A committed file of results is none of them, and the three samples behind each
median are not kept. These two assessments have measures in the YAML and no record.

### The conformance run as evidence

The coverage of an evidence document is six required counts. The check run proves that the
named scenarios passed and that none failed; it does not hold the number executed. A document
cannot say that a count is unknown, so `conformance` has no record.

### The viewport of a documentation measure

A value of `ux/baseline.json` belongs to a scenario and a viewport. A result has one scope item.
The records write the two as one text, `<scenario>/<viewport>`, under the scope `view`. That is
expressible, and it loses the scenario and the viewport as separate things to group by.

### The run a result of a workflow belongs to

The target of the kind `workflow_run` has no identifier for the run. The records name the run
only as free text under `evidence.source.location`.
