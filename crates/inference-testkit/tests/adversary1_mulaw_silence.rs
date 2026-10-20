//! Adversary pass 1: the mock synthesiser documents its frames as silence; in `pcm_mulaw`, silence
//! is the byte 0xFF (or 0x7F), and the byte 0x00 is the loudest negative sample G.711 encodes.

#![cfg(feature = "tts")]

use std::num::{NonZeroU16, NonZeroU32};

use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId};
use inference_core::model::ModelId;
use inference_testkit::tts::{MockConfiguration, MockSynthesiser};
use inference_text::OutputSegment;
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

#[test]
fn the_mock_synthesisers_silence_in_mulaw_is_mulaw_silence() {
    let mulaw = AudioFormat {
        encoding: AudioEncoding::PcmMulaw,
        sample_rate: NonZeroU32::new(8_000).expect("8000 is not zero"),
        channels: NonZeroU16::MIN,
    };
    for configuration in MockConfiguration::ALL {
        let mut synthesiser = MockSynthesiser::new(configuration, ModelId::generate(), mulaw);
        let mut handle = synthesiser.start().expect("a fresh synthesiser");
        let generation = Generation::start(StartGeneration {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
        })
        .0;
        let (tentative, _) =
            OutputSegment::emit(&generation, 1, "Hello there.".to_owned()).expect("Active");
        let segment = tentative.commit().expect("commits").0;
        handle.offer_segment(segment).expect("queued");
        handle.end_utterance().expect("ends once");
        let mut frames = 0;
        for event in handle.by_ref() {
            if let SynthesisEvent::Audio(audio) = event {
                frames += 1;
                let loud: Vec<u8> = audio
                    .frame()
                    .data
                    .iter()
                    .copied()
                    .filter(|byte| *byte != 0xFF && *byte != 0x7F)
                    .take(4)
                    .collect();
                assert!(
                    loud.is_empty(),
                    "{configuration:?}: a frame of silence carries mu-law bytes {loud:02X?}, \
                     which decode to about -32124"
                );
            }
        }
        assert!(frames > 0, "{configuration:?}: frames were emitted");
    }
}
