//! One synthesis of a [`SherpaOnnxSynthesiser`](crate::SherpaOnnxSynthesiser): the handle that
//! takes segments and yields their audio.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use inference_tts::inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::inference_core::model::Adapter;
use inference_tts::inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued, SegmentState,
};
use inference_tts::{
    ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt, PlaybackRefused,
    SynthesisEvent, SynthesisHandle, SynthesisOutput, UtteranceEnded,
};

use crate::engine::Engine;

/// The length of a frame a synthesis emits, in milliseconds; the last frame of a segment holds
/// what is left and may be shorter.
pub const FRAME_MS: u32 = 20;

/// A NUL byte in a text, at byte offset `at`: sherpa-onnx takes a text as a C string, which
/// cannot hold one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NulInText {
    pub at: usize,
}

impl fmt::Display for NulInText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the text holds a NUL byte at byte {}, which sherpa-onnx cannot be given",
            self.at
        )
    }
}

impl Error for NulInText {}

/// Whether sherpa-onnx can be given `text`.
///
/// # Errors
///
/// [`NulInText`] naming the first NUL byte of `text`.
pub fn speakable(text: &str) -> Result<(), NulInText> {
    text.find('\0').map_or(Ok(()), |at| Err(NulInText { at }))
}

/// Why a synthesis failed. Its usage record is then not final, and the segments it took and did
/// not speak stay Queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthesisFailed {
    /// The text of the segment holds a NUL byte at byte `at`; sherpa-onnx was not called for it.
    NulInText { segment_id: SegmentId, at: usize },
    /// sherpa-onnx generated no audio for the segment.
    BackendRefused { segment_id: SegmentId },
    /// The thread generating the segment could not be started or ended without a result.
    WorkerLost { segment_id: SegmentId },
    /// The TTS contract refused audio of the segment, which a segment taken by this synthesis
    /// never meets.
    Contract {
        segment_id: SegmentId,
        detail: String,
    },
}

impl fmt::Display for SynthesisFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NulInText { segment_id, at } => write!(
                f,
                "the text of segment {segment_id} holds a NUL byte at byte {at}, which \
                 sherpa-onnx cannot be given"
            ),
            Self::BackendRefused { segment_id } => {
                write!(f, "sherpa-onnx generated no audio for segment {segment_id}")
            }
            Self::WorkerLost { segment_id } => write!(
                f,
                "the thread generating segment {segment_id} did not start or ended without a \
                 result"
            ),
            Self::Contract { segment_id, detail } => write!(
                f,
                "the TTS contract refused audio of segment {segment_id}: {detail}"
            ),
        }
    }
}

impl Error for SynthesisFailed {}

/// The syntheses of one synthesiser that run, shared by its clones, which are the same adapter.
#[derive(Debug, Clone, Default)]
pub struct Running(Arc<AtomicUsize>);

impl Running {
    /// Counts one more running synthesis, if fewer than `max_concurrency` run.
    pub fn admit(&self, max_concurrency: i64) -> Result<Slot, ConcurrencyLimitReached> {
        let limit = usize::try_from(max_concurrency).unwrap_or(usize::MAX);
        let slot = Slot(Arc::clone(&self.0));
        if self.0.fetch_add(1, Ordering::AcqRel) < limit {
            Ok(slot)
        } else {
            drop(slot);
            Err(ConcurrencyLimitReached { max_concurrency })
        }
    }
}

/// The place of one running synthesis among [`Running`], given back when it is dropped.
#[derive(Debug)]
pub struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The generations of one synthesiser that run in sherpa-onnx at once. A generation holds a
/// [`Place`] from before it starts until the native call returns, also when its synthesis was
/// dropped or its segment discarded meanwhile, so a synthesiser never generates more at once than
/// its maximum concurrency.
#[derive(Debug, Default)]
pub struct Gate {
    held: Mutex<usize>,
    freed: Condvar,
}

