//! The TTS node of a voice pipeline, `inference-pipeline`'s
//! [`inference_pipeline::nodes::tts::TtsNode`], here over the [`MockSynthesiser`] by default and,
//! playing, through the [`MockSink`].

use super::{MockSink, MockSynthesiser};

pub use inference_pipeline::nodes::tts::{Spoken, TtsHandle, TtsPayload};

/// The TTS node over the synthesiser `S`, the [`MockSynthesiser`] by default, playing, when it
/// plays, through the [`MockSink`].
pub type TtsNode<S = MockSynthesiser> = inference_pipeline::nodes::tts::TtsNode<S, MockSink>;

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};

    use inference_core::envelope::ControlKind;
    use inference_core::id::RunId;
    use inference_core::model::{Adapter, ModelId};
    use inference_pipeline::NodeImplementation;
    use inference_tts::inference_audio::{AudioEncoding, AudioFormat, AudioFrame};
    use inference_tts::{ConcurrencyLimitReached, Synthesiser};

    use super::*;
    use crate::graph::RunLog;
    use crate::stream::StreamEvent;
    use crate::tts::MockConfiguration;

    /// A payload of stream events, frames and control events.
    #[derive(Debug, Clone)]
    enum Payload {
        Stream(StreamEvent),
        Audio,
        Control(ControlKind),
    }

    impl From<ControlKind> for Payload {
        fn from(kind: ControlKind) -> Self {
            Self::Control(kind)
        }
    }

    impl TtsPayload for Payload {
        fn stream(event: StreamEvent) -> Self {
            Self::Stream(event)
        }

        fn audio(_frame: AudioFrame) -> Self {
            Self::Audio
        }

        fn spoken(self) -> Spoken {
            match self {
                Self::Stream(event) => Spoken::Stream(event),
                Self::Control(kind) => Spoken::Control(kind),
                Self::Audio => Spoken::Other,
            }
        }
    }

    /// A synthesiser that is not the mock: the mock behind a type of its own, as an adapter's
    /// synthesiser is.
    #[derive(Debug, Clone)]
    struct Other(MockSynthesiser);

    impl Synthesiser for Other {
        type Handle = <MockSynthesiser as Synthesiser>::Handle;

        fn adapter(&self) -> &Adapter {
            self.0.adapter()
        }

        fn start(&mut self) -> Result<Self::Handle, ConcurrencyLimitReached> {
            self.0.start()
        }
    }

    fn implementation<P, N: NodeImplementation<P>>(node: N) -> N {
        node
    }

    #[test]
    fn the_node_synthesises_on_any_synthesiser_and_on_the_mock_by_default() {
        let mock = MockSynthesiser::new(
            MockConfiguration::IncrementalStreaming,
            ModelId::generate(),
            AudioFormat {
                encoding: AudioEncoding::PcmS16le,
                sample_rate: NonZeroU32::new(16_000).expect("16000"),
                channels: NonZeroU16::new(1).expect("1"),
            },
        );
        let other: TtsNode<Other> = implementation::<Payload, _>(
            TtsNode::new(RunId::generate(), Other(mock.clone()), RunLog::new()).playing(),
        );
        let default: TtsNode =
            implementation::<Payload, _>(TtsNode::new(RunId::generate(), mock, RunLog::new()));
        assert!(other.handle().played().is_empty());
        assert!(default.handle().played().is_empty());
        assert_eq!(
            other.synthesiser().adapter(),
            default.synthesiser().adapter()
        );
    }
}
