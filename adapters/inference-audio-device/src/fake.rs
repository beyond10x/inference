//! A fake audio host for tests without audio hardware: [`FakeHost`] implements the same
//! [`AudioHost`] the cpal backend does, and the test plays the device by calling the stream's
//! callback itself, [`FakeHost::feed`] for an input and [`FakeHost::pull`] for an output, so every
//! callback happens on the test's thread at the moment it chooses.
//!
//! It runs at most one input and one output stream at a time; opening a second of a direction
//! while one runs answers [`DeviceError::Busy`]. Dropping a stream's guard closes it.

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::capture::CaptureBuffer;
use crate::host::{
    AudioHost, DeviceConfig, DeviceError, DeviceInfo, DeviceSelector, Direction, MakeCapture,
    MakePlayout, OpenStream, ResolvedDevice, StreamGuard, select,
};
use crate::playout::PlayoutBuffer;

/// One device of a [`FakeHost`]: what it lists and the config its streams run at.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FakeDevice {
    pub info: DeviceInfo,
    pub config: DeviceConfig,
}

impl FakeDevice {
    /// An input device named `name` at `sample_rate` hertz with `channels` channels, the default
    /// input when `default` holds.
    #[must_use]
    pub fn input(name: &str, sample_rate: NonZeroU32, channels: NonZeroU16, default: bool) -> Self {
        Self {
            info: DeviceInfo {
                name: name.to_owned(),
                input: true,
                output: false,
                default_input: default,
                default_output: false,
            },
            config: DeviceConfig {
                sample_rate,
                channels,
            },
        }
    }

    /// An output device named `name` at `sample_rate` hertz with `channels` channels, the
    /// default output when `default` holds.
    #[must_use]
    pub fn output(
        name: &str,
        sample_rate: NonZeroU32,
        channels: NonZeroU16,
        default: bool,
    ) -> Self {
        Self {
            info: DeviceInfo {
                name: name.to_owned(),
                input: false,
                output: true,
                default_input: false,
                default_output: default,
            },
            config: DeviceConfig {
                sample_rate,
                channels,
            },
        }
    }
}

/// The streams a [`FakeHost`] runs.
#[derive(Debug, Default)]
struct Streams {
    input: Option<(u64, Arc<CaptureBuffer>)>,
    output: Option<(u64, NonZeroU16, Arc<PlayoutBuffer>)>,
    opened: u64,
}

/// Closes the stream it names when dropped.
struct Close {
    streams: Arc<Mutex<Streams>>,
    direction: Direction,
    id: u64,
}

impl Drop for Close {
    fn drop(&mut self) {
        let mut streams = lock(&self.streams);
        match self.direction {
            Direction::Input => {
                if streams.input.as_ref().is_some_and(|(id, _)| *id == self.id) {
                    streams.input = None;
                }
            }
            Direction::Output => {
                if streams
                    .output
                    .as_ref()
                    .is_some_and(|(id, ..)| *id == self.id)
                {
                    streams.output = None;
                }
            }
        }
    }
}

fn lock(streams: &Mutex<Streams>) -> MutexGuard<'_, Streams> {
    streams.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A fake audio host; see the module documentation. A clone drives the same streams.
#[derive(Debug, Clone)]
pub struct FakeHost {
    devices: Vec<FakeDevice>,
    streams: Arc<Mutex<Streams>>,
}

impl FakeHost {
    /// A host of `devices`.
    #[must_use]
    pub fn new(devices: Vec<FakeDevice>) -> Self {
        Self {
            devices,
            streams: Arc::default(),
        }
    }

    /// A host of a default stereo input and a default stereo output, both at 48 kHz, named
    /// `fake microphone` and `fake speaker`.
    #[must_use]
    pub fn laptop() -> Self {
        let rate = NonZeroU32::new(48_000).unwrap_or(NonZeroU32::MIN);
        let stereo = NonZeroU16::new(2).unwrap_or(NonZeroU16::MIN);
        Self::new(vec![
            FakeDevice::input("fake microphone", rate, stereo, true),
            FakeDevice::output("fake speaker", rate, stereo, true),
        ])
    }

