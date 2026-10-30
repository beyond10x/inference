---
format: aep.planning-md/3
id: dependency-blocker:ess-affects-witness
kind: dependency-blocker
status: open
title: ess synthesis and the interpreted target leave one refusal and one unsupported scenario
relations:
- blocks: story:ess-defect-fixes
revision: 12
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

## Under ess 0.69.2

Read on 2026-10-25 with the pin moved to ess 0.69.2, against ess 0.68.0 on the same specification:

| run | ess 0.68.0 | ess 0.69.2 |
|---|---|---|
| `ess verify conform synthesize --path ess --scenarios ess/scenarios` | 471 scenarios, 5 refusals | 475 scenarios, 1 refusal |
| `ess verify conform run … --target interpreted --report-format 2` | 469 passed, 1 failed, 1 unsupported | 474 passed, 0 failed, 1 unsupported |

- Synthesized now: `inference.duplex.CloseSpeechSession/outcome/closed`,
  `inference.duplex.FailSpeechSession/outcome/failed`,
  `inference.stream.CancelGeneration/outcome/cancelled` and
  `inference.stream.SupersedeGeneration/outcome/superseded` (ess `CHANGELOG.md` § 0.69.0, Fixed: "A
  moving `affects:` entry gets its scenario when the rows it moves are created by a branch below a
  `when:` refusal, or below a `when_related:` refusal of a state the related row is not held in").
- Still refused, ESS-SYNTH-001: `inference.graph.CancelRun/outcome/cancelled`, "`cancelled.affects[0]`
  is `inference.stream.Generation` and created only by `inference.stream.StartGeneration/started`,
  which no arrangement takes for a row the filter selects with the rows the scenario holds when it is
  sent". Its authored scenario `cancel-run-discards-unplayed.yaml` stays.
- `inference.usage/authored/unknown-quantity-stays-unknown` passes the interpreted run (ess
  `CHANGELOG.md` § 0.69.0, Fixed: "an asserted `value: null` matches a row that omits `value`").
- Still unsupported: `inference.graph.AddSubgraphNode/outcome/added` (ESS-CF-TARGET).

The blocker stays open: one refusal and one unsupported scenario remain, both owned by the ESS
stories above (ess 0.70.0). The four authored cascade scenarios stay until the last refusal clears.

## Under ess 0.76.0

Read on 2026-10-29 with the pin moved to ess 0.76.0, same specification:

| run | ess 0.69.2 | ess 0.76.0 |
|---|---|---|
| `ess verify conform synthesize --path ess --scenarios ess/scenarios` | 475 scenarios, 1 refusal | 475 scenarios, 1 refusal |
| `ess verify conform run … --target interpreted --report-format 2` | 474 passed, 0 failed, 1 unsupported | 474 passed, 0 failed, 1 unsupported |

- Still refused, ESS-SYNTH-001: `inference.graph.CancelRun/outcome/cancelled`, with the same reason
  as under 0.69.2 ("created only by `inference.stream.StartGeneration/started`, which no arrangement
  takes for a row the filter selects with the rows the scenario holds when it is sent"). ESS
  story:a-cascade-onto-rows-of-a-guarded-creator-is-witnessed is implemented in ESS and does not
  witness this cascade; the case is reported to ESS again with this specification as the repro.
- Still unsupported: `inference.graph.AddSubgraphNode/outcome/added` (ESS-CF-TARGET). ESS
  story:an-unfillable-required-field-is-named-by-validate-and-the-view is a draft in ESS.

The blocker stays open. The four authored cascade scenarios stay.

## Under ess 0.77.0

Read on 2026-10-30 with the pin moved to ess 0.77.0, same specification:

| run | ess 0.76.0 | ess 0.77.0 |
|---|---|---|
| `ess verify conform synthesize --path ess --scenarios ess/scenarios` | 475 scenarios, 1 refusal | 475 scenarios, 1 refusal |
| `ess verify conform run … --target interpreted --report-format 2` | 474 passed, 0 failed, 1 unsupported | 474 passed, 0 failed, 1 unsupported |

- Still refused, ESS-SYNTH-001: `inference.graph.CancelRun/outcome/cancelled`, same reason. ESS's own
  record of this case (on ESS story:a-cascade-onto-rows-of-a-guarded-creator-is-witnessed, which is
  implemented) names the remaining cause as a command reading two related rows, probably ESS
  story:a-command-with-two-related-selectors-arranges-both (draft in ESS, inferred there).
- Still unsupported: `inference.graph.AddSubgraphNode/outcome/added` (ESS-CF-TARGET). ESS
  story:an-unfillable-required-field-is-named-by-validate-and-the-view is still a draft in ESS.

The blocker stays open. The four authored cascade scenarios stay.
