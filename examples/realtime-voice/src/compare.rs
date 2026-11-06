//! A comparison of native speech providers on the same recorded utterances: latency and usage.
//!
//! [`run`] opens one session of a model and holds two turns in it, sending input paced in real
//! time as a microphone would, one frame of [`Plan::frame`] each time that much audio has passed,
//! silence between the utterances:
//!
//! 1. The question, then silence until its answer ends: the latency turn.
//! 2. With [`Plan::barge_in`], after [`Plan::pause`], the question again, then the second
//!    utterance over its answer, timed so that its speech starts when the answer has played for
//!    [`Plan::barge_after`]: the barge-in turn.
//!
//! Every time is taken on the client side: the instant the session handed a duplex event over
//! ([`SpeechSessionHandle::next_event`] returned it), on the clock the caller gives. The duplex
//! events carry no provider timestamps. Three measures run from the end of input speech, the
//! instant the client sent the frame holding the question's last voiced sample:
//!
//! * first audio: the answer's first `OutputAudio`;
//! * first transcript: the answer's first `OutputTranscriptDelta`;
//! * response end: the answer's `ResponseCompleted`, `ResponseCancelled` or `ResponseFailed`.
//!
//! The answer is the first response created, or giving audio or transcript, after the end of
//! input speech.
//!
//! Barge-in to output stop runs from the instant the client sent the second utterance's first
//! voiced frame to the end of the answer's voiced audio as the client plays it: each frame from
//! its receipt or from the end of the frame before it, whichever is later, cut where the client
//! stops playout. The client stops playout as `Conversation` does, when the session reports that
//! speech started or that the answer was cancelled, and with `auto_interrupt` off it cancels the
//! answer itself. A model that stops on its own side has stopped when its voiced audio pauses for
//! [`Plan::quiet`] after the barge-in; an answer whose voiced audio had finished playing before
//! the barge-in is not measured, and the run says so.
//!
//! [`run_cascade`] runs the same plan over the other topology, a [`Cascade`] of a recogniser, an
//! LLM and a synthesiser, with the same turns, pace and clock. The cascade has no session to
//! report the end of speech: its detector is the arm's own, a frame being voiced as [`Plan`]
//! says. The recogniser takes the frames of a turn until [`Cascade::hold`] of silence follows
//! speech, then its input ends; its committed spans are the turn's text, asked of the generator
//! as user text; each committed segment of the answer is offered to the synthesiser as it
//! arrives, and each frame of the synthesis is handed to the [`Playout`] as it arrives. Each step
//! runs on a worker thread of its own and reports its events to the arm's loop over a channel;
//! the loop paces the input and reads each event on the caller's clock as it arrives, so a step
//! that blocks, an LLM working on its next segment or a hosted synthesiser speaking one, holds no
//! frame back. [`run_cascade_live`] runs on the wall clock and waits on the reports until the
//! next frame is due; [`run_cascade`] runs on a simulated clock, which it moves on once every
//! worker is idle or parked on a pending future, or once the workers owed a report for
//! [`SIMULATED_STALL`] of wall-clock time and so wait on that clock. Three measures of the cascade's own steps run
//! from the end of input speech:
//!
//! * final transcript: the recogniser's last committed span of the turn;
//! * first token: the answer's first text delta;
//! * TTS first audio: the synthesis's first frame.
//!
//! Its shared measures keep their names and are taken as a native model's are. The answer's audio
//! is played on the client's model, not on a device: each frame from its receipt or from the end
//! of the frame before it, whichever is later, cut where the client stops playout. First audio is
//! the start of that playout, so it is the TTS first audio: an answer starts only once the one
//! before it has played out, and no device adds its own delay. Response end is the synthesis's
//! last event. Barge-in to output stop runs from the barge-in's first voiced frame to the end of
//! the answer's audio as the client plays it; the client stops playout, calling
//! [`Playout::stop`], once [`Cascade::attack`] of voiced input opened a turn while the answer
//! still played, and the cut is the return of that call. Every frame of the synthesis counts as
//! the answer's voiced audio: the arm has no detector on its output. The cascade has no first
//! transcript of the answer apart from its first token, and a native model has none of the
//! cascade's steps: the table shows such a cell as `n/a`, never as 0. [`hosted_tts`] makes the
//! hosted synthesiser the `compare` example binds, or says why there is none.
//!
//! [`Comparison`] collects the runs of each provider, or why it was skipped, and prints the
//! median and the spread (lowest and highest) of each measure over the runs that measured it, and
//! the usage of each run as the provider reports it: tokens, and seconds for a provider that
//! bills by the second.

use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::fmt;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Scope, ScopedJoinHandle};
use std::time::{Duration, Instant};

use inference_audio_device::convert::{LinearResampler, decode, encode};
use inference_duplex::inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionEvent, SpeechSessionHandle,
};
use inference_elevenlabs::{
    CredentialSource, ELEVENLABS_KEY_NAME, ElevenLabsSynthesiser, Endpoint, OutputFormat,
    VoiceConfig,
};
use inference_llm::llm_core::{Item, TurnRequest};
use inference_llm::{GenerationEvent, GenerationHandle, GenerationRequest, Generator};
use inference_stt::inference_core::generation::{Generation, StartGeneration};
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_text::{OutputSegment, SegmentId, TurnId};
use inference_stt::{Recogniser, RecognitionHandle, RecognitionRequest, TranscriptEvent};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

use crate::{Ending, Note, Reported, Tally, Totals, Usage};

/// A recorded utterance: mono samples in `[-1, 1]` at a sample rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    samples: Vec<f32>,
    rate: NonZeroU32,
}

impl Clip {
    /// The clip of mono `samples` at `rate` hertz.
    #[must_use]
    pub const fn new(samples: Vec<f32>, rate: NonZeroU32) -> Self {
        Self { samples, rate }
    }

    /// The clip of a RIFF WAV file of 16-bit PCM, its channels averaged.
    ///
    /// # Errors
    ///
    /// [`ClipError`] for bytes that are not such a file.
    pub fn from_wav(bytes: &[u8]) -> Result<Self, ClipError> {
        if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err(ClipError("not a RIFF WAV file".to_owned()));
        }
        let mut format = None;
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let size =
                u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
                    as usize;
            let body = &bytes[at + 8..(at + 8).saturating_add(size).min(bytes.len())];
            match &bytes[at..at + 4] {
                b"fmt " if body.len() >= 16 => {
                    let tag = u16::from_le_bytes([body[0], body[1]]);
                    let channels = u16::from_le_bytes([body[2], body[3]]);
                    let rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                    let bits = u16::from_le_bytes([body[14], body[15]]);
                    if !matches!(tag, 1 | 0xFFFE) || bits != 16 {
                        return Err(ClipError(format!(
                            "not 16-bit PCM (format tag {tag}, {bits} bits)"
                        )));
                    }
                    let (Some(rate), true) = (NonZeroU32::new(rate), channels > 0) else {
                        return Err(ClipError("no sample rate or no channel".to_owned()));
                    };
                    format = Some((rate, usize::from(channels)));
                }
                b"data" => {
                    let (rate, channels) =
                        format.ok_or_else(|| ClipError("data before fmt".to_owned()))?;
                    let samples = body
                        .as_chunks::<2>()
                        .0
                        .chunks_exact(channels)
                        .map(|group| {
                            group
                                .iter()
                                .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32_768.0)
                                .sum::<f32>()
                                / channels as f32
                        })
                        .collect();
                    return Ok(Self { samples, rate });
                }
                _ => {}
            }
            at = at
                .saturating_add(8)
                .saturating_add(size)
                .saturating_add(size % 2);
        }
        Err(ClipError("no fmt and data chunks".to_owned()))
    }

    /// The length of the clip.
    #[must_use]
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples.len() as f64 / f64::from(self.rate.get()))
    }

    /// The clip resampled to `format`, in frames of `frame` (the last one shorter), each with
    /// whether a sample of it reaches `voiced`.
    fn frames(&self, format: AudioFormat, frame: Duration, voiced: f32) -> Vec<Input> {
        let mut resampled = Vec::new();
        LinearResampler::new(self.rate, format.sample_rate).process(&self.samples, &mut resampled);
        let per_frame = samples_in(frame, format.sample_rate).max(1);
        resampled
            .chunks(per_frame)
            .map(|chunk| Input {
                data: encode(chunk, format.encoding),
                samples: chunk.len() as u32,
                voiced: chunk.iter().any(|sample| sample.abs() >= voiced),
                mark: Mark::None,
            })
            .collect()
    }
}

