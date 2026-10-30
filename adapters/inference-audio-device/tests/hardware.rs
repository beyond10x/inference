//! The hardware test (feature `device`, ignored by default): it opens the default microphone and
//! speaker of the machine it runs on. It records two seconds, then plays a tone and the recording
//! back. Run it on a machine with a microphone, with a person listening:
//!
//! ```text
//! cargo test -p inference-audio-device --test hardware -- --ignored --nocapture
//! ```
//!
//! The same flow (`record_and_play`) runs on every test run against a fake laptop, whose
//! microphone a thread feeds with a tone and whose speaker it pulls, both in device-sized chunks.

mod record_and_play;

use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use inference_audio_device::CpalHost;
use inference_audio_device::fake::FakeHost;

#[test]
#[ignore = "opens the default microphone and speaker; run it by hand on a laptop"]
fn records_two_seconds_and_plays_a_tone_and_the_recording_back() {
    record_and_play::records_two_seconds_and_plays_a_tone_and_the_recording_back(&CpalHost::new());
}

/// Plays the devices of the fake laptop, 48 kHz stereo both, until `done`: every millisecond
/// it feeds ten milliseconds of a 220 Hz tone into the microphone while one is open and pulls
/// ten milliseconds from the speaker while one is open.
fn play_the_devices(host: FakeHost, done: Arc<AtomicBool>) {
    const CHUNK: usize = 480;
    let mut sample = 0_usize;
    while !done.load(Ordering::Relaxed) {
        let chunk: Vec<f32> = (sample..sample + CHUNK)
            .flat_map(|n| {
                let level = (TAU * 220.0 * n as f32 / 48_000.0).sin() * 0.3;
                [level, level]
            })
            .collect();
        if host.feed(&chunk) {
            sample += CHUNK;
        }
        let _played = host.pull(CHUNK);
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn the_hardware_flow_passes_on_a_fake_laptop() {
    let host = FakeHost::laptop();
    let done = Arc::new(AtomicBool::new(false));
    let devices = {
        let host = host.clone();
        let done = Arc::clone(&done);
        thread::spawn(move || play_the_devices(host, done))
    };
    record_and_play::records_two_seconds_and_plays_a_tone_and_the_recording_back(&host);
    done.store(true, Ordering::Relaxed);
    devices.join().expect("the fake devices stop");
    assert!(!host.input_open());
    assert!(!host.output_open());
}
