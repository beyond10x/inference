//! The calls `docs/design/sdk.md` §1 writes, `.tool(local_time)`, and `.tool(local_time())`,
//! against a builder whose `tool` takes `impl IntoTool`, as story:sdk-agent plans for
//! `Agent::tool`.

use inference::tools::IntoTool;

/// The local time, as HH:MM.
#[inference::tool]
async fn local_time() -> String {
    "10:30".to_owned()
}

struct Builder;

impl Builder {
    fn tool(self, tool: impl IntoTool) -> Self {
        let _ = tool.into_tool();
        self
    }
}

fn main() {
    let _ = Builder.tool(local_time);
    let _ = Builder.tool(local_time());
}
