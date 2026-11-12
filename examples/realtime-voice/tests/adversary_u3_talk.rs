//! Adversary cases, pass 1, for story:talk-picks-elevenlabs: the conversation
//! `talk --provider elevenlabs-cascade` holds (`realtime_voice::talk::converse`), driven against
//! what its own module documentation says of a cut, of the end of the recognition and of the
//! end of the conversation. Nothing here reaches a real provider: the ElevenLabs recogniser of
//! the first case connects to a loopback server that refuses the upgrade.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use inference_audio_device::convert::encode;
use inference_duplex::inference_audio::{AudioEncoding, AudioFrame, SampleClockPosition};
use inference_elevenlabs::{
    Credential, CredentialFuture, CredentialSource, ElevenLabsRecognition, Endpoint, KeychainSource,
};
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

/// What the playout was handed and how often it was stopped.
#[derive(Debug, Default)]
struct Played {
    frames: usize,
    stops: usize,
}

struct Recorder(Rc<RefCell<Played>>);

impl Playout for Recorder {
    fn play(&mut self, _frame: &AudioFrame) {
        self.0.borrow_mut().frames += 1;
    }

    fn stop(&mut self) {
        self.0.borrow_mut().stops += 1;
    }
}

fn recogniser(script: Vec<Cue>) -> MockRecogniser {
    MockRecogniser::new(SttConfiguration::Incremental, ModelId::generate(), script)
}

fn generator(answer: &[String]) -> Generator {
    MockGenerator::new(
        LlmConfiguration::StreamingOnly,
        ModelId::generate(),
        answer.iter().cloned(),
    )
    .into()
}

fn mock_synthesiser() -> MockSynthesiser {
    MockSynthesiser::new(
        TtsConfiguration::IncrementalStreaming,
        ModelId::generate(),
        pcm16(16_000),
    )
}

/// One question, committed after the second frame.
fn one_question() -> Vec<Cue> {
    let cue = |after_frames, step| Cue { after_frames, step };
    vec![
        cue(1, ScriptStep::Hypothesis("tell me a story".to_owned())),
        cue(2, ScriptStep::Commit { span: 0 }),
    ]
}

/// An answer of `pieces` segments of `words` words each.
fn answer(pieces: usize, words: usize) -> Vec<String> {
    (0..pieces)
        .map(|piece| {
            let words: Vec<String> = (0..words).map(|word| format!("w{piece}x{word}")).collect();
            format!("{}. ", words.join(" "))
        })
        .collect()
}

/// A synthesiser that takes `per_segment` to take each segment it is offered, as a hosted
/// synthesiser such as ElevenLabs blocks while it speaks one, and records when each was offered.
struct Slow {
    inner: MockSynthesiser,
    offers: Arc<Mutex<Vec<Instant>>>,
    per_segment: Duration,
}

struct SlowSynthesis {
    inner: <MockSynthesiser as Synthesiser>::Handle,
    offers: Arc<Mutex<Vec<Instant>>>,
    per_segment: Duration,
}

impl Synthesiser for Slow {
    type Handle = SlowSynthesis;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn start(&mut self) -> Result<SlowSynthesis, ConcurrencyLimitReached> {
        Ok(SlowSynthesis {
            inner: self.inner.start()?,
            offers: Arc::clone(&self.offers),
            per_segment: self.per_segment,
        })
    }
}

impl Iterator for SlowSynthesis {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        self.inner.next()
    }
}