/// Why a WAV file did not read as a clip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipError(String);

impl fmt::Display for ClipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for ClipError {}

fn samples_in(duration: Duration, rate: NonZeroU32) -> usize {
    (duration.as_nanos() * u128::from(rate.get()) / 1_000_000_000) as usize
}

/// What one run sends and how long it waits.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The question asked in both turns.
    pub question: Clip,
    /// The utterance sent over the second answer; `None` runs the latency turn alone.
    pub barge_in: Option<Clip>,
    /// The length of one input frame, and the pace of sending.
    pub frame: Duration,
    /// How long the second answer has played when the barge-in's first voiced frame is sent;
    /// the silence leading the clip is sent before it.
    pub barge_after: Duration,
    /// The silence between the first answer's playout and the second question.
    pub pause: Duration,
    /// The pause in voiced output after the barge-in that counts as stopped.
    pub quiet: Duration,
    /// The longest a turn waits for what it measures.
    pub limit: Duration,
    /// The longest the run waits for a response in progress to end before it closes.
    pub drain: Duration,
    /// The smallest absolute sample, on the scale of 1, of voiced audio, input and output.
    pub voiced: f32,
}

impl Plan {
    /// A plan asking `question`, without a barge-in: frames of 20 ms, the barge-in after 500 ms
    /// of the answer, a pause of 1 s, quiet after 300 ms, 20 s per turn, 3 s of draining, voiced
    /// from 0.02.
    #[must_use]
    pub const fn new(question: Clip) -> Self {
        Self {
            question,
            barge_in: None,
            frame: Duration::from_millis(20),
            barge_after: Duration::from_millis(500),
            pause: Duration::from_secs(1),
            quiet: Duration::from_millis(300),
            limit: Duration::from_secs(20),
            drain: Duration::from_secs(3),
            voiced: 0.02,
        }
    }
}

/// How a provider bills: the token counts it reports, and its billed seconds, read from the
/// session once it is closed, for a provider that bills by the second.
#[derive(Debug, Clone, Copy)]
pub struct Billing<S> {
    pub reported: Reported,
    pub seconds: fn(&S) -> Option<f64>,
}

impl<S> Billing<S> {
    /// A provider billing tokens only, those of `reported`.
    #[must_use]
    pub fn tokens(reported: Reported) -> Self {
        Self {
            reported,
            seconds: |_| None,
        }
    }
}

/// How a provider answers speech: a native speech model, or a cascade of STT, LLM and TTS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Topology {
    /// One model hears speech and speaks its answer.
    #[default]
    Native,
    /// A recogniser, a text model and a synthesiser, one after the other.
    Cascade,
}

impl Topology {
    /// The topology's name in the table.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Cascade => "cascade",
        }
    }

    /// The topology named `name` in the table.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        [Self::Native, Self::Cascade]
            .into_iter()
            .find(|topology| topology.name() == name)
    }
}

/// What one run measured. A measure is `None` when the run did not reach it or its topology has
/// no such step; `notes` say why a measure it has is missing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Measured {
    /// The topology of the provider the run measured.
    pub topology: Topology,
    /// End of input speech to the answer's first audio.
    pub first_audio: Option<Duration>,
    /// End of input speech to the answer's first transcript.
    pub first_transcript: Option<Duration>,
    /// End of input speech to the answer's end.
    pub response_end: Option<Duration>,
    /// The barge-in's first voiced frame sent to the end of the answer's playout.
    pub barge_in_stop: Option<Duration>,
    /// End of input speech to the recogniser's final transcript, for a cascade.
    pub final_transcript: Option<Duration>,
    /// End of input speech to the LLM's first token, for a cascade.
    pub first_token: Option<Duration>,
    /// End of input speech to the TTS's first audio, for a cascade.
    pub tts_first_audio: Option<Duration>,
    /// The usage the session reported over the run.
    pub usage: Totals,
    /// The seconds the provider billed, for one that bills by the second.
    pub seconds: Option<f64>,
    /// What went wrong, and why a measure is missing.
    pub notes: Vec<String>,
}

/// One frame of input, with what the run reads off its sending.
#[derive(Debug, Clone)]
struct Input {
    data: Vec<u8>,
    samples: u32,
    voiced: bool,
    mark: Mark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    None,
    /// The frame holds the question's last voiced sample.
    QuestionEnd,
    /// The frame holds the barge-in's first voiced sample.
    BargeStart,
}

/// The answer a turn measures.
#[derive(Debug, Default)]
struct Answer {
    speech_end: Option<Instant>,
    id: Option<ResponseId>,
    first_audio: Option<Instant>,
    first_transcript: Option<Instant>,
    ended: Option<Instant>,
    /// Where the client's playout of the answer's audio received so far ends.
    cursor: Option<Instant>,
    /// The voiced stretches of that playout.
    voiced: Vec<(Instant, Instant)>,
    /// When the client stopped playout for the barge-in.
    stopped: Option<Instant>,
}

impl Answer {
    /// Whether an event of `response_id` received at `at` is of this answer; the first response
    /// heard of after the end of input speech becomes it.
    fn claim(&mut self, response_id: ResponseId, at: Instant) -> bool {
        if self.speech_end.is_none_or(|end| at < end) {
            return false;
        }
        *self.id.get_or_insert(response_id) == response_id
    }

    /// Plays `frame`, received at `at`, after what was received before.
    fn play(&mut self, frame: &AudioFrame, voiced: bool, at: Instant) {
        let start = self.cursor.map_or(at, |cursor| cursor.max(at));
        let end = start + frame.duration();
        self.cursor = Some(end);
        if voiced {
            match self.voiced.last_mut() {
                Some(last) if last.1 >= start => last.1 = end,
                _ => self.voiced.push((start, end)),
            }
        }
    }

    fn since_speech_end(&self, at: Option<Instant>) -> Option<Duration> {
        Some(at?.saturating_duration_since(self.speech_end?))
    }

