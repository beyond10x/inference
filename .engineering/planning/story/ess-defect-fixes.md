---
format: aep.planning-md/3
id: story:ess-defect-fixes
kind: story
status: active
title: Fix the 66 defects the hardening pass and five reviewers found in ess/
summary: Every confirmed finding of the merged spec review fixed in ess/, each citing its design line; validate, synthesize and the interpreted run pass.
relations:
- serves: vision:inference
scope:
- confidence: cited
  path: ess/domains/graph.yaml
- confidence: cited
  path: ess/domains/model.yaml
- confidence: cited
  path: ess/domains/policy.yaml
- confidence: cited
  path: ess/domains/session.yaml
- confidence: cited
  path: ess/domains/stream.yaml
- confidence: cited
  path: ess/domains/tools.yaml
- confidence: cited
  path: ess/system.yaml
revision: 14
transitions:
- {from: "draft", to: "proposed", at: "2026-10-18T23:48:27Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-18T23:48:27Z", actor: "human:timo", revision: 3}
---
## Context

A hardening pass (`ess:hardening`, technique 8 with a planted defect, spec diff, a spec-only
mutation audit, guard analysis) and five independent spec reviewers (design fidelity; lifecycles and
guards; relations and identity; commands, refusals and testability; an adversarial re-check of the
hardening findings) read `ess/` at `0.1.0` against `docs/design/inference.md` v0.2 on 2026-10-19.
Their 135 rows merge into 66 findings: 13 P1, 37 P2, 16 P3. All 13 P1 were re-read against the cited
spec and design lines by the merger. 31 of the 66 are touched by some story; 35 by none. Six
single-source rows could not be confirmed and may be false positives (R4-12, R4-25, H30, R2-15,
R3-12, R3-11).

The P1 defects, by merged id: M01 a superseded or cancelled generation still emits, commits, queues
and plays (`stream.yaml:459-555`); M02 supersede and cancel discard no segments though the summary
says they do (`stream.yaml:414-455`); M03 the provisional-input barrier tests the caller's
side-effect class, not the tool's (`tools.yaml:150-174`); M04 a run cannot fail from Draining and
the failure reason is a plain string (`graph.yaml:324-326, 746-768`); M05 CreateRun and OpenSession
accept a pipeline that is not Planned (`graph.yaml:621-637`, `session.yaml:54-73`); M06 no command
enforces profile locality (`policy.yaml:6-10`, `graph.yaml:531-618`); M07 DeclareProfile accepts
`offline_strict` admitting `external_provider` (`policy.yaml:54-76`); M08 graph edits on a Planned
pipeline (`graph.yaml:419-554`); M09 input admitted on a draining run (`stream.yaml:236-389`); M10
PrepareRun cannot deny admission (`graph.yaml:660-679`); M11 the barrier trusts the caller's
`input_basis` (`tools.yaml:152-161`); M12 a pipeline with an unbound node that needs an adapter can
plan and run (`graph.yaml:531-637`); M13 IncompatibleGraph and CapabilityMismatch carry no
diagnostic fields (`graph.yaml:366-372, 563-598`).

The owner asked for every defect to be fixed in the specification (2026-10-19). The plan's
stories enforce several of these only in Rust or accept them (`story:run-lifecycle:115` accepts
the M04 refusal as a scenario); spec first means the specification states the rule and the code
follows it.

## Steps

1. Fix every confirmed finding of the merged list in `ess/`, each change citing the design line it
   reads from, in the domain that owns the noun. A finding found to be a false positive on re-reading
   is left unchanged and its reason recorded below.
2. `ess specify validate --path ess` passes; `ess verify conform synthesize --path ess` yields 0
   refusals; the spec-only interpreted run passes every synthesized scenario.
3. `ess` spec diff against `0.1.0` is classified and the classification recorded here; the
   changes narrow the 0.1.0 contract before any implementation exists.
4. Stories whose bodies cite a spec line that moved, or that accept a defect this story fixes, are
   corrected through `aep plan artifact body`.

## Acceptance

`ess specify validate --path ess` exits 0 and `ess verify conform synthesize --path ess` reports 0
refusals on a specification in which each of the 13 P1 findings has a declared refusal, guard or
`affects` entry that a synthesized scenario exercises, named per finding below, and every P2 and P3
finding is either fixed with its change named or recorded as a false positive with the reason.

## Out of scope

Any Rust code; the conformance target (story:testkit-conformance-target).

## Outcome

Read on 2026-10-19 from `ess specify validate --path ess` (exit 0, 7 files, 4 authored scenarios),
`ess verify conform synthesize --path ess --scenarios ess/scenarios` (229 scenarios, 4 authored,
3 refusals) and `ess verify conform run --path ess --scenarios ess/scenarios --target interpreted
--report-format 2` (229 passed, 0 failed, 0 error, 0 unsupported), ess 0.60.0.

