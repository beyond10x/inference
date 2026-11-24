//! The conformance target's own tests: what each module of `tests/target/` answers, command by
//! command, without a suite.
//!
//! The tests are the files of `tests/target_self/`. This is the one binary that runs them; the
//! binaries that run a suite or a mutant against the target include `tests/target/` and no test
//! of it. The target stands here as `subject`, so the tests keep the names they had inside the
//! files they test: `target::graph::tests::…`.

#[path = "target/mod.rs"]
mod subject;
#[path = "target_self/mod.rs"]
mod target;
