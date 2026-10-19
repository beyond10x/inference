//! Speech recognition alone: feeds a fixture of audio frames to the mock recogniser of
//! `inference-testkit` through the STT contract, and returns each span it commits. The package
//! depends on `inference-stt` and the testkit's `stt` feature only; `examples/stt_only.rs` prints
//! what [`run`] returns.

use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_core::model::ModelId;
use inference_stt::inference_text::TurnId;
use inference_stt::{Recogniser, RecognitionHandle, RecognitionRequest, TranscriptEvent};
use inference_testkit::stt::{MockConfiguration, MockRecogniser, frames, script};

/// Runs the example and returns one line per committed span, in commit order.
///
/// # Panics
///
/// When the mock refuses input before its end was marked, which it does not.
#[must_use]
pub fn run() -> Vec<String> {
    let mut recogniser = MockRecogniser::new(
        MockConfiguration::Incremental,
        ModelId::generate(),
        script(),
    );
    let mut recognition = recogniser.start(RecognitionRequest::new(
        RunId::generate(),
        TurnId::generate(),
        StreamId::generate(),
        NodeId::generate(),
        "text_out",
    ));
    let mut events = Vec::new();
    for frame in frames(6) {
        recognition.offer_frame(frame).expect("input has not ended");
        events.extend(recognition.by_ref());
    }
    recognition.end_input().expect("input ends once");
    events.extend(recognition.by_ref());
    events
        .into_iter()
        .filter_map(|envelope| match envelope.payload {
            TranscriptEvent::Committed(committed) => Some(format!("committed: {}", committed.text)),
            _ => None,
        })
        .collect()
}
