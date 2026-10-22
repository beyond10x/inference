---
format: aep.planning-md/3
id: dependency-blocker:ess-affects-witness
kind: dependency-blocker
status: open
title: ess synthesis and the interpreted target leave six refusals and one unsupported scenario
relations:
- blocks: story:ess-defect-fixes
revision: 3
---
## What is blocked

`story:ess-defect-fixes` accepts on 0 synthesis refusals and a clean interpreted run. As of ess
0.67.0 (the same under 0.66.0), `ess verify conform synthesize --path ess --scenarios ess/scenarios`
gives 469 scenarios and six refusals:

| refusal | outcome or type | workaround |
|---|---|---|
| ESS-SYNTH-001 | `inference.duplex.CloseSpeechSession/outcome/closed` (`ess/domains/duplex.yaml:1132`) | authored scenario `close-delivers-received-usage.yaml` |
| ESS-SYNTH-001 | `inference.duplex.FailSpeechSession/outcome/failed` (`ess/domains/duplex.yaml:1166`) | authored scenario `fail-delivers-received-usage.yaml` |
| ESS-SYNTH-001 | `inference.graph.CancelRun/outcome/cancelled` (`ess/domains/graph.yaml:1854`) | authored scenario `cancel-run-discards-unplayed.yaml` |
| ESS-SYNTH-001 | `inference.stream.CancelGeneration/outcome/cancelled` (`ess/domains/stream.yaml:711`) | authored scenario `cancel-generation-discards-unplayed.yaml` |
| ESS-SYNTH-001 | `inference.stream.SupersedeGeneration/outcome/superseded` (`ess/domains/stream.yaml:681`) | authored scenario `supersede-discards-unplayed.yaml` |
| ESS-SYNTH-013 | type `inference.usage.Quantity` has no scenario (`ess/domains/usage.yaml:78`) | `Quantity::new` enforces the invariants; no view publishes the type outside the `quantities` list |

Synthesis finds no witness for each outcome's `affects[0]` move. For the two duplex outcomes the
target `inference.duplex.ReceivedSessionEvent` "has no three rows the filter selects that the
arranging commands can produce"; for the other three the target (`inference.stream.Generation` or
`inference.stream.OutputSegment`) "is moved by its entry only from the state the move arrives in
… so a target that skips the move reads the same". The authored scenarios execute every cascade and
pass, but do not discharge a synthesis obligation.

The interpreted run (`--target interpreted --report-format 2`) gives 468 passed and one unsupported,
`inference.graph.AddSubgraphNode/outcome/added` (`ess/domains/graph.yaml:953`): "ESS-CF-TARGET: cannot
expose interpreted view: the model does not determine a complete typed view observation". The run
exits 1, so it stays outside `task check`; the testkit target runs the scenario and passes.

`AGENTS.md` names the same six refusals and the one unsupported scenario, as of ess 0.67.0.

## What clears it

An ess release, installed under the toolchain pin, after which synthesis reports 0 refusals and the
interpreted run reports 0 unsupported. Each gap is reported to the ESS maintainers (2026-10-19 for
the first three cascades, 2026-10-22 for all three gaps against ess 0.67.0); once ESS records them
as stories, this body and `AGENTS.md` cite those story ids instead of the version. The workarounds
are dropped after the fixing release is verified.

## Decision

2026-10-22: record the six refusals and the unsupported scenario as of ess 0.67.0 in `AGENTS.md`
and here, in one pull request, with no build.
