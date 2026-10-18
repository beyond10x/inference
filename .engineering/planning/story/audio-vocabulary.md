---
format: aep.planning-md/3
id: story:audio-vocabulary
kind: story
status: draft
title: 'Audio frames, formats and sample clocks: declared in ess/ and carried by inference-audio'
summary: inference.audio is declared in ess/domains/audio.yaml (spec first); crates/inference-audio carries frame, format and sample clock position in audio frame time; the STT and TTS stories depend on it and nothing else waits for it.
relations:
- decomposes: epic:contracts-and-testkit
- serves: vision:inference
- depends_on: story:workspace-skeleton
scope:
- confidence: cited
  path: .engineering/planning/executable-system-specification/inference.md
- confidence: cited
  path: crates/inference-audio
- confidence: cited
  path: ess/domains/audio.yaml
- confidence: cited
  path: ess/ess-inputs.yaml
- confidence: cited
  path: ess/system.yaml
revision: 8
---
## Context

`inference-audio` is `docs/design/inference.md:193` and line 235 (audio media descriptors, frames, clocks, formats; depends on `core`). The audio row of §6.1 (line 293) names what a frame carries: sample clock, encoding, sample rate, channels, timestamps. Frames travel as bounded binary buffers, never base64 JSON (line 324); ownership of a borrowed buffer is explicit (line 325). Audio frame time is one of the four clocks of §11.5 (lines 643-645), distinct from inference execution time, wall time and playback time. The crate carries no concrete implementation (`AGENTS.md`, Rules).

**Audio frames, formats and sample clocks are not declared in `ess/`** (`docs/design/inference.md:293`, lines 324-328). The first step of this story declares them there (`types:` entries of `kind: struct` and `kind: enum`; a domain `inference.audio` in `ess/domains/audio.yaml`, listed in `ess/system.yaml` and in the `specification:` list of `ess/ess-inputs.yaml`, since a crate follows its domain), validates with `ess specify validate --path ess`, re-synthesizes the suite, and records the counts in `executable-system-specification:inference` with `aep plan artifact body --section`; only then does `inference-audio` name them. A `types:` entry declares no command outcome, lifecycle move, invariant or binding, the things a scenario is synthesized from (`crates/verify/ess-conformance/src/lib.rs` at tag `0.60.0` of `github.com/beyond10x/ess`), so the suite is expected to keep its 153 scenarios and `ess/SKIPPED.md` to need no new line; the re-synthesis is what shows it, and the counts are recorded either way.

This story is the audio half of what was one story with the STT contract. `story:tts-contract` consumes frames and `story:audio-and-stt-contract` produces spans from them, and neither needs the other, so the vocabulary stands alone behind `story:workspace-skeleton` and both contract stories depend on it; the TTS story no longer waits for the inbound-text and STT stories. The position of a frame is a sample clock position, a sample index at the format's sample rate, which is audio frame time: this story declares frame time itself and needs no edge to `story:inbound-text-spans-and-envelope`. The timebase enum the event envelope names (§11.5) lives in `inference-core` beside the envelope, with that story, because `inference-audio` depends on `core` and not the reverse (line 235); the epic's `inference-audio` item is narrowed to that in its body.

## Domain relations

- Audio frames have no entity relation in the design: a frame is a value that flows on an edge, and edges are `epic:graph-minimum`. No relation is declared for them here; the `types:` entries of `ess/domains/audio.yaml` declare no entity, command or view.

## Steps

1. Declare the audio nouns in `ess/` as above: `ess/domains/audio.yaml` with a `read from:` line per declaration like every other domain document, listed in `ess/system.yaml` and `ess/ess-inputs.yaml`; `ess specify validate --path ess` exits 0; `ess verify conform synthesize --path ess --out .engineering/drafts/suite.json` reports 0 refusals; record the counts in `executable-system-specification:inference`.
2. `crates/inference-audio/`: frame, format (encoding, sample rate, channels) and sample clock position, as the declaration says, with `serde`; depends on `inference-core` only (line 235); unit tests on format equality, frame duration arithmetic and the conversion of a sample clock position to audio frame time at the format's sample rate (position 16000 at 16 kHz is one second; position 0 is zero; a position at 48 kHz and the same position at 16 kHz differ by the factor three).