impl Gate {
    /// A place among `limit`, waiting on the calling thread while every place is held.
    fn take(self: &Arc<Self>, limit: usize) -> Place {
        let mut held = lock(&self.held);
        while *held >= limit {
            held = self
                .freed
                .wait(held)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *held += 1;
        Place(Arc::clone(self))
    }
}

/// One generation's place in a [`Gate`], given back when it is dropped.
struct Place(Arc<Gate>);

impl Drop for Place {
    fn drop(&mut self) {
        {
            let mut held = lock(&self.0.held);
            *held = held.saturating_sub(1);
        }
        self.0.freed.notify_all();
    }
}

/// The threads generating for one synthesiser, its clones and their syntheses, as a count: no
/// thread is joined. Dropping the last of those handles returns at once. A thread still inside
/// sherpa-onnx was told to stop by then; it keeps the engine alive, ends when the engine asks at
/// its next part whether to go on, and releases the engine then. So one thread per generation in
/// flight, and the engine with it, can outlive the synthesiser and its syntheses until that part
/// ends.
#[derive(Debug, Default)]
pub struct Workers {
    running: Mutex<usize>,
    ended: Condvar,
}

impl Workers {
    /// Counts one more thread; the thread holds what is returned until it has released the
    /// engine.
    fn enter(self: &Arc<Self>) -> Working {
        *lock(&self.running) += 1;
        Working(Arc::clone(self))
    }

    /// The threads that have not released the engine yet.
    #[cfg(test)]
    pub(crate) fn running(&self) -> usize {
        *lock(&self.running)
    }

    /// Waits until no thread is left, at most `within`; whether none is left.
    #[cfg(test)]
    pub(crate) fn wait_idle(&self, within: Duration) -> bool {
        let (running, _) = self
            .ended
            .wait_timeout_while(lock(&self.running), within, |running| *running > 0)
            .unwrap_or_else(PoisonError::into_inner);
        *running == 0
    }
}

/// One generating thread among [`Workers`], counted out when it is dropped.
struct Working(Arc<Workers>);

impl Drop for Working {
    fn drop(&mut self) {
        {
            let mut running = lock(&self.0.running);
            *running = running.saturating_sub(1);
        }
        self.0.ended.notify_all();
    }
}

/// What every synthesis of one synthesiser shares: the engine, the voice it speaks in, the
/// format of its frames, and the bounds on what runs at once.
#[derive(Clone)]
pub struct Voice {
    pub engine: Arc<dyn Engine>,
    pub format: AudioFormat,
    pub speaker: i32,
    pub gate: Arc<Gate>,
    pub workers: Arc<Workers>,
}

/// The waker of the task polling a synthesis, woken by the thread generating for it.
#[derive(Default)]
struct Wake(Mutex<Option<Waker>>);

impl Wake {
    fn set(&self, waker: &Waker) {
        let mut held = lock(&self.0);
        if !held.as_ref().is_some_and(|held| held.will_wake(waker)) {
            *held = Some(waker.clone());
        }
    }

