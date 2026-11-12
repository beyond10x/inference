---
format: aep.planning-md/3
id: review-result:elevenlabs-route-parallel-round-1
kind: review-result
status: active
title: Parallel-safety critic, ElevenLabs route, round 1
relations:
- reviews: story:elevenlabs-realtime-stt-adapter
- reviews: story:elevenlabs-tts-provider-cancel
- reviews: task:release-elevenlabs-route-p1
- reviews: story:elevenlabs-agent-session-overrides
- reviews: story:async-speech-handles
- reviews: story:duplex-session-config-additions
- reviews: story:status-page-voice-adapters
revision: 1
---
needs-revision

- story:elevenlabs-realtime-stt-adapter — both this and story:elevenlabs-tts-provider-cancel land in the `adapters/inference-elevenlabs` crate. That crate holds the crate doc and module list in `src/lib.rs`, `src/error.rs` (the new `ElevenLabsError` variants), `Cargo.toml` and `tests/claims_record.rs`. The cancel body cites `lib.rs:26-31` and the stt body names no file, so the shared files are cited on the cancel side and inferred on the stt side. Neither body says so and no edge orders them, although the epic's P1 order is stt then cancel. Remedies: an ordering edge that records the shared files as its reason, or splitting the surface so they no longer share it. — .engineering/planning/story/elevenlabs-realtime-stt-adapter.md:37-47, .engineering/planning/story/elevenlabs-tts-provider-cancel.md:19-36
- story:elevenlabs-agent-session-overrides — both this and story:duplex-session-config-additions land on `adapters/inference-elevenlabs/src/agent_duplex.rs`. Duplex-session-config adds flags to `SpeechCapabilities` and says "Every duplex binding states its flags", so `agent_capabilities` (`agent_duplex.rs:141-155`) changes (inferred for the file, cited for both bodies' intent). This body's acceptance requires `agent_capabilities` and the claims record "unchanged except for what usage adds", and neither body mentions the other. Remedies: an ordering edge that records the shared file as its reason, or splitting the surface. — .engineering/planning/story/elevenlabs-agent-session-overrides.md:33-41, .engineering/planning/story/duplex-session-config-additions.md:28-29
- story:async-speech-handles — it shares cited files with two other items and names neither. With story:elevenlabs-tts-provider-cancel it shares `crates/inference-tts/src/lib.rs` (`SynthesisHandle` at 1271, `SegmentAudio` at 218). With story:duplex-session-config-additions it shares `crates/inference-duplex/src/contract.rs` (`SpeechSessionHandle` at 361 and `SpeechCapabilities`) and a possible `ess/domains/duplex.yaml` change. It also shares `agent_duplex.rs` with story:elevenlabs-agent-session-overrides (inferred, since "every adapter implementing the three contracts"). Remedies for each pair: an ordering edge that records the shared file as its reason, or splitting the surface. — .engineering/planning/story/async-speech-handles.md:17-30, .engineering/planning/story/elevenlabs-tts-provider-cancel.md:31, .engineering/planning/story/duplex-session-config-additions.md:23-28
- story:async-speech-handles — its surface is three contract crates, "every adapter implementing the three contracts", the testkit mocks and the pipeline nodes, so it collides with every other item in the set. The body needs narrowing, for example one contract per story or an explicit statement that it is worked alone. — .engineering/planning/story/async-speech-handles.md:26-27
- story:async-speech-handles — its acceptance (20 concurrent sessions and the backend-neutral checks through the async form, each "of the three contracts") reads the `Recogniser` binding that story:elevenlabs-realtime-stt-adapter creates, and no `depends_on` edge runs from it to that story. The finding names the missing edge, or merging the two. — .engineering/planning/story/async-speech-handles.md:35-37, .engineering/planning/story/elevenlabs-realtime-stt-adapter.md:37-39
- story:duplex-session-config-additions — it names every duplex binding and a field added to `SpeechCapabilities`. Seven adapters, the testkit mock and `crates/inference/src/stack.rs` construct or consume that type (`git grep SpeechCapabilities`), so the surface is wide and collides with any item touching a duplex binding. The body needs narrowing, or the other items need to record the overlap. Those files are inferred. — .engineering/planning/story/duplex-session-config-additions.md:28-29
- story:status-page-voice-adapters — its acceptance requires each adapter's "Since" to match "the version whose `CHANGELOG.md` section introduced it". For ElevenLabs STT, that section is written by task:release-elevenlabs-route-p1, and the only `depends_on` edge goes to story:elevenlabs-realtime-stt-adapter. Before the release the page can only say "On main, in the next release" (`AGENTS.md:35`). The finding names the missing edge to the release, or merging the two. — .engineering/planning/story/status-page-voice-adapters.md:29-30, .engineering/planning/task/release-elevenlabs-route-p1.md:25-27
- task:release-elevenlabs-route-p1 — it rewrites the version in `Cargo.toml`, the `adapter_revision` of every capability record under `adapters/` and the `CHANGELOG.md` entry. A P2 or P3 story can be worked at the same time, and no edge says it waits for the release or the release waits for it. story:elevenlabs-agent-session-overrides edits `records/claims-agent.json`, and every consumer-visible story adds an `## [Unreleased]` line (`AGENTS.md:41`) in `CHANGELOG.md`. These surfaces are cited for the release and inferred for the stories. Remedies: an ordering edge that records the shared files as its reason, or splitting the surface. — .engineering/planning/task/release-elevenlabs-route-p1.md:24-27

What I read: 7 artifacts and `epic:elevenlabs-route`, with `aep plan artifact show` per id, `aep plan artifact graph`, `aep plan artifact relations` and `aep plan artifact waves --kind story --status draft`. I also read `AGENTS.md`, `adapters/inference-elevenlabs` (`lib.rs`, `Cargo.toml`, `tests/claims_record.rs`, `records/`), `crates/inference-docs/src/adapters.rs` and a `git grep SpeechCapabilities`.

Surface counts: 7 cited, 0 inferred, 0 unplaceable. Some individual surfaces inside those items are inferred, and each finding says which. I traced 5 acceptance reads to a producer in the set, and `depends_on` records 3 of them. The two missing are status-page to release and async-handles to the stt adapter.

What I could not establish:
- `aep plan artifact waves` reports six of the seven as "unassessed", because none carries a `scope:`. The scheduler cannot protect them, so the surfaces above have to reach it through the bodies or `scope:` fields.
- story:elevenlabs-tts-provider-cancel feeds time to first audio into the latency report of story:voice-topology-latency-per-step. That story is outside the set, so I did not judge it.
- Whether `ess/domains/audio.yaml` or `stream.yaml` is touched is conditional in the bodies ("a contract change"). I could not tell which of the two files would take it.
- Out of my lane, and not setting the verdict: the stt story's acceptance sits on `INFERENCE_ELEVENLABS_STT_CLIP`, an environment variable I did not check against `website/docs/reference/environment.md`. Whether the release's "carrying #39" is checkable belongs to acceptance, and design owns the split of the duplex-config story.

```findings
- file: .engineering/planning/story/elevenlabs-realtime-stt-adapter.md
  line: 37
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "both this and story:elevenlabs-tts-provider-cancel land in adapters/inference-elevenlabs (src/lib.rs, src/error.rs, Cargo.toml, tests/claims_record.rs; lib.rs cited on the cancel side, the stt side's files inferred) and neither says so; add an ordering edge recording the shared files or split the surface"
- file: .engineering/planning/story/elevenlabs-agent-session-overrides.md
  line: 33
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "both this and story:duplex-session-config-additions land on adapters/inference-elevenlabs/src/agent_duplex.rs (agent_capabilities gains the new SpeechCapabilities flags, file inferred for the duplex story), and this acceptance requires agent_capabilities unchanged; add an ordering edge recording the shared file or split the surface"
- file: .engineering/planning/story/async-speech-handles.md
  line: 17
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "shares crates/inference-tts/src/lib.rs with story:elevenlabs-tts-provider-cancel and crates/inference-duplex/src/contract.rs (and a possible ess/domains/duplex.yaml change) with story:duplex-session-config-additions, both cited, and agent_duplex.rs with story:elevenlabs-agent-session-overrides (inferred); neither body says so; add an ordering edge recording the shared file or split the surface per pair"
- file: .engineering/planning/story/async-speech-handles.md
  line: 26
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the surface is three contract crates, every adapter implementing them, the testkit mocks and the pipeline nodes, so it collides with every other item; narrow it per contract or state that it is worked alone"
- file: .engineering/planning/story/async-speech-handles.md
  line: 35
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the acceptance drives the Recogniser binding that story:elevenlabs-realtime-stt-adapter creates and no depends_on edge runs from this story to it; add the edge or merge the two"
- file: .engineering/planning/story/duplex-session-config-additions.md
  line: 28
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "a field added to SpeechCapabilities touches every duplex binding (seven adapters, the testkit mock and crates/inference/src/stack.rs, files inferred by git grep), so the surface collides with any item that touches a duplex binding; narrow it or record the overlap"
- file: .engineering/planning/story/status-page-voice-adapters.md
  line: 29
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the acceptance reads the CHANGELOG.md section introducing ElevenLabs STT, which task:release-elevenlabs-route-p1 writes, and the only depends_on edge goes to story:elevenlabs-realtime-stt-adapter; add a depends_on edge to the release task or merge the two"
- file: .engineering/planning/task/release-elevenlabs-route-p1.md
  line: 24
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the release rewrites Cargo.toml version, every capability record's adapter_revision and CHANGELOG.md, which P2 and P3 stories also edit (story:elevenlabs-agent-session-overrides on records/claims-agent.json, every consumer-visible story on the Unreleased CHANGELOG section, inferred for the stories), and no edge orders them; add an ordering edge recording the shared files or split the surface"
```
