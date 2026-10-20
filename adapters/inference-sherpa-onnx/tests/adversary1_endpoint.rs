//! Adversary pass 1: the endpoint path of the recogniser. The story's scope says an endpoint
//! (`is_endpoint`, endpoint rules on) commits the utterance's span and resets the stream, so a
//! pause longer than the trailing-silence rules inside one recognition commits the first span
//! before input ends and opens a second for what follows. The conformance tests stream one clip
//! with no such pause and so never reach that path.
#![cfg(feature = "stt")]

mod support;

use inference_stt::inference_audio::{AudioFrame, SampleClockPosition};
use inference_stt::{Recogniser, TranscriptEvent};

use support::{
    FIRST_PART_WORDS, Feed, PCM16_16K_MONO, SECOND_PART_WORDS, SPLIT, frames, open, reference_clip,
    request,
};

fn shifted(frames: Vec<AudioFrame>, offset: u64) -> Vec<AudioFrame> {
    frames
        .into_iter()
        .map(|mut frame| {
            frame.position =
                SampleClockPosition::new(frame.position.get() + offset).expect("the clip is short");
            frame
        })
        .collect()
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn a_long_pause_commits_the_first_utterance_before_the_end_of_input() {
    let mut recogniser = open(1);
    let mut handle = recogniser.start(request()).expect("nothing else runs");
    let clip = reference_clip();
    let (first_part, second_part) = clip.split_at(SPLIT);
    let mut samples = first_part.to_vec();
    samples.extend(std::iter::repeat_n(0_i16, 16_000 * 3));
    let first = frames(&samples, PCM16_16K_MONO);
    let offset: u64 = first
        .iter()
        .map(|frame| u64::from(frame.sample_count))
        .sum();
    let second = shifted(frames(second_part, PCM16_16K_MONO), offset);

    let mut feed = Feed::default();
    for frame in first.into_iter().chain(second) {
        feed.offer(&mut handle, frame);
    }
    feed.end(&mut handle);
    feed.print("endpoint");

    let commits: Vec<(bool, String)> = feed
        .deliveries
        .iter()
        .filter_map(|delivery| match &delivery.envelope.payload {
            TranscriptEvent::Committed(committed) => {
                Some((delivery.before_end, committed.text.to_lowercase()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        commits.len(),
        2,
        "two utterances separated by 3 s of silence are two committed spans: {commits:?}"
    );
    assert!(
        commits[0].0,
        "the endpoint commits the first span before the end of input: {commits:?}"
    );
    for word in FIRST_PART_WORDS {
        assert!(commits[0].1.contains(word), "{commits:?}");
        assert!(!commits[1].1.contains(word), "{commits:?}");
    }
    for word in SECOND_PART_WORDS {
        assert!(commits[1].1.contains(word), "{commits:?}");
    }
}
