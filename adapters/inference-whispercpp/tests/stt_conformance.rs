//! Adapter conformance of the recogniser (`docs/design/inference.md:1114-1120`), against the
//! pinned checkpoint and the fixture `tests/fixtures/question-16k.wav` ("What is the capital of
//! France?") offered in 20 ms frames: the claims of the sherpa-onnx recogniser's conformance,
//! inverted, so that neither incremental input nor output streaming is observed and neither is
//! claimed; the stream is ordered and finalized, timestamps and format transformations are
//! valid, a dropped recognition frees its place and leaks no output into the next, the maximum
//! concurrency is respected, and the model carries its provenance and licences.
//!
//! Every test needs the pinned checkpoint file `ggml-base.en.bin` at the path
//! `INFERENCE_WHISPERCPP_STT_MODEL`. The dropped and the interleaved recognitions need two
//! utterances that share no words, and a part of the synthetic fixture alone is not read reliably
//! (its "of France" comes out as "Friends"), so those two pair the whole fixture with the recorded
//! speech clip at the path `INFERENCE_WHISPERCPP_STT_CLIP`, the LibriSpeech `test_wavs/0.wav` of
//! the sherpa-onnx checkpoint, checked against its sha256. The tests are ignored by default and
//! fail naming the variable that is not set. Run them with
//! `INFERENCE_WHISPERCPP_STT_MODEL=<ggml-base.en.bin> INFERENCE_WHISPERCPP_STT_CLIP=<0.wav> cargo test -p inference-whispercpp -- --ignored`.
//! No test downloads weights or audio.

mod support;

use std::collections::HashSet;

use inference_stt::inference_audio::AudioFormat;
use inference_stt::inference_core::envelope::Timebase;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_core::model::{Capability, Modality};
use inference_stt::inference_text::{SpanState, TurnId};
use inference_stt::{
    CONTRACT_REVISION, ConcurrencyLimitReached, InputEnded, Recogniser, RecognitionHandle,
    RecognitionRequest, TranscriptEvent,
};
use inference_whispercpp::stt::{
    CHECKPOINT, CODE_LICENSE, FILE_NAME, SHA256, SOURCE_REVISION, SOURCE_URL, WEIGHT_LICENSE,
    checksum,
};

use support::{
    F32_16K_MONO, FIXTURE_WORDS, Feed, PCM16_8K_MONO, PCM16_16K_MONO, PCM16_16K_STEREO, fixture,
    frames, open, request, speech_clip, stream,
};

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
#[ignore = "needs the pinned ggml-base.en.bin at the path INFERENCE_WHISPERCPP_STT_MODEL"]
fn no_frame_is_taken_and_no_span_emitted_before_the_end_of_input_and_neither_is_claimed() {
    let mut recogniser = open(1);
    assert_eq!(
        recogniser.adapter().capabilities(),
        [],
        "the adapter claims neither incremental input nor output streaming"
    );

    let mut handle = recogniser.start(request()).expect("nothing else runs");
    let feed = stream(&mut handle, frames(&fixture(), PCM16_16K_MONO));
    feed.print("claims");

    assert!(!feed.taken_before_end.is_empty(), "frames were offered");
    for &(offered, taken) in &feed.taken_before_end {
        assert_eq!(
            taken, 0,
            "no incremental input: none of the {offered} frames offered is taken before the end \
             of input"
        );
    }
    assert!(
        feed.deliveries.iter().all(|delivery| !delivery.before_end),
        "no output streaming: nothing reaches the stream before the end of input"
    );
    assert_eq!(
        handle.output().frames_taken(),
        feed.taken_before_end.len(),
        "every frame offered is taken once input has ended"
    );
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert_reads(&feed, &FIXTURE_WORDS, "the fixture");
}

