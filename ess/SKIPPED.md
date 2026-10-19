# Scenarios the testkit target does not answer yet

Every scenario of the synthesized suite that `crates/inference-testkit/tests/conform.rs` reports
`unsupported`, one line each, under the domain it belongs to, naming the story or epic that will
answer it. The test asserts that the `unsupported` ids of its report equal the ids listed here, so
a story that makes a scenario pass removes its line, and a change to the scenario set changes
this file with it.

## inference.graph

## inference.stream

## inference.tools


## inference.duplex

## inference.session
