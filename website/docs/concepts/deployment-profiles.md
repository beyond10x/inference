---
title: Deployment profiles
sidebar_position: 7
description: The four profiles that say where a model may run, how offline_strict is enforced at planning and at run time, and how credentials reach hosted adapters.
lede: Self-hosted first - a hosted provider is an adapter you switch on, never a fallback.
source: crates/inference-core (profile), crates/inference-pipeline (planning), crates/inference-runtime (egress.rs), ess/domains/policy.yaml, adapters/inference-openai, adapters/inference-aws
---

# Deployment profiles

Self-hosted first. A hosted provider is an adapter you switch on, never a fallback.

Every pipeline is declared under a deployment profile. A profile row outside this table is refused
(`ProfileOutsideTable`).

| Profile | Admitted compute | Default egress |
|---|---|---|
| `offline_strict` | in-process or local workers only | denied |
| `self_hosted_lan` | local machine and approved internal services | internal allowlist |
| `self_hosted_cluster` | approved owned infrastructure | authenticated internal paths |
| `hybrid_opt_in` | approved self-hosted and explicitly selected remote services | per-provider allowlist |

Only `hybrid_opt_in` admits an adapter at locality `external_provider`, the locality of every
hosted adapter.

## `offline_strict`, enforced twice

1. **At planning**: an adapter whose locality the profile does not admit is refused with
   `LocalityDenied`, before anything runs. A local worker on loopback, such as `llama-server` on
   127.0.0.1, is admitted.
2. **At run time**, the egress watch: on Linux, a run whose profile denies egress fails once any
   socket of the process connects to an address that is neither loopback nor unspecified, or holds
   an unconnected datagram socket not bound to loopback. The failure names the address.

The egress watch is a tripwire and a test instrument, not a sandbox. What it misses is listed in
[Limitations](../status/limitations.md#the-egress-watch-is-not-a-sandbox).

## Credentials

Adapters never look a key up themselves. The OpenAI adapter takes an injected credential source:
the environment, or the OS keychain behind the `keychain` feature, read through
[Secrets](https://beyond10x.github.io/secrets/). Nova Sonic uses the AWS SDK's default credential
chain. AWS service errors are shown with ARNs and account ids redacted.
