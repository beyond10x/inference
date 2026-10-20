//! Playout: a [`Speaker`] taking `inference-audio` frames to an output device.
//!
//! Frames are decoded to mono and queued at their own rate in a [`PlayoutBuffer`] of fixed
//! capacity. The device callback resamples linearly from the queue to the device rate and writes
//! the same sample to every channel. It never blocks and never allocates: it takes the queue's
//! lock only if it is free, and otherwise writes silence. The producer side takes that lock only
//! to queue a frame that fits, to finish, to stop and to read the played position, each for a
//! copy's time at most; how much is queued, and whether a frame fits, it reads from atomics.
//!
//! The played position is what a barge-in truncates a model's output at: the sample clock
//! position, on the frames' own clock, of the first sample not yet handed to the device. A run of
//! frames each continuing the last is one segment; a frame that does not continue the one before
//! starts a new segment at its own position, so the position stays on the frames' clock across a
//! gap. [`Speaker::stop`] clears every queued sample at once and answers the position reached.
//!
//! An underrun is a callback, or a part of one, that wrote silence while audio was due: frames
//! were pushed and the sink was neither stopped nor [finished](Speaker::finish). Running dry
//! after [`Speaker::finish`] is the end of the audio, not an underrun.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

use inference_audio::{AudioFrame, SampleClockPosition};

use crate::capture::samples_in;
use crate::convert::{MalformedFrame, decode, playable};
use crate::host::{
    AudioHost, DeviceConfig, DeviceError, DeviceSelector, ResolvedDevice, StreamGuard,
};

/// How often a waiting producer looks at the queue again.
const POLL: Duration = Duration::from_millis(2);

/// The most segments the queue tracks at once.
const SEGMENTS: usize = 256;

/// How much a [`Speaker`] queues: at most `buffer` of audio at the frames' own rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlayoutConfig {
    pub buffer: Duration,
}

impl Default for PlayoutConfig {
    /// Two seconds of queued audio.
    fn default() -> Self {
        Self {
            buffer: Duration::from_secs(2),
        }
    }
}

/// Why a frame was not queued. Nothing of a refused frame is queued.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PushError {
    /// The queue has no room for the frame now; it will once the device has played more.
    Full,
    /// The frame is longer than the whole queue.
    TooLarge { samples: u64, capacity: usize },
    /// The frame's sample rate is not the rate of the frames queued since the last stop.
    RateChanged {
        expected: NonZeroU32,
        got: NonZeroU32,
    },
    /// The frame's data does not match its format and sample count.
    Malformed(MalformedFrame),
    /// The sink was stopped while the push waited for room.
    Stopped,
}

impl fmt::Display for PushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => f.write_str("the playout queue is full"),
            Self::TooLarge { samples, capacity } => write!(
                f,
                "a frame of {samples} samples is longer than the playout queue of {capacity}"
            ),
            Self::RateChanged { expected, got } => write!(
                f,
                "the frame is at {got} Hz where the frames queued since the last stop are at \
                 {expected} Hz"
            ),
            Self::Malformed(malformed) => fmt::Display::fmt(malformed, f),
            Self::Stopped => f.write_str("the sink was stopped while the frame waited for room"),
        }
    }
}

impl Error for PushError {}

/// A run of frames each continuing the last: `start` is the queue index of its first sample and
/// `position` that sample's position on the frames' clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Segment {
    start: u64,
    position: u64,
}

/// The queue and its clock since the last stop.
#[derive(Debug)]
struct Queue {
    rate: Option<NonZeroU32>,
    samples: VecDeque<f32>,
    capacity: usize,
    segments: VecDeque<Segment>,
    head: u64,
    end: u64,
    outputs: u64,
    finished: bool,
}

impl Queue {
    /// Whether audio is due: frames were pushed since the last stop and not finished.
    const fn due(&self) -> bool {
        self.rate.is_some() && !self.finished
    }

