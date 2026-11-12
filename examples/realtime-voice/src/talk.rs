//! What `examples/talk.rs` offers and what it needs: its providers ([`Provider`]), the
//! variables each needs before anything opens ([`Provider::required`]) and the exit status of a
//! missing one ([`USAGE_EXIT`]), and the conversation of its cascade provider ([`converse`]).
//!
//! `elevenlabs-cascade` is a cascade of a [`Cascade`]'s recogniser, LLM and synthesiser, as
//! `compare`'s cascade arm binds them, held as a conversation over a live input. One recognition
//! takes every frame of the input for the whole conversation; the recogniser cuts it into turns,
//! as the ElevenLabs recogniser does with its endpoint's voice activity detection, and each
//! committed transcript is a turn: its text is asked of the generator as user text, and each
//! committed segment of the answer is offered to the synthesiser and its frames handed to the
//! [`Playout`] as they arrive, on the steps' workers of `compare`.
//!
//! Barge-in is `compare`'s rule on a live input: once [`Cascade::attack`] of voiced input, a
//! frame being voiced at the root mean square of the local voice cascade's detector
//! ([`local_voice_cascade::VAD`]), arrives while the answer is in progress or its audio still
//! plays, the playout is stopped and the answer is cut, as `compare` cuts it: the generation is
//! told to stop, the synthesis is left to end with the segment it speaks and is then dropped, and
//! every later report of the answer, its frames among them, is dropped on the client. A hosted
//! synthesiser such as ElevenLabs still generates the segment it was speaking: this stops
//! playback, not generation or cost. A turn committed while an answer is in progress cuts that
//! answer the same way before its own starts. Each turn is asked on its own, without the turns
//! before it.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, Scope, ScopedJoinHandle};
use std::time::{Duration, Instant};

use inference_audio_device::convert::decode;
use inference_duplex::inference_audio::AudioFrame;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_text::TurnId;
use inference_stt::{Recogniser, RecognitionRequest, TranscriptEvent};
use inference_tts::{SynthesisEvent, Synthesiser};

use crate::compare::{
    Cascade, Generate, Heard, Listen, Owing, Playout, Report, Segmented, Segments, Speak, Task,
    hand, request, spawn_generation, spawn_recognition, spawn_synthesis,
};

/// The exit status of `talk` when a variable its provider needs is not set or not taken, or
/// the provider was not built: a usage error, as the command line's own.
pub const USAGE_EXIT: u8 = 2;

/// A provider `talk --provider` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    /// The OpenAI Realtime API.
    Openai,
    /// OpenAI GPT-Live.
    OpenaiLive,
    /// Amazon Nova Sonic on Bedrock.
    Nova,
    /// ElevenLabs speech to text, the local LLM, ElevenLabs text to speech.
    ElevenlabsCascade,
}

impl Provider {
    /// Every provider, in the order `--help` lists them.
    pub const ALL: [Self; 4] = [
        Self::Openai,
        Self::OpenaiLive,
        Self::Nova,
        Self::ElevenlabsCascade,
    ];

    /// The name `--provider` takes.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::OpenaiLive => "openai-live",
            Self::Nova => "nova",
            Self::ElevenlabsCascade => "elevenlabs-cascade",
        }
    }

    /// What `--help` says of it.
    #[must_use]
    pub const fn about(self) -> &'static str {
        match self {
            Self::Openai => "The OpenAI Realtime API",
            Self::OpenaiLive => "OpenAI GPT-Live",
            Self::Nova => "Amazon Nova Sonic on Bedrock",
            Self::ElevenlabsCascade => {
                "ElevenLabs speech to text, the local LLM (feature local), ElevenLabs text to \
                 speech"
            }
        }
    }

    /// The provider named `name`, exactly as [`Provider::name`] gives it.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.name() == name)
    }

    /// Whether every variable the provider needs is set, as `lookup` answers them; a blank value
    /// is unset. The OpenAI and Nova providers need none of the table: their keys and region are
    /// read when the model is made. `elevenlabs-cascade` needs [`CASCADE_VARIABLES`], in that
    /// order, and feature `local`.
    ///
    /// # Errors
    ///
    /// [`Refused::Missing`] naming the first variable not set, or [`Refused::Unbuilt`] for
    /// `elevenlabs-cascade` built without feature `local`.
    pub fn required(self, lookup: impl Fn(&str) -> Option<String>) -> Result<(), Refused> {
        match self {
            Self::Openai | Self::OpenaiLive | Self::Nova => Ok(()),
            Self::ElevenlabsCascade => cascade_required(lookup),
        }
    }
}

