---
format: aep.planning-md/3
id: epic:hosted-and-scale-out
kind: epic
status: draft
title: Hosted providers and scale-out (P7)
summary: Opt-in hosted adapters refused by the offline profile, the worker process protocol, quotas and egress audit.
relations:
- derived_from: architecture-design:inference
- depends_on: epic:local-voice-cascade
- serves: vision:inference
revision: 2
---
## Context

Stage P7, "hosted and scale-out" (`docs/design/inference.md:1165`): opt-in provider plugins, the
worker protocol, quotas, privacy auditing; exit "same contracts run across local and approved remote
workers". Hosted providers are adapters, never a replacement for the local-first model (§18, lines
956-982; ADR-010, line 1195); they are explicitly enabled, data moves to them only under an egress
policy, and automatic cloud fallback is off (§16.4, lines 903-907). Third-party plugins use a
versioned process or service protocol, not a Rust ABI (§16.2, lines 884-895; ADR-011, line 1196).
The deployment profiles and mandatory controls are §21 (lines 1040-1065).

The `llm` and `llm-gateway` repositories are treated as absent (owner, 2026-10-18); a hosted text-LLM
adapter here is its own crate.

## Outcome

One hosted adapter runs under `hybrid_opt_in` and is refused under `offline_strict`; one worker
runs behind the process protocol with the same contracts; tenant quotas and egress audit exist.

## Exit

- A-11 (line 1147).
- The `offline_strict` profile refuses an adapter with locality `external_provider` at planning
  (`inference.graph.PlanPipeline/outcome/mismatch` extended with a policy refusal declared in the
  specification first).
- §23.2 line 1120 holds for every local profile with a hosted adapter present but disabled.

## Not decomposed

Stories wait for P0 to P3; the process protocol (Q12, line 1237) is settled before the worker story.
