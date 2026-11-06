---
format: aep.planning-md/3
id: review-result:adversary-cartesia-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the Cartesia streaming TTS adapter
relations:
- reviews: story:cartesia-streaming-tts-adapter
revision: 1
---
unit: ADP-cartesia story:cartesia-streaming-tts-adapter, tree wt-38be68e170b8 at 9d49440 + adversary commit 6eeec32
verdict: NEEDS-CHANGE
cases: executed 23→29, red 1
origin: introduced 1 / pre-existing 0 / undecided 0
wrote-outside-worktree: 6 files in the unit scratch directory (listed in part 6)
needs-coordinator: the docs-bindings patch and the environment page are still open (see 4); otherwise no

One new case is red: when Cartesia sends a message type the binding doesn't know, the whole synthesis fails. The other five new cases pass.

**1. Diff since 9d49440** (`git --no-pager diff --stat 9d49440 HEAD`)
```
 .../tests/adversary_rf1_protocol.rs                | 499 +++++++++++++++++++++
 1 file changed, 499 insertions(+)
```
I only added this test file, committed as 6eeec32 by `b10x-bot[bot]`. No implementation file was changed. The file has its own scripted loopback server, and every case runs under `support::within`, which fails it at 25 s.

**2. Cases added** (`cargo-run.sh test -p inference-cartesia --test adversary_rf1_protocol`, run alone first: 5 passed, 1 failed, rc=101, secs=14)
- **RED** `a_message_type_added_within_the_pinned_version_does_not_fail_the_segment`:
  - Setup: an unknown `type` with a valid `context_id` arrives between two chunks.
  - Output: `panicked at adapters/inference-cartesia/tests/adversary_rf1_protocol.rs:223:9: an output type added within the pinned version failed the synthesis: Some(Protocol("a message of an unknown `type`"))`
- **green** `a_cancelled_contexts_late_messages_interleaved_with_the_next_context_never_reach_the_caller`: the cancelled context's late messages (a chunk, the final chunk, `done` and an `error`) arrive mixed in with the next context's chunks. None reaches the caller, and the next segment comes through whole.
- **green** `samples_split_into_one_and_two_byte_chunks_come_back_whole`: s16le and f32le samples split into chunks of 0, 1, 2, 3 and 5 bytes come back byte for byte.
- **green** `segments_offered_while_one_speaks_and_after_an_idle_gap_are_all_spoken_in_order`: segments offered while another plays, plus one offered after an idle gap of 3× the read timeout, are all synthesized in order.
- **green** `a_server_closing_between_segments_fails_the_next_segment_promptly`: the next segment fails in under 3 s, which matches what `protocol.md` says should happen.
- **green** `the_key_is_in_no_debug_and_no_error`

**3. Suite run** (after the cases existed: `cargo-run.sh test --no-fail-fast -p inference-cartesia --lib --test conformance --test cancel --test errors --test output_formats --test claims_record --test adversary_rf1_protocol`, rc=101, secs=2)

| target | passed | failed |
|---|---|---|
| lib | 3 | 0 |
| adversary_rf1_protocol | 5 | 1 |
| cancel | 3 | 0 |
| claims_record | 1 | 0 |
| conformance | 7 | 0 |
| errors | 6 | 0 |
| output_formats | 3 | 0 |

- `executed <before>` = 23: the same run with my target left out.
- `cargo fmt --check` passed, and clippy `--all-targets -D warnings` returned rc=0.
- Cargo time was about 0.5 min, wall time about 25 min, peak `target/` 473M.
- I ran `worktree discard-cache` on the tree and released my lease.

**4. Judgement findings** (they cover 9d49440)
- **Unknown message type** (`src/protocol.rs:204`). NEEDS-CHANGE, introduced.
  - Cartesia's API conventions page lists adding "variants to enum-like output values" as a non-breaking change within a pinned `Cartesia-Version`. `type` is one of those values, so the binding should expect new types without a version change.
  - Today an unknown `type` fails the whole synthesis.
  - Suggested fix: read an unknown `type` that carries a `context_id` as `ServerMessage::Ignored`, as the ElevenLabs agent parser already does (`other => Ignored`), and record the decision in `docs/protocol.md`.
- **One error ends everything** (`src/synthesiser.rs:313`). CONFIRMED note, introduced.
  - An `error` for the segment being spoken fails the whole synthesis, so every later segment is taken but never spoken. A per-context error such as `language_not_supported` therefore ends the utterance.
  - `protocol.md` documents this choice. It is not against the acceptance criteria.
- **Generated docs missing Cartesia.** CONFIRMED note, introduced.
  - `website/docs/reference/adapters.md` and `website/docs/reference/environment.md` don't mention Cartesia. Missing entries: `CARTESIA_API_KEY`, `CARTESIA_VOICE_ID`, `--cascade-voice`.
  - The scratch `inference-docs-bindings.patch` is still unapplied. Fixing this falls to the coordinator.

**5. Attacked and not broken**
- The request fields checked against the reference: `model_id` `sonic-3.6`, voice, `output_format` with `raw` and the four encodings at the six rates, `continue:false`, and `cartesia_version=2026-08-14`.
- `X-API-Key` sent as a sensitive header.
- Two orders of the end-of-segment messages: a chunk marked `done: true` followed by a `done` message, and a `done` message alone.
- A cancelled context's late messages interleaved with the next context's.
- Samples split across chunks.
- The handle being dropped (the existing test covers it).
- A gap longer than the read timeout between segments.
- Segments offered back to back, none lost.
- The plaintext `ws://` check for loopback only, including the `@` and `[::1]evil` tricks.
- The key in `Debug` and in errors.
- Usage: no message from the server reports billed characters, so the binding's own count is the only one.
- The `compare` wiring: the default stays ElevenLabs, and the Cartesia path uses PCM s16le at 24 kHz.

**6. Paths written outside the worktree** (all under `.cache/scratch/9f5a4edc-f114-47a7-863c-55ffc5ec02c6/wave-0.17.0/ADP-cartesia/`)
- `adv-rf1-red.log`
- `adv-rf1-suite.log`
- `adv-rf1-clippy.log`
- `adv-rf1-discard.log`
- `msg-adv-rf1.txt`
- `.session-cargo.lock`: the wrapper's lock, which sits one level up in the wave directory.

```findings
[
  {"file": "adapters/inference-cartesia/src/protocol.rs", "line": 204, "category": "contract-drift", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "an unknown server message type fails the whole synthesis although the API conventions allow new output enum variants within a pinned version; red test adversary_rf1_protocol::a_message_type_added_within_the_pinned_version_does_not_fail_the_segment"},
  {"file": "adapters/inference-cartesia/src/synthesiser.rs", "line": 313, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "an error naming the live context fails the whole synthesis, so later segments of the utterance are taken and never spoken; documented in protocol.md, no red test"},
  {"file": "website/docs/reference/environment.md", "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "CARTESIA_API_KEY, CARTESIA_VOICE_ID and --cascade-voice are not on the environment or adapters reference pages; the inference-docs bindings patch in scratch is unapplied; no red test"}
]
```
