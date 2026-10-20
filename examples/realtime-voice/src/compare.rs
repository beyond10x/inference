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
//! [`Comparison`] collects the runs of each provider, or why it was skipped, and prints the
//! median and the spread (lowest and highest) of each measure over the runs that measured it, and
//! the usage of each run as the provider reports it: tokens, and seconds for a provider that
//! bills by the second.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use inference_audio_device::convert::{LinearResampler, decode, encode};
use inference_duplex::inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionEvent, SpeechSessionHandle,
};

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

/// What one run measured. A measure is `None` when the run did not reach it; `notes` say why.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Measured {
    /// End of input speech to the answer's first audio.
    pub first_audio: Option<Duration>,
    /// End of input speech to the answer's first transcript.
    pub first_transcript: Option<Duration>,
    /// End of input speech to the answer's end.
    pub response_end: Option<Duration>,
    /// The barge-in's first voiced frame sent to the end of the answer's playout.
    pub barge_in_stop: Option<Duration>,
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

/// A measure of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    FirstAudio,
    FirstTranscript,
    ResponseEnd,
    BargeInStop,
}

impl Measure {
    /// Every measure, in the order the table shows them.
    pub const ALL: [Self; 4] = [
        Self::FirstAudio,
        Self::FirstTranscript,
        Self::ResponseEnd,
        Self::BargeInStop,
    ];

    /// The measure's column heading.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::FirstAudio => "first audio",
            Self::FirstTranscript => "first transcript",
            Self::ResponseEnd => "response end",
            Self::BargeInStop => "barge-in to stop",
        }
    }

    /// The measure of `run`.
    #[must_use]
    pub const fn of(self, run: &Measured) -> Option<Duration> {
        match self {
            Self::FirstAudio => run.first_audio,
            Self::FirstTranscript => run.first_transcript,
            Self::ResponseEnd => run.response_end,
            Self::BargeInStop => run.barge_in_stop,
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
    providers: Vec<(String, Outcome)>,
}

impl Comparison {
    /// A comparison of no provider.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Adds the runs of `provider`.
    pub fn ran(&mut self, provider: &str, runs: Vec<Measured>) {
        self.providers
            .push((provider.to_owned(), Outcome::Ran(runs)));
    }

    /// Adds `provider` as skipped for `reason`.
    pub fn skipped(&mut self, provider: &str, reason: &str) {
        self.providers
            .push((provider.to_owned(), Outcome::Skipped(reason.to_owned())));
    }

    /// What became of `provider`.
    #[must_use]
    pub fn outcome(&self, provider: &str) -> Option<&Outcome> {
        self.providers
            .iter()
            .find(|(name, _)| name == provider)
            .map(|(_, outcome)| outcome)
    }

    /// The spread of `measure` over the runs of `provider`; `None` when it was skipped or no run
    /// measured it.
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
            .filter_map(|(name, _)| Some((name.as_str(), self.spread(name, measure)?.median)))
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
            .map(|(name, _)| name.len())
            .chain(["provider".len()])
            .max()
            .unwrap_or(0);
        writeln!(
            f,
            "times in ms, taken on the client at the receipt of each duplex event: \
             median [lowest-highest] measured/runs"
        )?;
        writeln!(
            f,
            "from the end of the question's speech; barge-in from the barge-in's first voiced frame"
        )?;
        let mut heading = format!("{:<width$}", "provider");
        for measure in Measure::ALL {
            heading.push_str(&format!("  {:<20}", measure.name()));
        }
        writeln!(f, "{}", heading.trim_end())?;
        for (name, outcome) in &self.providers {
            match outcome {
                Outcome::Ran(runs) => {
                    let mut row = format!("{name:<width$}");
                    for measure in Measure::ALL {
                        let cell = self.spread(name, measure).map_or_else(
                            || format!("- 0/{}", runs.len()),
                            |spread| spread.to_string(),
                        );
                        row.push_str(&format!("  {cell:<20}"));
                    }
                    writeln!(f, "{}", row.trim_end())?;
                }
                Outcome::Skipped(reason) => writeln!(f, "{name:<width$}  skipped: {reason}")?,
            }
        }
        writeln!(f, "usage per run, as each provider reports it")?;
        for (name, outcome) in &self.providers {
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
