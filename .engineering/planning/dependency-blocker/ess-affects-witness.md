---
format: aep.planning-md/3
id: dependency-blocker:ess-affects-witness
kind: dependency-blocker
status: open
title: ess synthesis finds no witness for the three affects cascades
relations:
- blocks: story:ess-defect-fixes
revision: 1
---
## What is blocked

`story:ess-defect-fixes` accepts on 0 synthesis refusals. ess 0.60.0 refuses three outcomes with
ESS-SYNTH-001: `inference.graph.CancelRun/outcome/cancelled`,
`inference.stream.CancelGeneration/outcome/cancelled` and
`inference.stream.SupersedeGeneration/outcome/superseded`. Each carries an `affects` move onto
`inference.stream.OutputSegment` (or `inference.stream.Generation`), whose rows are created by a
command with a `when_related` guard (`EmitTextDelta`, the M01 fix). Synthesis finds no witness for
the move; seeds apply to none of them; ess 0.62.0 refuses the same three. Authored scenarios in
`ess/scenarios/` execute all three cascades and pass the interpreted run, but do not discharge a
synthesis obligation.

## What clears it

An ess release that synthesizes a witness for an `affects` move in this shape, installed under the
toolchain pin, after which `ess verify conform synthesize --path ess --scenarios ess/scenarios`
reports 0 refusals. Reported to the ESS maintainers on 2026-10-19.
