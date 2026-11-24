---
format: aep.planning-md/3
id: story:ticker-tests-hold-on-a-loaded-host
kind: story
status: active
title: The two ticker tests of the ElevenLabs async handles pass on a loaded host and still catch a blocked runtime
tags:
- from-finding
relations:
- decomposes: epic:defects-2026-11-10
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-elevenlabs/tests/async_handles.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T13:41:12Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T13:41:12Z", actor: "human:timo", revision: 3}
---
## Context

Two tests of `inference-elevenlabs` fail on the shared build node and pass on the forge's runner:
`twenty_syntheses_wait_for_the_server_without_blocking_the_runtime` and
`twenty_recognitions_wait_for_the_server_without_blocking_the_runtime`
(`adapters/inference-elevenlabs/tests/async_handles.rs:134` and `:192`). Each runs a 10 ms ticker
task on a current-thread runtime and asserts that the ticker's largest gap stays under 50 ms.

Measured on 2026-11-24, on `main` at `21cab7876` and on two unit branches alike:

- in the package lane, three runs: gaps of 75 to 95 ms, both tests fail (report of the implementor
  of story:signed-url-token-copies; one job log read `host busy: CPU pressure 71.2 %`);
- alone, three runs: 2 of 3 fail, "the ticker's largest gap 120 ms" (report of the implementor of
  story:test-run-step-under-budget);
- `task test-run` on the node exits 201 on the base for these two tests only, 4036 of 4038 pass.

The tests passed on the runner on pull request 61. The cause is not established: load on the node
is the reading of both implementors, not a measurement.

The consequence: the local test step is red on `main` on the build node, so a green local gate
cannot be shown for any change. story:synthesiser-drop-inside-runtime adds tests with the same
ticker and the same bound.

## Planned scope

- Find what the gap is on the node: the runtime blocked by the code under test, or the test's
  thread not scheduled by the host.
- The two tests tell those apart: a host that schedules late does not fail them, and a call that
  blocks the runtime still does.

## Acceptance

- `cargo nextest run -p inference-elevenlabs --all-features --test async_handles` passes 10 of 10
  runs on the build node.
- A change that blocks the runtime for 200 ms inside a synthesis or a recognition fails both tests.

Corrected on 2026-11-24: the first line read `... --all-features async_handles`, which selects no
test (the filter matches test names, not the binary; the unit's implementor ran it on the base:
"Starting 0 tests across 32 binaries (168 tests skipped)", exit 4).

## Out of scope

- The 50 ms bound of story:synthesiser-drop-inside-runtime's acceptance.
- A retry of the tests by the runner.

## Cause, established 2026-11-24

From the report of the unit's implementor, 2026-11-24, measured on the build node (64 CPUs, load
average 50 in the run quoted); the probe logs stay in the session's scratch directory and are not
committed.

- Linux grows a process's descriptor table when descriptor 64, 128 and 256 is first opened, and the
  opening thread waits for an RCU grace period inside that call. On the node that wait is 108 to
  142 ms. Twenty sessions hold about 130 descriptors, so two first polls meet it.
- Without the adapter and without tokio: 300 times `File::open("/dev/null")` gave "#57 (descriptor
  64) 116.26ms, #121 (descriptor 128) 109.58ms, #249 (descriptor 256) 119.25ms", the thread in state
  D at `__wait_rcu_gp`; the same 300 again after closing: "max 0.03ms".
- Twenty syntheses: "largest gap 254.23ms ... run-queue wait 0.00ms", with the first poll of two
  sessions taking 142.11 ms and 107.66 ms. With the table grown first: 11.89 ms (syntheses) and
  11.55 ms (recognitions).
- A second, smaller source, from one probe at load average 107: 104.37 ms of run-queue wait inside
  one gap of twenty syntheses.
- The agent test of the same file passes because it opens its sessions before the ticker starts.
- A control ticker on another thread does not show the wait (its largest gap stayed 10.35 ms while
  the runtime's thread waited 254 ms): only a thread that opens a descriptor waits.

The coordinator's reading: the wait is the host kernel's, inside a call any handle makes when it
opens a descriptor on the runtime's thread; it is not a blocking wait of the adapter's own. Why the
grace period is that long on this node and not on the forge's runner is not established.