    /// Calls the running input stream's callback with `interleaved`; false when no input stream
    /// runs.
    pub fn feed(&self, interleaved: &[f32]) -> bool {
        let buffer = lock(&self.streams)
            .input
            .as_ref()
            .map(|(_, b)| Arc::clone(b));
        buffer.is_some_and(|buffer| {
            buffer.write(interleaved);
            true
        })
    }

    /// Calls the running output stream's callback for `frames` device frames and answers the
    /// interleaved samples it wrote; `None` when no output stream runs.
    #[must_use]
    pub fn pull(&self, frames: usize) -> Option<Vec<f32>> {
        let (channels, buffer) = {
            let streams = lock(&self.streams);
            let (_, channels, buffer) = streams.output.as_ref()?;
            (usize::from(channels.get()), Arc::clone(buffer))
        };
        let mut interleaved = vec![f32::NAN; frames * channels];
        buffer.fill(&mut interleaved);
        Some(interleaved)
    }

    /// Reports `message` as an error of the running input stream; false when none runs.
    pub fn fail_input(&self, message: &str) -> bool {
        let buffer = lock(&self.streams)
            .input
            .as_ref()
            .map(|(_, b)| Arc::clone(b));
        buffer.is_some_and(|buffer| {
            buffer.report_error(message);
            true
        })
    }

    /// Whether an input stream runs.
    #[must_use]
    pub fn input_open(&self) -> bool {
        lock(&self.streams).input.is_some()
    }

    /// Whether an output stream runs.
    #[must_use]
    pub fn output_open(&self) -> bool {
        lock(&self.streams).output.is_some()
    }

    fn resolve(
        &self,
        direction: Direction,
        selector: &DeviceSelector,
    ) -> Result<ResolvedDevice, DeviceError> {
        let infos: Vec<DeviceInfo> = self.devices.iter().map(|d| d.info.clone()).collect();
        let info = select(&infos, direction, selector)?;
        let device = self
            .devices
            .iter()
            .find(|device| device.info == *info)
            .ok_or_else(|| DeviceError::Backend("the fake device vanished".to_owned()))?;
        Ok(ResolvedDevice {
            name: device.info.name.clone(),
            direction,
            config: device.config,
        })
    }
}

impl AudioHost for FakeHost {
    fn devices(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        Ok(self
            .devices
            .iter()
            .map(|device| device.info.clone())
            .collect())
    }

    fn open_input(
        &self,
        device: &DeviceSelector,
        make: MakeCapture,
    ) -> Result<OpenStream<CaptureBuffer>, DeviceError> {
        let device = self.resolve(Direction::Input, device)?;
        let mut streams = lock(&self.streams);
        if streams.input.is_some() {
            return Err(DeviceError::Busy(Direction::Input));
        }
        let buffer = make(&device);
        streams.opened += 1;
        let id = streams.opened;
        streams.input = Some((id, Arc::clone(&buffer)));
        drop(streams);
        Ok(OpenStream {
            device,
            buffer,
            guard: StreamGuard::new(Close {
                streams: Arc::clone(&self.streams),
                direction: Direction::Input,
                id,
            }),
        })
    }

    fn open_output(
        &self,
        device: &DeviceSelector,
        make: MakePlayout,
    ) -> Result<OpenStream<PlayoutBuffer>, DeviceError> {
        let device = self.resolve(Direction::Output, device)?;
        let mut streams = lock(&self.streams);
        if streams.output.is_some() {
            return Err(DeviceError::Busy(Direction::Output));
        }
        let buffer = make(&device);
        streams.opened += 1;
        let id = streams.opened;
        streams.output = Some((id, device.config.channels, Arc::clone(&buffer)));
        drop(streams);
        Ok(OpenStream {
            device,
            buffer,
            guard: StreamGuard::new(Close {
                streams: Arc::clone(&self.streams),
                direction: Direction::Output,
                id,
            }),
        })
    }
}