#[test]
#[ignore = "needs the pinned ggml-base.en.bin at the path INFERENCE_WHISPERCPP_STT_MODEL"]
fn the_stream_is_ordered_one_hypothesis_committed_at_once_and_finalized_at_the_end_of_input() {
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
    let feed = stream(&mut handle, frames(&fixture(), PCM16_16K_MONO));
    feed.print("order");

    let sequences: Vec<i64> = feed
        .deliveries
        .iter()
        .map(|delivery| delivery.envelope.sequence)
        .collect();
    assert_eq!(
        sequences,
        vec![first_sequence, first_sequence + 1, first_sequence + 2],
        "one hypothesis, its commit and the usage record, from the request's first sequence \
         without gaps"
    );
    assert_on_request(&feed, &request, "the recognition");

    let mut open_spans = HashSet::new();
    for delivery in &feed.deliveries {
        let payload = &delivery.envelope.payload;
        assert_eq!(
            delivery.envelope.kind,
            payload.name(),
            "the envelope's kind"
        );
        match payload {
            TranscriptEvent::Hypothesis(_) => {
                assert!(open_spans.insert(payload.span_id()), "a new span");
            }
            TranscriptEvent::Revision(_) => {
                panic!("a batch recogniser revises nothing: {payload:?}")
            }
            TranscriptEvent::Committed(_) => {
                assert!(
                    open_spans.remove(&payload.span_id()),
                    "a commit names the open hypothesis, once: {payload:?}"
                );
            }
            TranscriptEvent::Usage(_) => {
                assert!(
                    open_spans.is_empty(),
                    "the usage record follows the commit: {payload:?}"
                );
            }
        }
    }
    assert!(
        matches!(
            feed.deliveries
                .last()
                .map(|delivery| &delivery.envelope.payload),
            Some(TranscriptEvent::Usage(_))
        ),
        "the usage record comes last"
    );
    assert!(
        matches!(
            feed.deliveries[0].envelope.payload,
            TranscriptEvent::Hypothesis(_)
        ),
        "the hypothesis comes first"
    );
    assert!(open_spans.is_empty(), "the span is committed at once");
    assert_eq!(handle.output().spans().len(), 1);
    assert_eq!(handle.output().spans()[0].state(), SpanState::Committed);
    assert_eq!(handle.output().next_sequence(), Some(first_sequence + 3));
    assert!(handle.next().is_none(), "nothing follows the finalization");
    assert_eq!(
        handle.offer_frame(frames(&[0; 320], PCM16_16K_MONO).remove(0)),
        Err(InputEnded),
        "no frame is taken after the end of input"
    );
    assert!(handle.next().is_none(), "nothing follows a refused frame");
    assert_reads(&feed, &FIXTURE_WORDS, "the fixture");
}

/// Offers the fixture in `format` and checks timestamps and the committed text.
fn recognise_in(format: AudioFormat, what: &str) {
    let mut recogniser = open(1);
    let mut handle = recogniser.start(request()).expect("nothing else runs");
    let frames = frames(&fixture(), format);
    let offered: i64 = frames
        .iter()
        .map(|frame| i64::from(frame.sample_count))
        .sum();
    let feed = stream(&mut handle, frames);
    feed.print(what);

    assert!(
        !feed.deliveries.is_empty(),
        "{what}: the fixture is recognised"
    );
    for delivery in &feed.deliveries {
        let envelope = &delivery.envelope;
        assert_eq!(
            envelope.timebase,
            Timebase::AudioFrameTime,
            "{what}: timestamps are read on audio frame time"
        );
        assert_eq!(
            envelope.timestamp, offered,
            "{what}: every event is stamped at the end of the input, in samples of its format"
        );
    }
    assert_reads(&feed, &FIXTURE_WORDS, what);
}

#[test]
#[ignore = "needs the pinned ggml-base.en.bin at the path INFERENCE_WHISPERCPP_STT_MODEL"]
fn pcm16_at_16_khz_is_recognised_with_valid_timestamps() {
    recognise_in(PCM16_16K_MONO, "pcm_s16le 16 kHz mono");
}