## Acceptance

`task check` exits 0 on a tree where `ess/domains/audio.yaml` exists, is listed in `ess/system.yaml` and `ess/ess-inputs.yaml` and validates (`ess specify validate --path ess` exit 0), and where the `inference-audio` unit tests on format equality, frame duration arithmetic and position-to-time conversion at the format's sample rate pass.

## Ordering

`story:inbound-text-spans-and-envelope` depends on this story: both record facts in `executable-system-specification:inference` after a re-synthesis, and the order settles which total includes which. This story writes first, through `aep plan artifact body --section`, and touches two places only: the domain list (`inference.audio` as the seventh domain) and one new dated row of the Evidence table carrying its own validate and synthesize output. The "What it declares" counts and the summary are left to the inbound story, which records the envelope declaration and the counts after it.

## Out of scope

The STT and TTS contracts and their mocks (`story:audio-and-stt-contract`, `story:tts-contract`); any codec, resampling or format conversion; the timebase enum (`story:inbound-text-spans-and-envelope`); a port that carries a format (`story:pipeline-declaration`, `epic:graph-minimum`).

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `ess/domains/audio.yaml` (new) and `crates/inference-audio` (new) — cited: Steps 1 and 2; neither exists today (`ess/domains/` holds six documents, there is no `crates/` directory, the repository has no commits)
- **Files:** `ess/domains/audio.yaml` — cited (Step 1, Acceptance)
- **Files:** `ess/system.yaml` — cited: `domains:` list gains `inference.audio` as the seventh entry (Step 1, Ordering; six listed today)
- **Files:** `ess/ess-inputs.yaml` — cited: `specification:` list gains `domains/audio.yaml` (Step 1; seven entries today)
- **Files:** `crates/inference-audio` — cited (Step 2; the whole crate is new; depends on `inference-core` only, `docs/design/inference.md:235`)
- **Files:** `.engineering/planning/executable-system-specification/inference.md` — cited: Step 1 and Ordering write `executable-system-specification:inference` through `aep plan artifact body --section` (domain list plus one dated Evidence row); the id resolves to this file in the store
- **Symbols:** domain `inference.audio`; `types:` entries of `kind: struct` and `kind: enum` for frame, format (encoding, sample rate, channels) and sample clock position — cited (Context, Steps); nothing of the kind exists in `ess/` today (`grep -rn -i audio ess/` finds only `Modality` variant `Audio` in `model.yaml` and `AudioPlayed` in `stream.yaml`)
- **Also likely:** none — root `Cargo.toml`, `Taskfile.yml`, `AGENTS.md` and `README.md` appear in the body only as rules read or as `story:workspace-skeleton`'s surface — cited
- **Documents:** `docs/design/inference.md` is read (lines 193, 235, 293, 324-325, 645; every citation lands on the line named), not changed — cited
- **Confidence:** high — the body names each path it touches and the tree confirms the two new surfaces are absent and the three edited files hold the lists to extend
- **Would collide with:** any unit editing `ess/system.yaml` or `ess/ess-inputs.yaml` (every unit that adds a domain document); any unit writing `.engineering/planning/executable-system-specification/inference.md` (the body's Ordering sequences the inbound-text story's write after this one, and `story:inbound-text-spans-and-envelope depends_on story:audio-vocabulary` is in the store); any unit creating `crates/inference-audio` or `ess/domains/audio.yaml`
- **Safety fact:** a `types:` entry declares none of the five things the synthesizer obliges (a declared outcome, a lifecycle move, a move that must not be honoured, an entity invariant, a binding claim), so `ess/domains/audio.yaml` leaves the suite at 153 scenarios and the sibling story's recorded counts valid — step 2 (`crates/verify/ess-conformance/src/lib.rs:65-66` in `github.com/beyond10x/ess`, read at the local checkout tag 0.58.0; the body cites 0.60.0), unproven
