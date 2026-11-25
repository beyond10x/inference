---
format: aep.planning-md/3
id: story:spoken-form-check-before-synthesis
kind: story
status: draft
title: The spoken form of numbers and names is checked before synthesis
tags:
- from-consumer-gap
relations:
- decomposes: epic:sdk
- serves: vision:inference
revision: 1
---
## Context

A consumer dubbed a 60 s screencast on 2026-11-24. Its fourth gap: a check of the spoken form of
numbers and names before synthesis. It could check pronunciation only through the synthesiser's
text-to-phoneme step, after the fact.

A synthesiser reads "0.34.0", "64 CPUs" or a product name in whatever way its front end decides,
and the caller hears it only in the audio. Nothing in this repository shows a caller, before a
paid or slow synthesis, which tokens of a text have no single obvious spoken form. Serves O3:
nothing irreversible rests on a guess.

## Planned scope

- A function over text that returns the tokens a caller should review before synthesis: digits and
  version-like strings, units, abbreviations in capitals, and words not in a caller-supplied list of
  known names; each with its position in the text.
- A caller-supplied table of replacements (written form to spoken form) applied before synthesis,
  with the list of replacements that were applied returned beside the text.
- No model call and no network: the check is rules over text.

## Acceptance

- A test: a text with a version number, a unit, a capitalised abbreviation and an unknown name
  returns those four tokens with their byte ranges, and a text of plain words returns none.
- A test: a replacement table turns the written forms into the spoken forms, reports each
  replacement once per occurrence, and leaves text outside the matches byte-identical.
- The check runs on the text a `Voice` is about to synthesise only when the caller asks for it;
  synthesis without the check is unchanged.

## Out of scope

Phoneme output of a particular synthesiser; languages other than English in the first version;
automatic expansion of numbers into words.
