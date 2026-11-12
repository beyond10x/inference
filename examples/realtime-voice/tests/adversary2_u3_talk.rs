//! Adversary cases, pass 2, for story:talk-picks-elevenlabs: what the fix round added to
//! `talk --provider elevenlabs-cascade`, driven against what its own guide, changelog and module
//! documentation say. Nothing here reaches a real provider, a keychain or a microphone.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use inference_audio_device::convert::encode;
use inference_duplex::inference_audio::{AudioEncoding, AudioFrame, SampleClockPosition};
use inference_elevenlabs::{Credential, CredentialError, CredentialFuture, CredentialSource};
use inference_llm::Generator;
use inference_stt::inference_core::model::{Adapter, ModelId};
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep,
};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use inference_tts::inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued,
};
use inference_tts::{
    ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt, PlaybackRefused,
    SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};
use realtime_voice::compare::{Cascade, Playout};
use realtime_voice::talk::{CascadeNote, converse};
use realtime_voice::{elevenlabs, pcm16};

/// A key source shaped as the keychain's (`secrets-keychain` `KeychainBackend::blocking`): the
/// read runs on Tokio's blocking pool and the future awaits it. The read never answers in any
/// time a test waits for, as a Secret Service unlock prompt nobody answers does not.
struct UnansweredKeychain;

impl CredentialSource for UnansweredKeychain {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(async {
            tokio::task::spawn_blocking(|| {
                std::thread::sleep(Duration::from_secs(600));
                Ok::<_, CredentialError>(Credential::new(b"never".to_vec()))
            })
            .await
            .map_err(|_| CredentialError::Unavailable)?
        })
    }
}

/// `talk`'s guide, row `ELEVENLABS_API_KEY`: "the OS keychain, secret `elevenlabs-api-key`; exit
/// status 2 when the keychain does not open or gives no key", and `examples/talk.rs` `cascade::key`
/// checks it "so a missing key stops before anything opens". The fix round drives the source to
/// its end (`elevenlabs::resolves`, a runtime on a scoped thread) with no bound, so a keychain
/// that never answers holds `talk` before its first line of output, Enter not yet read, forever.
/// A bound on `block_on` alone does not end it either: dropping the runtime waits for the
/// blocking read.
#[test]
fn a_keychain_that_never_answers_does_not_hold_the_key_check_forever() {
    let (done, finished) = mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let _ = done.send(elevenlabs::resolves(&UnansweredKeychain));
    });
    let bound = Duration::from_secs(10);
    let outcome = finished.recv_timeout(bound);
    assert!(
        outcome.is_ok(),
        "elevenlabs::resolves had not returned {:?} after it was asked, over a keychain read \
         that does not answer: talk waits before anything opens, printing nothing",
        started.elapsed()
    );
}

/// A lookup that answers `set` and nothing else.
#[cfg(feature = "local")]
fn lookup(set: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    let set = set.to_vec();
    move |name| {
        set.iter()
            .find(|(variable, _)| *variable == name)
            .map(|(_, value)| (*value).to_owned())
    }
}

/// The changelog: "A variable it needs that is not set, or set to a value it does not take, exits
/// with status 2, naming it, the variables checked in the order the guide lists them
/// (`realtime_voice::talk::Provider::required`)", and the guide: "The variables are checked in
/// the order of the table", whose first row refuses "a host that is not loopback". `talk` runs
/// `Provider::required` first, and it checks only that the three are set: with a server URL that
/// is not loopback and no voice, `talk` names the voice, the third row, and never the URL.
#[cfg(feature = "local")]
#[test]
fn a_server_url_not_taken_is_named_before_a_later_variable_that_is_unset() {
    let refused = realtime_voice::talk::Provider::ElevenlabsCascade
        .required(lookup(&[
            ("INFERENCE_LLAMA_SERVER_URL", "http://192.0.2.1:8080/v1"),
            ("INFERENCE_LLAMA_SERVER_MODEL", "qwen2.5-1.5b-instruct"),
        ]))
        .expect_err("neither the URL nor the voice is taken");
    assert!(
        refused.to_string().contains("INFERENCE_LLAMA_SERVER_URL"),
        "the server URL, first in the guide's table, is not loopback, and talk names another \
         variable first: {refused}"
    );
}

/// Samples in one 20 ms frame at 16 kHz.
const FRAME_SAMPLES: u32 = 320;

/// The `index`th 20 ms frame of 16-bit PCM at 16 kHz, every sample `level`.
fn frame(index: u64, level: f32) -> AudioFrame {
    let samples = vec![level; FRAME_SAMPLES as usize];
    AudioFrame {
        format: pcm16(16_000),
        position: SampleClockPosition::new(index * u64::from(FRAME_SAMPLES)).expect("a position"),
        sample_count: FRAME_SAMPLES,
        data: encode(&samples, AudioEncoding::PcmS16le),
    }
}

