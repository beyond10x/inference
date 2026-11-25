# Not measured

What this repository does not measure today. Each entry says what is missing and what would
produce the value. No entry has a baseline. A number stands here only where a file of the tree
holds it.

The measures that have a command are in [README.md](README.md) and, per adapter crate, in
[adapters.md](adapters.md). A latency measure there has a command and no baseline: its command
was never run for these pages.

## No command produces it

### Latency of a session without a provider

Every command that prints a latency calls a hosted provider, a local server or an audio device.
[adapters.md](adapters.md) defines those measures. None runs in `task test-run`: the tests of
the examples that hold a latency figure run on fake clocks against scripted delays, and say
nothing about a real session. The workspace has no bench target. A benchmark that plays
recorded provider streams through a recipe on a named machine and reports percentiles would
produce a latency that needs no provider.

### Connect time, per binding

One binding has a command that prints its connect time: GPT-Live, as
`openai-gpt-live/session-start`. For every other binding no test and no example prints the time
from the start of the connect to the open connection, and the library keeps no such duration.

- OpenAI Realtime, Amazon Nova Sonic, the Deepgram Voice Agent, the ElevenLabs agent,
  ElevenLabs speech to text, Phonic and Deepslate Opal: no command prints it, alone or inside
  another figure of the adapter's own tests.
- Cartesia, Deepgram transcription, ElevenLabs text to speech through the example `live`, and
  OpenAI speech: the connect is inside the latency the command prints, and is not printed
  apart.
- llama-server: the HTTP request is inside the time to the first token, and is not printed
  apart.
- The example `compare` opens each session before its clock starts and does not time the open.

One line in each live test, printing the time from before the connect to the open connection,
would produce it.

### End of speech to first audio, from an adapter's own tests

The live tests of the Deepgram Voice Agent, the ElevenLabs agent, Phonic and Deepslate Opal
stop at the first audio of the answer and do not print its time. The two live tests of Amazon
Nova Sonic and the live tests of OpenAI Realtime print no latency at all. Only the example
`compare` gives the figure for these bindings, as `compare/first-audio`, and it is billed for
each. One line in each of those tests, printing the time from the last frame of the question to
the first audio, would produce it.

### Printed times that are not a latency

Some live commands print a time that no measure is defined for, because it is not the wait of a
user:

- Amazon Nova Sonic, the test file `nova_sonic_duplex_live`: `elapsed` is the time of the whole
  session, from the first frame to the close.
- ElevenLabs speech to text, the test file `stt_live`: the times of a span are positions in the
  audio.
- Deepgram transcription, the test file `live`: `provider duration` is the length of the audio
  the provider counted.
- The Deepgram Voice Agent, the ElevenLabs agent, Phonic and Deepslate Opal: the time in front
  of each transcript line counts from the open of the session, while the clip is still being
  sent.
- GPT-Live, the test file `live_gpt`: the lines that start with `t=` are a list of event times,
  not one value from one event to another.
- The ElevenLabs cancel probe: beside the first audio it prints the last audio, the gaps and
  the close of each part. The probe asserts nothing, and these describe the endpoint after a
  cancel.
- The example `talk` of `realtime-voice` and its ignored test `live_usage`: the time of the
  whole run.

### Time bounds no test fires, and defaults no test asserts

[adapters.md](adapters.md) names the tests that hold a bound. The rest, per binding:

- Cartesia: no test fires the connect timeout, `DEFAULT_CONNECT_TIMEOUT` in
  `adapters/inference-cartesia/src/config.rs`. No test asserts the value of that default or of
  `DEFAULT_READ_TIMEOUT`.
- Phonic: no test fires the connect timeout, `DEFAULT_CONNECT_TIMEOUT` in
  `adapters/inference-phonic/src/protocol.rs`. No test asserts the value of any of its four
  defaults.
- ElevenLabs agent: no test fires the connect timeout, `DEFAULT_CONNECT_TIMEOUT` in
  `adapters/inference-elevenlabs/src/agent.rs`. No test asserts the value of that default, of
  `DEFAULT_METADATA_TIMEOUT` or of the agent's `DEFAULT_READ_TIMEOUT`.
- ElevenLabs text to speech and speech to text: no test asserts the value of
  `DEFAULT_READ_TIMEOUT` or `DEFAULT_CONNECT_TIMEOUT`.
