//! Adversary pass 1 on `story:local-tts-adapter-under-licence-policy`: the synthesiser the local
//! cascade binds at `tts`, `inference-sherpa-onnx-tts` over its pinned checkpoint, at the edges
//! its own tests leave out: what a segment of one sentence and of no sentence end gives, texts
//! the voice was not trained on, a text of 100 000 characters, a reply offered delta by delta as
//! the cascade offers it, two syntheses at once, the async form beside a generation, and a drop
//! in the middle of one.
//!
//! The file is in this package because it is the adapter's one consumer and builds each file of
//! `tests/` as a test target of its own; the adapter's own test target lists its modules in a
//! file this pass does not edit.
//!
//! Every case but the first speaks, so it is ignored: it reads the directory of the unpacked
//! pinned checkpoint from `INFERENCE_SHERPA_ONNX_TTS_DIR` and needs a process started on the
//! sherpa-onnx native library built with text-to-speech, as the README of
//! `inference-sherpa-onnx-tts` shows. Run without the variable, each fails naming it.

#![cfg(all(target_os = "linux", feature = "local"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId};
use inference_sherpa_onnx_tts::{
    OpenRefused, SherpaOnnxSynthesis, SherpaOnnxSynthesiser, SpeakerOutOfRange,
};
use inference_text::{OutputSegment, SegmentId};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

/// The variable naming the directory of the unpacked pinned checkpoint.
const TTS_DIR: &str = "INFERENCE_SHERPA_ONNX_TTS_DIR";

/// One sentence of 24 words with no sentence end before its last character.
const ONE_SENTENCE: &str = "The weather in the valley stayed mild and bright through the whole of \
     the afternoon while the river ran quietly under the old stone bridge.";

/// Sixteen words and no punctuation.
const SIXTEEN_WORDS: &str = "one two three four five six seven eight nine ten eleven twelve \
     thirteen fourteen fifteen sixteen";

/// Opens the pinned checkpoint of [`TTS_DIR`] at most `max_concurrency` syntheses at once.
fn open(max_concurrency: i64) -> SherpaOnnxSynthesiser {
    let dir = std::env::var_os(TTS_DIR)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!("set {TTS_DIR} to the directory of the unpacked pinned checkpoint")
        });
    SherpaOnnxSynthesiser::open_with(dir, max_concurrency)
        .unwrap_or_else(|refused| panic!("{TTS_DIR} holds the pinned checkpoint: {refused}"))
}

fn generation() -> Generation {
    Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0
}

/// Offers `text` to `synthesis` as one Committed segment and returns its id.
fn offer(
    synthesis: &mut SherpaOnnxSynthesis,
    generation: &Generation,
    sequence: i64,
    text: &str,
) -> SegmentId {
    let (tentative, _) = OutputSegment::emit(generation, sequence, text.to_owned())
        .expect("a fresh generation is Active");
    let (segment, _) = tentative.commit().expect("a Tentative segment commits");
    let segment_id = segment.segment_id();
    synthesis
        .offer_segment(segment)
        .expect("a Committed segment is queued");
    segment_id
}

/// One event a synthesis delivered: when, since its iteration started, of which segment, and
/// the samples of its frame when it is audio.
struct Delivered {
    at: Duration,
    segment_id: Option<SegmentId>,
    samples: Option<u64>,
}

/// Iterates `synthesis` until it delivers nothing more.
fn drain(synthesis: &mut SherpaOnnxSynthesis) -> Vec<Delivered> {
    let started = Instant::now();
    let mut delivered = Vec::new();
    for event in synthesis.by_ref() {
        delivered.push(Delivered {
            at: started.elapsed(),
            segment_id: event.segment_id(),
            samples: match &event {
                SynthesisEvent::Audio(audio) => Some(u64::from(audio.frame().sample_count)),
                _ => None,
            },
        });
    }
    delivered
}

