//! Adversary pass on story:elevenlabs-tts-provider-cancel: a slow generation is not a stalled
//! one. A server that pauses 300 ms between the audio chunks of a segment, longer than any quiet
//! the binding might take for the end of a context, must still deliver every chunk, and the
//! segment must end Synthesized. A binding that closed a context once its audio had paused, and a
//! server that stopped at the close, would cut the segment short.

mod support;

use std::time::Duration;

use inference_elevenlabs::OutputFormat;
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixtureServer, Script, committed, within};

const FORMAT: OutputFormat = OutputFormat::Ulaw8000;

/// The pause between two audio chunks.
const PAUSE: Duration = Duration::from_millis(300);

/// A server that pauses 300 ms between its four audio chunks: every chunk reaches the caller and
/// the segment completes.
#[test]
fn a_pause_mid_generation_longer_than_the_default_quiet_does_not_cut_the_segment() {
    within(Duration::from_secs(20), || {
        let server = FixtureServer::paced(FORMAT, vec![Script::Serve], Duration::ZERO, PAUSE);
        let mut synthesiser = server.synthesiser(FORMAT);
        let mut handle = synthesiser.start().unwrap();
        let segment = committed(&["A long sentence the endpoint takes its time over."]).remove(0);
        let id = segment.segment_id();
        handle.offer_segment(segment).unwrap();
        handle.end_utterance().unwrap();
        let events: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(handle.failure().is_none(), "{:?}", handle.failure());
        assert_eq!(
            handle.output().segment(id).map(|segment| segment.state()),
            Some(SegmentState::Synthesized)
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SynthesisEvent::Audio(_)))
                .count(),
            4,
            "every audio chunk of the segment reaches the caller"
        );
        assert_eq!(server.violations(), Vec::<String>::new());
    });
}
