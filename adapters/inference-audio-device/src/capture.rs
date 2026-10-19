//! Capture: a [`Microphone`] producing `inference-audio` frames from an input device.
//!
//! The device callback hands interleaved samples to a [`CaptureBuffer`], which down-mixes them to
//! mono and keeps them in a ring of fixed capacity at the device rate. The ring has one writer,
//! the callback, and one reader, the microphone, and takes no lock: each slot is an atomic word
//! and the two sides meet only at two atomic counters. So the callback never blocks, never
//! allocates and never waits for the reader; a sample frame that finds the ring full is dropped
//! and counted. The consumer drains the ring, resamples to the requested rate and slices
//! fixed-length frames, numbered on the stream's own sample clock from position 0. Dropped audio
//! leaves no gap on that clock: the next frame continues where the last one ended, and
//! [`CaptureStats::dropped_device_frames`] says how much was lost.

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};

use crate::convert::{LinearResampler, encode, finite};
use crate::host::{AudioHost, DeviceError, DeviceSelector, ResolvedDevice, StreamGuard};

/// How often a waiting consumer looks at the ring again.
const POLL: Duration = Duration::from_millis(2);

/// What a [`Microphone`] produces and how much it buffers: mono frames of `encoding` at
/// `sample_rate` hertz, each `frame` long, and at most `buffer` of device audio held between the
/// device callback and the consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CaptureConfig {
    pub sample_rate: NonZeroU32,
    pub encoding: AudioEncoding,
    pub frame: Duration,
    pub buffer: Duration,
}

impl Default for CaptureConfig {
    /// Mono PCM16 at 16 kHz in 20 ms frames, with half a second of buffering.
    fn default() -> Self {
        Self {
            sample_rate: NonZeroU32::new(16_000).unwrap_or(NonZeroU32::MIN),
            encoding: AudioEncoding::PcmS16le,
            frame: Duration::from_millis(20),
            buffer: Duration::from_millis(500),
        }
    }
}

impl CaptureConfig {
    /// The format of the frames: mono, at the configured encoding and rate.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        AudioFormat {
            encoding: self.encoding,
            sample_rate: self.sample_rate,
            channels: NonZeroU16::MIN,
        }
    }

    /// The samples of one frame: `frame` at the sample rate, truncated.
    ///
    /// # Errors
    ///
    /// [`DeviceError::InvalidConfig`] when a frame holds no sample or more than `u32::MAX`.
    pub fn frame_samples(&self) -> Result<u32, DeviceError> {
        let samples = samples_in(self.frame, self.sample_rate.get());
        match u32::try_from(samples) {
            Ok(samples) if samples > 0 => Ok(samples),
            _ => Err(DeviceError::InvalidConfig(format!(
                "a frame of {:?} at {} Hz holds {samples} samples; it must hold 1 to {}",
                self.frame,
                self.sample_rate,
                u32::MAX
            ))),
        }
    }
}

/// The samples `time` holds at `rate` hertz, truncated.
pub(crate) fn samples_in(time: Duration, rate: u32) -> u128 {
    time.as_nanos() * u128::from(rate) / 1_000_000_000
}

/// The ring between an input device's callback and the [`Microphone`] reading it: mono samples
/// at the device rate, at most `capacity` of them, each slot holding the bits of one `f32`.
/// `written` and `read` count every sample ever written and read; the slot of sample `n` is
/// `n % capacity`. The writer publishes `written` after filling the slots, and the reader
/// publishes `read` after emptying them, so neither side ever touches a slot the other owns.
#[derive(Debug)]
pub struct CaptureBuffer {
    slots: Box<[AtomicU32]>,
    written: AtomicU64,
    read: AtomicU64,
    writing: AtomicBool,
    channels: usize,
    dropped: AtomicU64,
    errors: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl CaptureBuffer {
    /// An empty ring for a device of `channels` channels holding at most `capacity` mono
    /// samples, at least one; its storage is allocated here, never in the callback.
    #[must_use]
    pub fn new(channels: NonZeroU16, capacity: usize) -> Self {
        Self {
            slots: (0..capacity.max(1)).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            read: AtomicU64::new(0),
            writing: AtomicBool::new(false),
            channels: usize::from(channels.get()),
            dropped: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            last_error: Mutex::new(None),
        }
    }

    fn slot(&self, sample: u64) -> &AtomicU32 {
        &self.slots[(sample % self.slots.len() as u64) as usize]
    }

    /// The device callback: down-mixes each whole frame of `interleaved` to one mono sample and
    /// keeps it, made [finite](crate::convert::finite), or drops and counts it when the ring is
    /// full. A finite sample is kept as it is, headroom included. Never blocks, never allocates and
    /// never waits for the reader. The ring has one writer: a call that overlaps another, which
    /// no device does, drops its frames and counts them instead of writing.
    pub fn write(&self, interleaved: &[f32]) {
        let frames = interleaved.chunks_exact(self.channels);
        if self.writing.swap(true, Ordering::Acquire) {
            self.dropped
                .fetch_add(frames.len() as u64, Ordering::Relaxed);
            return;
        }
        let read = self.read.load(Ordering::Acquire);
        let mut written = self.written.load(Ordering::Relaxed);
        let capacity = self.slots.len() as u64;
        let scale = 1.0 / self.channels as f32;
        let mut dropped = 0_u64;
        for frame in frames {
            if written - read < capacity {
                let sample = finite(frame.iter().sum::<f32>() * scale);
                self.slot(written)
                    .store(sample.to_bits(), Ordering::Relaxed);
                written += 1;
            } else {
                dropped += 1;
            }
        }
        self.written.store(written, Ordering::Release);
        self.writing.store(false, Ordering::Release);
        if dropped > 0 {
            self.dropped.fetch_add(dropped, Ordering::Relaxed);
        }
    }

    /// Records an error the device stream reported.
    pub fn report_error(&self, message: &str) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        *self
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(message.to_owned());
    }

