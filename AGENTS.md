# inference

Rust contracts, adapters, pipelines and a runtime for AI inference. This file is for agents working
on this repository. `README.md` is for people.

## Start here

| read | for |
|---|---|
| `docs/design/inference.md` | the design (v0.2, 2026-10-16): vocabulary, contracts, streaming semantics, graph, runtime, roadmap, open questions |
| `docs/design/review-2026-10-18.md` | the review of that design: findings, what the plan does differently, and why |
| `docs/handoff/2026-10-19-implementation.md` | where the implementation waves stopped after 0.4.0: what is implemented, the stories left in order, the facts a next session needs |
| `docs/handoff/2026-10-19-planning.md` | where the planning session stopped: what is in the store, what is left (un-reviewed post-round fixes, open blockers) |
| `aep plan artifact list` | the plan: epics, stories, blockers and their status |
| `aep plan artifact waves --kind story --status draft` | what can be worked next |

## Layout

| path | what |
|---|---|
| `Cargo.toml`, `crates/` | the Cargo workspace; one crate per directory under `crates/`, `inference-core` first |
| `examples/` | example packages built and tested with the workspace, never published and with no binary target; `stt-only` recognises speech with the STT contract and the mock alone; `realtime-voice` wires microphone, a native speech model and speaker (barge-in, a local-time tool), tested with the duplex mock and the fake audio host, run with `cargo run -p realtime-voice --example talk -- --provider openai\|nova`; `local-voice-cascade` wires STT, LLM and TTS through commit gates to a speaker that acknowledges playback, tested with the testkit mocks and the fake audio host |
| `adapters/` | hosted and local integrations, one crate per provider, each enabled explicitly; `inference-openai` is the OpenAI Realtime client, its live test ignored and behind feature `live` |
| `docs/design/` | design documents and their reviews |
| `ess/` | the ESS specification: `system.yaml`, `ess-inputs.yaml` and one domain document per layer of the design under `domains/` (model, policy, graph, stream, tools, session); every declaration carries a `read from:` line naming the design line it was read from; `scenarios/` holds authored scenarios for what synthesis cannot witness |
| `.engineering/planning` | the AEP planning store; change it only through `aep plan artifact` verbs |
| `.engineering/drafts` | scratch bodies for `aep plan artifact … --from`; git-ignored |
| `.github/workflows/` | `check.yml` runs `task check` with pinned `ess` and `aep` releases; `shared-gates.yml` calls the common source gates of `beyond10x/gates` at an exact commit |
| `CHANGELOG.md` | one entry per release, written before the tag |

## Commands

```
task check                      # specification and planning store validation, then format, lint and tests
ess specify validate --path ess
ess verify conform synthesize --path ess --scenarios ess/scenarios --out .engineering/drafts/suite.json   # the obliged scenarios plus the authored ones; the only refusals allowed are the three affects cascades, each executed by an authored scenario
ess verify conform run --path ess --scenarios ess/scenarios --target interpreted --report-format 2 --report-out .engineering/drafts/report.json   # every scenario against the specification itself
aep plan artifact validate
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features   # --all-features builds the inference-testkit mocks behind its features run, stt, llm, tts and duplex
cargo test -p inference-testkit --test conform   # the testkit conformance target alone: synthesizes the suite, runs it in process, has `ess verify conform report` write the verdict; prints target/tmp/conform-<run>/, where suite.json, results.json and report.json stay, red or green
git add <paths> && b10x-gates bot -- commit -F <file>   # staging is plain git; the route refuses add
b10x-gates scan-text --policy <file> <path>...           # release and pull-request bodies before they are sent; no policy found is "not scanned", never "passed"
```

## Rules

- Anything committed here that runs is Rust, with clap derive for command lines. A model worker in
  another language is an external process this repository talks to over a versioned protocol; it is
  not committed here.
- Spec first: a story that introduces a noun declares it in `ess/` before code builds on it, checked
  with `ess specify validate --path ess`. A story's acceptance is a set of named conformance
  scenarios.
- This is a composition library. `inference-core`, the text, audio, STT, LLM, TTS, tools and duplex
  contract crates and `inference-pipeline` carry no concrete model, backend or provider
  implementation. A concrete integration is an adapter crate that depends on the contracts, never
  the reverse. The repository ships libraries: no binary, server or CLI.
- A local adapter's backend code is Apache-2.0 or MIT (or more permissive), and the weights it
  pins are CC BY 4.0 or more permissive. Each checkpoint's licence, source URL and hash go in the
  adapter's capability record at adoption; any other licence needs the owner's word for that
  checkpoint. Tests download no weights and start no model server: a test that needs one reads
  its path or URL from an environment variable and is ignored without it.
- `inference-llm` is a thin wrapper over the `llm` repository's model contract
  (`b10x-llm-core`, `llm_core::Model`) at a release tag, 0.8.0 now, and the workspace's
  `rust-version` is what that tag requires, 1.98 (owner decision, 2026-10-20, reversing the
  2026-10-18 rule for `llm` only). Its request, items, tool calls and usage are `llm`'s; the
  wrapper adds only graph concerns, and `llm`'s wire crates, being provider implementations, stay
  out of the contract crates. Other beyond10x repositories (`llm-gateway`, `loom`) are not
  dependencies and the plan treats them as absent (owner decision, 2026-10-18); an adapter to one
  may be written later. The `secrets` library is the exception, used only by adapters' local
  credential lookup behind a feature (owner decision, 2026-10-19).
- The planning store is the plan. Work is recorded there, never only in a document or a chat.
- Work in a managed worktree (`worktree create --purpose <what>`), build into its own `target/`.
- Every commit, push, pull request and tag goes through the bot route (`b10x-gates bot -- <git
  args>`); API writes through `b10x-gates api`. Local hooks are optional; the shared gates on
  GitHub are the authority, and `main` is integrated only with its `Security and privacy` and
  `inference / task check` runs green.
- A release is a bare annotated version tag (`0.1.0`) on gated `main`, pushed through the bot
  route, then `b10x-gates gh -- release create <tag> --verify-tag --notes-file <file>` with a
  notes file that passed `b10x-gates scan-text`. The `CHANGELOG.md` entry lands in the commit
  the tag points at. No binary is published: this repository ships libraries.
- The CI pins (`ess`, `aep`, go-task, the actions' commits, the `common.yml` commit) move only by
  a commit here that names the new version and its checksum.
- This repository is public. Committed text names no private repository, session or decision id,
  host detail or person.
- No comments in `Taskfile.yml`, `Cargo.toml` or any other config or manifest file.