/// The variables `elevenlabs-cascade` needs, in the order they are checked: the local LLM's
/// server URL and model, as the local voice cascade reads them, and the ElevenLabs voice. The
/// ElevenLabs key is `ELEVENLABS_API_KEY` or the keychain's `elevenlabs-api-key`, and
/// `ELEVENLABS_RESIDENCY` picks the host, global when unset.
#[cfg(feature = "local")]
pub const CASCADE_VARIABLES: [&str; 3] = [
    local_voice_cascade::local::SERVER_URL_VARIABLE,
    local_voice_cascade::local::SERVER_MODEL_VARIABLE,
    crate::compare::VOICE_VARIABLE,
];

#[cfg(feature = "local")]
fn cascade_required(lookup: impl Fn(&str) -> Option<String>) -> Result<(), Refused> {
    for variable in CASCADE_VARIABLES {
        if lookup(variable).is_none_or(|value| value.trim().is_empty()) {
            return Err(Refused::Missing(variable));
        }
    }
    Ok(())
}

#[cfg(not(feature = "local"))]
fn cascade_required(_lookup: impl Fn(&str) -> Option<String>) -> Result<(), Refused> {
    Err(Refused::Unbuilt { feature: "local" })
}

/// Why `talk` did not start a provider: each exits with [`USAGE_EXIT`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The variable is not set, or blank.
    Missing(&'static str),
    /// A variable is set to a value not taken; the reason names the variable, never the value.
    Invalid(String),
    /// The provider needs a feature `talk` was built without.
    Unbuilt { feature: &'static str },
}

impl Refused {
    /// The exit status `talk` ends with: [`USAGE_EXIT`].
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        USAGE_EXIT
    }
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(variable) => write!(f, "{variable} is not set"),
            Self::Invalid(reason) => f.write_str(reason),
            Self::Unbuilt { feature } => write!(
                f,
                "talk was built without feature {feature}, which binds the cascade's local LLM: \
                 run it with --features {feature}"
            ),
        }
    }
}

impl std::error::Error for Refused {}

/// Something the cascade conversation did or heard, for the caller to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CascadeNote {
    /// The recogniser committed what the user said: a turn.
    Heard(String),
    /// A committed segment of the answer, offered to the synthesiser.
    Said(String),
    /// The answer's synthesis delivered its last event.
    Answered,
    /// Voiced input while the answer was in progress or played: the playout stopped and the
    /// rest of the answer is dropped.
    BargeIn,
    /// A step refused or failed; the conversation goes on.
    Refused(String),
    /// The recognition ended, and with it the conversation; a [`CascadeNote::Refused`] before it
    /// says why, when the recogniser says.
    Ended,
}

/// The report tag of the recognition's worker; each answer's workers report under a tag above
/// it.
const LISTENER: u64 = 0;

/// Holds a conversation over `cascade` until `open` answers false or the recognition ends.
/// `input` hands the next frame of input in the cascade's input format, waiting briefly, or none
/// yet; the answer's audio is handed to `playout`; `failure` says why a recognition ended, when
/// its recogniser says; every [`CascadeNote`] goes to `note`. See the module documentation.
///
/// # Panics
///
/// When a step's worker panics, once every worker stopped.
pub fn converse<R, S>(
    cascade: &mut Cascade<R, S>,
    input: &mut dyn FnMut() -> Option<AudioFrame>,
    playout: &mut dyn Playout,
    open: &dyn Fn() -> bool,
    failure: fn(&R::Handle) -> Option<String>,
    note: &mut dyn FnMut(CascadeNote),
) where
    R: Recogniser,
    R::Handle: Send,
    S: Synthesiser,
    S::Handle: Send,
{
    let (sender, reports) = mpsc::channel();
    let owing = Arc::new(Owing {
        count: AtomicUsize::new(0),
        reports: sender.clone(),
    });
    thread::scope(|scope| {
        let mut talk = Talk {
            cascade,
            playout,
            note,
            scope,
            sender,
            owing,
            run_id: RunId::generate(),
            stt: NodeId::generate(),
            llm: NodeId::generate(),
            listen: None,
            turn: LISTENER,
            answering: false,
            generate: None,
            speak: None,
            workers: Vec::new(),
            segments: Segments::default(),
            utterance_ended: false,
            epoch: 0,
            cursor: None,
            voiced_for: Duration::ZERO,
            ended: false,
        };
        talk.listen(failure);
        while !talk.ended && open() {
            if let Some(frame) = input() {
                talk.hear(frame);
            }
            talk.take_all(&reports);
        }
        talk.cut();
        talk.listen = None;
    });
}

