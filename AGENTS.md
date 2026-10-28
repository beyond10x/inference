# inference

Rust contracts, adapters, pipelines and a runtime for AI inference. This file is for agents working
on this repository. `README.md` is for people.

## Serves

- **O4 — products run on the foundation.** `inference-llm` runs model turns over the released `llm`
  contract (`b10x-llm-core`, pinned by tag), and the crates are held to their ESS specification in
  `ess/`; nothing comes from the predecessor monorepo.

## Start here

| read | for |
|---|---|
| `docs/design/inference.md` | the design (v0.2, 2026-10-16): vocabulary, contracts, streaming semantics, graph, runtime, roadmap, open questions |
| `docs/design/review-2026-10-18.md` | the review of that design: findings, what the plan does differently, and why |
| `docs/handoff/2026-10-19-implementation.md`, `docs/handoff/2026-10-19-planning.md` | historical snapshots from 2026-10-19, before 0.5.0; the planning store and `CHANGELOG.md` are current, these are not |
| <https://beyond10x.github.io/inference/>, source in `website/docs/` | the user documentation: getting started, concepts, guides, examples, reference, status and limitations |
| `aep plan artifact list` | the plan: epics, stories, blockers and their status |
| `aep plan artifact waves --kind story --status draft` | what can be worked next |

## Layout

