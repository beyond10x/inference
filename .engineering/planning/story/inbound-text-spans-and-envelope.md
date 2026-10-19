---
format: aep.planning-md/3
id: story:inbound-text-spans-and-envelope
kind: story
status: draft
title: Inbound turns and spans, and the event envelope that sequences them
summary: inference-text gains Turn and Span; the §6.2 event envelope is declared in ess/ first and lands in inference-core; the target answers the 14 turn and span scenarios and the testkit log enforces event ids, generation ids and per-stream sequence (§23.1 test 6).
relations:
- decomposes: epic:contracts-and-testkit
- depends_on: story:outbound-text-generations-and-segments
- serves: vision:inference
- depends_on: story:audio-vocabulary
scope:
- confidence: cited
  path: .engineering/planning/executable-system-specification/inference.md
- confidence: cited
  path: crates/inference-core
- confidence: inferred
  path: crates/inference-core/src/envelope.rs
- confidence: cited
  path: crates/inference-core/src/lib.rs
- confidence: inferred
  path: crates/inference-testkit/src/stream.rs
- confidence: cited
  path: crates/inference-testkit/tests/envelope.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/stream.rs
- confidence: cited
  path: crates/inference-text
- confidence: inferred
  path: crates/inference-text/src/lib.rs
- confidence: cited
  path: ess/SKIPPED.md
- confidence: cited
  path: ess/domains/stream.yaml
revision: 28
---
## Context

Inbound text, `docs/design/inference.md:599-608`: a hypothesis is text subject to later replacement (line 603), a revision is an explicit change to an identified provisional span (line 604), a committed span is an immutable unit downstream may treat as stable (line 605), turn completion is a control boundary distinct from the commit of individual spans (line 606), and a stable prefix may be committed before the utterance is complete (line 608). The nouns are `ess/domains/stream.yaml`: `inference.stream.Turn` (Open, Completed) and `inference.stream.Span` (Hypothesis, Committed); the commands `OpenTurn`, `CompleteTurn`, `EmitHypothesis`, `ReviseSpan` and `CommitSpan`; the errors `TurnStateConflict`, `TurnNotFound`, `CommittedSpanImmutable`, `SpanNotFound` and `SpanStateConflict`, with `inference.graph.RunNotFound` and `inference.graph.RunStateConflict` answering the run guards of `OpenTurn` and `EmitHypothesis`; the events `TurnOpened`, `TurnCompleted`, `TranscriptHypothesis`, `TranscriptRevision` and `TranscriptCommitted`; the views `Turns` and `Spans`. They live in `inference-text` beside `OutputSegment` (answer to Q1 in `executable-system-specification:inference`), with `inference.stream.TurnId` and `inference.stream.SpanId`.

The second half is the canonical event envelope, `docs/design/inference.md:301-320`: `schema_version`, `event_id`, `run_id`, `session_id` (optional), `stream_id`, `sequence` (increasing per producer and per stream), `producer` and `port`, `kind`, `generation_id` (if any), `timestamp`, `causation_id` (if known) and `payload` (lines 307-318); no single global order across independently produced streams (line 320); a control event must not be mistaken for user content (line 297); the timestamp names its timebase explicitly, because audio frame time, inference execution time, wall time and playback time are distinct clocks (lines 643-645). The envelope is the noun behind §23.1 test 6 (line 1110: event IDs, generation IDs and per-stream sequence constraints are enforced), and spans already carry the per-stream sequence it governs (`Span.stream_id`, `Span.sequence`), which is why it lands with this story. **`ess/` does not declare the envelope.** The first step of this story is to declare it there, as a `types:` entry of `kind: struct` (which `ess specify validate` accepts) plus an enum for the timebase, by default in `ess/domains/stream.yaml` with a `read from:` line per field like every other declaration, then run `ess specify validate --path ess` and `ess verify conform synthesize --path ess --scenarios ess/scenarios --out .engineering/drafts/suite.json`, and record the new counts in `executable-system-specification:inference` with `aep plan artifact body --section`. Only then does Rust code name it. A `types:` entry declares no command outcome, lifecycle move, invariant or binding, the things a scenario is synthesized from (`crates/verify/ess-conformance/src/lib.rs` at tag `0.60.0` of `github.com/beyond10x/ess`), so the suite is expected to keep its 229 scenarios (225 synthesized and 4 authored) and `ess/SKIPPED.md` to need no new line; the re-synthesis is what shows it, and the counts are recorded either way.

