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

### Time bounds no test fires

[adapters.md](adapters.md) names the tests that hold a bound. Three lists further down name
what a test does and no record of a run holds yet: "The connect timeout of five bindings" has
the tests that fire that timeout and time the failure, "The close bound of six bindings" has
the tests that fire a close bound and compare the time with numbers, and "The value of each
time-bound default" has the tests that compare each default with its number. What is left, per
binding:

- ElevenLabs agent: beside the test listed further down, two tests fire the connect timeout, at
  200 ms against a handshake that is never answered, and assert no time: the test
  `a_handshake_that_timed_out_or_was_cancelled_spends_the_signed_url_too` of
  `adapters/inference-elevenlabs/tests/agent_duplex.rs` and the test
  `no_memory_holds_the_url_after_a_handshake_that_timed_out` of
  `adapters/inference-elevenlabs/tests/adversary2_w2_u3_signed_url_memory.rs`. With the
  timeout not applied both pass, 30 s later, when the fixture ends the connection.
- Amazon Nova Sonic: the adapter has no bound on its close, so there is none to fire. The close
  sends a command to the task of the session and does not wait for it, and the task ends the
  session on the stream without a timeout (`adapters/inference-aws/src/duplex.rs`).
- Every `CLOSE_BOUND`: six bindings have a test that fires it and compares the time with
  numbers, in the list "The close bound of six bindings". Cartesia text to speech and
  ElevenLabs text to speech have such tests too, where a synthesis is dropped, and the next
  three entries say what these tests hold and what they leave out. In these two bindings a
  close also runs as an exchange that the next step of the synthesis waits for with no
  deadline of its own, so the write of the close frame is its only bound
  (`adapters/inference-cartesia/src/synthesiser.rs`,
  `adapters/inference-elevenlabs/src/synthesiser.rs`): a test of Cartesia fires it, named in
  the entry on Cartesia, and no test of ElevenLabs text to speech does. No test fires what
  follows. In Phonic, the Deepgram Voice Agent,
  Deepslate Opal and the ElevenLabs agent no test fires one application of the constant alone:
  a close outside an async runtime bounds the write of the close frame and the wait for the
  connection task with the same constant, and the test of each binding passes with either of
  the two removed. A Deepgram transcription handle adds the constant to the read timeout for
  its deadline after the end of input (`adapters/inference-deepgram/src/recogniser.rs`). No
  test fires that deadline: the test
  `messages_trickling_in_after_close_stream_do_not_stretch_the_summary_deadline` of
  `adapters/inference-deepgram/tests/bounds.rs` and the test
  `a_server_silent_after_close_stream_stalls_within_the_read_timeout` of
  `adapters/inference-deepgram/tests/errors.rs` pass with the constant at 3 s, so the
  connection thread's own deadline ends both. A test asserts the value of each of the five
  constants, 1 s: one per crate, of Cartesia, Deepgram, Deepslate, ElevenLabs and Phonic. These
  tests are in the list "The value of each time-bound default". They fix the number and fire
  nothing.
- Cartesia: a drop fires `CLOSE_BOUND`, against a peer that reads nothing
  (`adapters/inference-cartesia/tests/support/deaf.rs`). The tests of a drop in
  `adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs` compare the wait with
  numbers written out, not with the constant. Outside a runtime the test
  `dropping_an_idle_synthesis_whose_close_cannot_be_written_outside_a_runtime_waits_close_bound`
  holds the wait of the drop to 900 ms to 1.5 s. Inside a Tokio runtime the test
  `dropping_an_idle_synthesis_whose_close_cannot_be_written_does_not_block_the_runtime` holds
  the end of the synthesis's own thread, which runs the close the drop handed off, to 900 ms to
  2 s after the drop. Both fail with the constant at 3 s. The code of a drop has the bound three
  times: on the wait outside a runtime, on the close handed off inside one
  (`adapters/inference-cartesia/src/synthesiser.rs`) and on the close of the connection
  (`adapters/inference-cartesia/src/connection.rs`). No test of a drop fails with one of the
  three removed, and none can: the close of the connection starts with the drop and bounds the
  same close as each of the other two, no later than they do, and each of those two bounds that
  close where the third is removed. With the first and the third removed the first test fails,
  at a limit of its fixture, 25 s. With the second and the third removed the second test fails.
  The third alone bounds the close of a synthesis that failed, which its next step waits for:
  the test `a_synthesis_that_fails_on_a_backed_up_connection_ends_at_the_close_bound` of the
  same file holds the end of that wait to the stall and 500 ms to 1.5 s more, fails with the
  third removed, at the limit of its fixture, 25 s, and fails with the constant at 3 s. No entry
  of [adapters.md](adapters.md) names these tests.
