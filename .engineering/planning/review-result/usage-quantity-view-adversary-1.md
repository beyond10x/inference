---
format: aep.planning-md/3
id: review-result:usage-quantity-view-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on the usage quantity view
relations:
- reviews: story:usage-quantity-view
revision: 1
---
## Verdict

needs-change, one finding, introduced by the unit.

```findings
- file: ess/domains/usage.yaml
  line: 78
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: no scenario records a quantity of source unknown, so a UsageRecords view publishing an unknown quantity with value 0 passes all 470 scenarios (crates/inference-testkit/tests/adversary_usage_view.rs:194 red)
```

## Attacked and held

- The view's 13 fields match the `UsageRecord` entity fields plus identity, with the same types and optionals.
- Leaving out `state` matches the other single-state views in `model.yaml`.
- Dropped quantities are caught by `RecordUsage/outcome/recorded` (the `DropQuantities` variant goes red).
- No generated page or `inference-docs` test reads the new view.
- The AGENTS.md and CHANGELOG counts match the synthesis run (470 scenarios, 5 refusals) and the interpreted run (1 unsupported).

## Case adopted

`crates/inference-testkit/tests/adversary_usage_view.rs` wraps the testkit target. It rewrites the `UsageRecords` rows three ways: unchanged, quantities dropped, and an unknown quantity read as 0. It then runs the whole synthesized suite against each variant.
