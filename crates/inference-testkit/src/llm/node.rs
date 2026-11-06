//! The model node of a text-only pipeline, `inference-pipeline`'s
//! [`inference_pipeline::nodes::llm::ModelNode`], here on the text pipeline's [`PipelinePayload`]
//! by default, which this module makes a [`ModelPayload`].

use inference_core::usage::UsageRecorded;

use crate::graph::PipelinePayload;
use crate::stream::StreamEvent;

pub use inference_pipeline::nodes::llm::{Carried, ModelHandle, ModelPayload};

/// The model node on edges carrying the payload `P`, the text pipeline's [`PipelinePayload`] by
/// default.
pub type ModelNode<P = PipelinePayload> = inference_pipeline::nodes::llm::ModelNode<P>;

impl ModelPayload for PipelinePayload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn usage(recorded: UsageRecorded) -> Option<Self> {
        Some(Self::Usage(recorded))
    }

    fn carried(self) -> Carried {
        match self {
            Self::Stream(event) => Carried::Stream(event),
            Self::Usage(recorded) => Carried::Usage(recorded),
            Self::Control(kind) => Carried::Control(kind),
        }
    }
}
