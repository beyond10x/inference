//! Microphone capture and speaker playout for the inference framework.
//!
//! `inference-audio-device` is an adapter: it turns audio devices into sources and sinks of
//! `inference-audio` frames, so a voice pipeline can be tried on a laptop.
//!
//! * A [`Microphone`] captures from an input device and produces mono frames in a requested
//!   [`CaptureConfig`]: PCM16 at 16 kHz in 20 ms frames by default, resampled and down-mixed
//!   from the device's native format, numbered on the stream's own sample clock.
//! * A [`Speaker`] plays frames to an output device, resampled to the device. It reports the
//!   played position, the sample clock position of the first sample not yet handed to the device,
//!   and [`Speaker::stop`] clears unplayed audio at once and answers that position, which is
//!   where a barge-in truncates a model's output.
//! * Between each device callback and its consumer sits a bounded buffer the callback never
//!   blocks on and never allocates in: capture counts the audio it drops, playout its underruns.
//!
//! Devices are reached through [`AudioHost`]: `CpalHost` (feature `device`, on by default)
//! over cpal, and [`fake::FakeHost`], which tests drive callback by callback without audio
//! hardware. With feature `pipeline`, `node::MicrophoneSource` and `node::SpeakerSink` are
//! the same handles as pipeline nodes.
//!
//! The resampler is linear and has no anti-aliasing filter ([`convert::LinearResampler`]).
//! There is no echo cancellation or noise suppression.

pub mod capture;
pub mod convert;
#[cfg(feature = "device")]
mod cpal_host;
pub mod fake;
pub mod host;
#[cfg(feature = "pipeline")]
pub mod node;
pub mod playout;

pub use capture::{CaptureBuffer, CaptureConfig, CaptureStats, Microphone};
#[cfg(feature = "device")]
pub use cpal_host::CpalHost;
pub use host::{
    AudioHost, DeviceConfig, DeviceError, DeviceInfo, DeviceSelector, Direction, OpenStream,
    ResolvedDevice, StreamGuard,
};
pub use playout::{PlayoutBuffer, PlayoutConfig, PlayoutStats, PushError, Speaker};