    /// Where playout stopped after the barge-in sent at `barge`, once that is decided at `now`:
    /// the end of the first voiced stretch ending at or after `barge` that is followed by
    /// `quiet`, by the client's stop, or by the answer's end. A stretch still to play is not
    /// decided while the client may yet stop it.
    fn stop(
        &self,
        barge: Instant,
        quiet: Duration,
        now: Instant,
    ) -> Option<Result<Instant, String>> {
        let cut = self.stopped;
        let voiced: Vec<(Instant, Instant)> = self
            .voiced
            .iter()
            .filter(|(start, _)| cut.is_none_or(|cut| *start < cut))
            .map(|&(start, end)| (start, cut.map_or(end, |cut| end.min(cut))))
            .collect();
        let final_audio = cut.is_some() || self.ended.is_some();
        for (at, &(_, end)) in voiced.iter().enumerate() {
            if end < barge {
                continue;
            }
            if cut.is_none() && end > now {
                return None;
            }
            let quiet_after = match voiced.get(at + 1) {
                Some(&(next, _)) => next.saturating_duration_since(end) >= quiet,
                None => final_audio || now.saturating_duration_since(end) >= quiet,
            };
            if quiet_after {
                return Some(Ok(end));
            }
        }
        let all_before = voiced.iter().all(|&(_, end)| end < barge);
        let after = |at: Instant| {
            self.first_audio
                .map_or(0, |first| at.saturating_duration_since(first).as_millis())
        };
        (final_audio && all_before).then(|| {
            Err(match voiced.last() {
                Some(&(_, end)) => format!(
                    "the answer's voiced audio had finished playing before the barge-in: it \
                     ended {} ms after its first audio, the barge-in's speech started {} ms \
                     after it",
                    after(end),
                    after(barge)
                ),
                None => "the answer had no voiced audio".to_owned(),
            })
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Latency,
    Pause(Instant),
    BargeIn,
    Drain(Instant),
    Done,
}

struct Runner<'a, S> {
    session: S,
    plan: &'a Plan,
    clock: &'a dyn Fn() -> Instant,
    format: AudioFormat,
    question: Vec<Input>,
    barge_in: Option<Vec<Input>>,
    /// The silence the barge-in clip sends before its first voiced frame.
    barge_lead: Duration,
    cancel_on_speech: bool,
    queue: VecDeque<Input>,
    position: u64,
    phase: Phase,
    turn_started: Instant,
    latency: Answer,
    barge: Answer,
    barge_queued: bool,
    barge_start: Option<Instant>,
    tally: Tally,
    measured: Measured,
}

/// The frames of the plan's question and barge-in in `format`, the question's last voiced frame
/// and the barge-in's first marked, and the silence the barge-in sends before its first voiced
/// frame.
fn inputs(plan: &Plan, format: AudioFormat) -> (Vec<Input>, Option<Vec<Input>>, Duration) {
    let mut question = plan.question.frames(format, plan.frame, plan.voiced);
    if let Some(last) = question.iter_mut().rev().find(|input| input.voiced) {
        last.mark = Mark::QuestionEnd;
    }
    let mut barge_lead = Duration::ZERO;
    let barge_in = plan.barge_in.as_ref().map(|clip| {
        let mut frames = clip.frames(format, plan.frame, plan.voiced);
        if let Some(lead) = frames.iter().position(|input| input.voiced) {
            frames[lead].mark = Mark::BargeStart;
            barge_lead = plan.frame * u32::try_from(lead).unwrap_or(u32::MAX);
        }
        frames
    });
    (question, barge_in, barge_lead)
}

/// Runs the plan once over a new session of `model` opened with `config`, on `clock`, and
/// returns what it measured and the usage the session reported, `billing` saying how to read it.
///
/// # Errors
///
/// [`OpenError`] when the session does not open; once it is open, every failure is a note of
/// the run.
pub fn run<M: NativeSpeechModel>(
    model: &mut M,
    config: SessionConfig,
    plan: &Plan,
    clock: &dyn Fn() -> Instant,
    billing: &Billing<M::Session>,
) -> Result<Measured, OpenError> {
    let format = config.input_format;
    let cancel_on_speech = !config.auto_interrupt && model.capabilities().client_cancel;
    let (question, barge_in, barge_lead) = inputs(plan, format);
    let session = model.open(config)?;
    let now = clock();
    let mut runner = Runner {
        session,
        plan,
        clock,
        format,
        queue: question.iter().cloned().collect(),
        question,
        barge_in,
        barge_lead,
        cancel_on_speech,
        position: 0,
        phase: Phase::Latency,
        turn_started: now,
        latency: Answer::default(),
        barge: Answer::default(),
        barge_queued: false,
        barge_start: None,
        tally: Tally::new(billing.reported),
        measured: Measured::default(),
    };
    runner.drive(now);
    runner.close();
    let mut measured = runner.measured;
    measured.first_audio = runner.latency.since_speech_end(runner.latency.first_audio);
    measured.first_transcript = runner
        .latency
        .since_speech_end(runner.latency.first_transcript);
    measured.response_end = runner.latency.since_speech_end(runner.latency.ended);
    measured.usage = runner.tally.totals();
    measured.seconds = (billing.seconds)(&runner.session);
    Ok(measured)
}

impl<S: SpeechSessionHandle> Runner<'_, S> {
    fn drive(&mut self, start: Instant) {
        let mut next_send = start + self.plan.frame;
        loop {
            let now = (self.clock)();
            self.decide(now);
            if self.phase == Phase::Done {
                return;
            }
            if now >= next_send {
                self.send(now);
                next_send += self.plan.frame;
                continue;
            }
            if let Some(event) = self.session.next_event(next_send - now) {
                let at = (self.clock)();
                self.handle(event, at);
            }
        }
    }

    fn note(&mut self, note: String) {
        self.measured.notes.push(note);
    }

    /// Moves the run on once what it waits for is there.
    fn decide(&mut self, now: Instant) {
        let limit = self.plan.limit;
        let late = now.saturating_duration_since(self.turn_started) > limit;
        match self.phase {
            Phase::Latency => {
                if let Some(ended) = self.latency.ended {
                    let played = self
                        .latency
                        .cursor
                        .map_or(ended, |cursor| cursor.max(ended));
                    self.phase = if self.barge_in.is_some() {
                        Phase::Pause(played + self.plan.pause)
                    } else {
                        Phase::Drain(now + self.plan.drain)
                    };
                } else if late {
                    self.note(format!("no end of the answer within {limit:?}"));
                    self.phase = Phase::Drain(now + self.plan.drain);
                }
            }
            Phase::Pause(until) => {
                if now >= until {
                    self.queue.extend(self.question.iter().cloned());
                    self.turn_started = now;
                    self.phase = Phase::BargeIn;
                }
            }
            Phase::BargeIn => self.decide_barge_in(now, late),
            Phase::Drain(until) => {
                if self.session.ledger().in_progress().is_none() || now >= until {
                    self.phase = Phase::Done;
                }
            }
            Phase::Done => {}
        }
    }

    fn decide_barge_in(&mut self, now: Instant, late: bool) {
        if !self.barge_queued
            && let Some(first) = self.barge.first_audio
            && now + self.barge_lead >= first + self.plan.barge_after
            && let Some(frames) = &self.barge_in
        {
            self.queue.extend(frames.iter().cloned());
            self.barge_queued = true;
        }
        if let Some(barge) = self.barge_start {
            match self.barge.stop(barge, self.plan.quiet, now) {
                Some(Ok(stop)) => {
                    self.measured.barge_in_stop = Some(stop.saturating_duration_since(barge));
                    self.phase = Phase::Drain(now + self.plan.drain);
                    return;
                }
                Some(Err(reason)) => {
                    self.note(format!("barge-in: {reason}"));
                    self.phase = Phase::Drain(now + self.plan.drain);
                    return;
                }
                None => {}
            }
        }
        if late {
            let limit = self.plan.limit;
            self.note(format!("barge-in: not measured within {limit:?}"));
            self.phase = Phase::Drain(now + self.plan.drain);
        }
    }

    /// Sends the next frame of input, silence when none is queued.
    fn send(&mut self, now: Instant) {
        let input = self.queue.pop_front().unwrap_or_else(|| {
            let samples = samples_in(self.plan.frame, self.format.sample_rate).max(1);
            Input {
                data: encode(&vec![0.0; samples], self.format.encoding),
                samples: samples as u32,
                voiced: false,
                mark: Mark::None,
            }
        });
        let Some(position) = SampleClockPosition::new(self.position) else {
            self.phase = Phase::Done;
            return;
        };
        let frame = AudioFrame {
            format: self.format,
            position,
            sample_count: input.samples,
            data: input.data,
        };
        if let Err(error) = self.session.append_audio(frame) {
            self.note(format!("input audio refused: {error}"));
            self.phase = Phase::Done;
            return;
        }
        self.position += u64::from(input.samples);
        match input.mark {
            Mark::QuestionEnd => match self.phase {
                Phase::Latency => self.latency.speech_end = Some(now),
                Phase::BargeIn => self.barge.speech_end = Some(now),
                _ => {}
            },
            Mark::BargeStart => self.barge_start = Some(now),
            Mark::None => {}
        }
    }

    /// The answer the current turn measures.
    const fn answer(&mut self) -> Option<&mut Answer> {
        match self.phase {
            Phase::Latency => Some(&mut self.latency),
            Phase::BargeIn => Some(&mut self.barge),
            _ => None,
        }
    }

    fn handle(&mut self, event: SessionEvent, at: Instant) {
        let voiced = self.plan.voiced;
        let barging = self.barge_start.is_some();
        match event {
            SessionEvent::ResponseCreated(created) => {
                if let Some(answer) = self.answer() {
                    answer.claim(created.response_id, at);
                }
            }
            SessionEvent::OutputAudio(audio) => {
                if let Some(answer) = self.answer()
                    && answer.claim(audio.response_id, at)
                {
                    answer.first_audio.get_or_insert(at);
                    let peak = decode(&audio.frame)
                        .map(|samples| samples.iter().any(|sample| sample.abs() >= voiced))
                        .unwrap_or(false);
                    answer.play(&audio.frame, peak, at);
                }
            }
            SessionEvent::OutputTranscriptDelta(delta) => {
                if let Some(answer) = self.answer()
                    && answer.claim(delta.response_id, at)
                {
                    answer.first_transcript.get_or_insert(at);
                }
            }
            SessionEvent::ResponseCompleted(done) => {
                self.ended(done.response_id, Ending::Completed, at)
            }
            SessionEvent::ResponseCancelled(cancelled) => {
                if barging && self.barge.id == Some(cancelled.response_id) {
                    self.barge.stopped.get_or_insert(at);
                }
                self.ended(cancelled.response_id, Ending::Cancelled, at);
            }
            SessionEvent::ResponseFailed(failed) => {
                self.ended(failed.response_id, Ending::Failed(failed.failure), at);
            }
            SessionEvent::SpeechStarted(_) => {
                if barging && self.phase == Phase::BargeIn {
                    self.barge.stopped.get_or_insert(at);
                    if self.cancel_on_speech
                        && let Some(id) = self.barge.id
                        && self.session.ledger().in_progress() == Some(id)
                        && let Err(error) = self.session.cancel_response(id)
                    {
                        self.note(format!("cancel refused: {error}"));
                    }
                }
            }
            SessionEvent::ResponseUsageRecorded(recorded) => {
                self.tally.record(&Note::Usage(Usage::Response {
                    response_id: recorded.response_id,
                    usage: recorded.usage,
                }));
            }
            SessionEvent::TranscriptionUsageRecorded(recorded) => {
                self.tally
                    .record(&Note::Usage(Usage::Transcription(recorded.usage)));
            }
            SessionEvent::InputTranscribed(input) => {
                self.tally.record(&Note::Heard(input.text));
            }
            SessionEvent::SpeechSessionFailed(failed) => {
                self.note(format!("the session failed: {}", failed.failure));
                self.phase = Phase::Done;
            }
            SessionEvent::SpeechSessionClosed(_) => {
                self.note("the session closed".to_owned());
                self.phase = Phase::Done;
            }
            _ => {}
        }
    }

    fn ended(&mut self, response_id: ResponseId, how: Ending, at: Instant) {
        for answer in [&mut self.latency, &mut self.barge] {
            if answer.id == Some(response_id) {
                answer.ended.get_or_insert(at);
            }
        }
        self.tally.record(&Note::Ended { response_id, how });
    }

    /// Closes the session and takes in what it reports last.
    fn close(&mut self) {
        if self.session.ledger().session().require_open().is_ok()
            && let Err(error) = self.session.close()
        {
            self.note(format!("close refused: {error}"));
        }
        while let Some(event) = self.session.next_event(Duration::ZERO) {
            match event {
                SessionEvent::ResponseUsageRecorded(_)
                | SessionEvent::TranscriptionUsageRecorded(_)
                | SessionEvent::ResponseCompleted(_)
                | SessionEvent::ResponseCancelled(_)
                | SessionEvent::ResponseFailed(_) => {
                    let at = (self.clock)();
                    self.handle(event, at);
                }
                _ => {}
            }
        }
    }
}

/// The bindings of a cascade arm: its recogniser, its LLM and its synthesiser, the format the
/// recogniser is offered input in, and the arm's detector.
#[derive(Debug)]
pub struct Cascade<R, S> {
    pub recogniser: R,
    pub generator: Generator,
    pub synthesiser: S,
    /// The format the recogniser is offered input in.
    pub input_format: AudioFormat,
    /// The voiced input that opens a turn: at a barge-in, the playout stops once this much of
    /// the barge-in's voiced input was sent.
    pub attack: Duration,
    /// The silence after speech that ends the recogniser's input.
    pub hold: Duration,
}

impl<R, S> Cascade<R, S> {
    /// The cascade of `recogniser`, `generator` and `synthesiser`, the recogniser offered
    /// `input_format`, its detector's attack and hold those of the local voice cascade,
    /// [`local_voice_cascade::VAD`].
    #[must_use]
    pub const fn new(
        recogniser: R,
        generator: Generator,
        synthesiser: S,
        input_format: AudioFormat,
    ) -> Self {
        Self {
            recogniser,
            generator,
            synthesiser,
            input_format,
            attack: local_voice_cascade::VAD.attack,
            hold: local_voice_cascade::VAD.hold,
        }
    }
}

/// Where a cascade arm hands the answer's audio. The arm times the audio on the client's model of
/// playout, whatever the playout does with it; only the return of [`Playout::stop`] is read.
pub trait Playout {
    /// Plays `frame` after the frames before it.
    fn play(&mut self, frame: &AudioFrame);