- Deepgram transcription: no test asserts the value of `DEFAULT_CONNECT_TIMEOUT`,
  `DEFAULT_READ_TIMEOUT` or `DEFAULT_KEEPALIVE_INTERVAL`.
- Deepgram Voice Agent: no test asserts the value of `DEFAULT_AGENT_OPEN_TIMEOUT` or
  `DEFAULT_AGENT_READ_TIMEOUT`. The keep-alive interval is asserted.
- Deepslate Opal: no test asserts the value of `DEFAULT_OPEN_TIMEOUT`, `DEFAULT_READ_TIMEOUT`
  or `DEFAULT_PING_INTERVAL`.
- OpenAI: the connect and write defaults are held only to be the constants and to lie between
  zero and a wide upper limit. No test asserts the idle limit of the default session
  configuration.
- Amazon Nova Sonic: no test asserts the default read timeout or the default idle limit. No
  test names the wait of the close for its task.
- Audio device: no test asserts the value of `PLAYBACK_PATIENCE`.
- Every `CLOSE_BOUND`: the tests compare a time with the constant, so a larger constant passes.

A test per binding that compares each default with its number, and one that starts a session
against a listener that never answers the handshake, would produce them.

### The default limits of the HTTP transport of llama-server

The adapter has no time constant of its own. Its configuration takes the default limits of the
HTTP client of `llm`, at the tag `Cargo.lock` pins. Their values are not in this tree, and no
test of the adapter holds them: the one test of a limit sets its own. A test that asserts the
three default limits of the configuration would produce it.

### Real-time factor and latency of the hosted speech-to-text bindings

The two local adapters have a measured real-time factor. Deepgram transcription and ElevenLabs
speech to text have none. `deepgram-live/first-event` is defined and depends on where the clip
starts to speak. The live test of ElevenLabs speech to text prints positions in the audio and
no latency. In `task test-run` the conformance checks of both hold only that the end of input
drains within 8 s, against a server on the loopback interface
(`adapters/inference-deepgram/tests/conformance.rs`,
`adapters/inference-elevenlabs/tests/stt_conformance.rs`). A live test that sends a clip at the
pace of speech and prints the time from the last voiced frame to the committed span would
produce a latency.

### Ignored tests per adapter crate

The log of the check run holds one number of skipped tests for the whole workspace, which the
record of `workspace-tests` keeps. It has no line per skipped test, so nothing gives the count
per package. A listing of the ignored tests by nextest, counted per package in the gate, would
produce it.

### Commits on `main` since the last release, and entries waiting for one

Git holds the commits since the last tag, and `CHANGELOG.md` holds the entries under
`## [Unreleased]`. No task prints either count, and nothing bounds how long an entry waits for
a release. A step that counts the commits from the last tag to `main` and the entries of that
section would produce both.

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

One thing about it is tested. `no_gpl_phonemiser_is_linked` in
`adapters/inference-sherpa-onnx/tests/licence.rs`, part of `task test-run`, scans the test binary
and every sherpa-onnx and onnxruntime library that binary loaded for the symbols of espeak-ng and
piper-phonemize, the two GPL phonemisers, and fails when it finds one. The archive is the build
without TTS, and `task sherpa-onnx-archive` pins it by its sha256.

Still not read: the licence text of each library that is linked. No file lists the shared
libraries of the archive and their licences, and the test knows two libraries by their symbols
and no other. A manifest of the archive, written when the pin changes and compared in the gate,
would produce it.

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

Both records under `adapters/inference-llama-server/capability/`, `reference-machine-cpu.json`
and `reference-machine-cpu-tools.json`, hold a peak resident memory of 1839 MiB and a load time
of 13911 ms. No command measures them, and no test reads them: the test that holds the two
records, `llama-server-records/held`, compares neither number with anything. An ignored test
like `stt_resources` of the speech-to-text adapters would produce them.

### Cost and last pass of the live tests against hosted providers

The tests that call a hosted provider are ignored and billed. A person runs them. No file of
the tree holds what a run cost or the day a test last passed. A scheduled run that writes one
record per binding would produce it. Their latency is the next section.

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

### The output of every live command

