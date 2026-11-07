//! The executor of `native_voice`'s tool executor node: each call routed by its tool to the
//! executor the recipe was given for that tool.

use std::fmt;
use std::future;
use std::sync::Arc;
use std::time::Instant;

use inference_tools::{
    CancellationSignal, ExecutionFuture, Tool, ToolCall, ToolExecutor, ToolFailure,
    ToolFailureKind, ToolId,
};

/// The tools a recipe was given, each with its executor. Clones share the executors.
#[derive(Clone, Default)]
pub(super) struct Dispatch {
    tools: Vec<(Tool, Arc<dyn ToolExecutor>)>,
}

impl fmt::Debug for Dispatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.tools.iter().map(|(tool, _)| tool.name()))
            .finish()
    }
}

impl Dispatch {
    /// Adds `tool` and its executor; a tool of the same name given earlier is replaced.
    pub(super) fn add(&mut self, tool: Tool, executor: Box<dyn ToolExecutor>) {
        self.tools.retain(|(given, _)| given.name() != tool.name());
        self.tools.push((tool, Arc::from(executor)));
    }

    /// The tools given, in the order they were given.
    pub(super) fn tools(&self) -> impl Iterator<Item = &Tool> {
        self.tools.iter().map(|(tool, _)| tool)
    }

    /// The identities of the tools given.
    pub(super) fn tool_ids(&self) -> Vec<ToolId> {
        self.tools().map(Tool::tool_id).collect()
    }
}

impl ToolExecutor for Dispatch {
    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        deadline: Instant,
        cancellation: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        match self
            .tools
            .iter()
            .find(|(tool, _)| tool.tool_id() == call.tool_id())
        {
            Some((_, executor)) => executor.execute(call, deadline, cancellation),
            None => Box::pin(future::ready(Err(ToolFailure {
                kind: ToolFailureKind::ExecutionError,
                detail: format!("no executor was given for the tool {}", call.tool_id()),
            }))),
        }
    }
}