Two items of the epic's `inference-core` scope are claimed here, both narrowed as the epic records. Control events: the start, stop, commit, revise, cancel and error events of §6.1 (line 297) are the `inference.stream` lifecycle events and typed errors `ess/domains/stream.yaml` declares, and the envelope's `kind` is how a consumer tells them from content (`TranscriptHypothesis`, `TextDelta`); this story declares no separate control-event type. Timebases: the timebase enum lives here, beside the envelope that names it, and not in `inference-audio`, because `inference-audio` depends on `core` and not the reverse (line 235); `story:audio-vocabulary` carries the audio-frame-time reading, the sample clock position, and depends on nothing of this story.

This story depends on `story:audio-vocabulary` for one reason only: both record facts in `executable-system-specification:inference` after re-synthesizing the suite, and the order settles which total includes which. The audio story writes first (the seventh domain and its own Evidence row); this story then writes, through `aep plan artifact body --section`, the "What it declares" counts, the summary line and one new dated Evidence row carrying its own validate and synthesize output, so no row of the other story is rewritten.

## Domain relations

- `inference.stream.Turn` references `inference.graph.Run`, cardinality one. Inferable: `ess/domains/stream.yaml`, entity `inference.stream.Turn`, relation `run`. `OpenTurn` refuses a `run_id` no run carries (`unknown-run`, `inference.graph.RunNotFound`) and a run that is not Active (`run-not-active`, `inference.graph.RunStateConflict`), and its scenario arranges a running Run first (`inference.policy.DeclareProfile`, `DeclarePipeline`, `ValidatePipeline`, `PlanPipeline`, `CreateRun`, `ValidateRun`, `PrepareRun`, `ActivateRun`; `.engineering/drafts/suite.json`, `inference.stream.OpenTurn/outcome/opened`), so the store reads the run's state from the graph domain's Run store.
- `inference.stream.Span` references `inference.graph.Run`, cardinality one. Inferable: `ess/domains/stream.yaml`, entity `inference.stream.Span`, relation `run`.
- `inference.stream.Span` references `inference.stream.Turn`, cardinality one; a committed span is durable transcript history and outlives the turn boundary it arrived in, so completing a turn moves no span and no span is deleted with its turn. Inferable: `ess/domains/stream.yaml`, entity `inference.stream.Span`, relation `turn`; decision D1 in `executable-system-specification:inference`. `EmitHypothesis` refuses an unknown run (`unknown-run`), a run that is not Active (`run-not-active`), an unknown turn (`unknown-turn`, `inference.stream.TurnNotFound`) and a Completed turn (`turn-not-open`, `inference.stream.TurnStateConflict`), and its scenario opens the turn first (`inference.stream.EmitHypothesis/outcome/emitted`), so the store requires the turn to exist and be Open. That the span's `run_id` matches its turn's run is not checked (`UNMAPPED` in `ess/domains/stream.yaml`).
- The relations of the envelope are read from the design and are in no `ess/1` document until the first step declares them; none is cited as decided before that: envelope to run, one (line 309); to session, zero or one (line 310); to stream, one (line 311); to producer node and port, one (line 313); to generation, zero or one (line 315); to the causing event, zero or one (line 317). Whether `port` is a port id or a port name the design does not say (line 313): the declaration carries an `UNMAPPED:` marker for it rather than a guess, and the Rust type holds it as an opaque string until `epic:graph-minimum` settles it.

## Steps