- Cartesia: a cancel waits for its send up to twice `CLOSE_BOUND`
  (`adapters/inference-cartesia/src/synthesiser.rs`). The test asserts once the constant and
  does not reach the longer wait.
- ElevenLabs text to speech: a drop fires `CLOSE_BOUND`. The adapter reads it where a handle
  is dropped (`adapters/inference-elevenlabs/src/synthesiser.rs`) and where a connection closes
  (`adapters/inference-elevenlabs/src/connection.rs`). The cancel of a segment does no I/O and
  does not wait for the server, so a test of a cancel does not reach the bound. A drop reaches
  it against a peer that reads nothing (`adapters/inference-elevenlabs/tests/support/deaf.rs`).
  The tests of a drop in `adapters/inference-elevenlabs/tests/adversary_w2_drop_backed_up.rs`
  compare the wait with numbers written out, not with the constant. Outside a runtime the test
  `dropping_an_idle_synthesis_whose_close_cannot_be_written_outside_a_runtime_waits_close_bound`
  holds the wait of the drop to 900 ms to 1.5 s, and the test
  `a_drop_inside_an_executor_that_is_not_tokio_waits_close_bound_and_not_the_grace` holds it to
  900 ms to 1.1 s inside an executor that is not Tokio. Inside a Tokio runtime the test
  `dropping_an_idle_synthesis_whose_close_cannot_be_written_does_not_block_the_runtime` holds
  the end of the synthesis's own thread to 900 ms to 2 s after the drop. All three fail with
  the constant at 3 s. The code of a drop has the bound three times, as in Cartesia, and for the
  same reason no test of a drop fails with one of the three removed. With the wait outside a
  runtime and the close of the connection unbounded the first two tests fail at a limit of
  their own, 10 s. With the close handed off and the close of the connection unbounded the
  third fails. Speech to text closes its connection with the same function as
  the last of the three: with the close of the connection unbounded alone the test
  `a_cancel_on_a_backed_up_connection_ends_its_thread_at_the_close_bound` of
  `adapters/inference-elevenlabs/tests/stt.rs` fails, at a limit of its own, 4 s. No entry of
  [adapters.md](adapters.md) names these tests. Other tests drop
  a handle, against a fixture that takes the close at once. The test
  `dropping_the_handle_mid_segment_closes_the_socket` of
  `adapters/inference-elevenlabs/tests/cancel.rs` asserts no time. The test
  `adversary_rf1_a_handle_dropped_after_a_cancel_while_the_context_streams_closes_the_socket`
  of `adapters/inference-elevenlabs/tests/adversary_rf1_multi_context.rs` asserts a limit of
  its own, 2 s, and by reading the code passes with the bound removed.
- `CLOSE_GRACE` of Cartesia and of ElevenLabs text to speech, the most a drop inside a Tokio
  runtime waits for its close: no test of a drop asserts its value. The test
  `a_drop_whose_close_cannot_be_written_holds_the_runtime_for_the_grace_and_no_longer` of
  `adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs`, and the one of that name
  of `adapters/inference-elevenlabs/tests/adversary_w2_drop_backed_up.rs`, holds it under the
  limit of its ticker, 50 ms, and fails with the constant at ten times that limit. Each also
  compares the hold of the drop, less the time its thread was ready to run without a processor,
  with the constant and 15 ms more, so it follows the constant; a drop that waits five times
  the constant fails it. A test asserts the value of each of the two constants, 5 ms; these
  tests are in the list "The value of each time-bound default".
- llama-server: no test fires the bound on establishing a connection. The adapter does not set
  it: it builds its HTTP client with `HttpClient::new` of `llm`
  (`adapters/inference-llama-server/src/lib.rs`), which takes `CONNECT_TIMEOUT` of `llm-http`,
  15 s at the tag `Cargo.lock` pins. The adapter's configuration has no field for it, so a
  caller cannot set it.

