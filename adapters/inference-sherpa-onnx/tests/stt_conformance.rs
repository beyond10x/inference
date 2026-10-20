//! Adapter conformance of the recogniser (`docs/design/inference.md:1114-1120`), against the
//! pinned checkpoint and its LibriSpeech reference clip streamed in 20 ms frames: the claimed
//! capabilities are observed, the stream is ordered and finalized, timestamps and format
//! transformations are valid, a dropped recognition frees its place and leaks no output into the
//! next, the maximum concurrency is respected, and the model carries its provenance and licences.
//!
//! Every test needs the unpacked pinned checkpoint in the directory
//! `INFERENCE_SHERPA_ONNX_STT_DIR`, the four files `stt::FILES` names and `stt::REFERENCE_CLIP`.
//! They are ignored by default and fail naming that variable when run without it. Run them with
//! `INFERENCE_SHERPA_ONNX_STT_DIR=<dir> cargo test -p inference-sherpa-onnx --all-features -- --ignored`.
//! No test downloads weights. The capability record of the adapter names these tests as the
//! evidence of its claims.
#![cfg(feature = "stt")]

mod support;

use std::collections::HashSet;

use inference_sherpa_onnx::stt::{
    CHECKPOINT, CODE_LICENSE, FILES, SOURCE_REVISION, SOURCE_URL, WEIGHT_LICENSE, checksum,
};
use inference_stt::inference_audio::AudioFormat;
use inference_stt::inference_core::envelope::Timebase;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_core::model::{Capability, Modality};
use inference_stt::inference_text::{SpanState, TurnId};
use inference_stt::{
    CONTRACT_REVISION, ConcurrencyLimitReached, InputEnded, Recogniser, RecognitionHandle,
    RecognitionRequest, TranscriptEvent,
};

use support::{
    F32_16K_MONO, FIRST_PART_WORDS, Feed, PCM16_8K_MONO, PCM16_16K_MONO, PCM16_16K_STEREO,
    SECOND_PART_WORDS, SPLIT, frames, open, reference_clip, request, stream,
};

/// Words of the reference clip as a whole.
const CLIP_WORDS: [&str; 3] = ["nightfall", "lamps", "brothels"];

fn assert_reads(feed: &Feed, words: &[&str], what: &str) {
    let committed = feed.committed();
    for word in words {
        assert!(
            committed.contains(word),
            "the committed text of {what} contains {word:?}: {committed:?}"
        );
    }
}

fn assert_does_not_read(feed: &Feed, words: &[&str], what: &str) {
    let every_text = feed.every_text();
    for word in words {
        assert!(
            !every_text.contains(word),
            "no event of {what} carries {word:?}: {every_text:?}"
        );
    }
}