**Acceptance is not met.** Synthesis reports 3 refusals, not 0, all ESS-SYNTH-001 on an `affects`
cascade: `inference.graph.CancelRun/outcome/cancelled`,
`inference.stream.CancelGeneration/outcome/cancelled`,
`inference.stream.SupersedeGeneration/outcome/superseded`. ess 0.60.0 finds no witness for an
`affects` move whose target rows are created by a command carrying a `when_related` guard;
synthesis seeds applied 0 of the moves and ess 0.62.0 refuses the same three. Each cascade is
executed by an authored scenario in `ess/scenarios/`, which the interpreted run passes; ESS states
that an authored scenario does not discharge a synthesis obligation. Removing the refusals means
dropping M01's guards on `EmitTextDelta` or M02's `affects` entries, which reopens a P1. The gap is
reported to the ESS maintainers; this story stays active until an ESS release witnesses the
cascades or the acceptance is changed by the owner.

### P1 findings

| id | covered by |
|---|---|
| M01 | `inference.stream.EmitTextDelta/outcome/stale-generation`, `.../unknown-generation`; the authored discard scenarios assert a later delta is refused |
| M02 | `affects` on `SupersedeGeneration/superseded`, `CancelGeneration/cancelled`, `CancelRun/cancelled`; executed by `inference.stream/authored/supersede-discards-unplayed-segments`, `.../cancel-generation-discards-unplayed-segments`, `.../cancel-run-discards-unplayed-segments` (the 3 refusals above) |
| M03 | `inference.tools.ProposeToolCall/outcome/barred` reads the span's state; `inference.tools.ProposeSpeculativeToolCall/outcome/side-effecting` reads the tool's class; no caller-supplied class |
| M04 | `inference.graph.FailRun/outcome/failed` with `failure: FailureReason`; Draining to Failed by `inference.graph/authored/draining-run-fails`; the id `inference.graph.Run/state/Draining/refuses/inference.graph.FailRun` is gone |
| M05 | `inference.graph.CreateRun/outcome/not-planned`, `.../unknown-pipeline`; `inference.session.OpenSession/outcome/not-planned`, `.../unknown-pipeline` |
| M06 | `inference.policy.DeclareProfile/outcome/remote-not-opted-in`; `inference.graph.PlanPipeline/outcome/locality-denied` is decided outside the specification (external) |
| M07 | `inference.policy.DeclareProfile/outcome/offline-admits-cluster`, `.../wrong-egress`, `.../remote-not-opted-in` |
| M08 | `inference.graph.AddNode/outcome/not-declared`, `ConnectPorts/outcome/not-declared`, `BindNode/outcome/not-declared`; AddPort has no guard (UNMAPPED, graph.yaml) |
| M09 | `inference.stream.OpenTurn/outcome/run-not-active`, `EmitHypothesis/outcome/run-not-active`, `StartGeneration/outcome/run-not-active` |
| M10 | `inference.graph.PrepareRun/outcome/refused` (external) |
| M11 | `ProposeToolCall` takes `basis_span_id` and `barred` reads that span's state; `InputBasis` is removed |
| M12 | `inference.graph.PlanPipeline/outcome/unbound` (external) |
| M13 | `inference.graph.PlanPipeline/outcome/mismatch` and the graph errors carry typed `GraphFinding`, node and capability fields |

### P2 and P3 findings

Fixed, with the change named in each owning file's `read from:` lines and the CHANGELOG: M14
(markers), M15, M16, M18, M19, M21, M22, M24, M25, M26, M27, M28 (schema part), M32, M34, M35,
M36, M37, M38, M40, M43 (cancel only), M48 (two guards), M49, M53, M55, M58, M59, M60 (marker),
M61, M62, M64, M66. Markers only, the design leaving the value open or ess 0.60.0 refusing the
guard: M17, M20, M23, M29, M30, M31, M39, M41, M44, M45, M47, M50, M52, M54, M57, M63.

False positives, unchanged:

| id | reason |
|---|---|
| M19 (graph part) | no graph change; fixed in `session.yaml` (`OpenSession/outcome/mode-mismatch`) |
| M28 (refusal part) | design 835-843 validates the schema after `ToolCallProposed`; refusing at proposal contradicts it |
| M33 | dropping the terminal flag of a one-state lifecycle is refused (`dead_end_state`); its lifecycle is a planned story |
| M42 | design 603's "replaced" is `ReviseSpan`; 684 is an edge overload policy, not a Span lifecycle |
| M46 | the unguarded `refused` branch covers what `declared` does not; the design names no incoherent locality and execution-form pair |
| M51 | ess 0.60.0 requires a lifecycle on every entity; a comment states the single state is not a design state |
| M56 | design 641 is a SHOULD on descriptors and metrics, not on commands |
| M65 | Binding already cites 519 |