    /// Stops playout and drops what is queued; it has stopped once the call returns.
    fn stop(&mut self);
}

/// The playout of a client without a speaker: nothing is played, and the arm's model of the
/// client's playout is the whole of it.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientPlayout;

impl Playout for ClientPlayout {
    fn play(&mut self, _frame: &AudioFrame) {}

    fn stop(&mut self) {}
}

/// The reason, and the start of every reason, a cascade arm without a hosted synthesiser is
/// skipped for.
pub const NO_HOSTED_TTS: &str = "no hosted TTS configured";

/// The variable naming the ElevenLabs voice the cascade arm speaks with.
pub const VOICE_VARIABLE: &str = "ELEVENLABS_VOICE_ID";

/// The ElevenLabs synthesiser of `voice` in `format` at `endpoint`, its key resolved through
/// `source` under `ELEVENLABS_KEY_NAME`, when that key resolves at once. Making it sends
/// nothing.
///
/// # Errors
///
/// A reason beginning with [`NO_HOSTED_TTS`]: the key did not resolve, no voice is named, or the
/// configuration was refused.
pub fn hosted_tts(
    endpoint: Endpoint,
    source: Arc<dyn CredentialSource>,
    voice: Option<&str>,
    format: OutputFormat,
) -> Result<ElevenLabsSynthesiser, String> {
    let resolved = {
        let mut future = source.resolve(ELEVENLABS_KEY_NAME);
        let mut context = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut context) {
            Poll::Ready(Ok(_)) => Ok(()),
            Poll::Ready(Err(error)) => Err(format!("the ElevenLabs key did not resolve: {error}")),
            Poll::Pending => Err("the ElevenLabs key did not resolve at once".to_owned()),
        }
    };
    resolved.map_err(|why| format!("{NO_HOSTED_TTS}: {why}"))?;
    let voice = voice
        .map(str::trim)
        .filter(|voice| !voice.is_empty())
        .ok_or_else(|| format!("{NO_HOSTED_TTS}: {VOICE_VARIABLE} names no voice"))?;
    ElevenLabsSynthesiser::new(
        endpoint,
        VoiceConfig::new(voice, format),
        source,
        ELEVENLABS_KEY_NAME,
    )
    .map_err(|error| format!("{NO_HOSTED_TTS}: {error}"))
}

/// What a cascade arm measured of one turn, each an instant on the caller's clock.
#[derive(Debug, Default)]
struct Steps {
    speech_end: Option<Instant>,
    final_transcript: Option<Instant>,
    first_token: Option<Instant>,
    /// The synthesis's first frame, and so the start of the client's playout.
    tts_first_audio: Option<Instant>,
    /// The synthesis's last event.
    ended: Option<Instant>,
    /// Where the client's playout of the answer's audio received so far ends.
    cursor: Option<Instant>,
    /// Whether the turn is over: answered, refused or failed.
    done: bool,
}

impl Steps {
    fn since_speech_end(&self, at: Option<Instant>) -> Option<Duration> {
        Some(at?.saturating_duration_since(self.speech_end?))
    }
}

/// Where a cascade arm's turn stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// No step runs.
    Idle,
    /// The recogniser takes the turn's input.
    Listening,
    /// The recogniser's input ended; its last events are awaited.
    Recognising,
    /// The generator answers and the synthesiser speaks the answer.
    Answering,
}

/// What a worker of a cascade arm hands the arm's loop.
enum Heard {
    /// An event of the recognition.
    Transcript(TranscriptEvent),
    /// The recognition delivered its last event after the end of its input.
    Recognised,
    /// An event of the generation, with the generation's failure when it was cancelled.
    Generated(GenerationEvent, Option<String>),
    /// The generation delivered its last event.
    Answered,
    /// An event of the synthesis.
    Spoken(SynthesisEvent),
    /// The synthesis delivered its last event after the end of the utterance.
    Synthesised,
    /// A step refused what it was given.
    Note(String),
    /// No worker owes the loop a report.
    Idle,
}

/// What a worker hands the loop, and the turn it belongs to: a report of a turn that was cut is
/// dropped.
struct Report {
    turn: u64,
    heard: Heard,
}

/// What the workers of a cascade arm owe its loop: one count for each task handed to a worker and
/// each wake of the generation, held until the worker reported what that task produced.
struct Owing {
    count: AtomicUsize,
    reports: Sender<Report>,
}

/// One count of [`Owing`], released when dropped: by the worker once it reported what its task
/// produced, or with a task that never reached a worker. The release of the last count reports
/// [`Heard::Idle`], so a loop waiting on its workers wakes.
struct Owed(Arc<Owing>);