The peer of the six tests, one that floods pings and stops reading, would fire most of what is
left of the close bounds, in a test per case: a close as an exchange in the two text-to-speech
bindings, a cancel in Cartesia, and in a session binding a case that reaches one application of
the constant without the other. In the drop tests of the two text-to-speech bindings a
comparison of the wait with a number, as the six tests have it, would assert the value of the
bound where it is fired. What would fire the deadline of a Deepgram transcription handle is not
worked out here.

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
per package. The assessor of `adapter-tests` therefore does not report skipped tests, and the
coverage of its record has 0 for them, as the format writes a count that is not reported
(`ratchet:spec/domains/assessor.yaml:182-187`). A listing of the ignored tests by nextest,
counted per package in the gate, would produce the count.

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

### The connect timeout of five bindings

`task test-run` runs one test per binding that fires the connect timeout and times the failure:
the test opens against a local listener that never answers the handshake, with a value of its own
in place of the binding's `DEFAULT_CONNECT_TIMEOUT`. No record of a run holds the result of any
of the five yet:

- Cartesia text to speech: the test
  `a_handshake_never_answered_fails_the_synthesis_at_the_connect_timeout` of
  `adapters/inference-cartesia/tests/errors.rs`.
- Phonic: the test `a_handshake_never_answered_fails_the_open_at_the_connect_timeout` of
  `adapters/inference-phonic/tests/duplex.rs`.
- ElevenLabs agent: the test
  `a_handshake_never_answered_fails_the_open_at_the_connect_timeout` of
  `adapters/inference-elevenlabs/tests/agent_duplex.rs`.
- ElevenLabs text to speech: the test
  `a_handshake_never_answered_fails_the_synthesis_at_the_connect_timeout` of
  `adapters/inference-elevenlabs/tests/closed_stream.rs`.
- Deepgram transcription: the test
  `a_handshake_never_answered_fails_the_recognition_at_the_connect_timeout` of
  `adapters/inference-deepgram/tests/errors.rs`.

A record of a run of the workflow `check` that ran these five tests would hold their results.

### The close bound of six bindings

`task test-run` runs one test per binding that fires `CLOSE_BOUND`. The local fixture sends a
flood of WebSocket pings and reads nothing more. The binding answers each ping with a pong, the
pongs fill the socket buffers, and the close frame behind them cannot be written: the bound is
what ends the close. Each test compares the time with two limits of its own, written out as
numbers and not read from the constant: no sooner than 1 s, and before 2 s. So it fails with a
constant of 3 s. Whether the connection was backed up is the fixture's to tell. It says how
many pongs it had read when the test let it read again; reading again, it counts the pongs and
looks for a close frame. The limits are asserted of a run in which it had read no pong until
then, counted fewer pongs than pings at the end and found no close frame. A flood whose pongs
all arrived is doubled, up to the largest the binding reads within the test's own wait: between
72,000 and 288,000 pings, by the test. The test fails when the largest arrived whole too. No
record of a run holds the result of any of the six yet:

- Phonic: the test `a_close_on_a_backed_up_connection_ends_at_the_close_bound` of
  `adapters/inference-phonic/tests/duplex.rs`.
- Deepgram Voice Agent: the test `a_close_on_a_backed_up_connection_ends_at_the_close_bound` of
  `adapters/inference-deepgram/tests/agent_duplex.rs`.
- Deepslate Opal: the test `a_close_on_a_backed_up_connection_ends_at_the_close_bound` of
  `adapters/inference-deepslate/tests/opal_duplex.rs`.
- ElevenLabs agent: the test `a_close_on_a_backed_up_connection_ends_at_the_close_bound` of
  `adapters/inference-elevenlabs/tests/agent_duplex.rs`.
- Deepgram transcription: the test
  `a_cancel_on_a_backed_up_connection_ends_its_thread_at_the_close_bound` of
  `adapters/inference-deepgram/tests/cancel.rs`.
- ElevenLabs speech to text: the test
  `a_cancel_on_a_backed_up_connection_ends_its_thread_at_the_close_bound` of
  `adapters/inference-elevenlabs/tests/stt.rs`.

What these tests do not tell apart:

- The four session tests time a close called outside an async runtime. There the binding
  applies the constant twice, to the write of the close frame and to the wait for the
  connection task, and either ends the close at the same time. A test passes with one of the
  two timeouts removed and does not say which of them held.