    /// The next device sample at `device_rate`, or `None` when the queue cannot give it yet.
    /// Device sample `k` lies at queue time `k * rate / device_rate`; it is interpolated between
    /// the queued samples around that time, and once the audio is finished the last sample holds
    /// for the time after it.
    fn next(&mut self, device_rate: u32) -> Option<f32> {
        let rate = u128::from(self.rate?.get());
        let device_rate = u128::from(device_rate);
        let time = u128::from(self.outputs) * rate;
        let index = u64::try_from(time / device_rate).ok()?;
        let remainder = time % device_rate;
        while self.head < index && self.samples.pop_front().is_some() {
            self.head += 1;
        }
        if self.head != index {
            return None;
        }
        let first = *self.samples.front()?;
        let sample = if remainder == 0 {
            first
        } else if let Some(&second) = self.samples.get(1) {
            first + (second - first) * (remainder as f32 / device_rate as f32)
        } else if self.finished {
            first
        } else {
            return None;
        };
        self.outputs += 1;
        Some(sample)
    }

    /// The queue index of the first sample not yet handed to the device at `device_rate`: the
    /// sample after the last one whose time the device reached, or, where the next device sample
    /// lies further on, as when downsampling skips samples, the sample at that next time; never
    /// past what was queued. Once the device was handed the last queued sample, everything is
    /// played, even while the next device sample waits for a sample not yet pushed.
    fn played_index(&self, device_rate: u32) -> u64 {
        let Some(rate) = self.rate else {
            return 0;
        };
        let Some(last) = self.outputs.checked_sub(1) else {
            return 0;
        };
        let (rate, device_rate) = (u128::from(rate.get()), u128::from(device_rate));
        let after_last = u128::from(last) * rate / device_rate + 1;
        let next = u128::from(self.outputs) * rate / device_rate;
        u64::try_from(after_last.max(next))
            .unwrap_or(u64::MAX)
            .min(self.end)
    }

    /// How many queued samples were not yet handed to the device at `device_rate`.
    fn queued(&self, device_rate: u32) -> usize {
        usize::try_from(self.end - self.played_index(device_rate)).unwrap_or(usize::MAX)
    }

    /// The played position on the frames' clock, if a frame was pushed since the last stop.
    fn played(&self, device_rate: u32) -> Option<SampleClockPosition> {
        let index = self.played_index(device_rate);
        let segment = self
            .segments
            .iter()
            .rev()
            .find(|segment| segment.start <= index)?;
        Some(
            SampleClockPosition::new(segment.position + (index - segment.start))
                .unwrap_or(SampleClockPosition::MAX),
        )
    }

    /// Forgets the segments wholly played: every one but the last that starts at or before the
    /// played index.
    fn prune(&mut self, device_rate: u32) {
        let index = self.played_index(device_rate);
        while self.segments.get(1).is_some_and(|next| next.start <= index) {
            self.segments.pop_front();
        }
    }
}

