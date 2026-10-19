//! The device layer: [`AudioHost`], the one trait both the cpal backend and the
//! [`FakeHost`](crate::fake::FakeHost) of the tests implement, and what it lists, selects and
//! opens.
//!
//! A host lists its devices, resolves a [`DeviceSelector`] to one device of a direction and its
//! native [`DeviceConfig`], and runs a stream on it whose device callback writes into a
//! [`CaptureBuffer`] or reads from a [`PlayoutBuffer`]. The buffer is made once the native config
//! is known, by the function the caller hands in, so it is sized for the device before the
//! first callback. The stream runs until its [`StreamGuard`] is dropped.

use std::error::Error;
use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};
use std::sync::{Arc, Mutex};

use crate::capture::CaptureBuffer;
use crate::playout::PlayoutBuffer;

/// Which way audio flows through a device: from it, a microphone, or to it, a speaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Capture: the device produces audio.
    Input,
    /// Playout: the device consumes audio.
    Output,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Input => "input",
            Self::Output => "output",
        })
    }
}

/// One device a host lists: its name, the directions it supports, and whether it is the host's
/// default device of each.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceInfo {
    pub name: String,
    pub input: bool,
    pub output: bool,
    pub default_input: bool,
    pub default_output: bool,
}

/// Which device to open: the host's default of the direction, or the device of that name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum DeviceSelector {
    /// The host's default device of the direction.
    #[default]
    Default,
    /// The device whose [`DeviceInfo::name`] is exactly this.
    Name(String),
}

/// The native format of a device stream: its sample rate in hertz and its channel count. The
/// samples cross the device layer as interleaved `f32` at full scale 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceConfig {
    pub sample_rate: NonZeroU32,
    pub channels: NonZeroU16,
}

/// The device a selector resolved to: its name, its direction and the config its stream runs
/// at.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedDevice {
    pub name: String,
    pub direction: Direction,
    pub config: DeviceConfig,
}

/// Why a device could not be listed, resolved or opened.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DeviceError {
    /// The host has no default device of the direction.
    NoDefault(Direction),
    /// No device of the direction carries the name.
    NotFound { name: String, direction: Direction },
    /// A device of the direction is already open on a host that runs one at a time.
    Busy(Direction),
    /// The requested capture or playout configuration cannot be met; the detail says why.
    InvalidConfig(String),
    /// The device or its native format is not supported by this adapter; the detail says why.
    Unsupported(String),
    /// The audio backend failed; the detail is its message.
    Backend(String),
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDefault(direction) => write!(f, "the host has no default {direction} device"),
            Self::NotFound { name, direction } => {
                write!(f, "no {direction} device is named {name:?}")
            }
            Self::Busy(direction) => write!(f, "an {direction} stream is already open"),
            Self::InvalidConfig(detail) => write!(f, "invalid configuration: {detail}"),
            Self::Unsupported(detail) => write!(f, "unsupported device: {detail}"),
            Self::Backend(detail) => write!(f, "audio backend error: {detail}"),
        }
    }
}

impl Error for DeviceError {}

/// Keeps a device stream running; dropping it stops the stream and releases the device.
pub struct StreamGuard {
    _running: Mutex<Box<dyn Send>>,
}

impl StreamGuard {
    /// A guard whose drop drops `running`, which stops the stream it holds.
    pub fn new(running: impl Send + 'static) -> Self {
        Self {
            _running: Mutex::new(Box::new(running)),
        }
    }
}

impl fmt::Debug for StreamGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamGuard").finish_non_exhaustive()
    }
}

/// A running device stream: the device it runs on, the buffer its callback serves, and the guard
/// that keeps it running.
#[derive(Debug)]
pub struct OpenStream<B> {
    pub device: ResolvedDevice,
    pub buffer: Arc<B>,
    pub guard: StreamGuard,
}

/// Makes the buffer `B` of a stream once its device is resolved.
pub type MakeBuffer<B> = Box<dyn FnOnce(&ResolvedDevice) -> Arc<B> + Send>;

/// Makes the capture buffer of an input stream once its device is resolved.
pub type MakeCapture = MakeBuffer<CaptureBuffer>;

/// Makes the playout buffer of an output stream once its device is resolved.
pub type MakePlayout = MakeBuffer<PlayoutBuffer>;

/// An audio host: lists devices and runs capture and playout streams on them. The cpal backend
/// (`CpalHost`, feature `device`) and the fake device of the tests
/// ([`FakeHost`](crate::fake::FakeHost)) implement it.
pub trait AudioHost: Send + Sync {
    /// Every device of the host.
    ///
    /// # Errors
    ///
    /// [`DeviceError::Backend`] when the host cannot enumerate its devices.
    fn devices(&self) -> Result<Vec<DeviceInfo>, DeviceError>;

    /// Starts a capture stream on the input device `device` selects, at its native config, its
    /// callback writing into the buffer `make` returns.
    ///
    /// # Errors
    ///
    /// [`DeviceError`] when no device matches or the stream cannot be started.
    fn open_input(
        &self,
        device: &DeviceSelector,
        make: MakeCapture,
    ) -> Result<OpenStream<CaptureBuffer>, DeviceError>;

    /// Starts a playout stream on the output device `device` selects, at its native config, its
    /// callback reading from the buffer `make` returns.
    ///
    /// # Errors
    ///
    /// [`DeviceError`] when no device matches or the stream cannot be started.
    fn open_output(
        &self,
        device: &DeviceSelector,
        make: MakePlayout,
    ) -> Result<OpenStream<PlayoutBuffer>, DeviceError>;
}

/// The device of `direction` that `selector` picks out of `devices`.
///
/// # Errors
///
/// [`DeviceError::NoDefault`] or [`DeviceError::NotFound`].
pub fn select<'d>(
    devices: &'d [DeviceInfo],
    direction: Direction,
    selector: &DeviceSelector,
) -> Result<&'d DeviceInfo, DeviceError> {
    let of_direction = |device: &&DeviceInfo| match direction {
        Direction::Input => device.input,
        Direction::Output => device.output,
    };
    match selector {
        DeviceSelector::Default => devices
            .iter()
            .filter(of_direction)
            .find(|device| match direction {
                Direction::Input => device.default_input,
                Direction::Output => device.default_output,
            })
            .ok_or(DeviceError::NoDefault(direction)),
        DeviceSelector::Name(name) => devices
            .iter()
            .filter(of_direction)
            .find(|device| &device.name == name)
            .ok_or_else(|| DeviceError::NotFound {
                name: name.clone(),
                direction,
            }),
    }
}
