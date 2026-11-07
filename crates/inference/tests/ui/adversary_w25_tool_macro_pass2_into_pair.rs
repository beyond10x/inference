//! The call `docs/design/sdk.md` §1 writes, `.tool(local_time)`, against the parameter
//! story:sdk-agent plans for `Agent::tool`.

use inference::tools::{Tool, ToolExecutor};

/// The local time, as HH:MM.
#[inference::tool]
async fn local_time() -> String {
    "10:30".to_owned()
}

struct Builder;

impl Builder {
    fn tool(self, tool: impl Into<(Tool, Box<dyn ToolExecutor>)>) -> Self {
        let _ = tool.into();
        self
    }
}

fn main() {
    let _ = Builder.tool(local_time);
}