/// The samples of the frames of `segment_id` among `delivered`.
fn samples_of(delivered: &[Delivered], segment_id: SegmentId) -> u64 {
    delivered
        .iter()
        .filter(|event| event.segment_id == Some(segment_id))
        .filter_map(|event| event.samples)
        .sum()
}

/// Counts how often the task was woken.
#[derive(Default)]
struct Woken(AtomicUsize);

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

/// Fails the case with `message` without dropping `synthesis` and `synthesiser`. At the head
/// this pass ran on, their drop waited for the generation in flight; it no longer does.
fn give_up(synthesis: SherpaOnnxSynthesis, synthesiser: SherpaOnnxSynthesiser, message: &str) -> ! {
    std::mem::forget(synthesis);
    std::mem::forget(synthesiser);
    panic!("{message}");
}

#[test]
fn a_directory_whose_path_holds_a_nul_byte_is_refused_naming_it_and_nothing_panics() {
    let dir = Path::new("no\0such directory");
    for opened in [
        SherpaOnnxSynthesiser::open(dir),
        SherpaOnnxSynthesiser::open_voice(dir),
    ] {
        match opened {
            Err(OpenRefused::FileMissing { path, .. }) => {
                assert!(path.starts_with(dir), "{}", path.display());
            }
            other => panic!("a path no file system holds is a missing file: {other:?}"),
        }
    }
}