1. Declare the envelope and the timebase in `ess/` as above; validate; re-synthesize; update the specification artifact.
2. `inference-text`: `Turn` and `Span` with their transitions; `Span::revise` answers `CommittedSpanImmutable` on a committed span (line 1105, the first contract test of the design) and leaves the text unchanged; the store answers `SpanNotFound` for an unknown id, `TurnNotFound` for an unknown turn, `TurnStateConflict` for a span on a Completed turn, and `RunNotFound` or `RunStateConflict` for a turn or span on an unknown or non-Active run.
3. `inference-core`: the envelope type, in a module of its own, over the identifiers of the skeleton story, `kind` separating control from content (line 297), `session_id`, `generation_id` and `causation_id` optional, the timebase enum beside it with the four clocks of line 645. `story:pipeline-declaration` adds the `inference.policy` types to `inference-core` in a module of its own and runs after this story (its `depends_on` edge to this story); the one file both touch is `crates/inference-core/src/lib.rs`, each adding its own `mod` line.
4. `inference-testkit`: the event log, in the `stream` store module the outbound story opened, wraps every recorded event in an envelope and refuses (a) a second event carrying an `event_id` already recorded, (b) an event on a `stream_id` whose `sequence` is not greater than the last one recorded on that stream, (c) an `inference.stream` event whose payload carries a `generation_id` while the envelope carries none or a different one. The `stream` store module and `tests/target/stream.rs` gain the Turn and Span stores, the five commands and the two views; the `## inference.stream` section of `ess/SKIPPED.md` loses the 3 ids under `## Scenarios` that arrange no run; the other 17 stay, naming `story:run-lifecycle` (`## Scenarios that pass with story:run-lifecycle`). `story:pipeline-declaration` fills the `graph` and `policy` modules and sections and `story:tool-call-lifecycle` the `tools` ones, so this story meets neither in a module nor in a section of `ess/SKIPPED.md`; `story:llm-contract` and `story:tts-contract`, in the same wave, fill their own feature-gated modules and touch no module of this story.
5. §23.1 test 6 as `crates/inference-testkit/tests/envelope.rs`: one test per rule, each shown to go red when its check is removed, recorded in the commit message.

## Acceptance

`cargo test -p inference-testkit` passes with the 3 ids under `## Scenarios` that arrange no run in the report's `passed` (`inference.stream.CommitSpan/outcome/wrong-state`, `inference.stream.CompleteTurn/outcome/wrong-state`, `inference.stream.ReviseSpan/outcome/no-such-span`), `failed` and `error` at 0, and the three tests of `tests/envelope.rs` green, on a tree where the declaration check of `## Checks` holds. The other 17 ids under `## Scenarios`, repeated under `## Scenarios that pass with story:run-lifecycle`, create a Run in their arrangement (`inference.graph.CreateRun`), which no story before `story:run-lifecycle` answers; they stay in `ess/SKIPPED.md` naming `story:run-lifecycle`.

## Checks

1. `ess/domains/stream.yaml` carries a `types:` entry of `kind: struct` for the envelope with the fields of `docs/design/inference.md:307-318` and a `types:` entry of `kind: enum` for the timebase with the four clocks of line 645, each with its `read from:` line; `ess specify validate --path ess` exits 0 and `ess verify conform synthesize --path ess --scenarios ess/scenarios --out .engineering/drafts/suite.json` reports no refusal beyond the three ESS-SYNTH-001 refusals the authored scenarios cover, with the counts recorded in `executable-system-specification:inference`.
2. The three tests of `tests/envelope.rs` refuse a duplicate event id, a non-increasing per-stream sequence and a mismatched generation id, and the commit message records each going red when its check is removed.

## Out of scope

The STT contract and its mock (`story:audio-and-stt-contract`); audio frames, formats and sample clocks (`inference-audio`, `story:audio-vocabulary`); any Node or Port.

## Scenarios

The 20 ids this story answers (3 move from `ess/SKIPPED.md` to `passed` when it lands, 17 when `story:run-lifecycle` does), read from `.engineering/drafts/suite.json` on 2026-10-18 and, for the six run and turn refusals, from the suite synthesized with `--scenarios ess/scenarios` on 2026-10-19:

- `inference.stream.CommitSpan/outcome/committed`
- `inference.stream.CommitSpan/outcome/wrong-state`
- `inference.stream.CompleteTurn/outcome/completed`
- `inference.stream.CompleteTurn/outcome/wrong-state`
- `inference.stream.EmitHypothesis/outcome/emitted`
- `inference.stream.EmitHypothesis/outcome/run-not-active`
- `inference.stream.EmitHypothesis/outcome/turn-not-open`
- `inference.stream.EmitHypothesis/outcome/unknown-run`
- `inference.stream.EmitHypothesis/outcome/unknown-turn`
- `inference.stream.OpenTurn/outcome/opened`
- `inference.stream.OpenTurn/outcome/run-not-active`
- `inference.stream.OpenTurn/outcome/unknown-run`
- `inference.stream.ReviseSpan/outcome/immutable`
- `inference.stream.ReviseSpan/outcome/no-such-span`
- `inference.stream.ReviseSpan/outcome/revised`
- `inference.stream.Span/state/Committed/refuses/inference.stream.CommitSpan`
- `inference.stream.Span/state/Committed/refuses/inference.stream.ReviseSpan`
- `inference.stream.Span/transition/commit/by/inference.stream.CommitSpan/committed`
- `inference.stream.Turn/state/Completed/refuses/inference.stream.CompleteTurn`
- `inference.stream.Turn/transition/complete/by/inference.stream.CompleteTurn/completed`

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** no single one dominates: the story is one vertical slice of `inference.stream` across `ess/domains/stream.yaml` (step 1), `crates/inference-text` (step 2), `crates/inference-core` (step 3) and the testkit's `stream` store module, `tests/target/stream.rs` and `tests/envelope.rs` (steps 4-5) — cited (body Steps 1-5; crate paths `docs/design/inference.md:191,192,202`; `crates/` absent in the tree)
- **Files:** `ess/domains/stream.yaml`, two new `types:` entries (envelope `kind: struct`, timebase `kind: enum`) in the block at `ess/domains/stream.yaml:23-47`, which today holds five `newtype`s and nothing else — cited (step 1, Checks 1)
- **Files:** `crates/inference-core/src/lib.rs`, one `mod` line for the envelope module; the one file shared with `story:pipeline-declaration`, which runs after this story — cited (step 3)
- **Files:** `crates/inference-testkit/tests/target/stream.rs`, the Turn and Span stores, five commands and two views answered by the target; `crates/inference-testkit/tests/envelope.rs`, new, the three §23.1 test-6 rules — cited (steps 4-5, Acceptance; per-domain file layout from `story:testkit-conformance-target` step 2)
- **Files:** `ess/SKIPPED.md`, the `## inference.stream` section loses the 14 ids under `## Scenarios`; the file does not exist yet and is created by `story:testkit-conformance-target` step 3 — cited (step 4)
- **Files:** `.engineering/planning/executable-system-specification/inference.md`, through `aep plan artifact body --section` only: the summary (`:7`), the counts under `## What it declares` (`:31`) and one new dated row under `## Evidence` (`:36`), after `story:audio-vocabulary` has written its row — cited (Context, fourth paragraph; Checks 1)
- **Symbols:** `inference.stream.Turn` (`ess/domains/stream.yaml:52`), `inference.stream.Span` (`:81`), `TurnId`/`SpanId` (`:24,28`), `OpenTurn`, `CompleteTurn`, `EmitHypothesis`, `ReviseSpan`, `CommitSpan` (`:303-`), `TurnStateConflict`, `CommittedSpanImmutable`, `SpanNotFound`, `SpanStateConflict` (`:235-`), `TurnOpened`…`TranscriptCommitted` (`:814-`), views `Turns`, `Spans` (`:922-`); the envelope fields `schema_version`…`payload` (`docs/design/inference.md:307-318`) and the four clocks (`:645`), which no `ess/` document names today — cited
- **Also likely:** `crates/inference-testkit/src/stream.rs`, the `stream` store module (module name cited in step 4 and in the testkit story; the single-file form is read from "every module an empty file until its story fills it", testkit step 1) — inferred
- **Also likely:** `crates/inference-core/src/envelope.rs`, the envelope "in a module of its own" (step 3); the body gives no name — inferred
- **Also likely:** `crates/inference-text/src/lib.rs`, `mod` lines if Turn and Span land in modules of their own beside `OutputSegment`; the body says only "beside `OutputSegment`" — inferred
- **Documents:** `ess/domains/stream.yaml`, `ess/SKIPPED.md` and the specification artifact as above; `.engineering/drafts/suite.json` is regenerated by step 1 but git-ignored (`.gitignore:2`) and per-worktree, so it is not recorded as a surface; `docs/design/inference.md` is read, not changed; `Taskfile.yml`, `AGENTS.md`, `README.md`, `ess/system.yaml`, `ess/ess-inputs.yaml` and every `Cargo.toml` are named by no step — cited
- **Confidence:** high — every surface is named in the body and confirmed against the tree (`ess/domains/stream.yaml:23-47` holds five newtypes and no envelope; `crates/`, `Cargo.toml`, `ess/SKIPPED.md` absent); the three inferred lines are file names inside crates that do not exist yet, each a new or self-owned file
- **Would collide with:** any unit declaring into `ess/domains/stream.yaml`; any unit editing `ess/SKIPPED.md` (`story:tool-call-lifecycle` edits its `## inference.tools` section of the same file with no ordering edge to this story: same file, different section); the module list of `crates/inference-core/src/lib.rs`; the testkit's `stream` store module and `tests/target/stream.rs`; any file of `crates/inference-text`; the summary, `## What it declares` counts and `## Evidence` table of `.engineering/planning/executable-system-specification/inference.md` (`:7,31,36`; `story:audio-vocabulary` writes the same artifact first, ordered by `depends_on`)
- **Safety fact:** no `ess/` document declares the envelope or a timebase today (`rg -i 'schema_version|event_id|causation_id|envelope|timebase' ess/` matches one comment, `ess/domains/session.yaml:51`; the `types:` block at `ess/domains/stream.yaml:23-47` is five `newtype`s), so step 1 adds two entries and rewrites no existing declaration; and a `types:` entry synthesizes no scenario, so the suite stays at 153 (today: 153 total, 60 `inference.stream`, each of the 14 ids present once in `.engineering/drafts/suite.json`) — first half step 2 (`file:line`), second half step 1 (the body's reading of `ess-conformance` 0.60.0, not re-synthesized here), unproven

## Scenarios that pass with story:run-lifecycle

The 17 ids of `## Scenarios` whose arrangement creates a Run, computed as in `story:outbound-text-generations-and-segments`.

- `inference.stream.CommitSpan/outcome/committed`
- `inference.stream.CompleteTurn/outcome/completed`
- `inference.stream.EmitHypothesis/outcome/emitted`
- `inference.stream.EmitHypothesis/outcome/run-not-active`
- `inference.stream.EmitHypothesis/outcome/turn-not-open`
- `inference.stream.EmitHypothesis/outcome/unknown-run`
- `inference.stream.EmitHypothesis/outcome/unknown-turn`
- `inference.stream.OpenTurn/outcome/opened`
- `inference.stream.OpenTurn/outcome/run-not-active`
- `inference.stream.OpenTurn/outcome/unknown-run`
- `inference.stream.ReviseSpan/outcome/immutable`
- `inference.stream.ReviseSpan/outcome/revised`
- `inference.stream.Span/state/Committed/refuses/inference.stream.CommitSpan`
- `inference.stream.Span/state/Committed/refuses/inference.stream.ReviseSpan`
- `inference.stream.Span/transition/commit/by/inference.stream.CommitSpan/committed`
- `inference.stream.Turn/state/Completed/refuses/inference.stream.CompleteTurn`
- `inference.stream.Turn/transition/complete/by/inference.stream.CompleteTurn/completed`

## Event order across domains

Added 2026-10-19 from wave 4 (`review-result:adversary-outbound-text-generations-and-segments-pass-2`, finding F1).

The testkit target issues consistency tokens from one target-wide sequence (`Stores::sequence` in `crates/inference-testkit/tests/target/mod.rs`), but each domain module numbers its observed events by its own position (`ObservedEvent::at` in `tests/target/model.rs` and `tests/target/stream.rs`), so a model event and a stream event can both sit at position 1. This story introduces the event envelope that sequences events; it numbers observed events in the target's one publication order, and flips `observed_event_positions_are_per_domain_until_the_envelope_sequences_them` in `crates/inference-testkit/tests/adversary3_target_sequence.rs` to expect positions 1, 2 and 3. No ess 0.60.0 code reads the position today (adversary grep), so nothing fails before then.