    fn wake(&self) {
        let waker = lock(&self.0).take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// What the thread generating a segment hands to its synthesis.
enum Part {
    /// Samples the engine handed over, in order.
    Samples(Vec<f32>),
    /// The generation ended after `spent` in the engine; `spoken` is false when the engine
    /// refused the text.
    Ended { spent: Duration, spoken: bool },
}

/// The segment being generated.
struct Live {
    segment_id: SegmentId,
    parts: Receiver<Part>,
    /// Set to stop the generation at the engine's next part.
    stop: Arc<AtomicBool>,
    wake: Arc<Wake>,
    /// The samples received that do not fill a frame yet.
    carry: Vec<f32>,
}

/// One synthesis of a [`SherpaOnnxSynthesiser`](crate::SherpaOnnxSynthesiser); it runs until it
/// is dropped.
///
/// Each segment is generated on a thread of its own, one segment at a time in the order offered,
/// and its samples come back part by part as sherpa-onnx hands them over, one part per sentence
/// of the segment's text. Iterating the synthesis blocks while it waits for the next part; its
/// async form, [`SynthesisHandle::poll_event`], returns `Pending` there instead and is woken when
/// the part arrives. Frames hold [`FRAME_MS`] of mono `pcm_s16le` at the voice's own rate, the
/// last of a segment what is left, at consecutive positions of the synthesis's sample clock.
///
/// A segment whose text holds no letter and no digit, blank or punctuation alone, has nothing to
/// pronounce: it completes without audio and is not given to sherpa-onnx.
///
/// A failure ends the iteration of the segments it took; [`SherpaOnnxSynthesis::failure`] says
/// why, the usage record then is not final, and the segments offered after it are taken and not
/// spoken.
pub struct SherpaOnnxSynthesis {
    output: SynthesisOutput,
    voice: Voice,
    limit: usize,
    frame_len: usize,
    pending: VecDeque<(SegmentId, String)>,
    delivered: VecDeque<SynthesisEvent>,
    live: Option<Live>,
    next_sample: u64,
    failure: Option<SynthesisFailed>,
    _slot: Slot,
}

impl fmt::Debug for SherpaOnnxSynthesis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SherpaOnnxSynthesis")
            .field("format", &self.voice.format)
            .field("speaker", &self.voice.speaker)
            .field("segments", &self.output.segments().len())
            .field("speaking", &self.live.as_ref().map(|live| live.segment_id))
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl SherpaOnnxSynthesis {
    pub(crate) fn start(adapter: &Adapter, voice: Voice, slot: Slot) -> Self {
        let rate = usize::try_from(voice.format.sample_rate.get()).unwrap_or(usize::MAX);
        let frame_len =
            (rate / 1_000 * FRAME_MS as usize + rate % 1_000 * FRAME_MS as usize / 1_000).max(1);
        Self {
            output: SynthesisOutput::start(adapter),
            limit: usize::try_from(adapter.max_concurrency()).unwrap_or(usize::MAX),
            voice,
            frame_len,
            pending: VecDeque::new(),
            delivered: VecDeque::new(),
            live: None,
            next_sample: 0,
            failure: None,
            _slot: slot,
        }
    }

    /// Why the synthesis failed, once it has.
    #[must_use]
    pub const fn failure(&self) -> Option<&SynthesisFailed> {
        self.failure.as_ref()
    }

    /// The format of every frame it emits.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.voice.format
    }

    /// Moves every segment offered and not yet taken into the queue of segments to speak.
    fn take_input(&mut self) {
        if let Ok(taken) = self.output.take_input() {
            self.pending.extend(
                taken
                    .iter()
                    .map(|segment| (segment.segment_id(), segment.text().to_owned())),
            );
        }
    }

    fn state(&self, segment_id: SegmentId) -> Option<SegmentState> {
        self.output.segment(segment_id).map(OutputSegment::state)
    }

    /// Starts generating the segment `segment_id` on a thread of its own.
    fn speak(&mut self, segment_id: SegmentId, text: String) {
        let (sender, parts) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(Wake::default());
        let worker = {
            let engine = Arc::clone(&self.voice.engine);
            let gate = Arc::clone(&self.voice.gate);
            let working = self.voice.workers.enter();
            let (limit, speaker) = (self.limit, self.voice.speaker);
            let (stop, wake) = (Arc::clone(&stop), Arc::clone(&wake));
            std::thread::Builder::new()
                .name("inference-sherpa-onnx-tts".to_owned())
                .spawn(move || {
                    let place = gate.take(limit);
                    let started = Instant::now();
                    let spoken = stop.load(Ordering::Acquire) || {
                        let (sender, stop, wake) =
                            (sender.clone(), Arc::clone(&stop), Arc::clone(&wake));
                        engine.generate(
                            &text,
                            speaker,
                            Box::new(move |samples| {
                                if stop.load(Ordering::Acquire) {
                                    return false;
                                }
                                let sent = sender.send(Part::Samples(samples.to_vec())).is_ok();
                                wake.wake();
                                sent
                            }),
                        )
                    };
                    let spent = started.elapsed();
                    drop(place);
                    let _ = sender.send(Part::Ended { spent, spoken });
                    wake.wake();
                    // The engine is released before the thread is counted out: whoever waits
                    // for the count then finds the engine gone when no handle holds it.
                    drop(engine);
                    drop(working);
                })
        };
        match worker {
            // The thread is not joined: it ends by itself, see [`Workers`].
            Ok(_) => {
                self.live = Some(Live {
                    segment_id,
                    parts,
                    stop,
                    wake,
                    carry: Vec::new(),
                });
            }
            Err(_) => self.fail(SynthesisFailed::WorkerLost { segment_id }),
        }
    }

    /// Acts on `part`, what the thread generating the segment being spoken handed over; none
    /// when the thread ended without a result.
    fn receive(&mut self, part: Option<Part>) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        let segment_id = live.segment_id;
        match part {
            Some(Part::Samples(samples)) => {
                live.carry.extend_from_slice(&samples);
                let whole = live.carry.len() - live.carry.len() % self.frame_len;
                let rest = live.carry.split_off(whole);
                let samples = std::mem::replace(&mut live.carry, rest);
                self.emit(segment_id, &samples);
            }
            Some(Part::Ended { spent, spoken }) => {
                let rest = std::mem::take(&mut live.carry);
                self.live = None;
                self.output.add_local_compute(spent);
                if !spoken {
                    self.fail(SynthesisFailed::BackendRefused { segment_id });
                    return;
                }
                self.emit(segment_id, &rest);
                if self.failure.is_none() {
                    match self.output.complete(segment_id, Vec::new()) {
                        Ok(events) => self.delivered.extend(events),
                        Err(refused) => self.fail(SynthesisFailed::Contract {
                            segment_id,
                            detail: refused.to_string(),
                        }),
                    }
                }
            }
            None => self.fail(SynthesisFailed::WorkerLost { segment_id }),
        }
    }