impl Owed {
    fn new(owing: &Arc<Owing>) -> Self {
        owing.count.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(owing))
    }
}

impl Drop for Owed {
    fn drop(&mut self) {
        if self.0.count.fetch_sub(1, Ordering::SeqCst) == 1 {
            let _ = self.0.reports.send(Report {
                turn: 0,
                heard: Heard::Idle,
            });
        }
    }
}

/// A task handed to a worker, and the count it holds until the worker is done with it.
struct Task<T> {
    work: T,
    owed: Owed,
}

/// Hands `work` to the worker `tasks` feeds, if there is one.
fn hand<T>(tasks: Option<&Sender<Task<T>>>, owing: &Arc<Owing>, work: T) {
    if let Some(tasks) = tasks {
        let _ = tasks.send(Task {
            work,
            owed: Owed::new(owing),
        });
    }
}

/// What the recogniser's worker is asked.
enum Listen {
    Frame(AudioFrame),
    End,
}

/// What the generator's worker is asked: to poll the generation, or to drop it.
enum Generate {
    Poll,
    Stop,
}

/// What the synthesiser's worker is asked.
enum Speak {
    Segment(OutputSegment),
    End,
}

/// The waker of the generator's worker: a wake hands the worker a [`Generate::Poll`], whose count
/// keeps a simulated clock still until the worker polled.
struct Prod {
    tasks: Sender<Task<Generate>>,
    owing: Arc<Owing>,
}

impl Wake for Prod {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        hand(Some(&self.tasks), &self.owing, Generate::Poll);
    }
}

/// The recogniser's worker: offers each frame, ends the input, and reports every event the
/// recognition has after each.
fn listening<H: RecognitionHandle>(
    mut recognition: H,
    tasks: Receiver<Task<Listen>>,
    report: impl Fn(Heard),
) {
    for Task { work, owed } in tasks {
        let ended = match work {
            Listen::Frame(frame) => {
                if let Err(error) = recognition.offer_frame(frame) {
                    report(Heard::Note(format!(
                        "the recogniser refused input: {error}"
                    )));
                }
                false
            }
            Listen::End => {
                if let Err(error) = recognition.end_input() {
                    report(Heard::Note(format!(
                        "the recogniser's input did not end: {error}"
                    )));
                }
                true
            }
        };
        for event in recognition.by_ref() {
            report(Heard::Transcript(event.payload));
        }
        if ended {
            report(Heard::Recognised);
        }
        drop(owed);
    }
}

/// The generator's worker: polls the generation at each wake and reports every event it is ready
/// with, until it is told to stop; a pending poll leaves the worker parked.
fn generating(
    mut generation: GenerationHandle,
    tasks: &Receiver<Task<Generate>>,
    waker: &Waker,
    report: impl Fn(Heard),
) {
    let mut context = Context::from_waker(waker);
    let mut ended = false;
    for Task { work, owed } in tasks {
        if matches!(work, Generate::Stop) {
            return;
        }
        while !ended {
            match generation.poll_event(&mut context) {
                Poll::Ready(Some(event)) => {
                    let failure = matches!(event, GenerationEvent::Cancelled(..)).then(|| {
                        generation.failure().map_or_else(
                            || "no failure given".to_owned(),
                            |error| format!("{error:?}"),
                        )
                    });
                    report(Heard::Generated(event, failure));
                }
                Poll::Ready(None) => {
                    ended = true;
                    report(Heard::Answered);
                }
                Poll::Pending => break,
            }
        }
        drop(owed);
    }
}

/// The synthesiser's worker: offers each segment, ends the utterance, and reports every event the
/// synthesis has after each.
fn speaking<H: SynthesisHandle>(
    mut synthesis: H,
    tasks: Receiver<Task<Speak>>,
    report: impl Fn(Heard),
) {
    for Task { work, owed } in tasks {
        let ended = match work {
            Speak::Segment(segment) => {
                if let Err(refused) = synthesis.offer_segment(segment) {
                    report(Heard::Note(format!(
                        "the synthesiser refused a segment: {refused:?}"
                    )));
                }
                false
            }
            Speak::End => {
                if let Err(error) = synthesis.end_utterance() {
                    report(Heard::Note(format!(
                        "the synthesis's utterance did not end: {error}"
                    )));
                }
                true
            }
        };
        for event in synthesis.by_ref() {
            report(Heard::Spoken(event));
        }
        if ended {
            report(Heard::Synthesised);
        }
        drop(owed);
    }
}

struct CascadeRunner<'a, 's, 'e, R: Recogniser, S: Synthesiser> {
    cascade: &'a mut Cascade<R, S>,
    plan: &'a Plan,
    clock: &'a dyn Fn() -> Instant,
    /// The simulated clock's advance; none on the wall clock.
    advance: Option<&'a dyn Fn(Duration)>,
    playout: &'a mut dyn Playout,
    scope: &'s Scope<'s, 'e>,
    reports: Receiver<Report>,
    sender: Sender<Report>,
    owing: Arc<Owing>,
    /// The turn the workers that run belong to.
    turn: u64,
    listen: Option<Sender<Task<Listen>>>,
    generate: Option<Sender<Task<Generate>>>,
    speak: Option<Sender<Task<Speak>>>,
    workers: Vec<ScopedJoinHandle<'s, ()>>,
    run_id: RunId,
    stt: NodeId,
    llm: NodeId,
    epoch: i64,
    question: Vec<Input>,
    barge_in: Option<Vec<Input>>,
    barge_lead: Duration,
    queue: VecDeque<Input>,
    position: u64,
    phase: Phase,
    turn_started: Instant,
    latency: Steps,
    barge: Steps,
    barge_queued: bool,
    barge_start: Option<Instant>,
    /// The barge-in's voiced input sent so far.
    barge_voiced: Duration,
    stage: Stage,
    heard: Vec<String>,
    /// Whether the turn's input was voiced yet, and the silence sent since its last voiced frame.
    voiced: bool,
    silent_for: Duration,
    record: Option<Generation>,
    tentative: HashMap<SegmentId, OutputSegment>,
    utterance_ended: bool,
    measured: Measured,
}

/// The wall-clock time a simulated run waits for a report its workers owe before it takes them
/// as waiting on its clock and moves the clock on to the next frame.
pub const SIMULATED_STALL: Duration = Duration::from_millis(250);

/// Runs the plan once over `cascade` on a simulated clock: `clock` is read, and `wait` moves it
/// on until the next frame is due. It is called once every worker is idle or parked on a pending
/// future, or when the workers owe a report and none came for [`SIMULATED_STALL`] of wall-clock
/// time: a step that blocks until the caller's clock reaches a time, or a future that wakes
/// itself until then, waits on the clock, and the clock then moves one frame per stall. A step
/// that computes for longer than that on the wall clock sees the simulated clock move meanwhile.
/// The answer's audio is played on `playout`. Returns what it measured; see the module
/// documentation. Every failure is a note of the run; a panic of a step or of the caller's
/// playout, clock or wait reaches the caller once every worker stopped.
pub fn run_cascade<R, S>(
    cascade: &mut Cascade<R, S>,
    plan: &Plan,
    clock: &dyn Fn() -> Instant,
    wait: &dyn Fn(Duration),
    playout: &mut dyn Playout,
) -> Measured
where
    R: Recogniser,
    R::Handle: Send,
    S: Synthesiser,
    S::Handle: Send,
{
    run_paced(cascade, plan, clock, Some(wait), playout)
}

/// Runs the plan once over `cascade` on the wall clock, waiting on the workers' reports until the
/// next frame is due, and playing the answer's audio on `playout`. Returns what it measured; see
/// the module documentation. Every failure is a note of the run.
pub fn run_cascade_live<R, S>(
    cascade: &mut Cascade<R, S>,
    plan: &Plan,
    playout: &mut dyn Playout,
) -> Measured
where
    R: Recogniser,
    R::Handle: Send,
    S: Synthesiser,
    S::Handle: Send,
{
    run_paced(cascade, plan, &Instant::now, None, playout)
}