/// Every delivered envelope is on `request`'s run, stream, producer and port, and its spans in
/// `request`'s turn.
fn assert_on_request(feed: &Feed, request: &RecognitionRequest, what: &str) {
    for delivery in &feed.deliveries {
        let envelope = &delivery.envelope;
        assert_eq!(envelope.run_id, request.run_id, "{what}: the run");
        assert_eq!(envelope.stream_id, request.stream_id, "{what}: the stream");
        assert_eq!(envelope.producer, request.node_id, "{what}: the producer");
        assert_eq!(envelope.port, request.port, "{what}: the port");
        if let TranscriptEvent::Hypothesis(hypothesis) = &envelope.payload {
            assert_eq!(hypothesis.turn_id, request.turn_id, "{what}: the turn");
        }
    }
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn incremental_input_and_output_streaming_are_observed_and_are_exactly_the_claims() {
    let mut recogniser = open(1);
    assert_eq!(
        recogniser.adapter().capabilities(),
        [Capability::IncrementalInput, Capability::OutputStreaming],
        "the adapter claims exactly incremental input and output streaming"
    );

    let clip = reference_clip();
    let total = i64::try_from(clip.len()).expect("the clip is short");
    let mut handle = recogniser.start(request()).expect("nothing else runs");
    let feed = stream(&mut handle, frames(&clip, PCM16_16K_MONO));
    feed.print("claims");

    for &(offered, taken) in &feed.taken_before_end {
        assert_eq!(
            taken, offered,
            "incremental input: every frame offered is taken before the end of input"
        );
    }
    let first_hypothesis = feed
        .deliveries
        .iter()
        .find(|delivery| matches!(delivery.envelope.payload, TranscriptEvent::Hypothesis(_)))
        .expect("the recognition emits a hypothesis");
    assert!(
        first_hypothesis.before_end,
        "output streaming: a hypothesis reaches the stream before the end of input"
    );
    assert!(
        first_hypothesis.offered_through < total / 2,
        "output streaming: the first hypothesis reaches the stream within the first half of the \
         clip, at {} of {total} samples, not once the utterance is complete",
        first_hypothesis.offered_through
    );
    assert_reads(&feed, &CLIP_WORDS, "the clip");
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn the_stream_is_ordered_revises_only_open_hypotheses_and_is_finalized_at_the_end_of_input() {
    let mut recogniser = open(1);
    let first_sequence = 41;
    let request = RecognitionRequest::continuing(
        RunId::generate(),
        TurnId::generate(),
        StreamId::generate(),
        first_sequence,
        NodeId::generate(),
        "text_out",
    )
    .expect("41 is a sequence");
    let mut handle = recogniser
        .start(request.clone())
        .expect("nothing else runs");
    let feed = stream(&mut handle, frames(&reference_clip(), PCM16_16K_MONO));
    feed.print("order");

    let sequences: Vec<i64> = feed
        .deliveries
        .iter()
        .map(|delivery| delivery.envelope.sequence)
        .collect();
    let expected: Vec<i64> = (first_sequence
        ..first_sequence + i64::try_from(sequences.len()).expect("few events"))
        .collect();
    assert_eq!(
        sequences, expected,
        "sequences start at the request's first sequence and run without gaps"
    );
    assert_on_request(&feed, &request, "the recognition");

    let mut open_spans = HashSet::new();
    let mut committed_spans = HashSet::new();
    let mut revisions = 0;
    for delivery in &feed.deliveries {
        let payload = &delivery.envelope.payload;
        assert_eq!(
            delivery.envelope.kind,
            payload.name(),
            "the envelope's kind"
        );
        let span_id = payload.span_id();
        match payload {
            TranscriptEvent::Hypothesis(_) => {
                assert!(
                    !open_spans.contains(&span_id) && !committed_spans.contains(&span_id),
                    "a hypothesis opens a new span: {payload:?}"
                );
                open_spans.insert(span_id);
            }
            TranscriptEvent::Revision(_) => {
                assert!(
                    !committed_spans.contains(&span_id),
                    "a committed span is never revised again: {payload:?}"
                );
                assert!(
                    open_spans.contains(&span_id),
                    "a revision names an open hypothesis: {payload:?}"
                );
                revisions += 1;
            }
            TranscriptEvent::Committed(_) => {
                assert!(
                    open_spans.remove(&span_id),
                    "a commit names an open hypothesis, once: {payload:?}"
                );
                committed_spans.insert(span_id);
            }
        }
    }
    eprintln!(
        "order: {} events, {revisions} revisions, {} committed spans",
        feed.deliveries.len(),
        committed_spans.len()
    );
    assert!(
        revisions > 0,
        "the clip's hypotheses are revised as it goes"
    );
    assert!(
        open_spans.is_empty(),
        "every span is committed once input has ended"
    );
    assert!(
        handle
            .output()
            .spans()
            .iter()
            .all(|span| span.state() == SpanState::Committed),
        "the recognition's own spans are all committed"
    );
    assert_eq!(
        handle.output().spans().len(),
        committed_spans.len(),
        "every span the recognition holds was delivered"
    );
    assert!(handle.next().is_none(), "nothing follows the finalization");
    assert_eq!(
        handle.offer_frame(frames(&[0; 320], PCM16_16K_MONO).remove(0)),
        Err(InputEnded),
        "no frame is taken after the end of input"
    );
    assert!(handle.next().is_none(), "nothing follows a refused frame");
    assert_reads(&feed, &CLIP_WORDS, "the clip");
}

/// Streams the reference clip in `format` and checks timestamps and the committed text.
fn recognise_in(format: AudioFormat, what: &str) {
    let mut recogniser = open(1);
    let mut handle = recogniser.start(request()).expect("nothing else runs");
    let clip = reference_clip();
    let frames = frames(&clip, format);
    let offered: i64 = frames
        .iter()
        .map(|frame| i64::from(frame.sample_count))
        .sum();
    let feed = stream(&mut handle, frames);
    feed.print(what);

    let mut previous = 0;
    for delivery in &feed.deliveries {
        let envelope = &delivery.envelope;
        assert_eq!(
            envelope.timebase,
            Timebase::AudioFrameTime,
            "{what}: timestamps are read on audio frame time"
        );
        assert!(
            envelope.timestamp >= previous,
            "{what}: timestamps never decrease: {} after {previous}",
            envelope.timestamp
        );
        assert!(
            envelope.timestamp <= delivery.offered_through,
            "{what}: a timestamp, {}, never passes the samples offered, {}",
            envelope.timestamp,
            delivery.offered_through
        );
        previous = envelope.timestamp;
    }
    assert_eq!(
        previous, offered,
        "{what}: the last event is stamped at the end of the input, in samples of its format"
    );
    assert_reads(&feed, &CLIP_WORDS, what);
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn pcm16_at_16_khz_is_recognised_with_valid_timestamps() {
    recognise_in(PCM16_16K_MONO, "pcm_s16le 16 kHz mono");
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn f32_at_16_khz_is_recognised_with_valid_timestamps() {
    recognise_in(F32_16K_MONO, "pcm_f32le 16 kHz mono");
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn pcm16_at_8_khz_is_resampled_and_recognised_with_valid_timestamps() {
    recognise_in(PCM16_8K_MONO, "pcm_s16le 8 kHz mono");
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn two_channels_are_averaged_and_recognised_with_valid_timestamps() {
    recognise_in(PCM16_16K_STEREO, "pcm_s16le 16 kHz two channels");
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn a_recognition_dropped_mid_utterance_frees_its_place_and_leaks_no_output() {
    let mut recogniser = open(1);
    let clip = reference_clip();
    let (first_part, second_part) = clip.split_at(SPLIT);

    let dropped_request = request();
    let mut dropped = recogniser
        .start(dropped_request.clone())
        .expect("nothing else runs");
    let mut dropped_feed = Feed::default();
    for frame in frames(first_part, PCM16_16K_MONO) {
        dropped_feed.offer(&mut dropped, frame);
    }
    dropped_feed.print("dropped");
    assert!(
        dropped_feed.every_text().contains(FIRST_PART_WORDS[0]),
        "the dropped recognition had recognised words of its own"
    );
    assert!(
        dropped
            .output()
            .spans()
            .iter()
            .any(|span| span.state() == SpanState::Hypothesis),
        "the recognition is dropped mid-utterance, with a hypothesis open"
    );
    assert_eq!(
        recogniser.start(request()).map(|_| ()),
        Err(ConcurrencyLimitReached { max_concurrency: 1 }),
        "while it runs, the recogniser at maximum concurrency 1 starts no other"
    );
    drop(dropped);

    let next_request = request();
    let mut next = recogniser
        .start(next_request.clone())
        .expect("the dropped recognition freed its place");
    let next_feed = stream(&mut next, frames(second_part, PCM16_16K_MONO));
    next_feed.print("next");

    assert_on_request(&next_feed, &next_request, "the next recognition");
    assert_eq!(
        next_feed
            .deliveries
            .first()
            .map(|delivery| delivery.envelope.sequence),
        Some(1),
        "the next recognition's stream starts afresh"
    );
    assert_does_not_read(&next_feed, &FIRST_PART_WORDS, "the next recognition");
    assert_reads(&next_feed, &SECOND_PART_WORDS, "the next recognition");
    assert_ne!(next_request.stream_id, dropped_request.stream_id);
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn two_interleaved_recognitions_run_at_maximum_concurrency_2_and_a_third_is_refused() {
    let mut recogniser = open(2);
    assert_eq!(recogniser.adapter().max_concurrency(), 2);
    let clip = reference_clip();
    let (first_part, second_part) = clip.split_at(SPLIT);

    let first_request = request();
    let second_request = request();
    let mut first = recogniser
        .start(first_request.clone())
        .expect("nothing else runs");
    let mut second = recogniser
        .start(second_request.clone())
        .expect("one of two places is taken");
    assert_eq!(
        recogniser.start(request()).map(|_| ()),
        Err(ConcurrencyLimitReached { max_concurrency: 2 }),
        "a third start is refused"
    );
    assert_eq!(
        recogniser.clone().start(request()).map(|_| ()),
        Err(ConcurrencyLimitReached { max_concurrency: 2 }),
        "a clone is the same adapter and refuses a third start too"
    );

    let mut first_frames = frames(first_part, PCM16_16K_MONO).into_iter();
    let mut second_frames = frames(second_part, PCM16_16K_MONO).into_iter();
    let mut first_feed = Feed::default();
    let mut second_feed = Feed::default();
    loop {
        let first_frame = first_frames.next();
        let second_frame = second_frames.next();
        if first_frame.is_none() && second_frame.is_none() {
            break;
        }
        if let Some(frame) = first_frame {
            first_feed.offer(&mut first, frame);
        }
        if let Some(frame) = second_frame {
            second_feed.offer(&mut second, frame);
        }
    }
    first_feed.end(&mut first);
    second_feed.end(&mut second);
    first_feed.print("first");
    second_feed.print("second");

    assert_on_request(&first_feed, &first_request, "the first recognition");
    assert_on_request(&second_feed, &second_request, "the second recognition");
    assert_reads(&first_feed, &FIRST_PART_WORDS, "the first recognition");
    assert_does_not_read(&first_feed, &SECOND_PART_WORDS, "the first recognition");
    assert_reads(&second_feed, &SECOND_PART_WORDS, "the second recognition");
    assert_does_not_read(&second_feed, &FIRST_PART_WORDS, "the second recognition");

    drop(first);
    recogniser
        .start(request())
        .expect("a place is free once a recognition is dropped");
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn the_declared_model_carries_the_pinned_provenance_and_licences() {
    let recogniser = open(1);
    let model = recogniser.model();
    let adapter = recogniser.adapter();

    for (field, value) in [
        ("family", model.family()),
        ("checkpoint", model.checkpoint()),
        ("code_license", model.code_license()),
        ("weight_license", model.weight_license()),
        ("checksum", model.checksum()),
        ("provenance", model.provenance()),
        ("source_revision", model.source_revision()),
        ("backend", adapter.backend()),
        ("contract_revision", adapter.contract_revision()),
    ] {
        assert!(!value.trim().is_empty(), "{field} is not empty");
    }
    assert_eq!(model.checkpoint(), CHECKPOINT);
    assert!(
        model.provenance().contains(SOURCE_URL),
        "the provenance names the source URL: {}",
        model.provenance()
    );
    assert_eq!(model.source_revision(), SOURCE_REVISION);
    assert_eq!(model.code_license(), CODE_LICENSE);
    assert_eq!(model.weight_license(), WEIGHT_LICENSE);
    assert_eq!(model.checksum(), checksum());
    for file in FILES {
        assert!(
            model.checksum().contains(file.sha256) && model.checksum().contains(file.name),
            "the checksum names {} and its sha256",
            file.name
        );
    }
    assert_eq!(model.input_modalities(), [Modality::Audio]);
    assert_eq!(model.output_modalities(), [Modality::Text]);
    assert_eq!(
        adapter.model_id(),
        model.model_id(),
        "the adapter references the model it declared"
    );
    assert_eq!(adapter.contract_revision(), CONTRACT_REVISION);
}
