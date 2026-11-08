//! The role is in the type: `AgentBuilder::llm` takes a `Bound<role::Llm>`, so an untyped
//! `Binding::named`, even of the `llm` role, is refused at compile time.

use std::collections::BTreeMap;

use inference::{Agent, Binding, Role};

fn main() {
    let keys = BTreeMap::new();
    let _ = Agent::voice().llm(Binding::named(Role::Llm, "mock", keys));
}