fn run_paced<R, S>(
    cascade: &mut Cascade<R, S>,
    plan: &Plan,
    clock: &dyn Fn() -> Instant,
    advance: Option<&dyn Fn(Duration)>,
    playout: &mut dyn Playout,
) -> Measured
where
    R: Recogniser,
    R::Handle: Send,
    S: Synthesiser,
    S::Handle: Send,
{
    let (question, barge_in, barge_lead) = inputs(plan, cascade.input_format);
    let (sender, reports) = mpsc::channel();
    let owing = Arc::new(Owing {
        count: AtomicUsize::new(0),
        reports: sender.clone(),
    });
    let now = clock();
    thread::scope(|scope| {
        let mut runner = CascadeRunner {
            cascade,
            plan,
            clock,
            advance,
            playout,
            scope,
            reports,
            sender,
            owing,
            turn: 0,
            listen: None,
            generate: None,
            speak: None,
            workers: Vec::new(),
            run_id: RunId::generate(),
            stt: NodeId::generate(),
            llm: NodeId::generate(),
            epoch: 0,
            queue: question.iter().cloned().collect(),
            question,
            barge_in,
            barge_lead,
            position: 0,
            phase: Phase::Latency,
            turn_started: now,
            latency: Steps::default(),
            barge: Steps::default(),
            barge_queued: false,
            barge_start: None,
            barge_voiced: Duration::ZERO,
            stage: Stage::Idle,
            heard: Vec::new(),
            voiced: false,
            silent_for: Duration::ZERO,
            record: None,
            tentative: HashMap::new(),
            utterance_ended: false,
            measured: Measured {
                topology: Topology::Cascade,
                ..Measured::default()
            },
        };
        runner.listen();
        runner.drive(now);
        runner.cut();
        let latency = &runner.latency;
        let mut measured = std::mem::take(&mut runner.measured);
        measured.first_audio = latency.since_speech_end(latency.tts_first_audio);
        measured.response_end = latency.since_speech_end(latency.ended);
        measured.final_transcript = latency.since_speech_end(latency.final_transcript);
        measured.first_token = latency.since_speech_end(latency.first_token);
        measured.tts_first_audio = latency.since_speech_end(latency.tts_first_audio);
        measured
    })
}

/// A runner dropped without its last `cut`, as when the caller's playout, clock or wait panics
/// on the arm's loop, tells the generator's worker to stop; the other workers end as their task
/// senders drop. The arm's thread scope then joins every worker, and the panic reaches the
/// caller instead of leaving the scope waiting on a worker its own waker keeps alive.
impl<R: Recogniser, S: Synthesiser> Drop for CascadeRunner<'_, '_, '_, R, S> {
    fn drop(&mut self) {
        if let Some(tasks) = self.generate.take() {
            hand(Some(&tasks), &self.owing, Generate::Stop);
        }
    }
}

