---
format: aep.planning-md/3
id: review-result:elevenlabs-route-p2-parallel-round-1
kind: review-result
status: active
title: Plan critic, parallel, round 1, on wave P2 of epic:elevenlabs-route
relations:
- reviews: epic:elevenlabs-route
revision: 1
---
needs-revision

- story:realtime-voice-barge-in-stop-flake — its test calls `run_cascade_live`, `Cascade` and `Measured` from `examples/realtime-voice/src/compare.rs`, and the "clock the test controls" fix has to change that file; the story scopes only the test file. story:compare-elevenlabs-cascade-and-ollama also changes the cascade runner in `src/compare.rs`, and its scope claims the whole `examples/realtime-voice/tests` directory, which holds this test file (cited for the test file, inferred for `src/compare.rs`). Neither body names the other, and `aep plan artifact waves` reports the collision on `adversary2_topology_workers.rs`. Either add an ordering edge that records the shared file as its reason, or split the surface so each story owns its own files. — .engineering/planning/story/realtime-voice-barge-in-stop-flake.md:24, .engineering/planning/story/compare-elevenlabs-cascade-and-ollama.md:28
- story:async-speech-handles — it says "no other story of the set is worked beside it", but no `depends_on` or other edge orders it against story:compare-elevenlabs-cascade-and-ollama, story:status-page-voice-adapters or story:realtime-voice-barge-in-stop-flake. Its surface (`adapters/inference-elevenlabs/src/recogniser.rs` and the three contract crates) is built by `examples/realtime-voice`, which compare changes (cited: `examples/realtime-voice/Cargo.toml:17-22`, compare body line 57). The set therefore reads as parallel while the body says it is not. Either record the shared surface as an ordering edge, or narrow the surface so the sentence can be dropped. — .engineering/planning/story/async-speech-handles.md:107
- story:async-speech-handles — the bare `adapters` scope entry (cited) claims every adapter directory, including `adapters/inference-llama-server`, which compare also claims (the llama-server overlap is inferred), and the waves collision list shows it overlapping dozens of stories. The body already lists each speech adapter file, so the directory entry is a scope that was never narrowed. Remove it, or list the crates. — .engineering/planning/story/async-speech-handles.md:19
- story:status-page-voice-adapters — it lists `CHANGELOG.md` in its scope (inferred), but the body only reads it ("the `CHANGELOG.md` section that introduced it"). Compare edits `CHANGELOG.md` (cited, compare body line 73) and the waves output reports the collision, and neither body says so. Either drop `CHANGELOG.md` from the scope as read-only, or state that it is shared and add an ordering edge as the reason. — .engineering/planning/story/status-page-voice-adapters.md:18

**What you read:** 6 artifacts, all with `aep plan artifact show`, plus `aep plan artifact waves`, `aep plan artifact graph` and `aep plan artifact relations`. I also read `story:talk-picks-elevenlabs`, `examples/realtime-voice/Cargo.toml` and `examples/realtime-voice/tests/adversary2_topology_workers.rs`, and grepped `examples/realtime-voice/src/compare.rs`.

**What you could not establish:**
- Surfaces: 6 placed from cited bodies (task:voice-latency-comparison-run and task:release-elevenlabs-route-p2 have no recorded scope, but their bodies cite the compare guide and the release files), 0 inferred, 0 unplaceable.
- Acceptance reads traced to a producer inside the set: 2 (task:voice-latency-comparison-run reads story:compare-elevenlabs-cascade-and-ollama's flags, and task:release-elevenlabs-route-p2 reads story:async-speech-handles' merge commit). A `depends_on` edge records both. Release also has edges to every other item in the set.
- Unease, not a finding: task:voice-latency-comparison-run has no edge to story:async-speech-handles, which rewrites the ElevenLabs recogniser and synthesiser. Latency evidence taken before or during that merge would describe a different commit than the one released.
- Out of my lane (scope critic): story:async-speech-handles (line 107) and task:release-elevenlabs-route-p2 still name story:elevenlabs-agent-session-overrides, which the operator archived.

```findings
- file: .engineering/planning/story/realtime-voice-barge-in-stop-flake.md
  line: 24
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the test calls run_cascade_live and Cascade from examples/realtime-voice/src/compare.rs, which the fix may need to change (inferred) and which story:compare-elevenlabs-cascade-and-ollama also changes; that story's scope claims the whole examples/realtime-voice/tests directory (cited, includes this test file), and neither body names the other"
- file: .engineering/planning/story/async-speech-handles.md
  line: 107
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the body says no other story of the set is worked beside it, but no edge orders it against compare, status-page or the flake story, and examples/realtime-voice (changed by compare) builds against the contract crates and the ElevenLabs recogniser it rewrites"
- file: .engineering/planning/story/async-speech-handles.md
  line: 19
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the bare adapters directory entry claims every adapter crate, including inference-llama-server which compare also claims (inferred), though the body already lists each speech adapter file"
- file: .engineering/planning/story/status-page-voice-adapters.md
  line: 18
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "CHANGELOG.md is in the scope (inferred) though the body only reads it, and story:compare-elevenlabs-cascade-and-ollama edits it; neither body names the shared file or orders the two"
```