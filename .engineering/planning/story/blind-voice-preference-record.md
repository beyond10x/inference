---
format: aep.planning-md/3
id: story:blind-voice-preference-record
kind: story
status: proposed
title: Record a blind A/B voice preference
summary: Render two voices on the same texts, vote under shuffled labels and total the votes.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T13:07:49Z", actor: "human:operator", revision: 2}
---
## Context

A downstream adopter weighs voices by how they are perceived first and by price after.

Public TTS boards rank voices by blind pairs: a listener hears two samples of the same text and
picks the more natural one. None of these boards plays 8 kHz phone audio, so the adopter's
survey says an in-house blind listening test on call audio is still needed.

inference has no way to record such a preference.

## Planned scope

- A command (Rust, clap) that:
  - renders the same texts through two configured voices;
  - optionally passes both through the 8 kHz μ-law chain;
  - presents each pair under shuffled labels with a recorded seed;
  - writes each vote as one record (pair, labels, seed, choice, time);
  - totals a set of records per voice, with a win rate and its interval.
- Playback goes to the speaker, which is the operator's device check; rendering, shuffling and
  totals are fixture-tested.
- The preference record is a new noun: its ESS domain comes first.

## Acceptance

- A fixed seed yields the same labelling.
- No label in the output names a voice until the vote is recorded.
- A scripted set of votes totals to the known win rate.