    /// How many device sample frames the callback dropped, the ring being full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// How many mono samples the ring holds.
    #[must_use]
    pub fn len(&self) -> usize {
        let read = self.read.load(Ordering::Acquire);
        let written = self.written.load(Ordering::Acquire);
        usize::try_from(written.saturating_sub(read)).unwrap_or(usize::MAX)
    }

    /// Whether the ring holds no sample.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The most mono samples the ring holds.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Moves every sample the ring holds to the end of `into`. The ring's one reader: only the
    /// [`Microphone`] owning the ring calls it, through `&mut self`.
    fn take(&self, into: &mut Vec<f32>) {
        let written = self.written.load(Ordering::Acquire);
        let mut read = self.read.load(Ordering::Relaxed);
        into.reserve(usize::try_from(written - read).unwrap_or(0));
        while read < written {
            into.push(f32::from_bits(self.slot(read).load(Ordering::Relaxed)));
            read += 1;
        }
        self.read.store(read, Ordering::Release);
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

/// What a [`Microphone`] has done: the frames it produced, the device sample frames its callback
/// dropped, and the errors the device stream reported, the last with its message.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CaptureStats {
    pub frames: u64,
    pub dropped_device_frames: u64,
    pub stream_errors: u64,
    pub last_error: Option<String>,
}

/// A microphone: an input device stream producing mono frames in the [`CaptureConfig`]'s format.
/// Dropping it stops the stream.
#[derive(Debug)]
pub struct Microphone {
    device: ResolvedDevice,
    config: CaptureConfig,
    buffer: Arc<CaptureBuffer>,
    resampler: LinearResampler,
    frame_samples: usize,
    drained: Vec<f32>,
    pending: Vec<f32>,
    next_position: u64,
    frames: u64,
    _stream: StreamGuard,
}

impl Microphone {
    /// Opens the input device `device` selects on `host` and starts capturing.
    ///
    /// # Errors
    ///
    /// [`DeviceError::InvalidConfig`] when `config` makes empty frames; otherwise what the host
    /// answered.
    pub fn open(
        host: &dyn AudioHost,
        device: &DeviceSelector,
        config: CaptureConfig,
    ) -> Result<Self, DeviceError> {
        let frame_samples = config.frame_samples()?;
        let buffer = config.buffer;
        let opened = host.open_input(
            device,
            Box::new(move |device: &ResolvedDevice| {
                let rate = device.config.sample_rate.get();
                let capacity = usize::try_from(samples_in(buffer, rate))
                    .unwrap_or(usize::MAX)
                    .max(1);
                Arc::new(CaptureBuffer::new(device.config.channels, capacity))
            }),
        )?;
        Ok(Self {
            resampler: LinearResampler::new(opened.device.config.sample_rate, config.sample_rate),
            device: opened.device,
            config,
            buffer: opened.buffer,
            frame_samples: frame_samples as usize,
            drained: Vec::new(),
            pending: Vec::new(),
            next_position: 0,
            frames: 0,
            _stream: opened.guard,
        })
    }

    /// The device capturing.
    #[must_use]
    pub const fn device(&self) -> &ResolvedDevice {
        &self.device
    }

    /// The format of the frames.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.config.format()
    }

    /// The next frame, if the device has captured enough for one; never waits.
    pub fn try_frame(&mut self) -> Option<AudioFrame> {
        if self.pending.len() < self.frame_samples {
            self.drained.clear();
            self.buffer.take(&mut self.drained);
            self.resampler.process(&self.drained, &mut self.pending);
        }
        if self.pending.len() < self.frame_samples {
            return None;
        }
        let frame = self.frame_of(&self.pending[..self.frame_samples]);
        self.pending.drain(..self.frame_samples);
        self.next_position += self.frame_samples as u64;
        self.frames += 1;
        Some(frame)
    }

    /// The next frame, waiting at most `patience` for the device to capture enough for one.
    pub fn next_frame(&mut self, patience: Duration) -> Option<AudioFrame> {
        let deadline = Instant::now() + patience;
        loop {
            if let Some(frame) = self.try_frame() {
                return Some(frame);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            thread::sleep(left.min(POLL));
        }
    }

    /// What the microphone has done so far.
    #[must_use]
    pub fn stats(&self) -> CaptureStats {
        let (stream_errors, last_error) = self.buffer.errors();
        CaptureStats {
            frames: self.frames,
            dropped_device_frames: self.buffer.dropped(),
            stream_errors,
            last_error,
        }
    }

    /// The frame of `samples` at the next position.
    fn frame_of(&self, samples: &[f32]) -> AudioFrame {
        AudioFrame {
            format: self.format(),
            position: SampleClockPosition::new(self.next_position)
                .unwrap_or(SampleClockPosition::MAX),
            sample_count: u32::try_from(samples.len()).unwrap_or(u32::MAX),
            data: encode(samples, self.config.encoding),
        }
    }
}
