//! The cpal backend: [`CpalHost`] runs capture and playout streams on the platform's default
//! audio host through cpal (ALSA on Linux, which reaches PipeWire and PulseAudio through their
//! ALSA plugins; CoreAudio; WASAPI).
//!
//! Each stream lives on a thread of its own, which builds it, starts it and holds it until the
//! stream's [`StreamGuard`] is dropped, so no handle depends on whether a platform's stream may
//! cross threads. A stream runs at its device's default config. Its callback converts the
//! device's sample format to and from `f32` in chunks on the stack, so it never allocates.

use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig, SupportedStreamConfig};

use crate::capture::CaptureBuffer;
use crate::host::{
    AudioHost, DeviceConfig, DeviceError, DeviceInfo, DeviceSelector, Direction, MakeBuffer,
    MakeCapture, MakePlayout, OpenStream, ResolvedDevice, StreamGuard,
};
use crate::playout::PlayoutBuffer;

/// The `f32` samples a callback converts at a time, on the stack.
const CHUNK: usize = 1024;

/// The platform's default audio host, through cpal.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpalHost;

impl CpalHost {
    /// The platform's default audio host.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn backend(error: impl fmt::Display) -> DeviceError {
    DeviceError::Backend(error.to_string())
}

/// The device `selector` picks of `direction` on `host`.
fn find(
    host: &cpal::Host,
    direction: Direction,
    selector: &DeviceSelector,
) -> Result<cpal::Device, DeviceError> {
    match selector {
        DeviceSelector::Default => match direction {
            Direction::Input => host.default_input_device(),
            Direction::Output => host.default_output_device(),
        }
        .ok_or(DeviceError::NoDefault(direction)),
        DeviceSelector::Name(name) => host
            .devices()
            .map_err(backend)?
            .find(|device| {
                let of_direction = match direction {
                    Direction::Input => device.supports_input(),
                    Direction::Output => device.supports_output(),
                };
                of_direction && device.to_string() == *name
            })
            .ok_or_else(|| DeviceError::NotFound {
                name: name.clone(),
                direction,
            }),
    }
}

/// The device's default config of `direction` and what it resolves to.
fn resolve(
    device: &cpal::Device,
    direction: Direction,
) -> Result<(SupportedStreamConfig, ResolvedDevice), DeviceError> {
    let supported = match direction {
        Direction::Input => device.default_input_config(),
        Direction::Output => device.default_output_config(),
    }
    .map_err(backend)?;
    let sample_rate = NonZeroU32::new(supported.sample_rate())
        .ok_or_else(|| DeviceError::Unsupported("the device reports 0 Hz".to_owned()))?;
    let channels = NonZeroU16::new(supported.channels())
        .ok_or_else(|| DeviceError::Unsupported("the device reports 0 channels".to_owned()))?;
    Ok((
        supported,
        ResolvedDevice {
            name: device.to_string(),
            direction,
            config: DeviceConfig {
                sample_rate,
                channels,
            },
        },
    ))
}

/// The samples a chunk converts: a whole number of device frames, at most [`CHUNK`].
fn chunk_len(channels: u16) -> usize {
    let channels = usize::from(channels).max(1);
    (CHUNK / channels).max(1) * channels
}

fn input_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    buffer: Arc<CaptureBuffer>,
) -> Result<cpal::Stream, DeviceError>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let len = chunk_len(config.channels).min(CHUNK);
    let on_error = Arc::clone(&buffer);
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mut converted = [0.0_f32; CHUNK];
                for chunk in data.chunks(len) {
                    for (to, from) in converted.iter_mut().zip(chunk) {
                        *to = f32::from_sample(*from);
                    }
                    buffer.write(&converted[..chunk.len()]);
                }
            },
            move |error| on_error.report_error(&error.to_string()),
            None,
        )
        .map_err(backend)
}

fn output_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    buffer: Arc<PlayoutBuffer>,
) -> Result<cpal::Stream, DeviceError>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let len = chunk_len(config.channels).min(CHUNK);
    let on_error = Arc::clone(&buffer);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let mut converted = [0.0_f32; CHUNK];
                for chunk in data.chunks_mut(len) {
                    let samples = &mut converted[..chunk.len()];
                    buffer.fill(samples);
                    for (to, from) in chunk.iter_mut().zip(samples.iter()) {
                        *to = T::from_sample(*from);
                    }
                }
            },
            move |error| on_error.report_error(&error.to_string()),
            None,
        )
        .map_err(backend)
}