[adapters.md](adapters.md) defines a latency measure for each command that prints one. Each
says `Baseline: none.` Nothing keeps the output of such a command:

- A live test prints to the terminal, as free text or as the debug form of a value. It writes
  no file.
- The example `compare` writes one JSON line per run to the file its option `--runs-out` names.
  No such file is committed.
- No task of `Taskfile.yml` and no workflow under `.github/workflows/` runs a live test, an
  example, a benchmark or a profile. `task test-run` compiles every live test and skips it.

A run by a person on a named machine and network, with the model, the voice and the clip
written down, and one evidence record written per command, would give each measure a baseline.
A scheduled workflow that holds the credentials would keep it current.

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

The `Docs UX` workflow runs on pull requests only. The baseline in the README is from a run that
checked out the merge of a pull request into `main`; the tree of that merge is the tree of
`main`. A run of the workflow on `main` would give a baseline at the commit itself.

## Measured, but not expressible in the formats at the pin

Each of these is checked or recorded by this repository today. The formats at commit
`f139eccc17a548444f0f4e1dc06520120c28739f` cannot say it, so it is not in the YAML or has no
record. No field was invented for it.

### A gate for a release

A release is a tag on `main`, and what it has to hold is on
[README.md](README.md#a-release). The stage of a gate is a free label, so a gate could be
named `release`. A gate has no field for what it judges: the candidate is given when the gate
is evaluated, and the selection of a target is the candidate or its merge result. Nothing says
"the commit the tag points at". A gate `release` would be the gate `check` under another name,
so it is not in the YAML. The requirement `release-statuses-held` is part of the gate `check`.

### A measure that is defined and was never observed

The latency measures are assessments with a command, a unit and a direction, and with no
record and no requirement. A plan has no state that says "defined, no observation yet". A
reader of the YAML cannot tell such a plan from one whose record is missing. The pages say it
with the line `Baseline: none.`, and the test holds that no record and no requirement names
the measure.

### What a command needs before it can run

An observation has one `command`. It has no field for a credential the command reads, for the
environment variables it needs, for a device, or for the fact that a provider bills the run.
The YAML holds the command alone. The lines `Needs:` and `Billed:` of an entry say the rest,
and nothing compares them with the code.

### What each test of a list holds

A time-bounds assessment lists its tests as an input, and its record has one result per test.
The list cannot say what a test bounds, which constant it belongs to, or whether the test
asserts the default value or only the mechanism with a shorter one. A requirement names the
whole measurement, not one test of it. That text is on [adapters.md](adapters.md) only.

### The digest of the native archive against its pinned value

`task sherpa-onnx-archive` compares the sha256 of the download with the pin in `Taskfile.yml`. A
result of the value type `digest` compares only with a generated reference, a committed file
that a generator reproduces. A downloaded archive is not generated from the tree. The YAML
therefore holds the assertion `native-archive/checksum-matches` and not the digest.

### Tests run, passed and skipped as a value that must not get worse

The record of `workspace-tests` holds the three counts as coverage. Coverage has fixed bounds
only: a least number executed, a most number skipped. It has no direction and no accepted value,
so "never fewer tests than before" cannot be required.

`adapter-tests` takes the other way: its assessor reports the tests run and passed per package
as outputs, and an output has a direction. No requirement is written on them, because nothing
in the repository fails when an adapter has fewer tests.

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

What is expressible is the committed file as a document that a test reads:
`sherpa-onnx-stt-record` and `whispercpp-stt-record` have the real-time factor as an exact value
of the file, with a record from the check run. The load time and the peak memory are not
repeated there, because no bound is held on them.

### The machine a resource profile was measured on

The capability records name the machine in one sentence: the CPU, the memory, the unused GPU.
The conditions of a plan have a text `runner`, a map of tools and a load bound. The plans name
the machine as the runner `reference-machine-i9-10900k` and state the execution on the CPU and
the thread count as conditions the assessor declares. The memory of the machine and the unused
GPU have no key and are not in the YAML. The build profile is `--release` in the command; the
capability records do not state it, so it is no condition either.

### A sampled measurement and an exact one in one plan

The plan `docs-ux` holds twelve exact values and two durations. The observation has one kind for
the whole plan. It is written as a controlled run with one run, which is true for the durations
and says nothing for the twelve.

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
