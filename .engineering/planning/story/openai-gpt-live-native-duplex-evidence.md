---
format: aep.planning-md/3
id: story:openai-gpt-live-native-duplex-evidence
kind: story
status: draft
title: The native_duplex claim of gpt-live-1 and its live evidence disagree
relations:
- derived_from: story:quality-live-lane-on-the-workstation
revision: 1
---
## Problem

The first keyed live run of `openai-gpt-live` (`task quality-live`, run 20261127T135923Z, main
2c76644c1, model `gpt-live-1`) passed 2 of its 3 tests and wrote no record. The test
`native_duplex_evidence_of_gpt_live` (`adapters/inference-openai/tests/live_gpt.rs:454`) failed:
the binding claims `native_duplex`, and the hosted model's answer did not depend on the input
sent while it answered:

    native_duplex evidence: Err(NotDependent { before: " Can you", after: " tell me what you mean by Okay. What would you like to know about it?" })
    assertion `left == right` failed: the binding claims native_duplex exactly when the hosted model gives the evidence, and only an outcome that put the model to the test decides it
      left: Ok(false)
     right: Ok(true)

One run only: whether the claim is wrong for `gpt-live-1`, or the probe decides on too little
(one exchange, one interruption), is not known. Each run costs 3 realtime sessions.

## Outcome

Either the binding's `native_duplex` claim matches what the hosted model shows over repeated
runs, or the probe states how many runs and which share decide it, and `openai-gpt-live` writes
a record from the two tests that measure start and close even when the evidence test decides
against the claim.

## Acceptance

To be written when the story is taken; a run of the live lane costs money and needs the owner's
yes to its count.
