---
format: aep.planning-md/3
id: dependency-blocker:ess-affects-witness
kind: dependency-blocker
status: open
title: ess synthesis and the interpreted target leave five refusals, one unsupported and one failed scenario
relations:
- blocks: story:ess-defect-fixes
revision: 8
---
## What is blocked

`story:ess-defect-fixes` accepts on 0 synthesis refusals and a clean interpreted run. As of ess
0.67.0 (the same under 0.66.0), `ess verify conform synthesize --path ess --scenarios ess/scenarios`
gave 469 scenarios and six refusals; after story:usage-quantity-view it gives 470 and five (the ESS-SYNTH-013 row is cleared):

| refusal | outcome or type | workaround |
|---|---|---|
| ESS-SYNTH-001 | `inference.duplex.CloseSpeechSession/outcome/closed` (`ess/domains/duplex.yaml:1132`) | authored scenario `close-delivers-received-usage.yaml` |
| ESS-SYNTH-001 | `inference.duplex.FailSpeechSession/outcome/failed` (`ess/domains/duplex.yaml:1166`) | authored scenario `fail-delivers-received-usage.yaml` |
| ESS-SYNTH-001 | `inference.graph.CancelRun/outcome/cancelled` (`ess/domains/graph.yaml:1854`) | authored scenario `cancel-run-discards-unplayed.yaml` |
| ESS-SYNTH-001 | `inference.stream.CancelGeneration/outcome/cancelled` (`ess/domains/stream.yaml:711`) | authored scenario `cancel-generation-discards-unplayed.yaml` |
| ESS-SYNTH-001 | `inference.stream.SupersedeGeneration/outcome/superseded` (`ess/domains/stream.yaml:681`) | authored scenario `supersede-discards-unplayed.yaml` |
| ESS-SYNTH-013 | type `inference.usage.Quantity` has no scenario (`ess/domains/usage.yaml:78`) | cleared by story:usage-quantity-view: the view `inference.usage.UsageRecords` publishes the type, and synthesis obliges its invariant scenario |

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

Under ess 0.67.0 and 0.68.0 the interpreted run also fails the authored scenario
`inference.usage/authored/unknown-quantity-stays-unknown` (471 scenarios: 469 passed, 1 failed, 1
unsupported). The interpreted `UsageRecords` view leaves an unknown quantity's `value` out; the
scenario asserts `value: null`, and with that line removed the run passes the scenario (470 passed).
The null stays: it is what fails a view that reads the unknown quantity as 0
(`crates/inference-testkit/tests/adversary_usage_view.rs`, `ZeroUnknown`).

`AGENTS.md` names the five remaining refusals, the unsupported scenario and the failed one, as of ess 0.68.0.

## What clears it

| result | owner | clears with |
|---|---|---|
| five ESS-SYNTH-001 cascades | ESS story:a-cascade-onto-rows-of-a-guarded-creator-is-witnessed | ess 0.70.0 |
| ESS-SYNTH-013 `inference.usage.Quantity` | this repository: `ess/domains/usage.yaml` declares no view, so nothing publishes `Quantity` | cleared by story:usage-quantity-view (view `inference.usage.UsageRecords`) |
| interpreted unsupported `AddSubgraphNode/outcome/added` | ESS story:an-unfillable-required-field-is-named-by-validate-and-the-view | ess 0.70.0 |
| interpreted failed `unknown-quantity-stays-unknown` | ESS: `contains` with `null` against an absent optional field, reported to ESS 2026-10-23 | not yet scheduled |

Cleared when the pin is on ess 0.70.0 or newer, the usage view has landed, synthesis reports 0
refusals and the interpreted run reports 0 unsupported and 0 failed. The workarounds are dropped after that run
is verified.

## Decision

2026-10-22: record the six refusals and the unsupported scenario as of ess 0.67.0 in `AGENTS.md`
and here, in one pull request, with no build. ESS answered the same day with the two story ids
above and placed the `Quantity` refusal in this repository; the usage view is the next unit because
the testkit target must answer it.