#[test]
#[ignore = "needs the pinned ggml-base.en.bin at the path INFERENCE_WHISPERCPP_STT_MODEL"]
fn f32_at_16_khz_is_recognised_with_valid_timestamps() {
    recognise_in(F32_16K_MONO, "pcm_f32le 16 kHz mono");
}

#[test]
#[ignore = "needs the pinned ggml-base.en.bin at the path INFERENCE_WHISPERCPP_STT_MODEL"]
fn pcm16_at_8_khz_is_resampled_and_recognised_with_valid_timestamps() {
    recognise_in(PCM16_8K_MONO, "pcm_s16le 8 kHz mono");
}

#[test]
#[ignore = "needs the pinned ggml-base.en.bin at the path INFERENCE_WHISPERCPP_STT_MODEL"]
fn two_channels_are_averaged_and_recognised_with_valid_timestamps() {
    recognise_in(PCM16_16K_STEREO, "pcm_s16le 16 kHz two channels");
}

#[test]
#[ignore = "needs ggml-base.en.bin at INFERENCE_WHISPERCPP_STT_MODEL and the speech clip at INFERENCE_WHISPERCPP_STT_CLIP"]
fn a_recognition_dropped_before_the_end_of_input_frees_its_place_and_leaks_no_output() {
    let mut recogniser = open(1);
    let speech = speech_clip();

    let dropped_request = request();
    let mut dropped = recogniser
        .start(dropped_request.clone())
        .expect("nothing else runs");
    let mut dropped_feed = Feed::default();
    for frame in frames(&fixture(), PCM16_16K_MONO) {
        dropped_feed.offer(&mut dropped, frame);
    }
    assert!(
        dropped_feed.deliveries.is_empty(),
        "nothing was delivered before the end of input"
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
    let next_feed = stream(&mut next, frames(&speech.samples, PCM16_16K_MONO));
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
    assert_does_not_read(&next_feed, &FIXTURE_WORDS, "the next recognition");
    assert_reads(&next_feed, &speech.words, "the next recognition");
    assert_ne!(next_request.stream_id, dropped_request.stream_id);
}

#[test]
#[ignore = "needs ggml-base.en.bin at INFERENCE_WHISPERCPP_STT_MODEL and the speech clip at INFERENCE_WHISPERCPP_STT_CLIP"]
fn two_interleaved_recognitions_run_at_maximum_concurrency_2_and_a_third_is_refused() {
    let mut recogniser = open(2);
    assert_eq!(recogniser.adapter().max_concurrency(), 2);
    let speech = speech_clip();

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

    let mut first_frames = frames(&fixture(), PCM16_16K_MONO).into_iter();
    let mut second_frames = frames(&speech.samples, PCM16_16K_MONO).into_iter();
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
    assert_reads(&first_feed, &FIXTURE_WORDS, "the first recognition");
    assert_does_not_read(&first_feed, &speech.words, "the first recognition");
    assert_reads(&second_feed, &speech.words, "the second recognition");
    assert_does_not_read(&second_feed, &FIXTURE_WORDS, "the second recognition");

    drop(first);
    recogniser
        .start(request())
        .expect("a place is free once a recognition is dropped");
}

#[test]
#[ignore = "needs the pinned ggml-base.en.bin at the path INFERENCE_WHISPERCPP_STT_MODEL"]
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
    assert!(
        model.checksum().contains(SHA256) && model.checksum().contains(FILE_NAME),
        "the checksum names {FILE_NAME} and its sha256"
    );
    assert_eq!(model.input_modalities(), [Modality::Audio]);
    assert_eq!(model.output_modalities(), [Modality::Text]);
    assert_eq!(
        adapter.model_id(),
        model.model_id(),
        "the adapter references the model it declared"
    );
    assert_eq!(adapter.contract_revision(), CONTRACT_REVISION);
    assert!(
        !adapter
            .capabilities()
            .contains(&Capability::IncrementalInput)
    );
    assert!(
        !adapter
            .capabilities()
            .contains(&Capability::OutputStreaming)
    );
}
