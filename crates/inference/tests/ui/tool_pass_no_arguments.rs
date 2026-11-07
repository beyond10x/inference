//! A tool without arguments is a function answering the pair `Agent::tool` takes.

use inference::tools::{Tool, ToolExecutor};

/// The local time, as HH:MM.
#[inference::tool]
async fn local_time() -> String {
    "10:30".to_owned()
}

#[allow(dead_code)]
fn offered() -> (Tool, Box<dyn ToolExecutor>) {
    local_time()
}

fn main() {}
