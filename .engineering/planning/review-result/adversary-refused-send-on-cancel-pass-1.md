---
format: aep.planning-md/3
id: review-result:adversary-refused-send-on-cancel-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on the refused send before a cancel: a refusal between a drain and its cancel still fails the node'
relations:
- reviews: story:refused-send-before-cancel-keeps-partial-usage
revision: 1
---
## Pass

Adversary pass 1 on story:refused-send-before-cancel-keeps-partial-usage, against `dd10365` on
`impl/refused-send-before-cancel-keeps-partial-usage`; the cases are `38da92e`,
`crates/inference-testkit/tests/adversary1_refused_send_on_cancel.rs` (test file only).

verdict: NEEDS-CHANGE
cases: executed 2→8, red 1
origin: introduced 0 / pre-existing 0 / undecided 1

## Cases

- `a_refusal_between_a_drain_and_the_cancel_that_abandons_it_ends_the_node_cancelled`: RED.
  `left: (Some("the receiving side of the edge is gone; event … was not sent"), Some(Active))
  right: (None, Some(Cancelled))`, `records: []`. Its first version passed for the wrong reason
  (the cancel won the node's next poll before the send); the case now waits for the node to let go
  of its input edge before cancelling.
- Green: the node lets go of its inputs after a refusal and waits (fails if the release of the
  inputs is removed, which the unit's own tests would not catch); a cancel taken while the refused
  send is pending; a refusal in turn 2 (turn 1 final 20 tokens, turn 2 non-final 30, each once); a
  refusal on the `usage` port then a cancel (cancelled, no record); a refusal on the `usage` port
  then a drain (fails with the refusal).
- Regression: `model_node_refused_send_on_cancel` 2, `usage_on_cancel` 5,
  `adversary1_llm_cut_node` 1; fmt and clippy clean.

Not broken: a refusal with neither cancel nor drain (the controller always drains or cancels
before it stops a node), two refusals, a record twice or a cut turn marked final. The `tool_call`
port was read, not run.

## Routing

The red case is the gap the implementor named as not done: a refusal after the node has taken a
drain that a cancel then abandons still ends failed. The finding is routed back to the unit as a
correction; origin is undecided because the base also fails every refusal.

```findings
[{"file":"crates/inference-testkit/src/llm/node.rs","line":1183,"category":"concurrency","severity":"warning","verdict":"NEEDS-CHANGE","origin":"undecided","message":"A drain already taken when an output send is refused as closed makes the node fail at once, so a cancel that follows (a cancel abandoning the run's drain and winding a consumer declared earlier down first) ends the node failed with the cut turn's usage record lost; red in crates/inference-testkit/tests/adversary1_refused_send_on_cancel.rs a_refusal_between_a_drain_and_the_cancel_that_abandons_it_ends_the_node_cancelled."}]
```