/// The queue between a [`Speaker`] and its output device's callback.
#[derive(Debug)]
pub struct PlayoutBuffer {
    device: DeviceConfig,
    buffer: Duration,
    queue: Mutex<Queue>,
    due: AtomicBool,
    unplayed: AtomicUsize,
    capacity: AtomicUsize,
    rate: AtomicU32,
    stops: AtomicU64,
    underruns: AtomicU64,
    errors: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl PlayoutBuffer {
    /// An empty queue for a device running at `device`, holding at most `buffer` of audio.
    #[must_use]
    pub fn new(device: DeviceConfig, buffer: Duration) -> Self {
        Self {
            device,
            buffer,
            queue: Mutex::new(Queue {
                rate: None,
                samples: VecDeque::new(),
                capacity: 0,
                segments: VecDeque::with_capacity(SEGMENTS),
                head: 0,
                end: 0,
                outputs: 0,
                finished: false,
            }),
            due: AtomicBool::new(false),
            unplayed: AtomicUsize::new(0),
            capacity: AtomicUsize::new(0),
            rate: AtomicU32::new(0),
            stops: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            last_error: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Mirrors what `queue` holds in the atomics read without the lock, so that reading the
    /// queued samples and refusing a frame to a full queue never take the lock the callback
    /// wants.
    fn publish(&self, queue: &Queue) {
        self.unplayed.store(
            queue.queued(self.device.sample_rate.get()),
            Ordering::Relaxed,
        );
        self.capacity.store(queue.capacity, Ordering::Relaxed);
        self.rate
            .store(queue.rate.map_or(0, NonZeroU32::get), Ordering::Relaxed);
    }

    /// The device callback: fills `interleaved` with the next queued audio at the device rate,
    /// the same sample on every channel, and silence past what is queued. Never blocks and never
    /// allocates.
    pub fn fill(&self, interleaved: &mut [f32]) {
        let mut queue = match self.queue.try_lock() {
            Ok(queue) => queue,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                interleaved.fill(0.0);
                if self.due.load(Ordering::Relaxed) {
                    self.underruns.fetch_add(1, Ordering::Relaxed);
                }
                return;
            }
        };
        let channels = usize::from(self.device.channels.get());
        let device_rate = self.device.sample_rate.get();
        let mut dry = false;
        for frame in interleaved.chunks_mut(channels) {
            match (!dry).then(|| queue.next(device_rate)).flatten() {
                Some(sample) => frame.fill(sample),
                None => {
                    dry = true;
                    frame.fill(0.0);
                }
            }
        }
        if dry && queue.due() {
            self.underruns.fetch_add(1, Ordering::Relaxed);
        }
        self.unplayed
            .store(queue.queued(device_rate), Ordering::Relaxed);
    }

    /// Records an error the device stream reported.
    pub fn report_error(&self, message: &str) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        *self
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(message.to_owned());
    }

    /// How many callbacks, or parts of one, wrote silence while audio was due.
    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    fn push(&self, frame: &AudioFrame) -> Result<(), PushError> {
        let mut samples = decode(frame).map_err(PushError::Malformed)?;
        for sample in &mut samples {
            *sample = playable(*sample);
        }
        let rate = frame.format.sample_rate;
        if self.rate.load(Ordering::Relaxed) == rate.get() {
            let capacity = self.capacity.load(Ordering::Relaxed);
            let room = capacity.saturating_sub(self.unplayed.load(Ordering::Relaxed));
            if samples.len() <= capacity && samples.len() > room {
                return Err(PushError::Full);
            }
        }
        let mut queue = self.lock();
        let capacity = match queue.rate {
            Some(expected) if expected != rate => {
                return Err(PushError::RateChanged {
                    expected,
                    got: rate,
                });
            }
            Some(_) => queue.capacity,
            None => usize::try_from(samples_in(self.buffer, rate.get()))
                .unwrap_or(usize::MAX)
                .max(1),
        };
        if samples.len() > capacity {
            return Err(PushError::TooLarge {
                samples: samples.len() as u64,
                capacity,
            });
        }
        let device_rate = self.device.sample_rate.get();
        if samples.len() > capacity.saturating_sub(queue.queued(device_rate)) {
            return Err(PushError::Full);
        }
        queue.prune(device_rate);
        let position = frame.position.get();
        let continues = queue
            .segments
            .back()
            .is_some_and(|last| last.position + (queue.end - last.start) == position);
        if !continues {
            if queue.segments.len() >= SEGMENTS {
                return Err(PushError::Full);
            }
            let start = queue.end;
            queue.segments.push_back(Segment { start, position });
        }
        if queue.rate.is_none() {
            queue.rate = Some(rate);
            queue.capacity = capacity;
            let spare = capacity.saturating_add(2);
            queue.samples.reserve(spare);
        }
        queue.samples.extend(samples.iter().copied());
        queue.end += samples.len() as u64;
        queue.finished = false;
        self.publish(&queue);
        self.due.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn finish(&self) {
        self.lock().finished = true;
        self.due.store(false, Ordering::Relaxed);
    }

    fn stop(&self) -> Option<SampleClockPosition> {
        let mut queue = self.lock();
        let reached = queue.played(self.device.sample_rate.get());
        queue.rate = None;
        queue.samples.clear();
        queue.segments.clear();
        queue.head = 0;
        queue.end = 0;
        queue.outputs = 0;
        queue.finished = false;
        queue.capacity = 0;
        self.publish(&queue);
        self.stops.fetch_add(1, Ordering::Relaxed);
        self.due.store(false, Ordering::Relaxed);
        reached
    }

    fn played(&self) -> Option<SampleClockPosition> {
        self.lock().played(self.device.sample_rate.get())
    }

    fn queued(&self) -> usize {
        self.unplayed.load(Ordering::Relaxed)
    }

    fn stops(&self) -> u64 {
        self.stops.load(Ordering::Relaxed)
    }

    fn errors(&self) -> (u64, Option<String>) {
        (
            self.errors.load(Ordering::Relaxed),
            self.last_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
    }
}

/// What a [`Speaker`] has done: its underruns and the errors the device stream reported, the
/// last with its message.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlayoutStats {
    pub underruns: u64,
    pub stream_errors: u64,
    pub last_error: Option<String>,
}

/// A speaker: an output device stream playing the frames pushed to it. Every method takes
/// `&self`, so one thread can push while another stops it for a barge-in. Dropping it stops the
/// stream.
#[derive(Debug)]
pub struct Speaker {
    device: ResolvedDevice,
    buffer: Arc<PlayoutBuffer>,
    _stream: StreamGuard,
}

impl Speaker {
    /// Opens the output device `device` selects on `host` and starts playing silence.
    ///
    /// # Errors
    ///
    /// What the host answered.
    pub fn open(
        host: &dyn AudioHost,
        device: &DeviceSelector,
        config: PlayoutConfig,
    ) -> Result<Self, DeviceError> {
        let buffer = config.buffer;
        let opened = host.open_output(
            device,
            Box::new(move |device: &ResolvedDevice| {
                Arc::new(PlayoutBuffer::new(device.config, buffer))
            }),
        )?;
        Ok(Self {
            device: opened.device,
            buffer: opened.buffer,
            _stream: opened.guard,
        })
    }

    /// The device playing.
    #[must_use]
    pub const fn device(&self) -> &ResolvedDevice {
        &self.device
    }

    /// Queues `frame` if the queue has room for it now.
    ///
    /// # Errors
    ///
    /// [`PushError`]; nothing of the frame is queued.
    pub fn try_push(&self, frame: &AudioFrame) -> Result<(), PushError> {
        self.buffer.push(frame)
    }

    /// Queues `frame`, waiting at most `patience` for room.
    ///
    /// # Errors
    ///
    /// [`PushError::Full`] when no room came in time, [`PushError::Stopped`] when the sink was
    /// stopped meanwhile, and the other refusals of [`Speaker::try_push`] at once.
    pub fn push(&self, frame: &AudioFrame, patience: Duration) -> Result<(), PushError> {
        let deadline = Instant::now() + patience;
        let stops = self.buffer.stops();
        loop {
            if self.buffer.stops() != stops {
                return Err(PushError::Stopped);
            }
            match self.buffer.push(frame) {
                Err(PushError::Full) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(PushError::Full);
                    }
                    thread::sleep(left.min(POLL));
                }
                answer => return answer,
            }
        }
    }