/// The grain of the adapter's `output_streaming`, as the limitations page and the adapters
/// reference state it: sherpa-onnx hands audio over one part per sentence. A segment of one
/// sentence is delivered once, whole, after it is generated; a segment of two sentences delivers
/// the frames of its first sentence before the second is generated.
#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn a_one_sentence_segment_is_delivered_once_and_two_sentences_part_by_part() {
    let mut synthesiser = open(1);

    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    let segment_id = offer(&mut synthesis, &generation(), 1, ONE_SENTENCE);
    let delivered = drain(&mut synthesis);
    assert_eq!(synthesis.failure(), None);
    let frames: Vec<&Delivered> = delivered
        .iter()
        .filter(|event| event.samples.is_some())
        .collect();
    let first = frames.first().expect("the sentence is spoken").at;
    let last = frames.last().expect("the sentence is spoken").at;
    eprintln!(
        "one sentence of {} characters: {} frames, {} samples; the first frame {first:?} after \
         the offer, the last {last:?} after it",
        ONE_SENTENCE.len(),
        frames.len(),
        samples_of(&delivered, segment_id),
    );
    assert!(
        (last - first) * 10 < last,
        "a segment of one sentence is one part, delivered once after it is generated: its first \
         frame came {first:?} after the offer and its last {last:?} after it"
    );
    drop(synthesis);

    let two_sentences = format!("{ONE_SENTENCE} {ONE_SENTENCE}");
    let mut synthesis = synthesiser
        .start()
        .expect("the synthesis before gave its slot back");
    let segment_id = offer(&mut synthesis, &generation(), 1, &two_sentences);
    let delivered = drain(&mut synthesis);
    assert_eq!(synthesis.failure(), None);
    let frames: Vec<&Delivered> = delivered
        .iter()
        .filter(|event| event.samples.is_some())
        .collect();
    let last = frames.last().expect("the sentences are spoken").at;
    // The longest wait between two frames: the generation of the second sentence.
    let (before, waited) = frames
        .windows(2)
        .enumerate()
        .map(|(index, pair)| (index + 1, pair[1].at - pair[0].at))
        .max_by_key(|(_, waited)| *waited)
        .expect("more than one frame is delivered");
    let samples_before: u64 = frames[..before]
        .iter()
        .filter_map(|event| event.samples)
        .sum();
    let samples_after = samples_of(&delivered, segment_id) - samples_before;
    eprintln!(
        "two sentences of {} characters: {} frames; {before} frames, {samples_before} samples, \
         then {waited:?} without a frame, then {samples_after} samples; the last frame {last:?} \
         after the offer",
        two_sentences.len(),
        frames.len(),
    );
    assert!(
        waited * 10 >= last && samples_before > 0 && samples_after > 0,
        "a segment of two sentences delivers the frames of its first sentence before the second \
         is generated: {samples_before} samples came, then nothing for {waited:?} of the \
         {last:?} the segment took, then {samples_after} samples"
    );
    assert!(
        samples_before * 10 >= samples_after * 5 && samples_before * 5 <= samples_after * 10,
        "the two parts are the two sentences, the same text twice: {samples_before} samples and \
         {samples_after} samples"
    );
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn a_long_text_without_a_sentence_end_is_spoken_to_its_end() {
    let long = [SIXTEEN_WORDS; 7].join(" ");
    let mut synthesiser = open(1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    let generation = generation();
    let short_id = offer(&mut synthesis, &generation, 1, SIXTEEN_WORDS);
    let long_id = offer(&mut synthesis, &generation, 2, &long);
    synthesis.end_utterance().expect("the utterance ends once");

    let delivered = drain(&mut synthesis);

    assert_eq!(synthesis.failure(), None);
    let short = samples_of(&delivered, short_id);
    let spoken = samples_of(&delivered, long_id);
    let bursts = |segment_id: SegmentId| {
        let mut at: Vec<u128> = delivered
            .iter()
            .filter(|event| event.segment_id == Some(segment_id) && event.samples.is_some())
            .map(|event| event.at.as_millis() / 100)
            .collect();
        at.dedup();
        at.len()
    };
    eprintln!(
        "16 words, {} characters: {short} samples in {} burst(s); the same 7 times over, {} \
         characters and no sentence end: {spoken} samples in {} burst(s), {:.2} times the 16 words",
        SIXTEEN_WORDS.len(),
        bursts(short_id),
        long.len(),
        bursts(long_id),
        spoken as f64 / short as f64,
    );
    assert!(short > 0, "the 16 words are spoken");
    assert!(
        spoken * 10 >= short * 63,
        "a text 7 times as long is spoken about 7 times as long, but its segment completed with \
         {spoken} samples against {short} for the 16 words: the end of the text was not spoken, \
         and the synthesis reports no failure"
    );
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn no_text_of_letters_or_digits_fails_the_synthesis() {
    let mut synthesiser = open(1);
    let texts = [
        "你好，世界",
        "こんにちは",
        "Привет, мир",
        "مرحبا",
        "ー",
        "²",
        "½",
        "ß",
        "Ω",
        "naïve café",
        "😀 ok",
        "a",
        "x",
        "42",
        "'s",
        " the",
    ];
    let mut failed = Vec::new();
    let mut silent = Vec::new();
    for text in texts {
        let mut synthesis = synthesiser
            .start()
            .expect("the synthesis before gave its slot back");
        let segment_id = offer(&mut synthesis, &generation(), 1, text);
        synthesis.end_utterance().expect("the utterance ends once");
        let delivered = drain(&mut synthesis);
        let samples = samples_of(&delivered, segment_id);
        eprintln!(
            "{text:?}: {samples} samples, failure {:?}",
            synthesis.failure()
        );
        if let Some(failure) = synthesis.failure() {
            failed.push((text, failure.to_string()));
        } else if samples == 0 {
            silent.push(text);
        }
    }
    eprintln!("completed without audio and without a failure: {silent:?}");
    assert!(
        failed.is_empty(),
        "a text with a letter or a digit that the voice cannot pronounce fails the whole \
         synthesis, and every segment offered after it is taken and not spoken: {failed:?}"
    );
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn every_speaker_of_the_voice_speaks_and_none_outside_it_is_taken() {
    let synthesiser = open(1);
    for speaker in [-1, 11, i32::MIN, i32::MAX] {
        assert_eq!(
            synthesiser.clone().with_speaker(speaker).map(|_| ()),
            Err(SpeakerOutOfRange {
                speaker,
                speakers: 11
            })
        );
    }
    let mut samples = Vec::new();
    for speaker in [0, 10] {
        let mut speaking = synthesiser
            .clone()
            .with_speaker(speaker)
            .expect("a speaker of the voice");
        assert_eq!(speaking.speaker(), speaker);
        let mut synthesis = speaking.start().expect("one synthesis runs");
        let segment_id = offer(&mut synthesis, &generation(), 1, "Hello there.");
        let delivered = drain(&mut synthesis);
        assert_eq!(synthesis.failure(), None);
        samples.push(samples_of(&delivered, segment_id));
    }
    eprintln!("speakers 0 and 10: {samples:?} samples");
    assert!(samples.iter().all(|&samples| samples > 0), "{samples:?}");
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn poll_event_returns_at_once_while_a_sentence_is_generated_and_its_part_wakes_the_task() {
    let mut synthesiser = open(1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    offer(&mut synthesis, &generation(), 1, ONE_SENTENCE);
    let woken = Arc::new(Woken::default());
    let waker = Waker::from(Arc::clone(&woken));
    let mut cx = Context::from_waker(&waker);
    let deadline = Instant::now() + Duration::from_secs(40);
    let (mut longest, mut pending, mut frames) = (Duration::ZERO, 0_u32, 0_u32);
    let mut woken_before_the_first_frame = 0;

    loop {
        let before = Instant::now();
        let polled = synthesis.poll_event(&mut cx);
        longest = longest.max(before.elapsed());
        match polled {
            Poll::Pending => {
                pending += 1;
                if Instant::now() > deadline {
                    give_up(synthesis, synthesiser, "the sentence is spoken within 40 s");
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Poll::Ready(Some(SynthesisEvent::Audio(_))) => {
                if frames == 0 {
                    woken_before_the_first_frame = woken.0.load(Ordering::Acquire);
                }
                frames += 1;
            }
            Poll::Ready(Some(_)) => {}
            Poll::Ready(None) => break,
        }
    }

    eprintln!(
        "{frames} frames after {pending} pending polls; the longest poll took {longest:?}; woken \
         {woken_before_the_first_frame} time(s) before the first frame"
    );
    assert_eq!(synthesis.failure(), None);
    assert!(frames > 0, "the sentence is spoken");
    assert!(pending > 0, "the generation outlasts one poll");
    assert!(woken_before_the_first_frame >= 1, "the part wakes the task");
    assert!(
        longest < Duration::from_millis(50),
        "a poll holds the task's thread for {longest:?}: a ticker beside it on a current-thread \
         runtime would miss its 50 ms"
    );
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn dropping_a_synthesis_does_not_wait_for_the_sentence_being_generated() {
    let mut synthesiser = open(1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    offer(&mut synthesis, &generation(), 1, ONE_SENTENCE);
    let waker = Waker::from(Arc::new(Woken::default()));
    let mut cx = Context::from_waker(&waker);
    assert!(
        synthesis.poll_event(&mut cx).is_pending(),
        "the generation started and has handed over nothing yet"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        synthesis.poll_event(&mut cx).is_pending(),
        "the sentence is still being generated 300 ms later"
    );

    let before = Instant::now();
    drop(synthesiser);
    let synthesiser_dropped = before.elapsed();
    let before = Instant::now();
    drop(synthesis);
    let synthesis_dropped = before.elapsed();

    eprintln!(
        "dropping the synthesiser took {synthesiser_dropped:?}, then dropping its synthesis in \
         the middle of a sentence took {synthesis_dropped:?}"
    );
    assert!(
        synthesiser_dropped < Duration::from_millis(500)
            && synthesis_dropped < Duration::from_millis(500),
        "`Drop for SherpaOnnxSynthesis` is documented as \"Tells the generation in flight to stop \
         at the engine's next part. It does not wait for it.\", but dropping the synthesiser took \
         {synthesiser_dropped:?} and dropping the synthesis {synthesis_dropped:?}: the thread \
         that drops the last of them is held until the sentence is generated to its end"
    );
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn two_syntheses_generating_at_once_each_speak_their_own_text() {
    const SHORT: &str = "Yes.";
    let mut synthesiser = open(2);
    let mut alone = Vec::new();
    for text in [SHORT, ONE_SENTENCE] {
        let mut synthesis = synthesiser.start().expect("a synthesis runs");
        let segment_id = offer(&mut synthesis, &generation(), 1, text);
        let delivered = drain(&mut synthesis);
        assert_eq!(synthesis.failure(), None);
        alone.push(samples_of(&delivered, segment_id));
    }

    let mut handles = [
        synthesiser.start().expect("the first of two runs"),
        synthesiser.start().expect("the second of two runs"),
    ];
    offer(&mut handles[0], &generation(), 1, ONE_SENTENCE);
    offer(&mut handles[1], &generation(), 1, SHORT);
    let waker = Waker::from(Arc::new(Woken::default()));
    let mut cx = Context::from_waker(&waker);
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut together = [0_u64; 2];
    let mut done = [false; 2];
    while done != [true, true] {
        assert!(Instant::now() < deadline, "both are spoken within 40 s");
        for ((handle, samples), done) in handles.iter_mut().zip(&mut together).zip(&mut done) {
            if *done {
                continue;
            }
            match handle.poll_event(&mut cx) {
                Poll::Pending => {}
                Poll::Ready(Some(SynthesisEvent::Audio(audio))) => {
                    *samples += u64::from(audio.frame().sample_count);
                }
                Poll::Ready(Some(_)) => {}
                Poll::Ready(None) => *done = true,
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    eprintln!(
        "alone: {SHORT:?} {} samples, the sentence {} samples; at once: {SHORT:?} {} samples, \
         the sentence {} samples",
        alone[0], alone[1], together[1], together[0]
    );
    assert_eq!(handles[0].failure(), None);
    assert_eq!(handles[1].failure(), None);
    for (at_once, alone) in [(together[1], alone[0]), (together[0], alone[1])] {
        assert!(
            at_once * 10 >= alone * 9 && at_once * 10 <= alone * 11,
            "a text spoken beside another generation is as long as spoken alone, within a tenth: \
             {at_once} samples against {alone}"
        );
    }
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn a_text_of_100_000_characters_is_stopped_at_its_next_sentence_by_a_discard() {
    let text = "The quick brown fox jumps over the lazy dog. ".repeat(2_223);
    assert!(text.len() >= 100_000);
    let mut synthesiser = open(1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    let generation = generation();
    let long = offer(&mut synthesis, &generation, 1, &text);
    let waker = Waker::from(Arc::new(Woken::default()));
    let mut cx = Context::from_waker(&waker);

    let offered = Instant::now();
    loop {
        match synthesis.poll_event(&mut cx) {
            Poll::Ready(Some(SynthesisEvent::Audio(_))) => break,
            Poll::Ready(Some(_)) => {}
            Poll::Ready(None) => {
                give_up(
                    synthesis,
                    synthesiser,
                    "the text is spoken, not passed over",
                );
            }
            Poll::Pending => {
                if offered.elapsed() > Duration::from_secs(25) {
                    give_up(
                        synthesis,
                        synthesiser,
                        "no frame of a text of 100 000 characters in sentences of 44 arrived \
                         within 25 s of its offer",
                    );
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    let first_frame = offered.elapsed();

    synthesis
        .discard(long)
        .expect("a Queued segment is discarded");
    let next = offer(&mut synthesis, &generation, 2, "Yes, of course.");
    let discarded = Instant::now();
    loop {
        match synthesis.poll_event(&mut cx) {
            Poll::Ready(Some(event)) => {
                if event.segment_id() == Some(long) {
                    give_up(
                        synthesis,
                        synthesiser,
                        "an event of the discarded segment is delivered after its discard",
                    );
                }
                if matches!(event, SynthesisEvent::Audio(_)) && event.segment_id() == Some(next) {
                    break;
                }
            }
            Poll::Ready(None) => {
                give_up(synthesis, synthesiser, "the next segment is spoken");
            }
            Poll::Pending => {
                if discarded.elapsed() > Duration::from_secs(20) {
                    give_up(
                        synthesis,
                        synthesiser,
                        "the segment offered after the discard of the long one was not spoken \
                         within 20 s: the generation of the discarded text did not stop",
                    );
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    let waited = discarded.elapsed();

    let before = Instant::now();
    drop(synthesis);
    drop(synthesiser);
    eprintln!(
        "{} characters: the first frame {first_frame:?} after the offer; the next segment's first \
         frame {waited:?} after the discard; both dropped in {:?}",
        text.len(),
        before.elapsed()
    );
    assert!(
        waited < Duration::from_secs(10),
        "the segment after the discarded one waited {waited:?} for its first frame"
    );
}

/// A known limit, recorded and not a rule to keep: the cascade offers a reply as one segment per
/// delta of the language model, the synthesiser speaks each segment on its own, and a reply
/// spoken that way lasts more than half as long again as the reply spoken whole. The limitations
/// page and the guide of the offline cascade state it with the numbers this case prints (7133 ms
/// against 3186 ms). The draft story on speaking whole sentences is to remove the limit; when this
/// case turns red, the limit is gone and the pages and this case are rewritten.
#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn known_limit_a_reply_spoken_delta_by_delta_lasts_more_than_half_as_long_again_as_spoken_whole() {
    let deltas = [
        "It", " is", " twenty", " two", " degrees", " outside", " and", " it", " will", " not",
        " rain", " today", ".",
    ];
    let whole = deltas.concat();
    let mut synthesiser = open(1);

    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    let segment_id = offer(&mut synthesis, &generation(), 1, &whole);
    synthesis.end_utterance().expect("the utterance ends once");
    let started = Instant::now();
    let delivered = drain(&mut synthesis);
    let whole_spent = started.elapsed();
    assert_eq!(synthesis.failure(), None);
    let whole_samples = samples_of(&delivered, segment_id);
    drop(synthesis);

    let mut synthesis = synthesiser
        .start()
        .expect("the synthesis before gave its slot back");
    let generation = generation();
    let ids: Vec<SegmentId> = deltas
        .iter()
        .zip(1..)
        .map(|(delta, sequence)| offer(&mut synthesis, &generation, sequence, delta))
        .collect();
    synthesis.end_utterance().expect("the utterance ends once");
    let started = Instant::now();
    let delivered = drain(&mut synthesis);
    let deltas_spent = started.elapsed();
    assert_eq!(synthesis.failure(), None);
    let each: Vec<u64> = ids.iter().map(|id| samples_of(&delivered, *id)).collect();
    let delta_samples: u64 = each.iter().sum();

    eprintln!(
        "{whole:?} as one segment: {whole_samples} samples, {} ms of audio, generated in \
         {whole_spent:?}; as {} deltas: {delta_samples} samples, {} ms of audio, generated in \
         {deltas_spent:?}; per delta {each:?}",
        whole_samples * 1_000 / 24_000,
        deltas.len(),
        delta_samples * 1_000 / 24_000,
    );
    assert!(whole_samples > 0, "the reply is spoken");
    assert!(
        delta_samples * 2 > whole_samples * 3,
        "the documented limit: a reply offered as one segment per delta of the language model \
         lasts more than half as long again as the reply spoken whole. It lasted {} ms against \
         {} ms spoken whole, so the limit no longer holds as the limitations page states it",
        delta_samples * 1_000 / 24_000,
        whole_samples * 1_000 / 24_000,
    );
}
