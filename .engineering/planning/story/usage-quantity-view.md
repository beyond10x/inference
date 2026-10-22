---
format: aep.planning-md/3
id: story:usage-quantity-view
kind: story
status: draft
title: The usage domain publishes its quantities through a view
revision: 1
---
## Outcome

`ess/domains/usage.yaml` declares a view that publishes a list of `inference.usage.Quantity`, and
the testkit conformance target answers it. Synthesis then obliges a scenario for the `Quantity`
invariants, and the ESS-SYNTH-013 refusal (as of ess 0.67.0) disappears.

## Acceptance

- `ess verify conform synthesize --path ess --scenarios ess/scenarios` reports no ESS-SYNTH-013.
- `task check` passes, including the new view's scenarios against the testkit target.
- `AGENTS.md` and `dependency-blocker:ess-affects-witness` no longer list the `Quantity` refusal.

## Source

ESS placed this refusal in this repository on 2026-10-22: the usage domain has no views, so nothing
publishes `Quantity`.
