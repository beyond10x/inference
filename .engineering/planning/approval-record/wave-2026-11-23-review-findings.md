---
format: aep.planning-md/3
id: approval-record:wave-2026-11-23-review-findings
kind: approval-record
status: draft
title: 'Wave approved: review findings 4, 5, 6, 7 and 13 of release 0.31.0, one pull request (2026-11-23)'
relations:
- decides: story:chat-recipe-usage
- decides: story:usage-dedup-fan-out
- decides: story:final-message-commit
revision: 2
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

## Delivered

Delivered on 2026-11-23 on `wave/2026-11-23-review-findings`:

| unit | story | branch head | merge into the wave branch |
|---|---|---|---|
| U3 | story:final-message-commit | 44537e0a | 646bdd44 |
| U2 | story:usage-dedup-fan-out | 1b6c58f4 | 405becef |
| U1 | story:chat-recipe-usage | c3e3b246 | b90ef12f |

story:chat-recipe-usage and story:final-message-commit arrived implemented with their branches.
story:usage-dedup-fan-out arrived active; it moved active -> implemented on its clippy run
(`cargo clippy -p inference-facade --all-targets --all-features -- -D warnings`, exit 0) and the
wave gate. Each story has a `test_result` record of the wave gate at b90ef12f.

Conflicts: `CHANGELOG.md` only, in two merges; every unit's line is kept under `### Fixed` of
`## [Unreleased]`. No code file conflicted. `website/docs/concepts/usage.md` merged cleanly. No
generated file changed (`inference-docs generate --check` exit 0).

The changed signatures of `BoundRecogniser::node` and `Synthesis::Plain` are `pub(crate)`: no
`Breaking` section.

Gate at b90ef12f, every step exit 0:

| step | time |
|---|---|
| `cargo fmt --all --check` | 23 s |
| `cargo run -p inference-docs -- generate --check` | 39 s |
| `ess specify validate --path ess` | 0 s |
| `aep plan artifact validate` | 1 s |
| `cargo test --no-fail-fast -p inference-llm --all-features`: 84 passed | 30 s |
| `cargo test --no-fail-fast -p inference-facade --all-features`: 123 passed | 86 s |
| `cargo test --no-fail-fast -p inference-pipeline --all-features`: 161 passed | 44 s |
| `cargo test --no-fail-fast -p inference-runtime --all-features`: 287 passed | 115 s |
| `cargo test --no-fail-fast -p inference --features testing,cost --lib --test recipes --test stack`: 129 passed | 135 s |
| `cargo test --no-fail-fast -p inference --features testing,cost --test agent`: 66 passed | 134 s |
| `cargo clippy -p inference-llm --all-targets --all-features -- -D warnings` | 37 s |
| `cargo clippy -p inference-facade --all-targets --all-features -- -D warnings` | 62 s |
| `cargo clippy -p inference --all-targets --features testing,cost -- -D warnings` | 48 s |

Times include compilation. Over the test budget, on the base already and not changed by this wave:
the `agent` target runs `the_builders_role_methods_refuse_a_binding_of_another_type` (122.5 s) and
`every_send_of_an_input_the_model_closed_at_is_ok_in_1000_rounds_of_16_sessions` (76.1 s); its
step ran 134 s, 124 s of it in tests.

Not fixed in this wave: the voice cascade recipe's usage record (story:cascade-recipe-usage,
draft), and a message sent whole before a streamed one is committed after it.