impl SynthesisHandle for SlowSynthesis {
    fn output(&self) -> &SynthesisOutput {
        self.inner.output()
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        self.offers.lock().expect("the offers").push(Instant::now());
        std::thread::sleep(self.per_segment);
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

/// How a conversation over the slow synthesiser went.
struct SlowRun {
    notes: Vec<(Instant, CascadeNote)>,
    offers: Vec<Instant>,
    /// When `open` first answered false.
    closed: Option<Instant>,
    /// When `converse` returned.
    returned: Instant,
}

/// A conversation over a [`Slow`] synthesiser answering one question with `pieces` segments;
/// the input turns voiced once the answer's audio reaches the playout when `barge` is set, and
/// the conversation is closed once `done` says so.
fn slow_run(
    pieces: usize,
    per_segment: Duration,
    barge: bool,
    done: fn(&[(Instant, CascadeNote)], &Played) -> bool,
) -> SlowRun {
    let offers = Arc::new(Mutex::new(Vec::new()));
    let synthesiser = Slow {
        inner: mock_synthesiser(),
        offers: Arc::clone(&offers),
        per_segment,
    };
    let mut cascade = Cascade::new(
        recogniser(one_question()),
        generator(&answer(pieces, 5)),
        synthesiser,
        pcm16(16_000),
    );
    let played = Rc::new(RefCell::new(Played::default()));
    let notes = Rc::new(RefCell::new(Vec::new()));
    let closed = Rc::new(RefCell::new(None));
    let mut playout = Recorder(Rc::clone(&played));
    let started = Instant::now();
    let mut sent = 0;
    let mut input = {
        let played = Rc::clone(&played);
        move || {
            let level = if barge && played.borrow().frames > 0 {
                0.5
            } else {
                0.0
            };
            std::thread::sleep(Duration::from_millis(2));
            sent += 1;
            Some(frame(sent, level))
        }
    };
    let open = {
        let notes = Rc::clone(&notes);
        let played = Rc::clone(&played);
        let closed = Rc::clone(&closed);
        move || {
            let open = !done(&notes.borrow(), &played.borrow())
                && started.elapsed() < Duration::from_secs(20);
            if !open && closed.borrow().is_none() {
                *closed.borrow_mut() = Some(Instant::now());
            }
            open
        }
    };
    let mut note = {
        let notes = Rc::clone(&notes);
        move |note| notes.borrow_mut().push((Instant::now(), note))
    };
    converse(
        &mut cascade,
        &mut input,
        &mut playout,
        &open,
        |_| None,
        &mut note,
    );
    let returned = Instant::now();
    let notes = notes.borrow().clone();
    let offers = offers.lock().expect("the offers").clone();
    let closed = *closed.borrow();
    SlowRun {
        notes,
        offers,
        closed,
        returned,
    }
}

/// The module documentation of `talk`: at a barge-in "the synthesis is left to end with the
/// segment it speaks and is then dropped". An LLM commits its segments faster than a hosted
/// synthesiser speaks them, so at a barge-in most of the answer is queued on the synthesis's
/// worker. Dropping the task sender (`talk.rs` `cut`, `self.speak = None`) does not drop what is
/// queued: `compare.rs` `speaking` iterates its receiver, which yields every queued task before
/// it ends, so each queued segment is still offered to the synthesiser, spoken and paid for.
#[test]
fn after_a_barge_in_the_synthesiser_is_offered_no_further_segment() {
    let run = slow_run(12, Duration::from_millis(150), true, |notes, _| {
        notes.iter().any(|(_, note)| *note == CascadeNote::BargeIn)
    });
    let barge = run
        .notes
        .iter()
        .find(|(_, note)| *note == CascadeNote::BargeIn)
        .map(|(at, _)| *at)
        .expect("the scenario needs a barge-in");
    let after: Vec<Duration> = run
        .offers
        .iter()
        .filter(|at| **at > barge)
        .map(|at| at.duration_since(barge))
        .collect();
    assert!(
        after.is_empty(),
        "{} segments were offered to the synthesiser after the barge-in cut the answer \
         (at {after:?} after it); {} offers in all",
        after.len(),
        run.offers.len()
    );
}

/// The same queue at the end of the conversation: `talk` documents "Enter ends the
/// conversation", and `converse` cuts the answer when `open` answers false. The thread scope
/// then joins the synthesis worker, which first offers every queued segment, so `converse`, and
/// `talk` with it, returns only once the whole queued answer was synthesised.
#[test]
fn closing_the_conversation_while_an_answer_is_queued_returns_promptly() {
    let run = slow_run(12, Duration::from_millis(150), false, |_, played| {
        played.frames > 0
    });
    let closed = run
        .closed
        .expect("the scenario needs the conversation closed");
    let waited = run.returned.duration_since(closed);
    assert!(
        waited < Duration::from_millis(600),
        "converse returned {waited:?} after the conversation was closed, having offered {} \
         segments",
        run.offers.len()
    );
}

/// `Talk::playing` is "the answer is in progress or its audio still plays", and a barge-in stops
/// the playout in either case. A turn the recogniser commits is handled by `answer`, which stops
/// the playout only `if self.answering`: once the answer is fully synthesised its audio may still
/// be queued on the speaker for most of `PLAYOUT_BUFFER` (120 s), and the next answer's audio is
/// queued behind it. Speech the endpoint's voice activity detection commits without the local
/// detector's attack (quieter than its threshold) reaches this.
#[test]
fn a_turn_committed_while_the_last_answer_still_plays_stops_that_playout() {
    let cue = |after_frames, step| Cue { after_frames, step };
    let script = vec![
        cue(1, ScriptStep::Hypothesis("tell me a story".to_owned())),
        cue(2, ScriptStep::Commit { span: 0 }),
        cue(
            300,
            ScriptStep::Hypothesis("stop and tell me the time".to_owned()),
        ),
        cue(301, ScriptStep::Commit { span: 1 }),
    ];
    let mut cascade = Cascade::new(
        recogniser(script),
        generator(&answer(8, 50)),
        mock_synthesiser(),
        pcm16(16_000),
    );
    let played = Rc::new(RefCell::new(Played::default()));
    let notes = Rc::new(RefCell::new(Vec::new()));
    let mut playout = Recorder(Rc::clone(&played));
    let started = Instant::now();
    let mut sent = 0;
    let mut input = move || {
        std::thread::sleep(Duration::from_millis(1));
        sent += 1;
        Some(frame(sent, 0.0))
    };
    let open = {
        let notes = Rc::clone(&notes);
        move || {
            let heard = notes
                .borrow()
                .iter()
                .filter(|note| matches!(note, CascadeNote::Heard(_)))
                .count();
            heard < 2 && started.elapsed() < Duration::from_secs(20)
        }
    };
    let mut note = {
        let notes = Rc::clone(&notes);
        move |note| notes.borrow_mut().push(note)
    };
    converse(
        &mut cascade,
        &mut input,
        &mut playout,
        &open,
        |_| None,
        &mut note,
    );
    let notes = notes.borrow().clone();
    let answered = notes
        .iter()
        .position(|note| *note == CascadeNote::Answered)
        .expect("the scenario needs the first answer fully synthesised");
    let second = notes
        .iter()
        .rposition(|note| matches!(note, CascadeNote::Heard(_)))
        .expect("the scenario needs a second turn");
    assert!(answered < second, "{notes:?}");
    let audio = Duration::from_millis(10) * u32::try_from(played.borrow().frames).unwrap_or(0);
    assert!(
        started.elapsed() < audio,
        "the scenario needs the first answer's audio still playing at the second turn: \
         {audio:?} of audio, {:?} elapsed",
        started.elapsed()
    );
    assert!(
        played.borrow().stops >= 1,
        "a second turn was committed while {audio:?} of the first answer's audio was still \
         queued, and the playout was never stopped: {notes:?}"
    );
}

/// `talk --provider elevenlabs-cascade` without `ELEVENLABS_API_KEY` takes the OS keychain
/// (`examples/talk.rs` `cascade::key`), the path its guide documents first ("store the
/// ElevenLabs key once with `secretsctl put elevenlabs-api-key`"), and checks it with
/// `realtime_voice::elevenlabs::resolves`, which polls the source's future once with a no-op
/// waker on the calling thread. The keychain source's read runs on `tokio::task::spawn_blocking`,
/// which needs a Tokio runtime on that thread; `talk`'s main thread has none. The check must say
/// whether the key resolved, not panic and not refuse a source because it is asynchronous.
/// Opening the keychain reads nothing, and the read never starts: the panic comes first.
#[test]
fn the_keychain_source_talk_documents_is_checked_without_a_panic() {
    let Ok(source) = KeychainSource::native() else {
        eprintln!("no platform keychain here: the scenario needs one");
        return;
    };
    let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        elevenlabs::resolves(&source)
    }));
    let checked = checked.map_err(|panic| {
        panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_default()
    });
    assert!(
        checked.is_ok(),
        "checking the keychain source panicked: {checked:?}"
    );
    assert!(
        !checked
            .as_ref()
            .is_ok_and(|resolved| resolved.as_ref().is_err_and(|why| why.contains("at once"))),
        "the keychain source is refused for being asynchronous: {checked:?}"
    );
}