- The two transcription tests time the end of the recognition's connection thread, because the
  cancel itself returns at once. They find the thread by its name under `/proc/self/task`, so
  they run on Linux only and rely on cargo-nextest, which runs each test in a process of its
  own. They fire the bound through a cancel and not through a dropped handle or the end of
  input.
- The limits are on the time measured, which is the bound and what a close takes besides it.
  So they pass a bound a little under 1 s and any bound from there to a little under 2 s: with
  a constant of 999 ms the close of the Phonic test held 1.000308908 s and the test passed, and
  all six passed with 1,500 ms and with 1,900 ms. They failed with 500 ms and with 3 s. The
  number itself is fixed by the tests of the list "The value of each time-bound default".
- Deepgram has one constant for its two bindings and ElevenLabs one for its three, so the two
  tests of a crate follow the same constant.

A record of a run of the workflow `check` that ran these six tests would hold their results.

### The value of each time-bound default

`task test-run` runs the tests below. Each compares time-bound defaults of one binding with
their numbers written out in the test. A test reads the public constant where there is one. It
also reads the value from what a caller gets without setting it, where that can be read without
a connection: the field of a new endpoint or configuration, and the reader of a new model. A
constant or a literal changed in the source fails its test, and so does an endpoint,
configuration or model read this way that starts with another value. A test does not read the
places where an adapter uses the value later. Two defaults a caller gets are read by no test:

- OpenAI Realtime and GPT-Live, the clients: `RealtimeClient::connect` and `LiveClient::connect`
  start with the connect deadline and the write deadline of `Deadlines::default()`
  (`adapters/inference-openai/src/client.rs`, `adapters/inference-openai/src/live.rs`). A
  client gives no reader of them before its connection is open, and no test opens a client this
  way and reads them. A client that starts with other deadlines fails no test.
- llama-server, the bound on establishing a connection: the test reads the constant of
  `llm-http` and a client built as the adapter builds its own. The adapter gives no reader of
  its client. An adapter that builds its client with another bound fails no test.

No record of a run holds a result of any of these comparisons yet:

- Audio device: the test `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-audio-device/tests/acknowledging.rs`, for `PLAYBACK_PATIENCE`, and the
  test `a_sink_given_no_patience_waits_the_stated_two_seconds_for_a_stalled_speaker` of
  `adapters/inference-audio-device/tests/acknowledging.rs`, for the patience of a sink that was
  given none. The second reads it from the fault of a drain on a speaker that stopped, and
  waits about 2 s for that fault.
- Amazon Nova Sonic: the test `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-aws/tests/nova_sonic_stall_and_bounds.rs`, for the read timeout a session
  configuration starts with and the idle limit of the default session configuration.
- Cartesia text to speech: the test `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-cartesia/tests/errors.rs`, for `DEFAULT_READ_TIMEOUT`,
  `DEFAULT_CONNECT_TIMEOUT`, `CLOSE_BOUND` and `CLOSE_GRACE`.
- Deepgram transcription: the test `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-deepgram/tests/errors.rs`, for `DEFAULT_CONNECT_TIMEOUT`,
  `DEFAULT_READ_TIMEOUT`, `DEFAULT_KEEPALIVE_INTERVAL` and `CLOSE_BOUND`.
- Deepgram Voice Agent: the test `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-deepgram/tests/agent_duplex.rs`, for `DEFAULT_AGENT_OPEN_TIMEOUT` and
  `DEFAULT_AGENT_READ_TIMEOUT`.
- Deepslate Opal: the test `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-deepslate/tests/opal_duplex.rs`, for `DEFAULT_OPEN_TIMEOUT`,
  `DEFAULT_READ_TIMEOUT`, `DEFAULT_PING_INTERVAL` and `CLOSE_BOUND`.
- ElevenLabs text to speech and speech to text: the test
  `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-elevenlabs/tests/closed_stream.rs`, for `DEFAULT_READ_TIMEOUT`,
  `DEFAULT_CONNECT_TIMEOUT`, `CLOSE_BOUND` and, of text to speech alone, `CLOSE_GRACE`.
- ElevenLabs agent: the test `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-elevenlabs/tests/agent_duplex.rs`, for the agent's
  `DEFAULT_CONNECT_TIMEOUT`, `DEFAULT_METADATA_TIMEOUT` and `DEFAULT_READ_TIMEOUT`.
