---
format: aep.planning-md/3
id: story:docs-build-without-warnings
kind: story
status: draft
title: The workspace's docs build without warnings and task check holds it
tags:
- from-finding
relations:
- informed_by: review-result:adversary-w25-sdk-payload-pass-1
revision: 1
---
## Context

`RUSTDOCFLAGS='-D warnings' cargo doc -p inference --no-deps` fails with
`unresolved link to 'transcribe'` at `crates/inference/src/lib.rs:3`: the crate doc links an item
that exists only with a feature. No gate builds the documentation, so broken intra-doc links in a
build without features go unnoticed. Found while checking the second SDK wave's payload docs.

## Planned scope

- `crates/inference/src/lib.rs`: the crate doc names feature-gated items as code, not as links.
- `Taskfile.yml`: `task check` builds the workspace's docs with `-D warnings` in a build without
  features.

## Acceptance

- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` exits 0, and `task check` runs it.