/// A key the loopback server never checks.
struct FixedKey;

impl CredentialSource for FixedKey {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::ready(Ok(Credential::new(
            b"not-a-key".to_vec(),
        ))))
    }
}

/// A loopback server that refuses every websocket upgrade with 403, as a host refuses a key of
/// another residency stack; its URL.
fn refusing_server() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let url = format!("ws://{}", listener.local_addr().expect("an address"));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                return;
            };
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => request.extend_from_slice(&buffer[..read]),
                }
            }
            let _ = stream.write_all(
                b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            );
        }
    });
    url
}

/// The module documentation of `talk`: `CascadeNote::Ended` is "the recognition ended, and with
/// it the conversation; a `CascadeNote::Refused` before it says why", and `talk` passes
/// `ElevenLabsRecognition::failure` for that. The ElevenLabs recognition fails at once here (the
/// host refuses the upgrade), but before the end of input its iteration only yields `None`: the
/// usage record, the one event `listening` asks `failure` at, is refused until input has ended,
/// and `converse` never ends the input. The conversation goes on over a dead recognition, saying
/// nothing, until the operator presses Enter.
#[test]
fn a_recognition_that_fails_before_the_end_of_input_ends_the_conversation_and_says_why() {
    let endpoint = Endpoint::new()
        .with_url(refusing_server())
        .with_connect_timeout(Duration::from_secs(2))
        .with_read_timeout(Duration::from_secs(2));
    let recogniser =
        elevenlabs::recogniser(endpoint, Arc::new(FixedKey)).expect("the recogniser is made");
    let mut cascade = Cascade::new(
        recogniser,
        generator(&answer(1, 3)),
        mock_synthesiser(),
        pcm16(16_000),
    );
    let mut playout = Recorder(Rc::new(RefCell::new(Played::default())));
    let notes = Rc::new(RefCell::new(Vec::new()));
    let started = Instant::now();
    let deadline = Duration::from_secs(8);
    let mut sent = 0;
    let mut input = move || {
        std::thread::sleep(Duration::from_millis(5));
        sent += 1;
        Some(frame(sent, 0.0))
    };
    let open = || started.elapsed() < deadline;
    let mut note = {
        let notes = Rc::clone(&notes);
        move |note| notes.borrow_mut().push(note)
    };
    converse(
        &mut cascade,
        &mut input,
        &mut playout,
        &open,
        |recognition: &ElevenLabsRecognition| recognition.failure().map(ToString::to_string),
        &mut note,
    );
    let notes = notes.borrow().clone();
    assert!(
        notes.contains(&CascadeNote::Ended),
        "the recognition failed (the host refused it) and the conversation was still open \
         after {:?}, with notes {notes:?}",
        started.elapsed()
    );
    assert!(
        notes
            .iter()
            .any(|note| matches!(note, CascadeNote::Refused(_))),
        "no note says why the recognition ended: {notes:?}"
    );
}