| path | what |
|---|---|
| `Cargo.toml`, `crates/` | the Cargo workspace; one crate per directory under `crates/`, `inference-core` first |
| `examples/` | example packages built and tested with the workspace, never published and with no binary target: `stt-only`, `stt-sherpa-onnx`, `realtime-voice` (with its `compare` example), `local-voice-cascade` (feature `local` binds the local adapters) and `repo-podcast`. What each does, how to run it and the variables it reads are on the site (`website/docs/examples/`, `website/docs/guides/`, `website/docs/reference/environment.md`); keep them there, not here. Live tests are ignored and read their checkpoints and servers from the environment |
| `crates/inference-docs` | the documentation site's generator and drift check (`publish = false`, never shipped): `generate` writes the generated files below, `generate --check` fails on a missing, stale or hand-edited one and checks every page's front matter, admonition titles, code fences and dependency lines; `provenance` writes the built site's `.well-known/b10x-site.json` |
| `adapters/` | hosted and local integrations, one crate per provider, each enabled explicitly; `inference-openai` is the OpenAI Realtime client and, behind feature `gpt-live`, the GPT-Live client (`v1/live/sessions`, module `live`) with its duplex binding (`live_duplex`, which cuts GPT-Live's continuous output into responses by silence), its live tests ignored and behind feature `live` (`--features live,gpt-live` for GPT-Live, billed per session second); `inference-llama-server` is an output-streaming text generator over a local llama.cpp `llama-server`, through `llm`'s Chat Completions client at the pinned `llm` tag (`llm`'s wire crates are allowed in adapters, never in contract crates), with its capability records in `capability/` (`reference-machine-cpu.json` for the default adapter, `reference-machine-cpu-tools.json` for the adapter with tools on) and its live test ignored and behind feature `live` |
| `adapters/inference-sherpa-onnx` | local adapters over the sherpa-onnx runtime as native FFI; feature `stt` is the streaming recogniser over one pinned, hash-checked checkpoint, its live and conformance tests ignored and reading the unpacked checkpoint from `INFERENCE_SHERPA_ONNX_STT_DIR`, its capability record on the reference machine in `records/`; the native library is the shared archive built without TTS (no espeak-ng or piper-phonemize), fetched and checked against its pinned sha256 by `task sherpa-onnx-archive` (which `task check` runs first) into `target/sherpa-onnx`, where `.cargo/config.toml` points `SHERPA_ONNX_LIB_DIR`, so a build without it fails instead of downloading |
| `adapters/inference-whispercpp` | a local batch speech recogniser over whisper.cpp through `whisper-rs` as native FFI, claiming neither incremental input nor output streaming, over one pinned, hash-checked `ggml-base.en.bin`; its conformance and live tests are ignored and read the checkpoint file from `INFERENCE_WHISPERCPP_STT_MODEL` (two conformance tests, the live and the resource test also a recorded speech clip from `INFERENCE_WHISPERCPP_STT_CLIP`), its capability record on the reference machine in `records/`; whisper.cpp is compiled from the sources `whisper-rs-sys` ships, CPU only, so every builder needs `cmake`, a C++ compiler and `libclang` |
| `docs/design/` | design documents and their reviews |
| `docs/brand/` | the banner and the pitch slide as hand-written SVG |
| `website/` | the documentation site, Docusaurus on `@beyond10x/docs-system` at a pinned commit (`npm --prefix website ci --allow-git=root`, `npm --prefix website run build`, output `website/build`, git-ignored); `product.json` is the landing page, `sidebars.ts` the page tree. Concept and guide pages carry `lede` and `source` in their front matter; a status on a page is "Since 0.x.0", "On main, in the next release" or planned, and moves with the release that ships the thing. Every command shown on a page was run, its output pasted from the run |
| Generated files | `website/docs/reference/crates.md`, `website/docs/reference/adapters.md`, `website/docs/reference/errors.md`, `website/docs/status.mdx`, `website/data/status.json` and `website/data/terminal/stt-only.json` are written by `task docs-generate` and never edited by hand; `task docs-check` (part of `task check`) fails when one has drifted. Each adapter's hosted claims are a JSON record under its `records/`, held equal to the adapter's code by its `claims_record` test |
| `ess/` | the ESS specification: `system.yaml`, `ess-inputs.yaml` and one domain document per layer of the design under `domains/` (audio, duplex, graph, model, policy, session, stream, tools, usage); every declaration carries a `read from:` line naming the design line it was read from; `scenarios/` holds authored scenarios for what synthesis cannot witness |
| `.engineering/planning` | the AEP planning store; change it only through `aep plan artifact` verbs |
| `.engineering/drafts` | scratch bodies for `aep plan artifact … --from`; git-ignored |
| `.github/workflows/` | `check.yml` runs `task check` with pinned `ess` and `aep` releases; `shared-gates.yml` calls the common source gates of `beyond10x/gates` at an exact commit; `pages.yml` ("Documentation validation") checks the generated pages, builds `website/` and binds the build to its commit on every pull request and push to `main`, and uploads it as the `b10x-project-site` artifact from `main`; `b10x-docs-site.yml` hands that artifact to the shared project-site workflow, which publishes it, so a documentation fix ships from `main` without a release |
| `CHANGELOG.md` | one entry per release, written before the tag; a change a consumer can see goes under `## [Unreleased]` when it lands on `main` |

## Commands

Tools: Rust stable 1.98 or later, [go-task](https://taskfile.dev), `ess` and `aep`; Node 22 and npm
for the site only.

```
task check                      # the sherpa-onnx archive, specification and planning store validation, the documentation drift check, then format, lint and tests
task docs-generate              # rewrite the generated files of the site
npm --prefix website ci --allow-git=root && npm --prefix website run build   # build the site into website/build
ess specify validate --path ess
ess verify conform synthesize --path ess --scenarios ess/scenarios --out .engineering/drafts/suite.json   # the obliged scenarios plus the authored ones; as of ess 0.72.0 the only refusal allowed is ESS-SYNTH-001 for the `affects` cascade `inference.graph.CancelRun/outcome/cancelled`, executed by the authored scenario `cancel-run-discards-unplayed.yaml` until ESS story:a-cascade-onto-rows-of-a-guarded-creator-is-witnessed (ess 0.70.0); the other four cascades are synthesized since ess 0.69.0, and their authored scenarios stay until that refusal clears
ess verify conform run --path ess --scenarios ess/scenarios --target interpreted --report-format 2 --report-out .engineering/drafts/report.json   # every scenario against the specification itself; as of ess 0.72.0 one is unsupported, `inference.graph.AddSubgraphNode/outcome/added` (ESS-CF-TARGET: the model does not determine a complete typed view observation), so the run exits 1 and stays outside `task check`, whose testkit target runs the scenario and passes, until ESS story:an-unfillable-required-field-is-named-by-validate-and-the-view (ess 0.70.0)
aep plan artifact validate
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features   # --all-features builds the inference-testkit mocks behind its features run, stt, llm, tts and duplex
cargo test -p inference-testkit --features run --test conform   # the testkit conformance target alone: synthesizes the suite, runs it in process, has `ess verify conform report` write the verdict; prints target/tmp/conform-<run>/, where suite.json, results.json and report.json stay, red or green
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
  the reverse. The repository ships libraries: no binary, server or CLI. `inference-docs` is a
  repository tool for the documentation site, `publish = false`, and is never shipped.
- A local adapter's backend code is Apache-2.0 or MIT (or more permissive), and the weights it
  pins are CC BY 4.0 or more permissive. Each checkpoint's licence, source URL and hash go in the
  adapter's capability record at adoption; any other licence needs the owner's word for that
  checkpoint. Tests download no weights and start no model server: a test that needs one reads
  its path or URL from an environment variable and is ignored without it.
- `inference-llm` is a thin wrapper over the `llm` repository's model contract
  (`b10x-llm-core`, `llm_core::Model`) at a release tag, 0.8.3 now, and the workspace's
  `rust-version` is what that tag requires, 1.98 (owner decision, 2026-10-20, reversing the
  2026-10-18 rule for `llm` only). Its request, items, tool calls and usage are `llm`'s; the
  wrapper adds only graph concerns, and `llm`'s wire crates, being provider implementations, stay
  out of the contract crates. `b10x-llm-cost` at the same tag, its `sqlite` feature off, is the
  one other `llm` crate admitted, for pricing only and only behind `inference-facade`'s `cost`
  feature: its price document format, exact decimal money and unknown-preserving arithmetic are
  versioned there, and a second implementation here would fork the price document format (owner
  decision, 2026-10-20). Other beyond10x repositories (`llm-gateway`, `loom`) are not
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
  the tag points at, and so does the version in `Cargo.toml` and the `adapter_revision` of every capability record under `adapters/`. No binary is published: this repository ships libraries.
- The CI pins (`ess`, `aep`, go-task, the actions' commits, the `common.yml` commit) move only by
  a commit here that names the new version and its checksum.
- Each release moves the `ess` pin (CI's `ESS_VERSION` and `ESS_SHA256`, and the testkit's
  `ess-conformance` and `ess-primitives` tags) to the newest `ess` release in its release
  commit, with one `CHANGELOG.md` line; anything beyond the pin, regeneration and small fixes
  is reported before the release goes out.
- This repository is public. Committed text names no private repository, session or decision id,
  host detail or person.
- No comments in `Taskfile.yml`, `Cargo.toml` or any other config or manifest file.
