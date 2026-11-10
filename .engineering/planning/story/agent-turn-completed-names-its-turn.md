---
format: aep.planning-md/3
id: story:agent-turn-completed-names-its-turn
kind: story
status: draft
title: The agent node answers a TurnCompleted only for the turn it names
tags:
- from-finding
relations:
- informed_by: review-result:adversary-u2-cascade-burst-pass-2
revision: 1
---
## Context

Found by review-result:adversary-u2-cascade-burst-pass-2 (2026-11-10). It is pre-existing: the
base read the completion the same way, without its turn id. `AgentNode`'s `turn_completed`
(`crates/inference-pipeline/src/nodes/agent.rs`) ignores the turn id the `TurnCompleted` carries.
It completes whatever turn is open.

Here is how that goes wrong:
1. A turn reopens over one that never completed.
2. A late `TurnCompleted` arrives for the old opening.
3. That completion answers the new turn before its spans have all arrived, and the spans that
   arrive afterwards are dropped.

Nothing in the repository was shown to reach this. The turn policy sends no completion that its
store refused.

## Acceptance

- When a `TurnCompleted` names a turn other than the open one, the agent node starts no
  generation and keeps the open turn's spans. A testkit case covers it.