fn input_of_format(
    device: &cpal::Device,
    config: StreamConfig,
    buffer: Arc<CaptureBuffer>,
    format: SampleFormat,
) -> Result<cpal::Stream, DeviceError> {
    match format {
        SampleFormat::F32 => input_stream::<f32>(device, config, buffer),
        SampleFormat::F64 => input_stream::<f64>(device, config, buffer),
        SampleFormat::I8 => input_stream::<i8>(device, config, buffer),
        SampleFormat::I16 => input_stream::<i16>(device, config, buffer),
        SampleFormat::I32 => input_stream::<i32>(device, config, buffer),
        SampleFormat::U8 => input_stream::<u8>(device, config, buffer),
        SampleFormat::U16 => input_stream::<u16>(device, config, buffer),
        SampleFormat::U32 => input_stream::<u32>(device, config, buffer),
        other => Err(DeviceError::Unsupported(format!(
            "the device's sample format {other} is not one this adapter converts"
        ))),
    }
}

fn output_of_format(
    device: &cpal::Device,
    config: StreamConfig,
    buffer: Arc<PlayoutBuffer>,
    format: SampleFormat,
) -> Result<cpal::Stream, DeviceError> {
    match format {
        SampleFormat::F32 => output_stream::<f32>(device, config, buffer),
        SampleFormat::F64 => output_stream::<f64>(device, config, buffer),
        SampleFormat::I8 => output_stream::<i8>(device, config, buffer),
        SampleFormat::I16 => output_stream::<i16>(device, config, buffer),
        SampleFormat::I32 => output_stream::<i32>(device, config, buffer),
        SampleFormat::U8 => output_stream::<u8>(device, config, buffer),
        SampleFormat::U16 => output_stream::<u16>(device, config, buffer),
        SampleFormat::U32 => output_stream::<u32>(device, config, buffer),
        other => Err(DeviceError::Unsupported(format!(
            "the device's sample format {other} is not one this adapter converts"
        ))),
    }
}

/// The thread holding one running stream: dropping it tells the thread to drop the stream and
/// waits until it has.
struct Running {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _stream_thread_ended = thread.join();
        }
    }
}

/// Starts a stream of `direction` on its own thread: `open` resolves the device, makes the buffer
/// and builds the stream there, and the thread holds the started stream until the guard drops.
fn spawn<B: Send + Sync + 'static>(
    direction: Direction,
    selector: DeviceSelector,
    make: MakeBuffer<B>,
    stream: fn(
        &cpal::Device,
        StreamConfig,
        Arc<B>,
        SampleFormat,
    ) -> Result<cpal::Stream, DeviceError>,
) -> Result<OpenStream<B>, DeviceError> {
    let (ready, opened) = mpsc::channel();
    let (stop, stopped) = mpsc::channel::<()>();
    let thread = thread::Builder::new()
        .name(format!("audio-{direction}"))
        .spawn(move || {
            let started = (|| {
                let host = cpal::default_host();
                let device = find(&host, direction, &selector)?;
                let (supported, resolved) = resolve(&device, direction)?;
                let buffer = make(&resolved);
                let running = stream(
                    &device,
                    supported.config(),
                    Arc::clone(&buffer),
                    supported.sample_format(),
                )?;
                running.play().map_err(backend)?;
                Ok((resolved, buffer, running))
            })();
            match started {
                Ok((resolved, buffer, running)) => {
                    if ready.send(Ok((resolved, buffer))).is_ok() {
                        let _until_the_guard_drops = stopped.recv();
                    }
                    drop(running);
                }
                Err(error) => {
                    let _caller_gone = ready.send(Err(error));
                }
            }
        })
        .map_err(backend)?;
    let running = Running {
        stop: Some(stop),
        thread: Some(thread),
    };
    let (device, buffer) = opened
        .recv()
        .map_err(|_| DeviceError::Backend("the audio stream thread ended".to_owned()))??;
    Ok(OpenStream {
        device,
        buffer,
        guard: StreamGuard::new(running),
    })
}

impl AudioHost for CpalHost {
    fn devices(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        let host = cpal::default_host();
        let default_input = host.default_input_device();
        let default_output = host.default_output_device();
        Ok(host
            .devices()
            .map_err(backend)?
            .map(|device| DeviceInfo {
                name: device.to_string(),
                input: device.supports_input(),
                output: device.supports_output(),
                default_input: default_input.as_ref() == Some(&device),
                default_output: default_output.as_ref() == Some(&device),
            })
            .collect())
    }

    fn open_input(
        &self,
        device: &DeviceSelector,
        make: MakeCapture,
    ) -> Result<OpenStream<CaptureBuffer>, DeviceError> {
        spawn(Direction::Input, device.clone(), make, input_of_format)
    }

    fn open_output(
        &self,
        device: &DeviceSelector,
        make: MakePlayout,
    ) -> Result<OpenStream<PlayoutBuffer>, DeviceError> {
        spawn(Direction::Output, device.clone(), make, output_of_format)
    }
}