    /// Emits `samples` as frames of the Queued segment `segment_id`, each [`FRAME_MS`] long but
    /// the last, at the next positions of the synthesis's sample clock.
    fn emit(&mut self, segment_id: SegmentId, samples: &[f32]) {
        for samples in samples.chunks(self.frame_len) {
            let frame = SampleClockPosition::new(self.next_sample)
                .zip(u32::try_from(samples.len()).ok())
                .map(|(position, sample_count)| AudioFrame {
                    format: self.voice.format,
                    position,
                    sample_count,
                    data: samples.iter().flat_map(|&sample| pcm16(sample)).collect(),
                })
                .ok_or_else(|| "the sample clock passed its largest position".to_owned())
                .and_then(|frame| {
                    self.output
                        .audio(segment_id, frame)
                        .map_err(|refused| refused.to_string())
                });
            match frame {
                Ok(event) => {
                    self.next_sample += samples.len() as u64;
                    self.delivered.push_back(event);
                }
                Err(detail) => {
                    self.fail(SynthesisFailed::Contract { segment_id, detail });
                    return;
                }
            }
        }
    }

    /// Ends the synthesis as failed by `error`: the generation in flight told to stop, the call
    /// marked failed, and nothing more spoken.
    fn fail(&mut self, error: SynthesisFailed) {
        self.stop_live();
        self.pending.clear();
        self.output.mark_failed();
        self.failure = Some(error);
    }

    /// Tells the generation in flight, if there is one, to stop at the engine's next part, and
    /// forgets it: nothing more of its segment is emitted.
    fn stop_live(&mut self) {
        if let Some(live) = self.live.take() {
            live.stop.store(true, Ordering::Release);
        }
    }