- llama-server: the test `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-llama-server/tests/offline.rs`, for the three limits of a new
  configuration: 60 s to the response headers, 180 s idle and 600 s in total. The adapter has
  no time constant of its own. Its configuration takes the default limits of the HTTP client of
  `llm`, at the tag `Cargo.lock` pins, and their values are not in the adapter's source. A move
  of that pin that changes a limit fails the test. The same test compares the bound on
  establishing a connection with 15 s. It is `CONNECT_TIMEOUT` of `llm-http` at the same tag,
  and a move of the pin that changes it fails the test.
- OpenAI Realtime: the test `the_deadlines_are_stated_and_settable` of
  `adapters/inference-openai/tests/transport_deadlines.rs`, for `DEFAULT_CONNECT_TIMEOUT` and
  `DEFAULT_WRITE_TIMEOUT`, and the test `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-openai/tests/duplex.rs`, for the idle limit of the default session
  configuration and for the connect deadline and the write deadline of a model built with
  `OpenAiRealtime::new`. The first of the two is older than its comparison: the record of
  `openai-time-bounds` holds a result of it from a run before it compared the numbers.
- OpenAI GPT-Live: the test `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-openai/tests/gpt_live_duplex.rs`, for the idle limit of the default
  session configuration, the read timeout of the endpoint `Endpoint::live` gives, and the
  connect deadline and the write deadline of a model built with `OpenAiLive::new`.
- OpenAI speech: the test `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-openai/tests/speech_request.rs`, for the connect timeout and the read
  timeout of the default endpoint. The binding has no constant of its own: the two are
  `DEFAULT_CONNECT_TIMEOUT` and `DEFAULT_READ_TIMEOUT` of the Realtime binding.
- Phonic: the test `the_time_bound_defaults_are_the_stated_numbers` of
  `adapters/inference-phonic/tests/duplex.rs`, for the four defaults of
  `adapters/inference-phonic/src/protocol.rs` and `CLOSE_BOUND`.

A record of a run of the workflow `check` that ran these tests would hold their results. Until
[adapters.md](adapters.md) is written from such a run, it does not name the new tests, and its
lines that say no test fixes or asserts one of these defaults are older than the tests.

## Measured, but not expressible in the formats at the pin

Each of these is checked or recorded by this repository today. The formats at commit
`f139eccc17a548444f0f4e1dc06520120c28739f` cannot say it, so it is not in the YAML or has no
record. No field was invented for it.

### A gate for a release

A release is a tag on `main`, and what it has to hold is on
[README.md](README.md#a-release). The stage of a gate is a free label, so a gate could be
named `release`. A gate has no field for what it judges: the candidate is given when the gate
is evaluated, and the selection of a target is the candidate or its merge result. Nothing says
"the commit the tag points at" (`ratchet:spec/domains/specification.yaml:953-992`, the fields
of a gate; `:227-229`, the selection; `:622-628`, the stage). The gate the documents leave out
would require what the gates `check`, `slow` and `docs-ux` require, and a free stage label
lets one write that list. What cannot be written at the pin is its binding to the commit a tag
points at. Without it the gate would say nothing a release could be held to, so it is not in
the YAML. The requirement `release-statuses-held` is part of the gate `check`.

### A measure that is defined and was never observed

The latency measures are assessments with a command, a unit and a direction, and with no
record and no requirement. The specification names that state: "An assessment with no
requirement in any gate is in observe mode" (`ratchet:spec/domains/specification.yaml:850-851`).
What the formats cannot say is whether evidence was ever collected for a plan. A plan in
observe mode whose command never ran and one whose record is missing look the same in the
YAML. The pages say it with the line `Baseline: none.`, and the test holds that no record and
no requirement names the measure.

### What a command needs before it can run

An observation has one `command` (`ratchet:spec/domains/specification.yaml:461-463`). It has no
field for a credential the command reads, for the environment variables it needs, for a device,
or for the fact that a provider bills the run. The YAML holds the command alone. The lines
`Needs:` and `Billed:` of an entry say the rest, and nothing compares them with the code.

### What each test of a list holds

A time-bounds assessment lists its tests as an input, and its record has one result per test.
The list cannot say what a test bounds, which constant it belongs to, or whether the test
asserts the default value or only the mechanism with a shorter one: a declared input has a
type, a default and no text per value (`ratchet:spec/domains/specification.yaml:692-727`). A
requirement names the whole measurement, not one test of it (`:901-936`). That text is on
[adapters.md](adapters.md) only.

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
