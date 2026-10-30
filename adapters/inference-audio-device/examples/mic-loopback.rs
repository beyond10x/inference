//! Plays the default microphone back through the default output with a fixed delay of half a
//! second, for twenty seconds, so the devices can be checked by ear. Use a headset: a laptop's
//! own speaker feeds back into its microphone.
//!
//! ```text
//! cargo run -p inference-audio-device --example mic-loopback
//! ```

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use inference_audio_device::loopback::{self, LoopbackConfig, LoopbackEvent};
use inference_audio_device::{AudioHost, CpalHost, DeviceError};

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

    let config = LoopbackConfig {
        delay: DELAY,
        run: RUN,
        progress: Duration::from_secs(1),
        drain: DELAY * 2,
        ..LoopbackConfig::default()
    };
    let report = loopback::run(
        &host,
        &config,
        &AtomicBool::new(false),
        |event| match event {
            LoopbackEvent::Opened {
                microphone,
                speaker,
            } => println!(
                "capturing from {:?} at {} Hz, playing on {:?} at {} Hz, {} ms behind",
                microphone.name,
                microphone.config.sample_rate,
                speaker.name,
                speaker.config.sample_rate,
                DELAY.as_millis()
            ),
            LoopbackEvent::Refused { error, .. } => println!("a frame was not played: {error}"),
            LoopbackEvent::Progress {
                capture,
                playout,
                queued,
            } => println!(
                "dropped device frames {}, underruns {}, queued {queued} samples",
                capture.dropped_device_frames, playout.underruns
            ),
            LoopbackEvent::Pushed { .. } | LoopbackEvent::Draining { .. } => {}
        },
    )?;
    println!("capture: {:?}", report.capture);
    println!("playout: {:?}", report.playout);
    Ok(())
}