impl<'s, R, S> CascadeRunner<'_, 's, '_, R, S>
where
    R: Recogniser,
    R::Handle: Send + 's,
    S: Synthesiser,
    S::Handle: Send + 's,
{
    /// Sends a frame each time one is due and takes the workers' reports as they arrive; on a
    /// simulated clock, moves the clock on only once no worker owes a report.
    fn drive(&mut self, start: Instant) {
        let mut next_send = start + self.plan.frame;
        loop {
            let now = (self.clock)();
            self.decide(now);
            if self.phase == Phase::Done {
                return;
            }
            if now >= next_send {
                self.send(now);
                next_send += self.plan.frame;
                continue;
            }
            if let Ok(report) = self.reports.try_recv() {
                self.take(report);
                continue;
            }
            let until = next_send - now;
            let report = match self.advance {
                None => self.reports.recv_timeout(until).ok(),
                Some(advance) if self.owing.count.load(Ordering::SeqCst) == 0 => {
                    let report = self.reports.try_recv().ok();
                    if report.is_none() {
                        advance(until);
                    }
                    report
                }
                Some(advance) => match self.reports.recv_timeout(SIMULATED_STALL) {
                    Ok(report) => Some(report),
                    Err(_) => {
                        advance(until);
                        None
                    }
                },
            };
            if let Some(report) = report {
                self.take(report);
            }
        }
    }

    fn note(&mut self, note: String) {
        self.measured.notes.push(note);
    }

    /// The turn the run is in.
    const fn steps(&mut self) -> Option<&mut Steps> {
        match self.phase {
            Phase::Latency => Some(&mut self.latency),
            Phase::BargeIn => Some(&mut self.barge),
            _ => None,
        }
    }

    /// The reporter of a worker of the turn that runs.
    fn reporter(&self) -> impl Fn(Heard) + Send + 'static {
        let reports = self.sender.clone();
        let turn = self.turn;
        move |heard| {
            let _ = reports.send(Report { turn, heard });
        }
    }

    /// Moves the run on once what it waits for is there.
    fn decide(&mut self, now: Instant) {
        let limit = self.plan.limit;
        let late = now.saturating_duration_since(self.turn_started) > limit;
        match self.phase {
            Phase::Latency => {
                if self.latency.done {
                    self.phase = match (self.latency.ended, &self.barge_in) {
                        (Some(ended), Some(_)) => {
                            let played = self
                                .latency
                                .cursor
                                .map_or(ended, |cursor| cursor.max(ended));
                            Phase::Pause(played + self.plan.pause)
                        }
                        _ => Phase::Done,
                    };
                } else if late {
                    self.note(format!("no end of the answer within {limit:?}"));
                    self.phase = Phase::Done;
                }
            }
            Phase::Pause(until) => {
                if now >= until {
                    self.queue.extend(self.question.iter().cloned());
                    self.turn_started = now;
                    self.phase = Phase::BargeIn;
                    self.listen();
                }
            }
            Phase::BargeIn => {
                if !self.barge_queued
                    && let Some(first) = self.barge.tts_first_audio
                    && now + self.barge_lead >= first + self.plan.barge_after
                    && let Some(frames) = &self.barge_in
                {
                    self.queue.extend(frames.iter().cloned());
                    self.barge_queued = true;
                }
                if self.barge.done && self.barge.tts_first_audio.is_none() {
                    self.note("barge-in: the answer had no audio".to_owned());
                    self.phase = Phase::Done;
                } else if late {
                    self.note(format!("barge-in: not measured within {limit:?}"));
                    self.phase = Phase::Done;
                }
            }
            Phase::Drain(_) | Phase::Done => self.phase = Phase::Done,
        }
    }

    /// Starts the recognition of a turn on its worker, after dropping every step of the one
    /// before.
    fn listen(&mut self) {
        self.cut();
        self.voiced = false;
        self.silent_for = Duration::ZERO;
        self.heard.clear();
        let request = RecognitionRequest::new(
            self.run_id,
            TurnId::generate(),
            StreamId::generate(),
            self.stt,
            "transcript",
        );
        match self.cascade.recogniser.start(request) {
            Ok(recognition) => {
                let (tasks, receiver) = mpsc::channel();
                let report = self.reporter();
                self.workers.push(
                    self.scope
                        .spawn(move || listening(recognition, receiver, report)),
                );
                self.listen = Some(tasks);
                self.stage = Stage::Listening;
            }
            Err(error) => {
                self.note(format!("the recogniser refused the turn: {error}"));
                self.finish();
            }
        }
    }

    /// Drops every step that runs: tells each worker of the turn to stop and waits until it
    /// stopped, so a step's next turn finds its handle dropped.
    fn cut(&mut self) {
        self.turn += 1;
        self.listen = None;
        self.speak = None;
        if let Some(tasks) = self.generate.take() {
            hand(Some(&tasks), &self.owing, Generate::Stop);
        }
        for worker in self.workers.drain(..) {
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
        }
        self.record = None;
        self.tentative.clear();
        self.utterance_ended = false;
        self.stage = Stage::Idle;
    }

    /// Ends the turn and drops its steps.
    fn finish(&mut self) {
        if let Some(steps) = self.steps() {
            steps.done = true;
        }
        self.cut();
    }

    /// Sends the next frame of input, silence when none is queued, to the recogniser's worker
    /// while it takes the turn's input and to the arm's detector.
    fn send(&mut self, now: Instant) {
        let input = self.queue.pop_front().unwrap_or_else(|| {
            let format = self.cascade.input_format;
            let samples = samples_in(self.plan.frame, format.sample_rate).max(1);
            Input {
                data: encode(&vec![0.0; samples], format.encoding),
                samples: samples as u32,
                voiced: false,
                mark: Mark::None,
            }
        });
        let Some(position) = SampleClockPosition::new(self.position) else {
            self.phase = Phase::Done;
            return;
        };
        let frame = AudioFrame {
            format: self.cascade.input_format,
            position,
            sample_count: input.samples,
            data: input.data,
        };
        let length = frame.duration();
        self.position += u64::from(input.samples);
        match input.mark {
            Mark::QuestionEnd => {
                if let Some(steps) = self.steps() {
                    steps.speech_end = Some(now);
                }
            }
            Mark::BargeStart => self.barge_start = Some(now),
            Mark::None => {}
        }
        if self.stage == Stage::Listening {
            hand(self.listen.as_ref(), &self.owing, Listen::Frame(frame));
            if input.voiced {
                self.voiced = true;
                self.silent_for = Duration::ZERO;
            } else if self.voiced {
                self.silent_for += length;
                if self.silent_for >= self.cascade.hold {
                    hand(self.listen.as_ref(), &self.owing, Listen::End);
                    self.stage = Stage::Recognising;
                }
            }
        }
        if self.phase == Phase::BargeIn && self.barge_start.is_some() && input.voiced {
            self.barge_voiced += length;
            if self.barge_voiced >= self.cascade.attack {
                self.interrupt(now);
            }
        }
    }

    /// The detector opened a turn at the barge-in, at `now`: stops the playout if the answer
    /// still plays or may play more, measures the barge-in to the end of the answer's audio as
    /// the client plays it, cut at the stop, and drops every step of the answer.
    fn interrupt(&mut self, now: Instant) {
        let Some(barge) = self.barge_start else {
            return;
        };
        let cursor = self.barge.cursor;
        let playing = cursor.is_some_and(|cursor| cursor > now) || !self.barge.done;
        let cut = playing.then(|| {
            self.playout.stop();
            (self.clock)()
        });
        let end = match (cursor, cut) {
            (Some(cursor), Some(cut)) => Some(cursor.min(cut)),
            (cursor, _) => cursor,
        };
        match end {
            Some(end) if end >= barge => {
                self.measured.barge_in_stop = Some(end.saturating_duration_since(barge));
            }
            end => {
                let after = |at: Instant| {
                    self.barge
                        .tts_first_audio
                        .map_or(0, |first| at.saturating_duration_since(first).as_millis())
                };
                let note = match end {
                    Some(end) => format!(
                        "barge-in: the answer had finished playing before the barge-in: it ended \
                         {} ms after its first audio, the barge-in's speech started {} ms after it",
                        after(end),
                        after(barge)
                    ),
                    None => "barge-in: the answer had no audio".to_owned(),
                };
                self.note(note);
            }
        }
        self.cut();
        self.phase = Phase::Done;
    }

    /// Takes a worker's report as it arrives; one of a turn that was cut is dropped.
    fn take(&mut self, report: Report) {
        if report.turn != self.turn {
            return;
        }
        match report.heard {
            Heard::Transcript(event) => self.transcribed(event),
            Heard::Recognised => {
                if self.stage == Stage::Recognising {
                    self.recognised();
                }
            }
            Heard::Generated(event, failure) => self.generated(event, failure),
            Heard::Answered => self.end_utterance(),
            Heard::Spoken(event) => self.spoken(event),
            Heard::Synthesised => {
                if self
                    .steps()
                    .is_some_and(|steps| steps.tts_first_audio.is_none())
                {
                    self.note("the synthesis gave no audio".to_owned());
                }
                self.finish();
            }
            Heard::Note(note) => self.note(note),
            Heard::Idle => {}
        }
    }

    fn transcribed(&mut self, event: TranscriptEvent) {
        if let TranscriptEvent::Committed(committed) = event {
            let at = (self.clock)();
            self.heard.push(committed.text);
            if let Some(steps) = self.steps() {
                steps.final_transcript = Some(at);
            }
        }
    }

    /// The recognition ended: asks the generator for an answer to its text and starts the
    /// synthesis of the answer, each on its worker.
    fn recognised(&mut self) {
        self.listen = None;
        let text = self.heard.join(" ").trim().to_owned();
        if text.is_empty() {
            self.note("the recogniser committed no text".to_owned());
            self.finish();
            return;
        }
        self.epoch += 1;
        let model = self
            .cascade
            .generator
            .model()
            .provenance()
            .model
            .as_str()
            .to_owned();
        let request = GenerationRequest {
            run_id: self.run_id,
            node_id: self.llm,
            epoch: self.epoch,
            turn: TurnRequest::new(model.as_str(), vec![Item::user(text)]),
        };
        match self.cascade.generator.start(request) {
            Ok(generation) => {
                let (tasks, receiver) = mpsc::channel();
                let waker = Waker::from(Arc::new(Prod {
                    tasks: tasks.clone(),
                    owing: Arc::clone(&self.owing),
                }));
                hand(Some(&tasks), &self.owing, Generate::Poll);
                let report = self.reporter();
                self.workers.push(
                    self.scope
                        .spawn(move || generating(generation, &receiver, &waker, report)),
                );
                self.generate = Some(tasks);
            }
            Err(error) => {
                self.note(format!("the generator refused the turn: {error}"));
                self.finish();
                return;
            }
        }
        match self.cascade.synthesiser.start() {
            Ok(synthesis) => {
                let (tasks, receiver) = mpsc::channel();
                let report = self.reporter();
                self.workers.push(
                    self.scope
                        .spawn(move || speaking(synthesis, receiver, report)),
                );
                self.speak = Some(tasks);
            }
            Err(error) => {
                self.note(format!("the synthesiser refused the turn: {error}"));
                self.finish();
                return;
            }
        }
        self.utterance_ended = false;
        self.stage = Stage::Answering;
    }

    fn generated(&mut self, event: GenerationEvent, failure: Option<String>) {
        match event {
            GenerationEvent::Started(started) => {
                let (record, _) = Generation::start_as(
                    started.generation_id,
                    StartGeneration {
                        run_id: started.run_id,
                        node_id: self.llm,
                        epoch: started.epoch,
                    },
                );
                self.record = Some(record);
            }
            GenerationEvent::TextDelta(delta) => {
                let at = (self.clock)();
                if let Some(steps) = self.steps() {
                    steps.first_token.get_or_insert(at);
                }
                let Some(record) = self.record.as_ref() else {
                    return;
                };
                match OutputSegment::emit_as(delta.segment_id, record, delta.sequence, delta.text) {
                    Ok((segment, _)) => {
                        self.tentative.insert(delta.segment_id, segment);
                    }
                    Err(refused) => self.note(format!("a delta was refused: {refused:?}")),
                }
            }
            GenerationEvent::SegmentCommitted(committed) => {
                let Some(segment) = self.tentative.remove(&committed.segment_id) else {
                    return;
                };
                match segment.commit() {
                    Ok((segment, _)) => {
                        hand(self.speak.as_ref(), &self.owing, Speak::Segment(segment));
                    }
                    Err(refused) => self.note(format!("a segment did not commit: {refused:?}")),
                }
            }
            GenerationEvent::Completed(_) => self.end_utterance(),
            GenerationEvent::Cancelled(..) => {
                let failure = failure.unwrap_or_else(|| "no failure given".to_owned());
                self.note(format!("the generation was cancelled: {failure}"));
                self.end_utterance();
            }
            GenerationEvent::Refused(refused) => {
                self.note(format!("the model refused: {}", refused.text));
            }
            _ => {}
        }
    }

    /// The answer is complete: the synthesis's utterance ends.
    fn end_utterance(&mut self) {
        if self.utterance_ended {
            return;
        }
        self.utterance_ended = true;
        hand(self.speak.as_ref(), &self.owing, Speak::End);
    }

    /// Takes an event of the synthesis: a frame is played on the client's model from its receipt
    /// or from the end of the frame before it, whichever is later, and handed to the playout.
    fn spoken(&mut self, event: SynthesisEvent) {
        let at = (self.clock)();
        if let Some(steps) = self.steps() {
            steps.ended = Some(at);
        }
        let SynthesisEvent::Audio(audio) = event else {
            return;
        };
        let length = audio.frame().duration();
        if let Some(steps) = self.steps() {
            steps.tts_first_audio.get_or_insert(at);
            let start = steps.cursor.map_or(at, |cursor| cursor.max(at));
            steps.cursor = Some(start + length);
        }
        self.playout.play(audio.frame());
    }
}

/// A measure of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    FirstAudio,
    FirstTranscript,
    /// The turn latency: end of speech to the answer's end.
    ResponseEnd,
    BargeInStop,
    FinalTranscript,
    FirstToken,
    TtsFirstAudio,
}