    /// Marks the end of the audio pushed so far: running dry after it is no underrun. A frame
    /// pushed later continues the same clock.
    pub fn finish(&self) {
        self.buffer.finish();
    }

    /// Clears every queued sample at once and answers the played position reached, or `None`
    /// when no frame was pushed since the last stop. The next frame pushed starts a new clock at
    /// its own position, at any rate.
    pub fn stop(&self) -> Option<SampleClockPosition> {
        self.buffer.stop()
    }

    /// The played position: the position, on the frames' clock, of the first sample not yet
    /// handed to the device; `None` when no frame was pushed since the last stop.
    #[must_use]
    pub fn played(&self) -> Option<SampleClockPosition> {
        self.buffer.played()
    }

    /// How many samples, at the frames' rate, are queued and not yet played.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.buffer.queued()
    }

    /// Waits at most `patience` until every queued sample was played.
    #[must_use]
    pub fn wait_played(&self, patience: Duration) -> bool {
        let deadline = Instant::now() + patience;
        loop {
            if self.queued() == 0 {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            thread::sleep(left.min(POLL));
        }
    }

    /// What the speaker has done so far.
    #[must_use]
    pub fn stats(&self) -> PlayoutStats {
        let (stream_errors, last_error) = self.buffer.errors();
        PlayoutStats {
            underruns: self.buffer.underruns(),
            stream_errors,
            last_error,
        }
    }
}
