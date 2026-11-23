---
format: aep.planning-md/3
id: approval-record:wave-2026-11-23-review-findings
kind: approval-record
status: draft
title: 'Wave approved: review findings 4, 5, 6, 7 and 13 of release 0.31.0, one pull request (2026-11-23)'
revision: 1
---
## Decision

On 2026-11-23 the operator asked to address the three failures a review of release 0.31.0
highlighted:

- a completed chat reply lost its usage record (finding 6);
- a graph that hands one usage record to two outputs counted one request twice (finding 7);
- a two-message model reply lost its second message before speech (finding 4).

That request is the approval of this wave. Three units were implemented and committed on their own
branches before the wave opened, each from origin/main at 0.31.0 (8c2d6f15):

| unit | story | findings | branch |
|---|---|---|---|
| U1 | story:chat-recipe-usage | 6 | `impl/chat-recipe-usage` |
| U2 | story:usage-dedup-fan-out | 7 | `impl/usage-dedup-fan-out` |
| U3 | story:final-message-commit | 4, 5, 13 | `impl/final-message-commit` |

U1 also files story:cascade-recipe-usage as a draft: the voice cascade recipe's usage wiring is
not part of this wave.

## Breaking

U1 changes the signature of `BoundRecogniser::node` and of `Synthesis::Plain`, and updates every
caller in this repository in the same change.

## Integration

Branch `wave/2026-11-23-review-findings`, each unit merged with `--no-ff`. `CHANGELOG.md` keeps every
unit's Unreleased line; generated files are regenerated with `inference-docs generate`, never
merged by hand.

## Gates

Once, on the merged tree: `cargo fmt --all --check`; the tests of `inference-llm`,
`inference-facade`, `inference-pipeline` and `inference-runtime` with all features, and of
`inference` with `testing,cost` (its lib and the `recipes`, `stack` and `agent` targets); clippy
with warnings denied on `inference-llm`, `inference-facade` and `inference`;
`inference-docs generate --check`; `ess specify validate --path ess`; `aep plan artifact validate`.
The whole-workspace test run is not started on the shared host: it exceeds the test budget.

## Commits authorised

The merges of the three unit branches into the wave branch, the store commits that open and close
the wave, fixes of merge fallout, and one pull request to `main` through the bot route. No merge
of that pull request by this wave's run, no release, no tag, no version bump.

## Not fixed in this wave

- The voice cascade recipe's usage record (story:cascade-recipe-usage, draft).
- A message sent whole before a streamed one is committed after it.
