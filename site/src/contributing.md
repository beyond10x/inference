# Contributing

The repository's `AGENTS.md` holds the full working rules; this page is the short version.

## Before you start

| Read | For |
|---|---|
| `docs/design/inference.md` | the design: vocabulary, contracts, streaming semantics, graph, runtime, roadmap, open questions |
| `docs/design/review-2026-10-18.md` | the review of that design, and what the plan does differently |
| `aep plan artifact list` | the plan: epics, stories, blockers and their status |
| `aep plan artifact waves --kind story --status draft` | what can be worked next |

## Spec first

A story that introduces a noun declares it in the specification under `ess/` before code builds on
it. A story's acceptance is a set of named conformance scenarios.

```sh
ess specify validate --path ess
cargo test -p inference-testkit --test conform
```

## Check

```sh
task check
```

Specification and planning-store validation, `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings` and
`cargo test --workspace --all-features`. CI runs the same task on every pull request, beside
shared source gates.

## Rules that shape a change

- **Contracts stay clean.** `inference-core`, the contract crates and `inference-pipeline` carry no
  concrete model, backend or provider. An integration is an adapter crate that depends on the
  contracts, never the reverse.
- **Libraries only.** No binary, server or CLI is shipped. Anything committed that runs is Rust,
  with clap derive for command lines. A model worker in another language is an external process
  reached over a versioned protocol.
- **Licences.** Local backend code is Apache-2.0 or MIT, or more permissive; pinned weights are
  CC BY 4.0, or more permissive; each checkpoint's licence, source URL and hash go into its
  capability record.
- **Tests fetch nothing.** No weight downloads and no model servers in tests: a test that needs
  one reads a path or URL from an environment variable and is ignored without it.
- **The planning store is the plan.** Work is recorded there, never only in a document or a chat;
  change it only through `aep plan artifact` commands.
- **No comments in config or manifest files** (`Taskfile.yml`, `Cargo.toml` and the rest).
- **Releases** are annotated version tags on a green `main`, with the `CHANGELOG.md` entry in the
  tagged commit.

## Tools

| Tool | Used for |
|---|---|
| Rust stable 1.98 or later | the workspace |
| [go-task](https://taskfile.dev) | `task check` |
| `ess` | the executable specification: validate, synthesize and run conformance |
| `aep` | the planning store |

Apache-2.0.