/// A synthesiser that delivers each event `per_event` after it was asked for it, as a hosted
/// synthesiser such as ElevenLabs reads each chunk of a segment's audio off its socket, and
/// counts the events asked for.
struct Streaming {
    inner: MockSynthesiser,
    reads: Arc<AtomicUsize>,
    per_event: Duration,
}

struct StreamingSynthesis {
    inner: <MockSynthesiser as Synthesiser>::Handle,
    reads: Arc<AtomicUsize>,
    per_event: Duration,
}

impl Synthesiser for Streaming {
    type Handle = StreamingSynthesis;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn start(&mut self) -> Result<StreamingSynthesis, ConcurrencyLimitReached> {
        Ok(StreamingSynthesis {
            inner: self.inner.start()?,
            reads: Arc::clone(&self.reads),
            per_event: self.per_event,
        })
    }
}

impl Iterator for StreamingSynthesis {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.per_event);
        self.inner.next()
    }
}

impl SynthesisHandle for StreamingSynthesis {
    fn output(&self) -> &SynthesisOutput {
        self.inner.output()
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        self.inner.offer_segment(segment)
    }

    fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        self.inner.end_utterance()
    }

    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        self.inner.acknowledge_playback(segment_id, receipt)
    }

    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        self.inner.discard(segment_id)
    }
}

/// How many frames the playout was handed.
struct Counter(Rc<RefCell<usize>>);

impl Playout for Counter {
    fn play(&mut self, _frame: &AudioFrame) {
        *self.0.borrow_mut() += 1;
    }

    fn stop(&mut self) {}
}

/// `compare.rs` `speaking`: "Once `cut` is set, it offers nothing more, reads no further event
/// and ends", and `talk`'s module documentation: "The end of the conversation cuts the answer
/// too, and returns without waiting for the rest of it". One segment of 40 words is 40 frames of
/// the mock, each read 50 ms after it is asked for: closing the conversation at its first frame
/// leaves about 2 s of the segment unread, which the worker must not read.
#[test]
fn a_cut_synthesis_reads_no_further_event_of_the_segment_it_speaks() {
    let cue = |after_frames, step| Cue { after_frames, step };
    let script = vec![
        cue(1, ScriptStep::Hypothesis("tell me a story".to_owned())),
        cue(2, ScriptStep::Commit { span: 0 }),
    ];
    let words: Vec<String> = (0..40).map(|word| format!("w{word}")).collect();
    let answer = vec![format!("{}. ", words.join(" "))];
    let reads = Arc::new(AtomicUsize::new(0));
    let generator: Generator =
        MockGenerator::new(LlmConfiguration::StreamingOnly, ModelId::generate(), answer).into();
    let mut cascade = Cascade::new(
        MockRecogniser::new(SttConfiguration::Incremental, ModelId::generate(), script),
        generator,
        Streaming {
            inner: MockSynthesiser::new(
                TtsConfiguration::IncrementalStreaming,
                ModelId::generate(),
                pcm16(16_000),
            ),
            reads: Arc::clone(&reads),
            per_event: Duration::from_millis(50),
        },
        pcm16(16_000),
    );
    let played = Rc::new(RefCell::new(0_usize));
    let mut playout = Counter(Rc::clone(&played));
    let closed: Rc<RefCell<Option<(Instant, usize)>>> = Rc::new(RefCell::new(None));
    let started = Instant::now();
    let mut sent = 0;
    let mut input = move || {
        std::thread::sleep(Duration::from_millis(2));
        sent += 1;
        Some(frame(sent, 0.0))
    };
    let open = {
        let played = Rc::clone(&played);
        let closed = Rc::clone(&closed);
        let reads = Arc::clone(&reads);
        move || {
            let open = *played.borrow() == 0 && started.elapsed() < Duration::from_secs(20);
            if !open && closed.borrow().is_none() {
                *closed.borrow_mut() = Some((Instant::now(), reads.load(Ordering::SeqCst)));
            }
            open
        }
    };
    let mut note = |_: CascadeNote| {};
    converse(
        &mut cascade,
        &mut input,
        &mut playout,
        &open,
        |_| None,
        &mut note,
    );
    let returned = Instant::now();
    let (closed_at, reads_at_close) = closed
        .borrow()
        .expect("the scenario needs the conversation closed");
    assert!(
        *played.borrow() > 0,
        "the scenario needs the first frame played"
    );
    let read_after = reads.load(Ordering::SeqCst) - reads_at_close;
    let waited = returned.duration_since(closed_at);
    assert!(
        read_after <= 2 && waited < Duration::from_millis(400),
        "the cut synthesis read {read_after} more events after the conversation was closed, and \
         converse returned {waited:?} after it"
    );
}
