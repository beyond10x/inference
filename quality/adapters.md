# The quality of each adapter

One section per crate under `adapters/`, in the order of the directory. Each says what the
adapter is for, how many tests hold it, which time bounds and records its tests keep, and which
commands print a latency. The general rules are in [README.md](README.md). What has no command
is in [not-measured.md](not-measured.md).

- "Check run" is the run of that name in the
  [table of runs](README.md#the-runs-the-baselines-were-read-from).
- A path is from the root of the repository.
- A latency entry has no baseline. Its command was not run for this page, and no file keeps its
  output. Most of these commands are billed by the provider.

## Tests per adapter crate

The three measures below have one value per crate. The values are in the table of each section.

### `adapter-tests/tests-run`

- For a user: how many tests hold the adapter in every `task check`.
- Command: `task test-run`. The value is the number of test lines of the package in the log. The
  command that counts them is beside the table of runs.
- Unit and direction: tests; higher is better.
- Bound: recorded, no bound.
- Baseline: one value per crate, in the table of its section. Check run.

### `adapter-tests/tests-passed`

- For a user: every test of the adapter passed in the run the values are read from.
- Command: `task test-run`. The value is the number of test lines of the package that say `PASS`.
- Unit and direction: tests; higher is better.
- Bound: recorded, no bound. `task test-run` fails when one test fails.
- Baseline: one value per crate, in the table of its section. Check run.

### `adapter-tests/slowest-test`

- For a user: how long the slowest test of the adapter keeps a change waiting.
- Command: `task test-run`. The value is the longest time on a test line of the package.
- Unit and direction: seconds; lower is better.
- Bound: recorded, no bound per crate. `workspace-tests/slowest-test` bounds every test of the
  workspace.
- Baseline: one value per crate, in the table of its section. Check run.

Tests that are ignored have no count per crate. The log holds one number for the whole
workspace. See [not-measured.md](not-measured.md).

The coverage of the record of `adapter-tests` counts the tests of the eleven crates and of no
other package. Its `skipped` is 0 because the assessor does not report that count. It is not
the number of ignored tests.

## How a time bound is written

A time bound is a constant of the adapter and a test that holds it. Each line of a
`<adapter>-time-bounds/held` entry names what is bounded, the constant with its value, and the
test. It says one of two things about the test:

- "asserts the value": the test compares the default with the number. A change of the default
  fails it.
- "holds the mechanism": the test sets a shorter value and waits for the failure. A change of
  the default does not fail it.

A test that compares a time with the constant itself, such as `CLOSE_BOUND`, follows the
constant. It does not fix the number. The close tests the entries name do not fire the bound
either. Each closes against a local fixture that takes the close at once; in the Deepgram
transcription test the socket is dropped before a close frame is due. So the wait never reaches
the constant, and by reading the code each of these tests passes with the bound enlarged and
with the bound removed. What they hold is that a close or a cancel returns soon against a peer
that answers. Tests of `inference-cartesia` and of `inference-elevenlabs` drop a synthesis
against a peer that does not take the write, which is the case the bound is for. They fire the
bound and follow the constant, and no entry of this page names them yet. See
[not-measured.md](not-measured.md).

## `inference-audio-device`

Captures the microphone and plays to the speaker, so that a pipeline can run on a laptop.

| Measure | Baseline | Read from |
|---|---|---|
| `adapter-tests/tests-run` | 127 tests | Check run |
| `adapter-tests/tests-passed` | 127 tests | Check run |
| `adapter-tests/slowest-test` | 1.226 s, `inference-audio-device::adversary2_loopback a_one_second_delay_on_the_default_buffers_reports_no_audio_dropped_after_forwarding` | Check run |

It has no committed record and no command that prints a latency.

### `audio-device-time-bounds/held`

- For a user: playback that the speaker stops taking fails after a stated time and does not hang.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 2 of 2 tests. Check run.
- Drain, stopped callback: `PLAYBACK_PATIENCE` is 2 s
  (`adapters/inference-audio-device/src/acknowledging.rs`). The test
  `a_speaker_whose_callback_stopped_fails_the_drain_within_the_stated_patience` of
  `adapters/inference-audio-device/tests/acknowledging.rs` holds the mechanism, at 300 ms.
- Drain, frame waiting for room: the same constant. The test
  `a_speaker_that_stops_while_a_frame_waits_for_room_fails_the_drain_within_the_patience` of
  `adapters/inference-audio-device/tests/adversary_rf1_acknowledging.rs` holds the mechanism, at
  300 ms.

## `inference-aws`

Speech in and speech out with Amazon Nova Sonic on Amazon Bedrock, over one bidirectional stream.

| Measure | Baseline | Read from |
|---|---|---|
| `adapter-tests/tests-run` | 134 tests | Check run |
| `adapter-tests/tests-passed` | 134 tests | Check run |
| `adapter-tests/slowest-test` | 1.110 s, `inference-aws::unreadable_nova audio_appended_after_the_close_is_refused_and_drops_nothing` | Check run |

### `aws-time-bounds/held`

- For a user: a session that the service stops answering fails after a stated time and does not
  hang.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 4 of 4 tests. Check run.
- Open, default: `OPEN_TIMEOUT` is 10 s (`adapters/inference-aws/src/nova_sonic/client.rs`). The
  test `the_open_deadline_is_stated` of
  `adapters/inference-aws/tests/nova_sonic_stall_and_bounds.rs` asserts the value.
- Open, fired: the same constant. The test
  `start_against_a_server_that_never_answers_fails_within_its_deadline` of
  `adapters/inference-aws/tests/nova_sonic_stall_and_bounds.rs` holds the mechanism, at 300 ms
  against a listener that never answers.
- Read: the read timeout of a session is 30 s, a field default without a constant
  (`adapters/inference-aws/src/nova_sonic/config.rs`). The test
  `a_silent_service_is_a_stall_after_the_read_timeout` of
  `adapters/inference-aws/tests/nova_sonic_session.rs` holds the mechanism, at 50 ms.
- Idle: the idle limit of the default session configuration is 30 s
  (`adapters/inference-aws/src/duplex.rs`). The test
  `a_silent_provider_yields_a_stall_failure_at_the_bound` of
  `adapters/inference-aws/tests/nova_sonic_duplex.rs` holds the mechanism, at 300 ms.

### `aws-records/held`

- For a user: the capabilities the documentation states for the binding are the ones the code
  declares, at this version.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 1 of 1 tests. Check run.
- Nova Sonic: `adapters/inference-aws/records/claims-nova-sonic.json`. The test
  `the_nova_sonic_record_holds_the_claims_of_the_binding` of
  `adapters/inference-aws/tests/claims_record.rs` compares its claims, model, binding, features
  and `held_by` with the code, and its `adapter_revision` with the version of the crate.
- Second holder: `task docs-check` refuses a record whose `adapter_revision` is not the version
  of the workspace (`crates/inference-docs/src/adapters.rs`).

No command of this adapter prints a latency. Its live test prints the time of the whole session.
See [not-measured.md](not-measured.md).

## `inference-cartesia`

Turns text into speech with Cartesia Sonic, streamed over a WebSocket.

| Measure | Baseline | Read from |
|---|---|---|
| `adapter-tests/tests-run` | 37 tests | Check run |
| `adapter-tests/tests-passed` | 37 tests | Check run |
| `adapter-tests/slowest-test` | 1.032 s, `inference-cartesia::adversary_rf1_protocol segments_offered_while_one_speaks_and_after_an_idle_gap_are_all_spoken_in_order` | Check run |

### `cartesia-time-bounds/held`

- For a user: a synthesis that the server stops answering fails after a stated time, and a cancel
  does not wait for the server.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 2 of 2 tests. Check run.
- Read: `DEFAULT_READ_TIMEOUT` is 10 s (`adapters/inference-cartesia/src/config.rs`). The test
  `silence_fails_the_synthesis_as_stalled_within_the_read_timeout` of
  `adapters/inference-cartesia/tests/errors.rs` holds the mechanism, at 300 ms.
- Close: `CLOSE_BOUND` is 1 s (`adapters/inference-cartesia/src/connection.rs`). The test
  `a_cancel_does_not_wait_for_the_server` of `adapters/inference-cartesia/tests/cancel.rs`
  compares the time of a cancel with the constant itself. The fixture takes the cancel at once,
  so this test does not fire the bound. The cancel waits for its send up to twice the constant
  (`adapters/inference-cartesia/src/synthesiser.rs`), and the test asserts once the constant.
  Tests that no entry of this page names yet fire the bound where a synthesis is dropped
  against a peer that does not take the write. See [not-measured.md](not-measured.md).

### `cartesia-records/held`

- For a user: the capabilities the documentation states for the binding are the ones the code
  declares, at this version.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 1 of 1 tests. Check run.
- Text to speech: `adapters/inference-cartesia/records/claims-tts.json`. The test
  `the_tts_record_holds_the_claims_of_the_binding` of
  `adapters/inference-cartesia/tests/claims_record.rs` compares its claims, model, binding,
  features and `held_by` with the code, and its `adapter_revision` with the version of the crate.
- Second holder: `task docs-check` refuses a record whose `adapter_revision` is not the version
  of the workspace (`crates/inference-docs/src/adapters.rs`).

### `cartesia-live/first-audio`

- For a user: how long a caller waits for the first sound of an utterance.
- Command: `cargo test -p inference-cartesia --features live --test live -- --ignored
  --nocapture`. The test `one_utterance_in_mulaw_at_8_khz` of
  `adapters/inference-cartesia/tests/live.rs` prints it as `first audio after`.
- Needs: the feature `live`; `CARTESIA_API_KEY` or the key in the OS keychain;
  `CARTESIA_VOICE_ID`.
- Billed: yes, by Cartesia.
- Unit and direction: milliseconds; lower is better. The test prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: from the return of `end_utterance()` to the first audio frame. The segments are only
  queued before that. The connection opens when the handle is first read, after the clock
  started, so the connect is inside.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

## `inference-deepgram`

Turns speech into text with Deepgram's live transcription and, with the feature `duplex`, talks
to a Deepgram Voice Agent.

| Measure | Baseline | Read from |
|---|---|---|
| `adapter-tests/tests-run` | 126 tests | Check run |
| `adapter-tests/tests-passed` | 126 tests | Check run |
| `adapter-tests/slowest-test` | 3.011 s, `inference-deepgram::bounds messages_trickling_in_after_close_stream_do_not_stretch_the_summary_deadline` | Check run |

### `deepgram-time-bounds/held`

- For a user: a connection that goes silent fails after a stated time, a quiet connection is
  kept alive, and a cancel closes it at once.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 7 of 7 tests. Check run.
- Keep-alive, transcription: `DEFAULT_KEEPALIVE_INTERVAL` is 4 s
  (`adapters/inference-deepgram/src/config.rs`). The test `a_keep_alive_is_sent_after_silence` of
  `adapters/inference-deepgram/tests/audio.rs` holds the mechanism, at 100 ms.
- Read, transcription: `DEFAULT_READ_TIMEOUT` is 10 s. The test
  `a_server_silent_after_close_stream_stalls_within_the_read_timeout` of
  `adapters/inference-deepgram/tests/errors.rs` holds the mechanism, at 3 s.
- Close, transcription: `CLOSE_BOUND` is 1 s (`adapters/inference-deepgram/src/connection.rs`).
  The test
  `a_cancel_during_the_opening_handshake_closes_the_connection_within_the_close_bound` of
  `adapters/inference-deepgram/tests/security1_boundaries.rs` compares the time of the close
  with the constant plus 500 ms.
- Open, agent: `DEFAULT_AGENT_OPEN_TIMEOUT` is 15 s. The test
  `settings_applied_not_arriving_within_the_deadline_fails_the_open_and_closes` of
  `adapters/inference-deepgram/tests/agent_duplex.rs` holds the mechanism, at 300 ms.
- Keep-alive, agent, default: `DEFAULT_AGENT_KEEPALIVE_INTERVAL` is 8 s. The unit test
  `the_agent_endpoint_takes_a_plain_scheme_and_a_checked_url` of
  `adapters/inference-deepgram/src/config.rs` asserts the value.
- Keep-alive, agent, fired: the same constant. The test
  `a_keepalive_goes_out_after_silence_and_not_while_audio_flows` of
  `adapters/inference-deepgram/tests/agent_duplex.rs` holds the mechanism, at 300 ms.
- Close, agent: `CLOSE_BOUND`. The test
  `dropping_a_session_mid_reply_closes_the_connection_within_the_close_bound` of
  `adapters/inference-deepgram/tests/agent_duplex.rs` compares the time of the close with the
  constant itself.

### `deepgram-records/held`

- For a user: the capabilities the documentation states for each binding are the ones the code
  declares, at this version.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 2 of 2 tests. Check run.
- Transcription: `adapters/inference-deepgram/records/claims-stt.json`. The test
  `the_stt_record_holds_the_claims_of_the_binding` of
  `adapters/inference-deepgram/tests/claims_record.rs` compares its claims, model, binding,
  features and `held_by` with the code, and its `adapter_revision` with the version of the crate.
- Voice Agent: `adapters/inference-deepgram/records/claims-voice-agent.json`. The test
  `the_voice_agent_record_holds_the_claims_of_the_binding` of
  `adapters/inference-deepgram/tests/claims_record.rs` compares the same fields.
- Second holder: `task docs-check` refuses a record whose `adapter_revision` is not the version
  of the workspace (`crates/inference-docs/src/adapters.rs`).

### `deepgram-live/first-event`

- For a user: how long a speaker waits for the first words of a transcript.
- Command: `cargo test -p inference-deepgram --features live --test live -- --ignored
  --nocapture`. The test `a_clip_is_transcribed_with_interim_results_and_a_committed_span` of
  `adapters/inference-deepgram/tests/live.rs` prints it as `first event after`.
- Needs: the feature `live`; `DEEPGRAM_API_KEY` or the key in the OS keychain;
  `INFERENCE_DEEPGRAM_STT_CLIP`, a speech clip.
- Billed: yes, by Deepgram.
- Unit and direction: milliseconds; lower is better. The test prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: from the return of `start()`, before the first frame is offered, to the first event
  the recogniser delivers. The clip is sent at the pace of speech meanwhile, so the value
  depends on where the clip starts to speak. The connection opens with the first frame, so the
  connect is inside.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

The live test of the Voice Agent prints no latency. See [not-measured.md](not-measured.md).

## `inference-deepslate`

Speech in and speech out with Deepslate Opal, a speech-to-speech model hosted in the EU.

| Measure | Baseline | Read from |
|---|---|---|
| `adapter-tests/tests-run` | 70 tests | Check run |
| `adapter-tests/tests-passed` | 70 tests | Check run |
| `adapter-tests/slowest-test` | 2.603 s, `inference-deepslate::adversary_rf1_tool_turn an_answer_in_a_new_turn_leaves_nothing_owed` | Check run |

### `deepslate-time-bounds/held`

- For a user: a session that does not open or goes silent fails after a stated time, and a close
  does not wait for the server.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 4 of 4 tests. Check run.
- Open: `DEFAULT_OPEN_TIMEOUT` is 15 s (`adapters/inference-deepslate/src/config.rs`). The test
  `a_refused_or_failed_opening_opens_nothing` of
  `adapters/inference-deepslate/tests/opal_duplex.rs` holds the mechanism, at 300 ms.
- Ping: `DEFAULT_PING_INTERVAL` is 10 s. The test
  `a_ping_goes_out_at_the_interval_and_a_pong_is_no_answer_from_the_model` of
  `adapters/inference-deepslate/tests/opal_duplex.rs` holds the mechanism, at 100 ms, with an
  idle limit of the session of 800 ms.
- Read against ping: `DEFAULT_READ_TIMEOUT` is 30 s. The unit test
  `a_read_timeout_not_past_the_ping_interval_is_refused` of
  `adapters/inference-deepslate/src/config.rs` holds the rule that a read timeout is longer
  than the ping interval. It fires no timeout.
- Close: `CLOSE_BOUND` is 1 s (`adapters/inference-deepslate/src/connection.rs`). The test
  `closing_cancels_the_response_in_progress_and_closes_the_connection` of
  `adapters/inference-deepslate/tests/opal_duplex.rs` compares the time of the close with twice
  the constant.

### `deepslate-records/held`

- For a user: the capabilities the documentation states for the binding are the ones the code
  declares, at this version.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 1 of 1 tests. Check run.
- Opal: `adapters/inference-deepslate/records/claims-opal.json`. The test
  `the_opal_record_holds_the_claims_of_the_binding` of
  `adapters/inference-deepslate/tests/claims_record.rs` compares its claims, model, binding,
  features and `held_by` with the code, and its `adapter_revision` with the version of the crate.
- Second holder: `task docs-check` refuses a record whose `adapter_revision` is not the version
  of the workspace (`crates/inference-docs/src/adapters.rs`).

No command of this adapter prints a latency. See [not-measured.md](not-measured.md).

## `inference-elevenlabs`

Turns text into speech with ElevenLabs; with the feature `stt` it turns speech into text, and
with `duplex` it talks to an ElevenLabs agent.

| Measure | Baseline | Read from |
|---|---|---|
| `adapter-tests/tests-run` | 163 tests | Check run |
| `adapter-tests/tests-passed` | 163 tests | Check run |
| `adapter-tests/slowest-test` | 4.006 s, `inference-elevenlabs::security_u1_stt a_handshake_still_pending_when_the_read_timeout_after_input_passes_fails_the_recognition` | Check run |

### `elevenlabs-time-bounds/held`

- For a user: a connection that does not open or goes silent fails after a stated time, and an
  agent session that is dropped closes its connection soon.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 6 of 6 tests. Check run.
- Read, text to speech: `DEFAULT_READ_TIMEOUT` is 10 s
  (`adapters/inference-elevenlabs/src/config.rs`). The test
  `a_silent_server_is_a_stall_within_the_read_timeout` of
  `adapters/inference-elevenlabs/tests/closed_stream.rs` holds the mechanism, at 300 ms.
- Close, text to speech: not held. `CLOSE_BOUND` is 1 s
  (`adapters/inference-elevenlabs/src/connection.rs`), and no test of the gate asserts it.
  Tests that no entry of this page names yet fire it where a synthesis is dropped against a
  peer that does not take the write. The cancel of a segment does no I/O, so a test of a
  cancel does not reach the bound. See [not-measured.md](not-measured.md).
- Connect, speech to text: `DEFAULT_CONNECT_TIMEOUT` is 10 s
  (`adapters/inference-elevenlabs/src/config.rs`). The test
  `a_handshake_never_answered_fails_within_the_connect_timeout` of
  `adapters/inference-elevenlabs/tests/stt.rs` holds the mechanism, at 500 ms.
- Read, speech to text: `DEFAULT_READ_TIMEOUT`. The test
  `a_server_silent_after_the_end_of_input_with_a_span_open_fails_as_stalled_within_the_bound` of
  `adapters/inference-elevenlabs/tests/stt.rs` holds the mechanism, at 2 s.
- Metadata, agent: `DEFAULT_METADATA_TIMEOUT` is 10 s
  (`adapters/inference-elevenlabs/src/agent.rs`). The test
  `no_metadata_within_the_deadline_fails_the_open_and_closes` of
  `adapters/inference-elevenlabs/tests/agent_duplex.rs` holds the mechanism, at 200 ms.
- Read, agent: the agent's `DEFAULT_READ_TIMEOUT` is 30 s
  (`adapters/inference-elevenlabs/src/agent.rs`). The test
  `a_silent_connection_fails_the_session_at_its_read_deadline` of
  `adapters/inference-elevenlabs/tests/agent_duplex.rs` holds the mechanism, at 300 ms.
- Close, agent: `CLOSE_BOUND`. The test
  `dropping_a_session_mid_stream_closes_the_connection_within_the_close_bound` of
  `adapters/inference-elevenlabs/tests/agent_duplex.rs` compares the time of the close with the
  constant itself.

### `elevenlabs-records/held`

- For a user: the capabilities the documentation states for each binding are the ones the code
  declares, at this version.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 3 of 3 tests. Check run.
- Text to speech: `adapters/inference-elevenlabs/records/claims-tts.json`. The test
  `the_tts_record_holds_the_claims_of_the_binding` of
  `adapters/inference-elevenlabs/tests/claims_record.rs` compares its claims, model, binding,
  features and `held_by` with the code, and its `adapter_revision` with the version of the crate.
- Speech to text: `adapters/inference-elevenlabs/records/claims-stt.json`. The test
  `the_stt_record_holds_the_claims_of_the_binding` of
  `adapters/inference-elevenlabs/tests/claims_record.rs` compares the same fields.
- Agent: `adapters/inference-elevenlabs/records/claims-agent.json`. The test
  `the_agent_record_holds_the_claims_of_the_binding` of
  `adapters/inference-elevenlabs/tests/claims_record.rs` compares the same fields.
- Second holder: `task docs-check` refuses a record whose `adapter_revision` is not the version
  of the workspace (`crates/inference-docs/src/adapters.rs`).

### `elevenlabs-live-example/first-audio`

- For a user: how long a caller waits for the first sound of an utterance.
- Command: `cargo run -p inference-elevenlabs --features live --example live`. It prints
  `first audio after` once per output format.
- Needs: the feature `live`; `ELEVENLABS_API_KEY` or the key in the OS keychain;
  `ELEVENLABS_VOICE_ID`. Optional: `ELEVENLABS_RESIDENCY`, for a residency key.
- Billed: yes, by ElevenLabs.
- Unit and direction: milliseconds; lower is better.
- Clock: from the return of `start()`, before the first segment is offered, to the first audio
  frame. The connection opens for the first segment, so the connect is inside.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `elevenlabs-cancel-probe/first-audio`

- For a user: how long the endpoint takes from the text to its first sound, without the
  adapter's synthesiser in between.
- Command: `cargo test -p inference-elevenlabs --features live --test tts_cancel_live --
  --ignored --nocapture`. The test `tts_cancel_probe_prints_the_endpoints_context_behaviour` of
  `adapters/inference-elevenlabs/tests/tts_cancel_live.rs` prints it in the line `P1 result`.
- Needs: the feature `live`; `ELEVENLABS_API_KEY` or the key in the OS keychain;
  `ELEVENLABS_VOICE_ID`. Optional: `ELEVENLABS_RESIDENCY`, for a residency key.
- Billed: yes, by ElevenLabs.
- Unit and direction: milliseconds; lower is better.
- Clock: from the send of the text on a socket that is already open to the first audio message.
  The connect is not inside. The probe speaks the protocol itself and asserts nothing about the
  endpoint.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

The other live tests of this adapter print no latency. See [not-measured.md](not-measured.md).

## `inference-llama-server`

Generates text with a llama.cpp `llama-server` that runs on the same machine.

| Measure | Baseline | Read from |
|---|---|---|
| `adapter-tests/tests-run` | 47 tests | Check run |
| `adapter-tests/tests-passed` | 47 tests | Check run |
| `adapter-tests/slowest-test` | 0.314 s, `inference-llama-server::cancellation a_server_silent_past_the_idle_limit_cancels_the_generation_with_the_deadline_error` | Check run |

### `llama-server-time-bounds/held`

- For a user: a generation that the server stops feeding fails after a stated time and does not
  hang.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 1 of 1 tests. Check run.
- Idle: the crate has no constant. The limits are those of the HTTP transport it is given. The
  test `a_server_silent_past_the_idle_limit_cancels_the_generation_with_the_deadline_error` of
  `adapters/inference-llama-server/tests/cancellation.rs` holds the mechanism, at 300 ms. No test
  holds the default limits: see [not-measured.md](not-measured.md).

### `llama-server-records/held`

- For a user: the capability records the documentation shows for the adapter fit what the
  adapter declares.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 1 of 1 tests. Check run.
- Both records: `adapters/inference-llama-server/capability/reference-machine-cpu.json` and
  `adapters/inference-llama-server/capability/reference-machine-cpu-tools.json`. The test
  `each_committed_capability_record_checks_against_the_declaration_of_its_adapter` of
  `adapters/inference-llama-server/tests/offline.rs` holds that the directory has exactly these
  two files, that each loads against its adapter, that neither states a GPU or an audio format,
  that `--jinja` stands exactly in the one for tools, and that every evidence test exists.
- Not compared by the test: the `adapter_revision`, and the load time and peak memory of the
  resource profile. No test reads those two numbers.
- Second holder: `task docs-check` refuses a record whose `adapter_revision` is not the version
  of the workspace (`crates/inference-docs/src/adapters.rs`). It is the only holder of that
  field for these two records.

The three live commands need a `llama-server` started by hand. They are not billed. Each test
returns at once when a variable is not set, and prints that it skipped.

### `llama-server-live/first-token`

- For a user: how long a turn waits for the first word of the answer.
- Command: `cargo test -p inference-llama-server --features live --test live -- --ignored
  --nocapture`. The test
  `a_running_server_streams_a_generation_and_refuses_input_during_output` of
  `adapters/inference-llama-server/tests/live.rs` prints it as `first after`.
- Needs: the feature `live`; `INFERENCE_LLAMA_SERVER_URL`; `INFERENCE_LLAMA_SERVER_MODEL`.
- Billed: no.
- Unit and direction: milliseconds; lower is better. The test prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: from before the call that starts the generation to the first text delta. The HTTP
  request to the server is inside.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `llama-server-live/generation`

- For a user: how long a whole answer takes.
- Command: `cargo test -p inference-llama-server --features live --test live -- --ignored
  --nocapture`. The test
  `a_running_server_streams_a_generation_and_refuses_input_during_output` of
  `adapters/inference-llama-server/tests/live.rs` prints it as `completed after`.
- Needs: the feature `live`; `INFERENCE_LLAMA_SERVER_URL`; `INFERENCE_LLAMA_SERVER_MODEL`.
- Billed: no.
- Unit and direction: milliseconds; lower is better. The test prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: from before the call that starts the generation to the event that completes it. The
  value depends on the length of the answer.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `llama-server-live-cancellation/first-token`

- For a user: how long a turn waits for the first word of the answer, in the run that is then
  cancelled.
- Command: `cargo test -p inference-llama-server --features live --test live_cancellation --
  --ignored --nocapture`. The test
  `a_running_server_cancelled_after_the_first_delta_delivers_no_more_text_and_the_cut_turns_record`
  of `adapters/inference-llama-server/tests/live_cancellation.rs` prints it as `first delta`.
- Needs: the feature `live`; `INFERENCE_LLAMA_SERVER_URL`; `INFERENCE_LLAMA_SERVER_MODEL`.
- Billed: no.
- Unit and direction: milliseconds; lower is better. The test prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: from the start of the generation to the first text delta.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `llama-server-live-cancellation/slots-idle`

- For a user: how long the server keeps computing after a turn was cancelled.
- Command: `cargo test -p inference-llama-server --features live --test live_cancellation --
  --ignored --nocapture`. The test
  `a_running_server_cancelled_after_the_first_delta_delivers_no_more_text_and_the_cut_turns_record`
  of `adapters/inference-llama-server/tests/live_cancellation.rs` prints it as
  `no slot processing`.
- Needs: the feature `live`; `INFERENCE_LLAMA_SERVER_URL`; `INFERENCE_LLAMA_SERVER_MODEL`.
- Billed: no.
- Unit and direction: milliseconds; lower is better. The test prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: from the return of the cancel to the first poll of the server that shows no slot at
  work. The first poll comes after the test has drained and dropped the handle. Each round is
  one request for `/slots` and, while a slot is at work, a sleep of twenty milliseconds.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `llama-server-live-tools/tool-proposal`

- For a user: how long a turn waits until the model has proposed a tool call.
- Command: `cargo test -p inference-llama-server --features live,tools --test live_tools --
  --ignored --nocapture`. The test
  `a_running_server_proposes_a_tool_call_and_completes_after_its_result_on_one_generation` of
  `adapters/inference-llama-server/tests/live_tools.rs` prints it in the line `proposal`.
- Needs: the features `live` and `tools`; a server started with `--jinja`;
  `INFERENCE_LLAMA_SERVER_URL`; `INFERENCE_LLAMA_SERVER_MODEL`.
- Billed: no.
- Unit and direction: milliseconds; lower is better. The test prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: from the start of the generation to the proposal, which the test takes once the stream
  of the first turn has ended.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `llama-server-live-tools/generation`

- For a user: how long an answer takes that calls one tool on the way.
- Command: `cargo test -p inference-llama-server --features live,tools --test live_tools --
  --ignored --nocapture`. The test
  `a_running_server_proposes_a_tool_call_and_completes_after_its_result_on_one_generation` of
  `adapters/inference-llama-server/tests/live_tools.rs` prints it as `completed after`.
- Needs: the features `live` and `tools`; a server started with `--jinja`;
  `INFERENCE_LLAMA_SERVER_URL`; `INFERENCE_LLAMA_SERVER_MODEL`.
- Billed: no.
- Unit and direction: milliseconds; lower is better. The test prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: from the start of the generation to the moment the test prints the value. That is
  after the second turn, the one that follows the result of the tool, has ended, and after a
  request for `/props` that the test makes for the line it prints first. The request is inside.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

## `inference-openai`

Speech in and speech out with OpenAI Realtime and GPT-Live and, with the feature `speech`, text
into speech over the speech endpoint.

| Measure | Baseline | Read from |
|---|---|---|
| `adapter-tests/tests-run` | 349 tests | Check run |
| `adapter-tests/tests-passed` | 349 tests | Check run |
| `adapter-tests/slowest-test` | 2.607 s, `inference-openai::adversary16_u1_live_split adversary16_live_close_after_a_stalled_write_keeps_its_documented_bound` | Check run |

### `openai-time-bounds/held`

- For a user: a connection that does not open, stops reading or goes silent fails after a stated
  time, and a close gives up after a stated time.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 11 of 11 tests. Check run.
- Connect and write, defaults: `DEFAULT_CONNECT_TIMEOUT` is 10 s and `DEFAULT_WRITE_TIMEOUT` is
  10 s (`adapters/inference-openai/src/config.rs`). The test
  `the_deadlines_are_stated_and_settable` of
  `adapters/inference-openai/tests/transport_deadlines.rs` holds that the defaults are the two
  constants and that each is over zero and under 80 s. It does not fix the numbers.
- Connect, Realtime: the test
  `a_server_that_never_answers_the_upgrade_fails_connect_within_the_deadline` of
  `adapters/inference-openai/tests/transport_deadlines.rs` holds the mechanism, at 300 ms.
- Connect, GPT-Live: the test
  `a_server_that_never_answers_the_upgrade_fails_live_connect_within_the_deadline` of
  `adapters/inference-openai/tests/transport_deadlines.rs` holds the mechanism, at 300 ms.
- Write, Realtime: the test `a_peer_that_stops_reading_fails_a_write_within_the_deadline` of
  `adapters/inference-openai/tests/transport_deadlines.rs` holds the mechanism, at 300 ms.
- Read, default: `DEFAULT_READ_TIMEOUT` is 30 s. The test
  `endpoint_is_built_and_destructured_with_its_0_16_0_fields` of
  `adapters/inference-openai/tests/endpoint_literal.rs` asserts the value.
- Read, Realtime: the test `realtime_stall_bound_holds_across_dropped_reads` of
  `adapters/inference-openai/tests/stall_bound.rs` holds the mechanism, at 400 ms.
- Read, GPT-Live: the test `live_stall_bound_holds_across_dropped_reads` of
  `adapters/inference-openai/tests/gpt_live_stall_bound.rs` holds the mechanism, at 400 ms.
- Read, speech: the test `a_server_that_never_answers_stalls_within_the_read_timeout` of
  `adapters/inference-openai/tests/adversary_rf1_speech_transport.rs` holds the mechanism, at
  1 s.
- Idle, Realtime session: the idle limit of the default session configuration is 30 s
  (`adapters/inference-openai/src/duplex.rs`). The test
  `a_silent_provider_fails_the_session_as_stalled_at_the_bound` of
  `adapters/inference-openai/tests/duplex.rs` holds the mechanism, at 400 ms.
- Close, GPT-Live, default: `CLOSE_TIMEOUT` is 15 s (`adapters/inference-openai/src/live.rs`).
  The test `a_client_keeps_the_documented_close_timeout_unless_given_another` of
  `adapters/inference-openai/tests/gpt_live.rs` asserts the value.
- Close, GPT-Live, fired: the same constant. The test
  `close_gives_up_at_its_bound_across_every_read` of `adapters/inference-openai/tests/gpt_live.rs`
  holds the mechanism, at 2 s.

### `openai-records/held`

- For a user: the capabilities the documentation states for each binding are the ones the code
  declares, at this version.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 3 of 3 tests. Check run.
- Realtime: `adapters/inference-openai/records/claims-realtime.json`. The test
  `the_realtime_record_holds_the_claims_of_the_binding` of
  `adapters/inference-openai/tests/claims_record.rs` compares its claims, model, binding,
  features and `held_by` with the code, and its `adapter_revision` with the version of the crate.
- GPT-Live: `adapters/inference-openai/records/claims-gpt-live.json`. The test
  `the_gpt_live_record_holds_the_claims_of_the_binding` of
  `adapters/inference-openai/tests/claims_record.rs` compares the same fields.
- Speech: `adapters/inference-openai/records/claims-speech.json`. The test
  `the_speech_record_holds_the_claims_of_the_binding` of
  `adapters/inference-openai/tests/speech_claims_record.rs` compares the same fields.
- Second holder: `task docs-check` refuses a record whose `adapter_revision` is not the version
  of the workspace (`crates/inference-docs/src/adapters.rs`).

### `openai-gpt-live/session-start`

- For a user: how long a caller waits until a GPT-Live session is ready. This is the connect time
  of the binding.
- Command: `cargo test -p inference-openai --features live,gpt-live --test live_gpt -- --ignored
  --nocapture`. The test `the_client_hears_speech_and_answers_with_audio_and_transcript` of
  `adapters/inference-openai/tests/live_gpt.rs` prints it as `started in`. The command runs the
  other ignored tests of the file too.
- Needs: the features `live` and `gpt-live`; the key in the OS keychain, or
  `INFERENCE_OPENAI_KEY_SOURCE` set to `env` and `OPENAI_API_KEY`. Optional:
  `INFERENCE_OPENAI_SECRET_NAME`, another name of the secret in the keychain.
- Billed: yes, by OpenAI, per second of session.
- Unit and direction: milliseconds; lower is better.
- Clock: from before the call that connects the client to its return. The lookup of the
  credential, the connect and the start of the session up to the server's confirmation are
  inside.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `openai-gpt-live/session-close`

- For a user: how long a GPT-Live session takes to close.
- Command: `cargo test -p inference-openai --features live,gpt-live --test live_gpt -- --ignored
  --nocapture`. The test `the_client_hears_speech_and_answers_with_audio_and_transcript` of
  `adapters/inference-openai/tests/live_gpt.rs` prints it as `closed in`.
- Needs: the features `live` and `gpt-live`; the key in the OS keychain, or
  `INFERENCE_OPENAI_KEY_SOURCE` set to `env` and `OPENAI_API_KEY`. Optional:
  `INFERENCE_OPENAI_SECRET_NAME`, another name of the secret in the keychain.
- Billed: yes, by OpenAI, per second of session.
- Unit and direction: milliseconds; lower is better.
- Clock: from before the call that closes the client to its return.
- Bound: none. Nothing fails on this value. `CLOSE_TIMEOUT` ends the wait: see
  `openai-time-bounds/held`.
- Baseline: none. No run of this command is kept.

### `openai-speech-live/first-audio`

- For a user: how long a caller waits for the first sound of an utterance.
- Command: `cargo test -p inference-openai --features live,speech --test speech_live -- --ignored
  --nocapture`. The test `speaks_an_utterance_as_mulaw_8k` of
  `adapters/inference-openai/tests/speech_live.rs` prints it as `first frame after`.
- Needs: the features `live` and `speech`; the key in the OS keychain, or
  `INFERENCE_OPENAI_KEY_SOURCE` set to `env` and `OPENAI_API_KEY`. Optional:
  `INFERENCE_OPENAI_SECRET_NAME`, another name of the secret in the keychain.
- Billed: yes, by OpenAI.
- Unit and direction: milliseconds; lower is better. The test prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: from before the start of the synthesis to the first audio frame. The HTTP request and
  its connect are inside.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

The live tests of Realtime print no latency. See [not-measured.md](not-measured.md).

## `inference-phonic`

Speech in and speech out with Phonic over its realtime WebSocket.

| Measure | Baseline | Read from |
|---|---|---|
| `adapter-tests/tests-run` | 31 tests | Check run |
| `adapter-tests/tests-passed` | 31 tests | Check run |
| `adapter-tests/slowest-test` | 1.857 s, `inference-phonic::adversary_rf1_phonic adversary_rf1_a_caller_running_its_tool_past_the_idle_limit_is_not_a_stalled_provider` | Check run |

### `phonic-time-bounds/held`

- For a user: a session that does not get ready or goes deaf fails after a stated time, and a
  dropped session closes its connection at once.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 3 of 3 tests. Check run.
- Ready: `DEFAULT_READY_TIMEOUT` is 10 s (`adapters/inference-phonic/src/protocol.rs`). The test
  `no_ready_within_the_deadline_fails_the_open_and_closes_the_connection` of
  `adapters/inference-phonic/tests/duplex.rs` holds the mechanism, at 200 ms.
- Read and ping: `DEFAULT_READ_TIMEOUT` is 30 s and `DEFAULT_PING_INTERVAL` is 10 s. The test
  `a_quiet_server_that_answers_pings_stays_open_and_a_deaf_one_stalls` of
  `adapters/inference-phonic/tests/duplex.rs` holds the mechanism, at 400 ms and 100 ms.
- Close: `CLOSE_BOUND` is 1 s (`adapters/inference-phonic/src/duplex.rs`). The test
  `dropping_a_session_mid_stream_closes_the_connection_within_the_close_bound` of
  `adapters/inference-phonic/tests/duplex.rs` compares the time of the close with the constant
  itself.

### `phonic-records/held`

- For a user: the capabilities the documentation states for the binding are the ones the code
  declares, at this version.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 1 of 1 tests. Check run.
- Speech to speech: `adapters/inference-phonic/records/claims-duplex.json`. The test
  `the_duplex_record_holds_the_claims_of_the_binding` of
  `adapters/inference-phonic/tests/claims_record.rs` compares its claims, model, binding,
  features and `held_by` with the code, and its `adapter_revision` with the version of the crate.
- Second holder: `task docs-check` refuses a record whose `adapter_revision` is not the version
  of the workspace (`crates/inference-docs/src/adapters.rs`).

No command of this adapter prints a latency. See [not-measured.md](not-measured.md).

## `inference-sherpa-onnx`

Turns speech into text on the local CPU with the sherpa-onnx runtime, as the audio arrives.

| Measure | Baseline | Read from |
|---|---|---|
| `adapter-tests/tests-run` | 40 tests | Check run |
| `adapter-tests/tests-passed` | 40 tests | Check run |
| `adapter-tests/slowest-test` | 0.494 s, `inference-sherpa-onnx::stt_network the_archive_download_is_reached_only_through_build_dependencies` | Check run |

The crate has no time bound as a constant. Its measures of load time, real-time factor and peak
memory are in [README.md](README.md#local-speech-to-text-adapters), with the committed record
`adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json` as their baseline. The test
`the_record_of_the_reference_machine_loads_and_evidences_exactly_the_claims` of
`adapters/inference-sherpa-onnx/tests/stt_record.rs` compares the `adapter_revision` of that
record with the version of the crate, and `task docs-check` compares it with the version of the
workspace. `sherpa-onnx-stt-record/real-time-factor` holds the bound on its real-time factor.

## `inference-whispercpp`

Turns speech into text on the local CPU with whisper.cpp, one turn at a time.

| Measure | Baseline | Read from |
|---|---|---|
| `adapter-tests/tests-run` | 42 tests | Check run |
| `adapter-tests/tests-passed` | 42 tests | Check run |
| `adapter-tests/slowest-test` | 0.334 s, `inference-whispercpp::stt_network the_build_downloads_nothing` | Check run |

The crate has no time bound as a constant. Its measures of load time, real-time factor and peak
memory are in [README.md](README.md#local-speech-to-text-adapters), with the committed record
`adapters/inference-whispercpp/records/stt-i9-10900k-cpu.json` as their baseline. The test
`the_record_of_the_reference_machine_loads_and_names_no_evidence_for_no_claim` of
`adapters/inference-whispercpp/tests/stt_record.rs` compares the `adapter_revision` of that
record with the version of the crate, and `task docs-check` compares it with the version of the
workspace. `whispercpp-stt-record/real-time-factor` holds the bound on its real-time factor.

## Examples that run several adapters

Two examples measure a whole turn across adapters. Their measures have no adapter section.

### `compare/first-audio`

- For a user: how long a caller waits for the first sound of the answer, per provider.
- Command: `cargo run -p realtime-voice --example compare -- --providers
  openai,openai-live,nova,elevenlabs,deepgram,phonic,deepslate --runs 2 --runs-out runs.jsonl`.
  It writes one JSON line per run to the file, with the key `first_audio_ms`.
- Needs: the key or the credentials of each provider that is named, as the head of
  `examples/realtime-voice/examples/compare.rs` lists them. OpenAI: the key in the OS keychain,
  or `INFERENCE_OPENAI_KEY_SOURCE` set to `env` and `OPENAI_API_KEY`. AWS: credentials and a
  region from the default chain of the AWS SDK, such as `AWS_REGION` or a profile. ElevenLabs:
  `ELEVENLABS_AGENT_ID`, or `ELEVENLABS_AGENT_SIGNED_URL`, which opens one session: with it
  the arm measures its first run and records each later run as refused. Deepgram:
  `DEEPGRAM_API_KEY`. Phonic:
  `PHONIC_API_KEY`. Deepslate: `DEEPSLATE_API_KEY`, `DEEPSLATE_VENDOR_ID`,
  `DEEPSLATE_ORGANIZATION_ID` and `DEEPSLATE_VOICE_ID`. Optional, each for another model:
  `INFERENCE_OPENAI_REALTIME_MODEL`, `INFERENCE_NOVA_SONIC_MODEL`, `DEEPSLATE_MODEL`.
- Billed: yes, by every hosted provider that runs.
- Unit and direction: milliseconds; lower is better.
- Clock: from the end of the input speech, the instant the client sent the frame with the last
  voiced sample of the question, to the receipt of the first audio of the answer on the client.
  The session is opened before and is not timed, so the connect is not inside.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `compare/first-transcript`

- For a user: how long until the first text of the answer is there, per provider.
- Command: `cargo run -p realtime-voice --example compare -- --providers
  openai,openai-live,nova,elevenlabs,deepgram,phonic,deepslate --runs 2 --runs-out runs.jsonl`.
  The key is `first_transcript_ms`.
- Needs: as `compare/first-audio`.
- Billed: yes, by every hosted provider that runs.
- Unit and direction: milliseconds; lower is better.
- Clock: from the end of the input speech to the receipt of the first transcript of the answer
  on the client. The connect is not inside.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `compare/response-end`

- For a user: how long until the answer has ended, per provider.
- Command: `cargo run -p realtime-voice --example compare -- --providers
  openai,openai-live,nova,elevenlabs,deepgram,phonic,deepslate --runs 2 --runs-out runs.jsonl`.
  The key is `response_end_ms`.
- Needs: as `compare/first-audio`.
- Billed: yes, by every hosted provider that runs.
- Unit and direction: milliseconds; lower is better.
- Clock: from the end of the input speech to the receipt of the end of the answer on the client.
  The value depends on the length of the answer. The connect is not inside.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `compare/barge-in-stop`

- For a user: how long the answer keeps playing after the caller speaks over it, per provider.
- Command: `cargo run -p realtime-voice --example compare -- --providers
  openai,openai-live,nova,elevenlabs,deepgram,phonic,deepslate --runs 2 --runs-out runs.jsonl`.
  The key is `barge_in_stop_ms`.
- Needs: as `compare/first-audio`, and `INFERENCE_COMPARE_BARGE_IN_WAV`, a file of speech.
  Without it no barge-in is sent and the value is missing.
- Billed: yes, by every hosted provider that runs.
- Unit and direction: milliseconds; lower is better.
- Clock: from the send of the first voiced frame of the barge-in to the end of the playout of
  the answer.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `compare-cascade/final-transcript`

- For a user: how long the recogniser of a cascade takes to commit the question.
- Command: `cargo run -p realtime-voice --features local --example compare -- --providers
  cascade --runs 2 --runs-out runs.jsonl`. The key is `final_transcript_ms`. The same run
  writes three keys of `compare` for the cascade: `first_audio_ms`, `response_end_ms` and
  `barge_in_stop_ms`. `first_transcript_ms` is written for a native provider only.
- Needs: the feature `local`; the local recogniser and the `llama-server` that
  `cascade-talk/first-audio` needs; `ELEVENLABS_API_KEY` or the key in the OS keychain, and
  `ELEVENLABS_VOICE_ID`. Optional: `ELEVENLABS_RESIDENCY`, for a residency key.
- Billed: yes, by ElevenLabs for the speech.
- Unit and direction: milliseconds; lower is better.
- Clock: from the end of the input speech to the final transcript of the recogniser.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `compare-cascade/first-token`

- For a user: how long until the language model of a cascade gives its first word.
- Command: `cargo run -p realtime-voice --features local --example compare -- --providers
  cascade --runs 2 --runs-out runs.jsonl`. The key is `first_token_ms`.
- Needs: as `compare-cascade/final-transcript`.
- Billed: yes, by ElevenLabs for the speech.
- Unit and direction: milliseconds; lower is better.
- Clock: from the end of the input speech to the first token of the language model.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `compare-cascade/tts-first-audio`

- For a user: how long until the synthesiser of a cascade gives its first sound.
- Command: `cargo run -p realtime-voice --features local --example compare -- --providers
  cascade --runs 2 --runs-out runs.jsonl`. The key is `tts_first_audio_ms`.
- Needs: as `compare-cascade/final-transcript`.
- Billed: yes, by ElevenLabs for the speech.
- Unit and direction: milliseconds; lower is better.
- Clock: from the end of the input speech to the first audio of the synthesiser.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `compare-cascade/tts-offer-to-audio`

- For a user: how long the first sentence of the answer waits for its first sound, inside the
  synthesiser.
- Command: `cargo run -p realtime-voice --features local --example compare -- --providers
  cascade --runs 2 --runs-out runs.jsonl`. The key is `tts_offer_to_audio_ms`.
- Needs: as `compare-cascade/final-transcript`.
- Billed: yes, by ElevenLabs for the speech.
- Unit and direction: milliseconds; lower is better.
- Clock: from the offer of the first segment of the answer to its first frame, as the contract
  of the synthesiser measures it. It is the first value of `tts_segment_first_audio_ms`.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `compare-cascade/tts-segment-first-audio`

- For a user: how long each sentence of the answer waits for its first sound.
- Command: `cargo run -p realtime-voice --features local --example compare -- --providers
  cascade --runs 2 --runs-out runs.jsonl`. The key is `tts_segment_first_audio_ms`, a list with
  one value per segment.
- Needs: as `compare-cascade/final-transcript`.
- Billed: yes, by ElevenLabs for the speech.
- Unit and direction: milliseconds; lower is better.
- Clock: from the offer of a segment to its first frame, as the contract of the synthesiser
  measures it.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `cascade-talk/first-audio`

- For a user: how long a speaker waits for the answer of the local cascade to start.
- Command: `cargo run -p local-voice-cascade --features local --example talk`. It prints
  `committed input to first playable audio` after each turn.
- Needs: the feature `local`; a microphone and a speaker; `INFERENCE_SHERPA_ONNX_STT_DIR`, or
  `INFERENCE_CASCADE_STT` and `INFERENCE_WHISPERCPP_STT_MODEL` for whisper.cpp;
  `INFERENCE_LLAMA_SERVER_URL`; `INFERENCE_LLAMA_SERVER_MODEL`.
- Billed: no.
- Unit and direction: milliseconds; lower is better. The example prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: from the moment the input gate passes the end of the turn to the moment the first
  frame of the answer is handed to the speaker. The synthesiser is a mock, so no speech
  synthesis is inside.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `cascade-talk/interruption-stop`

- For a user: how long the answer keeps playing after the speaker talks over it.
- Command: `cargo run -p local-voice-cascade --features local --example talk`. It prints
  `interruption to playback stop` after each interruption.
- Needs: as `cascade-talk/first-audio`.
- Billed: no.
- Unit and direction: milliseconds; lower is better. The example prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: from the moment the output takes the opening of the new turn to the return of the stop
  of the speaker.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `cascade-local-adapters/first-audio`

- For a user: how long the answer of the local cascade takes to start, on a recorded clip and
  without an audio device.
- Command: `cargo test -p local-voice-cascade --features local --test local_adapters --
  --ignored --nocapture`. The test
  `the_clip_is_answered_offline_and_every_segment_is_played_through_receipts` of
  `examples/local-voice-cascade/tests/local_adapters.rs` prints it, and so do the other two
  ignored tests of the file.
- Needs: the feature `local`; `INFERENCE_SHERPA_ONNX_STT_DIR` with the clip;
  `INFERENCE_LLAMA_SERVER_URL`; `INFERENCE_LLAMA_SERVER_MODEL`. Optional:
  `INFERENCE_CASCADE_STT` selects the recogniser, sherpa-onnx when it is unset; whisper.cpp
  then needs `INFERENCE_WHISPERCPP_STT_MODEL`.
- Billed: no.
- Unit and direction: milliseconds; lower is better. The test prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: as `cascade-talk/first-audio`, with a fake audio host in place of the devices.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.

### `cascade-local-adapters/interruption-stop`

- For a user: how long the answer keeps playing after the clip is said again over it.
- Command: `cargo test -p local-voice-cascade --features local --test local_adapters --
  --ignored --nocapture`. The test
  `the_clip_said_again_while_the_answer_plays_stops_it_and_supersedes_its_generation` of
  `examples/local-voice-cascade/tests/local_adapters.rs` is the one with an interruption.
- Needs: as `cascade-local-adapters/first-audio`.
- Billed: no.
- Unit and direction: milliseconds; lower is better. The test prints a Rust `Duration` in its
  debug form, so the printed unit changes with the size of the value.
- Clock: as `cascade-talk/interruption-stop`, with a fake audio host in place of the devices.
- Bound: none. Nothing fails on this value.
- Baseline: none. No run of this command is kept.