### Spec diff against 0.1.0

`ess verify diff` exits 4: 180 changes, 13 breaking, 48 compatible, 119 unknown. Breaking: enum
members `OverloadPolicy.cancel`, `.supersede`, `Capability.orchestrated_duplex`,
`Session.State.Ready`, `.Draining`, `.Closed`, `OutputSegment.State.Synthesized`; inputs
`FailRun.failure`, `DeclareModel.provenance`, `.source_revision`, `DeclareTool.schema`,
`ProposeToolCall.basis_span_id`; outcome `DeclareProfile/wrong-egress`. No implementation of
graph, stream, tools, session or policy exists yet; the model changes are carried by
`story:core-model-sync`.

### Owner calls taken by the fix

- The tool barrier is split: `ProposeToolCall` bars every proposal on a Hypothesis span, which is
  stricter than design 624 for read-only tools; `ProposeSpeculativeToolCall` takes read-only tools
  on any span.
- Locality, admission and unbound-node refusals are decided outside the specification (external).
- A denied admission leaves the run Validating; `DescribeNodeInstance` accepts Preparing and Active
  only; profile egress stays an input checked against the design's table.

### Stories corrected (step 4)

All 22 other stories, 84 sections through `aep plan artifact body --section`: moved spec line citations, the four
scenario ids gone from the suite (`CancelRun/outcome/cancelled`, `CancelGeneration/outcome/cancelled`,
`SupersedeGeneration/outcome/superseded`, `Run/state/Draining/refuses/FailRun`) replaced by the authored ones,
renamed inputs (`input_basis` to `basis_span_id`, `reason` to `failure`), the new refusals each story's ids now
include, and `--scenarios ess/scenarios` on every synthesize line. Stream scenarios now create a Run in their
arrangement, so the 54 such ids of `story:outbound-text-generations-and-segments` and 17 of
`story:inbound-text-spans-and-envelope` pass with `story:run-lifecycle`, 9 ids of `story:pipeline-declaration`
pass with `story:pipeline-validation`, and `story:tool-call-lifecycle` depends on `story:run-lifecycle`. Scope prose
counts were left as the scoper wrote them; the typed `scope` entries are unchanged.

## Re-check 2026-10-29

Re-checked on 2026-10-29 with ess 0.76.0 on the specification of `origin/main` at `de88601`, and
left out of that wave because `dependency-blocker:ess-affects-witness` is still open:

- `ess specify validate --path ess`: exit 0, 11 files, 9 scenarios, one warning (ESS-ENTITY-019 on
  `inference.session.HistoryEntry.call_id`, as under 0.75.0).
- `ess verify conform synthesize --path ess --scenarios ess/scenarios`: 475 scenarios (9 authored),
  1 refusal, ESS-SYNTH-001 `inference.graph.CancelRun/outcome/cancelled`.
- `ess verify conform run --path ess --scenarios ess/scenarios --target interpreted --report-format 2`:
  474 passed, 0 failed, 0 error, 1 unsupported (`inference.graph.AddSubgraphNode/outcome/added`).

The acceptance needs 0 refusals. Both remaining results are owned by ESS; nothing in this
repository clears them.

## Re-check 2026-10-30

Re-checked on 2026-10-30 with ess 0.77.0 on the specification of `origin/main` at `5ff03a6`, for the
wave that moves the pin to 0.77.0 (approval-record:wave-0-9-0), and left out of it because
`dependency-blocker:ess-affects-witness` is still open:

- `ess specify validate --path ess`: exit 0, 11 files, 9 scenarios, one warning (ESS-ENTITY-019 on
  `inference.session.HistoryEntry.call_id`, as under 0.76.0).
- `ess verify conform synthesize --path ess --scenarios ess/scenarios`: 475 scenarios (9 authored),
  1 refusal, ESS-SYNTH-001 `inference.graph.CancelRun/outcome/cancelled`, with the same reason as
  under 0.76.0.
- `ess verify conform run --path ess --scenarios ess/scenarios --target interpreted --report-format 2`:
  474 passed, 0 failed, 0 error, 1 unsupported (`inference.graph.AddSubgraphNode/outcome/added`), exit 1.

The acceptance needs 0 refusals. Both remaining results are owned by ESS (ESS
story:a-command-with-two-related-selectors-arranges-both and
story:an-unfillable-required-field-is-named-by-validate-and-the-view, both draft in ESS on
2026-10-30); nothing in this repository clears them.
