//! Plays the default microphone back through the default output with a fixed delay of half a
//! second, for twenty seconds, so the devices can be checked by ear. Use a headset: a laptop's
//! own speaker feeds back into its microphone.
//!
//! ```text
//! cargo run -p inference-audio-device --example mic-loopback
//! ```

use std::time::{Duration, Instant};

use inference_audio::{AudioFrame, SampleClockPosition};
use inference_audio_device::convert::encode;
use inference_audio_device::{
    AudioHost, CaptureConfig, CpalHost, DeviceError, DeviceSelector, Microphone, PlayoutConfig,
    Speaker,
};

const DELAY: Duration = Duration::from_millis(500);
const RUN: Duration = Duration::from_secs(20);

fn main() -> Result<(), DeviceError> {
    let host = CpalHost::new();
    for device in host.devices()? {
        let marks = match (device.default_input, device.default_output) {
            (true, true) => " (default input and output)",
            (true, false) => " (default input)",
            (false, true) => " (default output)",
            (false, false) => "",
        };
        println!("{}{marks}", device.name);
    }

    let config = CaptureConfig::default();
    let mut microphone = Microphone::open(&host, &DeviceSelector::Default, config)?;
    let speaker = Speaker::open(&host, &DeviceSelector::Default, PlayoutConfig::default())?;
    println!(
        "capturing from {:?} at {} Hz, playing on {:?} at {} Hz, {} ms behind",
        microphone.device().name,
        microphone.device().config.sample_rate,
        speaker.device().name,
        speaker.device().config.sample_rate,
        DELAY.as_millis()
    );

    let format = microphone.format();
    let frame_samples = config.frame_samples()?;
    let delay_frames = DELAY.as_millis() / config.frame.as_millis().max(1);
    let silence = encode(&vec![0.0; frame_samples as usize], format.encoding);
    let mut next = 0_u64;
    for _ in 0..delay_frames {
        let frame = AudioFrame {
            format,
            position: SampleClockPosition::new(next).unwrap_or(SampleClockPosition::MAX),
            sample_count: frame_samples,
            data: silence.clone(),
        };
        if let Err(refused) = speaker.push(&frame, Duration::from_secs(1)) {
            println!("the delay was not queued: {refused}");
        }
        next += u64::from(frame_samples);
    }

    let started = Instant::now();
    let mut reported = Instant::now();
    while started.elapsed() < RUN {
        let Some(mut frame) = microphone.next_frame(Duration::from_millis(100)) else {
            continue;
        };
        frame.position = SampleClockPosition::new(next).unwrap_or(SampleClockPosition::MAX);
        next += u64::from(frame.sample_count);
        if let Err(refused) = speaker.push(&frame, Duration::from_millis(100)) {
            println!("a frame was not played: {refused}");
        }
        if reported.elapsed() >= Duration::from_secs(1) {
            reported = Instant::now();
            println!(
                "dropped device frames {}, underruns {}, queued {} samples",
                microphone.stats().dropped_device_frames,
                speaker.stats().underruns,
                speaker.queued()
            );
        }
    }
    speaker.finish();
    let _played_out = speaker.wait_played(DELAY * 2);
    println!("capture: {:?}", microphone.stats());
    println!("playout: {:?}", speaker.stats());
    Ok(())
}