    /// The next event: blocking while it waits for the engine's next part, or with `cx`, the
    /// async form's, `Pending` there instead.
    fn advance(&mut self, mut cx: Option<&mut Context<'_>>) -> Poll<Option<SynthesisEvent>> {
        loop {
            if let Some(event) = self.delivered.pop_front() {
                return Poll::Ready(Some(event));
            }
            if self.failure.is_some() {
                self.take_input();
                return Poll::Ready(self.output.usage().ok());
            }
            if let Some(live) = self.live.as_ref() {
                let part = match cx.as_deref_mut() {
                    None => live.parts.recv().ok(),
                    Some(cx) => {
                        live.wake.set(cx.waker());
                        match live.parts.try_recv() {
                            Ok(part) => Some(part),
                            Err(TryRecvError::Empty) => return Poll::Pending,
                            Err(TryRecvError::Disconnected) => None,
                        }
                    }
                };
                self.receive(part);
                continue;
            }
            self.take_input();
            let Some((segment_id, text)) = self.pending.pop_front() else {
                return Poll::Ready(self.output.usage().ok());
            };
            if self.state(segment_id) != Some(SegmentState::Queued) {
                continue;
            }
            if let Err(NulInText { at }) = speakable(&text) {
                self.fail(SynthesisFailed::NulInText { segment_id, at });
                continue;
            }
            if !text.chars().any(char::is_alphanumeric) {
                match self.output.complete(segment_id, Vec::new()) {
                    Ok(events) => self.delivered.extend(events),
                    Err(refused) => self.fail(SynthesisFailed::Contract {
                        segment_id,
                        detail: refused.to_string(),
                    }),
                }
                continue;
            }
            self.speak(segment_id, text);
        }
    }
}

/// One sample in `-1.0..=1.0` as `pcm_s16le`; a sample outside it is clipped.
pub(crate) fn pcm16(sample: f32) -> [u8; 2] {
    let scaled = (sample * 32_767.0)
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX));
    // The value is clamped to the range of i16, and NaN casts to 0.
    let sample = scaled as i16;
    sample.to_le_bytes()
}

impl Iterator for SherpaOnnxSynthesis {
    type Item = SynthesisEvent;

    /// The next event. It blocks while a segment is being generated; `None` once nothing more is
    /// delivered until more is offered or the utterance ends.
    fn next(&mut self) -> Option<SynthesisEvent> {
        match self.advance(None) {
            Poll::Ready(event) => event,
            Poll::Pending => None,
        }
    }
}

impl Drop for SherpaOnnxSynthesis {
    /// Tells the generation in flight to stop at the engine's next part and returns without
    /// waiting for it, also when this is the last handle of its synthesiser. The thread of that
    /// generation, and the engine it holds, live on until the part in flight ends: sherpa-onnx
    /// asks after each part, one sentence, whether to go on. A process that returns from `main`
    /// before that ends the thread with it, and exits with the status `main` gave: the case
    /// `process_end` of the crate's tests runs such a process.
    fn drop(&mut self) {
        self.stop_live();
    }
}

impl SynthesisHandle for SherpaOnnxSynthesis {
    fn output(&self) -> &SynthesisOutput {
        &self.output
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        self.output.offer(segment)
    }

    fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        self.output.end_utterance()
    }

    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        self.output.acknowledge_playback(segment_id, receipt)
    }

    /// Takes the next event, `Pending` where iterating blocks: while the engine generates the
    /// next part, whose arrival wakes `cx`.
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<SynthesisEvent>> {
        self.advance(Some(cx))
    }

    /// Discards the segment through [`SynthesisOutput::discard`] and drops every event of it not
    /// yet delivered. When it is the segment being generated, a barge-in, its generation is told
    /// to stop: sherpa-onnx asks after each part whether to go on and is answered no, so it
    /// ends at the end of the sentence it is generating. The discard does not wait for that, and
    /// the time the stopped generation spent is not counted in the usage record.
    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        let event = self.output.discard(segment_id)?;
        self.delivered
            .retain(|event| event.segment_id() != Some(segment_id));
        if self
            .live
            .as_ref()
            .is_some_and(|live| live.segment_id == segment_id)
        {
            self.stop_live();
        }
        Ok(event)
    }
}
