//! The STT node of a voice pipeline, `inference-pipeline`'s
//! [`inference_pipeline::nodes::stt::SttNode`], here over the [`MockRecogniser`] by default.

use super::MockRecogniser;

pub use inference_pipeline::nodes::stt::{Heard, SttHandle, SttPayload, SttPorts};

/// The STT node over the recogniser `R`, the [`MockRecogniser`] by default.
pub type SttNode<R = MockRecogniser> = inference_pipeline::nodes::stt::SttNode<R>;

#[cfg(test)]
mod tests {
    use inference_core::envelope::ControlKind;
    use inference_core::id::RunId;
    use inference_core::model::{Adapter, ModelId};
    use inference_pipeline::{NodeImplementation, PortId};
    use inference_stt::{ConcurrencyLimitReached, Recogniser, RecognitionRequest};

    use super::*;
    use crate::graph::RunLog;
    use crate::stream::StreamEvent;
    use crate::stt::MockConfiguration;

    /// A payload of frames, stream events and control events.
    #[derive(Debug, Clone)]
    enum Payload {
        Stream(StreamEvent),
        Control(ControlKind),
    }

    impl From<ControlKind> for Payload {
        fn from(kind: ControlKind) -> Self {
            Self::Control(kind)
        }
    }

    impl SttPayload for Payload {
        fn stream(event: StreamEvent) -> Self {
            Self::Stream(event)
        }

        fn heard(self) -> Heard {
            match self {
                Self::Stream(event) => Heard::Stream(event),
                Self::Control(kind) => Heard::Control(kind),
            }
        }
    }

    /// A recogniser that is not the mock: the mock behind a type of its own, as an adapter's
    /// recogniser is.
    #[derive(Debug, Clone)]
    struct Other(MockRecogniser);

    impl Recogniser for Other {
        type Handle = <MockRecogniser as Recogniser>::Handle;

        fn adapter(&self) -> &Adapter {
            self.0.adapter()
        }

        fn start(
            &mut self,
            request: RecognitionRequest,
        ) -> Result<Self::Handle, ConcurrencyLimitReached> {
            self.0.start(request)
        }
    }

    fn implementation<P, N: NodeImplementation<P>>(node: N) -> N {
        node
    }

    #[test]
    fn the_node_recognises_on_any_recogniser_and_on_the_mock_by_default() {
        let ports = SttPorts {
            audio: PortId::generate(),
            turns: PortId::generate(),
            committed_text: PortId::generate(),
            turns_out: PortId::generate(),
            transcript: None,
        };
        let mock = MockRecogniser::new(
            MockConfiguration::Incremental,
            ModelId::generate(),
            Vec::new(),
        );
        let other: SttNode<Other> = implementation::<Payload, _>(SttNode::new(
            RunId::generate(),
            Other(mock.clone()),
            ports,
            RunLog::new(),
        ));
        let default: SttNode = implementation::<Payload, _>(SttNode::new(
            RunId::generate(),
            mock,
            ports,
            RunLog::new(),
        ));
        assert!(other.handle().spans().is_empty());
        assert!(default.handle().spans().is_empty());
        assert_eq!(other.recogniser().adapter(), default.recogniser().adapter());
    }
}