impl Measure {
    /// Every measure, in the order the table shows them: those of a native model, then the
    /// cascade's steps.
    pub const ALL: [Self; 7] = [
        Self::FirstAudio,
        Self::FirstTranscript,
        Self::ResponseEnd,
        Self::BargeInStop,
        Self::FinalTranscript,
        Self::FirstToken,
        Self::TtsFirstAudio,
    ];

    /// The measure's column heading.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::FirstAudio => "first audio",
            Self::FirstTranscript => "first transcript",
            Self::ResponseEnd => "response end",
            Self::BargeInStop => "barge-in to stop",
            Self::FinalTranscript => "final transcript",
            Self::FirstToken => "first token",
            Self::TtsFirstAudio => "tts first audio",
        }
    }

    /// Whether a provider of `topology` has the step the measure ends at: first audio, response
    /// end and barge-in to stop both have; the first transcript of the answer is a native
    /// model's, and the three steps after it the cascade's.
    #[must_use]
    pub const fn applies(self, topology: Topology) -> bool {
        match self {
            Self::FirstAudio | Self::ResponseEnd | Self::BargeInStop => true,
            Self::FirstTranscript => matches!(topology, Topology::Native),
            Self::FinalTranscript | Self::FirstToken | Self::TtsFirstAudio => {
                matches!(topology, Topology::Cascade)
            }
        }
    }

    /// The measure of `run`; `None` as well when its topology has no such step.
    #[must_use]
    pub const fn of(self, run: &Measured) -> Option<Duration> {
        if !self.applies(run.topology) {
            return None;
        }
        match self {
            Self::FirstAudio => run.first_audio,
            Self::FirstTranscript => run.first_transcript,
            Self::ResponseEnd => run.response_end,
            Self::BargeInStop => run.barge_in_stop,
            Self::FinalTranscript => run.final_transcript,
            Self::FirstToken => run.first_token,
            Self::TtsFirstAudio => run.tts_first_audio,
        }
    }
}

/// The median and the range of a measure over runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spread {
    /// The middle value, the mean of the two middle ones for an even count.
    pub median: Duration,
    pub lowest: Duration,
    pub highest: Duration,
    /// The runs that measured it.
    pub measured: usize,
    /// The runs.
    pub runs: usize,
}

impl Spread {
    /// The spread of the measured `values`; `None` when none was measured.
    #[must_use]
    pub fn of(values: &[Option<Duration>]) -> Option<Self> {
        let mut measured: Vec<Duration> = values.iter().flatten().copied().collect();
        measured.sort_unstable();
        let (&lowest, &highest) = (measured.first()?, measured.last()?);
        let middle = measured.len() / 2;
        let median = if measured.len().is_multiple_of(2) {
            (measured[middle - 1] + measured[middle]) / 2
        } else {
            measured[middle]
        };
        Some(Self {
            median,
            lowest,
            highest,
            measured: measured.len(),
            runs: values.len(),
        })
    }
}

impl fmt::Display for Spread {
    /// `320 [300-340] 3/3`, in milliseconds.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}-{}] {}/{}",
            self.median.as_millis(),
            self.lowest.as_millis(),
            self.highest.as_millis(),
            self.measured,
            self.runs
        )
    }
}

/// What became of a provider in a comparison.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Its runs, in order.
    Ran(Vec<Measured>),
    /// It was not run, for this reason.
    Skipped(String),
}

/// The runs of each provider, in the order they were added, and the table of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Comparison {
    providers: Vec<(String, Topology, Outcome)>,
}

impl Comparison {
    /// A comparison of no provider.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Adds the runs of `provider`, of the topology its first run names; with no run, of the
    /// topology `provider` is named after, else native. [`Comparison::ran_as`] names it.
    pub fn ran(&mut self, provider: &str, runs: Vec<Measured>) {
        let topology = runs.first().map_or_else(
            || Topology::named(provider).unwrap_or_default(),
            |run| run.topology,
        );
        self.ran_as(provider, topology, runs);
    }

    /// Adds the runs of `provider` of `topology`, whether or not there is a run.
    pub fn ran_as(&mut self, provider: &str, topology: Topology, runs: Vec<Measured>) {
        self.providers
            .push((provider.to_owned(), topology, Outcome::Ran(runs)));
    }

    /// Adds `provider`, a native model, as skipped for `reason`.
    pub fn skipped(&mut self, provider: &str, reason: &str) {
        self.skipped_as(provider, Topology::Native, reason);
    }

    /// Adds `provider` of `topology` as skipped for `reason`.
    pub fn skipped_as(&mut self, provider: &str, topology: Topology, reason: &str) {
        self.providers.push((
            provider.to_owned(),
            topology,
            Outcome::Skipped(reason.to_owned()),
        ));
    }

    /// What became of `provider`.
    #[must_use]
    pub fn outcome(&self, provider: &str) -> Option<&Outcome> {
        self.providers
            .iter()
            .find(|(name, _, _)| name == provider)
            .map(|(_, _, outcome)| outcome)
    }

    /// The topology of `provider`.
    #[must_use]
    pub fn topology(&self, provider: &str) -> Option<Topology> {
        self.providers
            .iter()
            .find(|(name, _, _)| name == provider)
            .map(|&(_, topology, _)| topology)
    }

    /// The spread of `measure` over the runs of `provider`; `None` when it was skipped, its
    /// topology has no such step or no run measured it.
    #[must_use]
    pub fn spread(&self, provider: &str, measure: Measure) -> Option<Spread> {
        match self.outcome(provider)? {
            Outcome::Ran(runs) => {
                Spread::of(&runs.iter().map(|run| measure.of(run)).collect::<Vec<_>>())
            }
            Outcome::Skipped(_) => None,
        }
    }

    /// The providers that measured `measure`, lowest median first; ties keep their order.
    #[must_use]
    pub fn ranking(&self, measure: Measure) -> Vec<&str> {
        let mut ranked: Vec<(&str, Duration)> = self
            .providers
            .iter()
            .filter_map(|(name, _, _)| Some((name.as_str(), self.spread(name, measure)?.median)))
            .collect();
        ranked.sort_by_key(|&(_, median)| median);
        ranked.into_iter().map(|(name, _)| name).collect()
    }
}

impl fmt::Display for Comparison {
    /// The latency table, the usage of each run, and the notes of the runs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let width = self
            .providers
            .iter()
            .map(|(name, _, _)| name.len())
            .chain(["provider".len()])
            .max()
            .unwrap_or(0);
        let kind = "topology".len();
        writeln!(
            f,
            "times in ms, taken on the client at the receipt of each event: \
             median [lowest-highest] measured/runs; n/a: a step the topology does not have"
        )?;
        writeln!(
            f,
            "from the end of the question's speech; barge-in from the barge-in's first voiced \
             frame; response end is the turn latency"
        )?;
        let mut heading = format!("{:<width$}  {:<kind$}", "provider", "topology");
        for measure in Measure::ALL {
            heading.push_str(&format!("  {:<20}", measure.name()));
        }
        writeln!(f, "{}", heading.trim_end())?;
        for (name, topology, outcome) in &self.providers {
            let label = topology.name();
            match outcome {
                Outcome::Ran(runs) => {
                    let mut row = format!("{name:<width$}  {label:<kind$}");
                    for measure in Measure::ALL {
                        let cell = if !measure.applies(*topology) {
                            "n/a".to_owned()
                        } else {
                            self.spread(name, measure).map_or_else(
                                || format!("- 0/{}", runs.len()),
                                |spread| spread.to_string(),
                            )
                        };
                        row.push_str(&format!("  {cell:<20}"));
                    }
                    writeln!(f, "{}", row.trim_end())?;
                }
                Outcome::Skipped(reason) => {
                    writeln!(f, "{name:<width$}  {label:<kind$}  skipped: {reason}")?;
                }
            }
        }
        writeln!(f, "usage per run, as each provider reports it")?;
        for (name, _, outcome) in &self.providers {
            let Outcome::Ran(runs) = outcome else {
                continue;
            };
            for (at, run) in runs.iter().enumerate() {
                write!(f, "{name:<width$}  run {}: {}", at + 1, run.usage)?;
                if let Some(seconds) = run.seconds {
                    write!(f, "; billed {seconds:.1} s")?;
                }
                writeln!(f)?;
                for note in &run.notes {
                    writeln!(f, "{:<width$}    {note}", "")?;
                }
            }
        }
        Ok(())
    }
}
