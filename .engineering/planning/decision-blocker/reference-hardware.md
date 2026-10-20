---
format: aep.planning-md/3
id: decision-blocker:reference-hardware
kind: decision-blocker
status: cleared
title: Nobody has named the reference hardware for the first self-hosted suite (Q9)
relations:
- blocks: epic:local-stt-adapter
- blocks: epic:local-llm-adapter
- blocks: epic:local-tts-adapter
- blocks: epic:local-voice-cascade
revision: 4
transitions:
- {from: "open", to: "cleared", at: "2026-10-20T07:48:51Z", actor: "human:timo", revision: 4}
---
## Question

Open question 9 of the design: "What is the reference hardware target for the first self-hosted
integration suite?" (`docs/design/inference.md:1234`). The capability record of every adapter carries
a "resource profile for target hardware, not a universal performance promise" (line 951), and
admission must answer `not_enough_vram` and `backend_busy` (line 693). Nothing in the repository
names a machine.

## What it stops

The three local adapter epics and the cascade: which backend builds, which checkpoint fits, and
whether the cascade speaks offline on the machine at hand all depend on it.

## Default on silence

The machine at hand when the first adapter story starts, named here with its CPU, GPU and memory
once known. The decision is reversible: a second hardware target is a second capability record,
not a contract change.

## Clears when

The owner names the machine, or accepts the default, and the answer is written into this body.

## Decision

The owner took the default on 2026-10-20. The reference machine for the first self-hosted suite:

| part | value |
|---|---|
| CPU | Intel Core i9-10900K, 10 cores, 20 threads |
| memory | 62 GiB |
| GPU | NVIDIA GeForce RTX 3090, 24 GiB VRAM |
| OS | Linux x86_64 |

Every local adapter's capability record states its resource profile against this machine. CPU-only
execution is the baseline each adapter's tests run on; the GPU is an option a backend may declare,
never a requirement of a test. A second machine is a second capability record, not a contract
change.
