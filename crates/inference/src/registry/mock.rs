//! The `mock` registration: the testkit mocks for every role (feature `testing`).
//!
//! Bound by name, each role runs the testkit default here: the recogniser replays the testkit's
//! script, the generator answers [`MOCK_REPLY`] piece by piece, the synthesiser speaks PCM16 at
//! 24 kHz and the native speech model has no scripted turn. A mock bound as a value
//! (`Binding::llm(MockGenerator::new(..))`) runs its own script instead.

use inference_audio::{AudioEncoding, AudioFormat};
use inference_core::model::{Locality, ModelId};
use inference_testkit::duplex::mock::MockSpeechModel;
use inference_testkit::llm::{self, MockGenerator};
use inference_testkit::stt::{self, MockRecogniser};
use inference_testkit::tts::{self, MockSynthesiser};

use super::AdapterDescriptor;
use crate::stack::{Binding, Role, default_session_config};

/// What the generator of the `mock` registration answers, piece by piece.
const MOCK_REPLY: [&str; 3] = ["Hello", " from", " the mock"];

pub(super) fn descriptor() -> AdapterDescriptor {
    let pcm16 = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: std::num::NonZeroU32::new(24_000).unwrap_or(std::num::NonZeroU32::MIN),
        channels: std::num::NonZeroU16::MIN,
    };
    AdapterDescriptor::new("mock", Locality::SameProcess)
        .feature("testing")
        .role(Role::Stt, |_| {
            Ok(Binding::stt(MockRecogniser::new(
                stt::MockConfiguration::Incremental,
                ModelId::generate(),
                stt::script(),
            ))
            .into())
        })
        .role(Role::Llm, |_| {
            Ok(Binding::llm(MockGenerator::new(
                llm::MockConfiguration::StreamingOnly,
                ModelId::generate(),
                MOCK_REPLY,
            ))
            .into())
        })
        .role(Role::Tts, move |_| {
            Ok(Binding::tts(MockSynthesiser::new(
                tts::MockConfiguration::IncrementalStreaming,
                ModelId::generate(),
                pcm16,
            ))
            .into())
        })
        .role(Role::Speech, |_| {
            Ok(Binding::speech(MockSpeechModel::native(Vec::new())).into())
        })
        .session_config(default_session_config())
}
