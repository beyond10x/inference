//! Tool proposals of the server as the tools domain takes them, behind the `tools` feature.
//!
//! A call the server streamed reaches the caller as an `llm` [`ToolCall`], taken from the
//! generation's handle once its turn ended. [`proposal`] makes of it the input of
//! `inference.tools.ProposeToolCall`: the tool its name was offered for, the generation that
//! made it and the span it rests on, so the resulting `inference.tools.ToolCall` carries that
//! generation's id.

use std::error::Error;
use std::fmt;

use inference_core::id::GenerationId;
use inference_llm::tools::tool_id;
use inference_text::SpanId;
use inference_tools::ProposeToolCall;
use llm_core::{ToolCall, ToolName};

/// A call naming a tool under a name no declared tool was offered under: one that
/// [`inference_llm::tools::tool_name`] does not give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeclaredTool {
    pub name: ToolName,
}

impl fmt::Display for UndeclaredTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no declared tool was offered as `{}`",
            self.name.as_str()
        )
    }
}

impl Error for UndeclaredTool {}

/// The `inference.tools.ProposeToolCall` of `call`, made by the generation `generation_id` and
/// resting on the span `basis_span_id`: the tool its name names and its arguments as they
/// arrived.
///
/// # Errors
///
/// [`UndeclaredTool`] carrying the name when it is not one a declared tool was offered under.
pub fn proposal(
    call: &ToolCall,
    generation_id: GenerationId,
    basis_span_id: SpanId,
) -> Result<ProposeToolCall, UndeclaredTool> {
    let tool_id = tool_id(&call.name).ok_or_else(|| UndeclaredTool {
        name: call.name.clone(),
    })?;
    Ok(ProposeToolCall {
        tool_id,
        generation_id,
        basis_span_id,
        arguments: call.arguments.clone(),
    })
}