struct Talk<'a, 's, 'e, R, S> {
    cascade: &'a mut Cascade<R, S>,
    playout: &'a mut dyn Playout,
    note: &'a mut dyn FnMut(CascadeNote),
    scope: &'s Scope<'s, 'e>,
    sender: Sender<Report>,
    owing: Arc<Owing>,
    run_id: RunId,
    stt: NodeId,
    llm: NodeId,
    listen: Option<Sender<Task<Listen>>>,
    /// The tag the answer that runs, or ran last, reports under.
    turn: u64,
    /// Whether an answer is in progress: generated or synthesised.
    answering: bool,
    generate: Option<Sender<Task<Generate>>>,
    speak: Option<Sender<Task<Speak>>>,
    /// The workers of the answers, joined before the next answer starts.
    workers: Vec<ScopedJoinHandle<'s, ()>>,
    segments: Segments,
    utterance_ended: bool,
    epoch: i64,
    /// Where the playout of the answer's audio received so far ends, on the client's model of it.
    cursor: Option<Instant>,
    /// The voiced input received since the last quiet frame.
    voiced_for: Duration,
    ended: bool,
}

impl<'s, R, S> Talk<'_, 's, '_, R, S>
where
    R: Recogniser,
    R::Handle: Send + 's,
    S: Synthesiser,
    S::Handle: Send + 's,
{
    /// The reporter of a worker reporting under `turn`.
    fn reporter(&self, turn: u64) -> impl Fn(Heard) + Send + 'static {
        let reports = self.sender.clone();
        move |heard| {
            let _ = reports.send(Report { turn, heard });
        }
    }

    /// Starts the conversation's one recognition on its worker.
    fn listen(&mut self, failure: fn(&R::Handle) -> Option<String>) {
        let request = RecognitionRequest::new(
            self.run_id,
            TurnId::generate(),
            StreamId::generate(),
            self.stt,
            "transcript",
        );
        match self.cascade.recogniser.start(request) {
            Ok(recognition) => {
                let report = self.reporter(LISTENER);
                let (tasks, _worker) = spawn_recognition(self.scope, recognition, report, failure);
                self.listen = Some(tasks);
            }
            Err(error) => {
                (self.note)(CascadeNote::Refused(format!(
                    "the recogniser refused the conversation: {error}"
                )));
                (self.note)(CascadeNote::Ended);
                self.ended = true;
            }
        }
    }

    /// Whether the answer is in progress or its audio still plays.
    fn playing(&self) -> bool {
        self.answering || self.cursor.is_some_and(|cursor| cursor > Instant::now())
    }

    /// Hands a frame of input to the recognition, and stops the answer once the detector heard
    /// [`Cascade::attack`] of voiced input over it.
    fn hear(&mut self, frame: AudioFrame) {
        let length = frame.duration();
        let voiced = decode(&frame).is_ok_and(|samples| {
            let squares: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
            !samples.is_empty()
                && (squares / samples.len() as f64).sqrt() >= local_voice_cascade::VAD.threshold
        });
        hand(self.listen.as_ref(), &self.owing, Listen::Frame(frame));
        if !voiced {
            self.voiced_for = Duration::ZERO;
            return;
        }
        self.voiced_for += length;
        if self.voiced_for >= self.cascade.attack && self.playing() {
            self.barge_in();
        }
    }

    /// Stops the playout and drops the answer.
    fn barge_in(&mut self) {
        self.playout.stop();
        self.cursor = None;
        self.cut();
        (self.note)(CascadeNote::BargeIn);
    }

    /// Drops the answer that runs: its later reports are dropped, the generation is told to stop
    /// and the synthesis's utterance is left, its worker ending with the segment it speaks.
    fn cut(&mut self) {
        self.turn += 1;
        self.answering = false;
        self.speak = None;
        if let Some(tasks) = self.generate.take() {
            hand(Some(&tasks), &self.owing, Generate::Stop);
        }
        self.segments = Segments::default();
        self.utterance_ended = false;
    }

    /// Takes every report there is now.
    fn take_all(&mut self, reports: &Receiver<Report>) {
        while let Ok(report) = reports.try_recv() {
            self.take(report);
            if self.ended {
                return;
            }
        }
    }

    fn take(&mut self, report: Report) {
        if report.turn == LISTENER {
            self.listened(report.heard);
            return;
        }
        if report.turn != self.turn || !self.answering {
            return;
        }
        match report.heard {
            Heard::Generated(event, failure) => {
                match self.segments.take(self.llm, event, failure) {
                    Segmented::Delta(None) | Segmented::Nothing => {}
                    Segmented::Delta(Some(note)) | Segmented::Note(note) => {
                        (self.note)(CascadeNote::Refused(note));
                    }
                    Segmented::Committed(segment) => {
                        (self.note)(CascadeNote::Said(segment.text().to_owned()));
                        hand(self.speak.as_ref(), &self.owing, Speak::Segment(segment));
                    }
                    Segmented::Ended(failure) => {
                        if let Some(failure) = failure {
                            (self.note)(CascadeNote::Refused(format!(
                                "the generation was cancelled: {failure}"
                            )));
                        }
                        self.end_utterance();
                    }
                }
            }
            Heard::Answered => self.end_utterance(),
            Heard::Spoken(SynthesisEvent::Audio(audio), _) => {
                let at = Instant::now();
                let start = self.cursor.map_or(at, |cursor| cursor.max(at));
                self.cursor = Some(start + audio.frame().duration());
                self.playout.play(audio.frame());
            }
            Heard::Synthesised => {
                self.cut();
                (self.note)(CascadeNote::Answered);
            }
            Heard::Note(note) => (self.note)(CascadeNote::Refused(note)),
            Heard::Spoken(..) | Heard::Transcript(_) | Heard::Recognised | Heard::Idle => {}
        }
    }

    /// Takes a report of the recognition: a committed transcript is a turn, the usage record
    /// its end.
    fn listened(&mut self, heard: Heard) {
        match heard {
            Heard::Transcript(TranscriptEvent::Committed(committed)) => {
                let text = committed.text.trim().to_owned();
                if text.is_empty() {
                    return;
                }
                (self.note)(CascadeNote::Heard(text.clone()));
                self.answer(text);
            }
            Heard::Transcript(TranscriptEvent::Usage(_)) | Heard::Recognised => {
                self.ended = true;
                (self.note)(CascadeNote::Ended);
            }
            Heard::Note(note) => (self.note)(CascadeNote::Refused(note)),
            _ => {}
        }
    }

    /// Starts the answer to `text`, after dropping the one in progress and waiting until its
    /// workers stopped, so the steps find their handles dropped.
    fn answer(&mut self, text: String) {
        if self.answering {
            self.playout.stop();
            self.cursor = None;
        }
        self.cut();
        for worker in self.workers.drain(..) {
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
        }
        self.epoch += 1;
        let request = request(
            &self.cascade.generator,
            self.run_id,
            self.llm,
            self.epoch,
            text,
        );
        match self.cascade.generator.start(request) {
            Ok(generation) => {
                let report = self.reporter(self.turn);
                let (tasks, worker) = spawn_generation(self.scope, generation, &self.owing, report);
                self.generate = Some(tasks);
                self.workers.push(worker);
            }
            Err(error) => {
                (self.note)(CascadeNote::Refused(format!(
                    "the generator refused the turn: {error}"
                )));
                return;
            }
        }
        match self.cascade.synthesiser.start() {
            Ok(synthesis) => {
                let report = self.reporter(self.turn);
                let (tasks, worker) = spawn_synthesis(self.scope, synthesis, report);
                self.speak = Some(tasks);
                self.workers.push(worker);
            }
            Err(error) => {
                (self.note)(CascadeNote::Refused(format!(
                    "the synthesiser refused the turn: {error}"
                )));
                self.cut();
                return;
            }
        }
        self.answering = true;
    }

    /// The answer is complete: the synthesis's utterance ends.
    fn end_utterance(&mut self) {
        if self.utterance_ended {
            return;
        }
        self.utterance_ended = true;
        hand(self.speak.as_ref(), &self.owing, Speak::End);
    }
}
