//! The hardware test (feature `device`, ignored by default): it opens the default microphone and
//! speaker of the machine it runs on. It records two seconds, then plays a tone and the recording
//! back. Run it on a machine with a microphone, with a person listening:
//!
//! ```text
//! cargo test -p inference-audio-device --test hardware -- --ignored --nocapture
//! ```

use std::f32::consts::TAU;
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFrame, SampleClockPosition};
use inference_audio_device::convert::{decode, encode};
use inference_audio_device::{
    AudioHost, CaptureConfig, CpalHost, DeviceSelector, Microphone, PlayoutConfig, Speaker,
};

const RATE: u32 = 16_000;
const FRAME: usize = 320;

#[test]
#[ignore = "opens the default microphone and speaker; run it by hand on a laptop"]
fn records_two_seconds_and_plays_a_tone_and_the_recording_back() {
    let host = CpalHost::new();
    for device in host.devices().expect("the host lists its devices") {
        println!(
            "device {:?}: input {} output {} default input {} default output {}",
            device.name, device.input, device.output, device.default_input, device.default_output
        );
    }

    let mut microphone =
        Microphone::open(&host, &DeviceSelector::Default, CaptureConfig::default())
            .expect("the default input opens");
    println!("recording two seconds from {:?}", microphone.device());
    let mut recording: Vec<AudioFrame> = Vec::new();
    while recording.len() < 100 {
        let frame = microphone
            .next_frame(Duration::from_secs(2))
            .expect("the microphone delivers a frame within two seconds");
        recording.push(frame);
    }
    let capture = microphone.stats();
    drop(microphone);
    println!("capture: {capture:?}");
    for (index, frame) in recording.iter().enumerate() {
        assert_eq!(frame.position.get(), index as u64 * FRAME as u64);
    }
    let samples: Vec<f32> = recording
        .iter()
        .flat_map(|frame| decode(frame).expect("a well-formed frame"))
        .collect();
    let level = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    println!("recorded level (rms, full scale 1): {level:.5}");
    assert!(
        level > 0.0,
        "a live microphone records more than digital silence"
    );

    let speaker = Speaker::open(&host, &DeviceSelector::Default, PlayoutConfig::default())
        .expect("the default output opens");
    println!("playing a one-second 440 Hz tone on {:?}", speaker.device());
    let tone: Vec<f32> = (0..RATE as usize)
        .map(|n| (TAU * 440.0 * n as f32 / RATE as f32).sin() * 0.2)
        .collect();
    for (index, chunk) in tone.chunks(FRAME).enumerate() {
        let frame = AudioFrame {
            format: recording[0].format,
            position: SampleClockPosition::new((index * FRAME) as u64).expect("a position"),
            sample_count: FRAME as u32,
            data: encode(chunk, AudioEncoding::PcmS16le),
        };
        speaker
            .push(&frame, Duration::from_secs(5))
            .expect("the speaker takes the tone");
    }
    speaker.finish();
    assert!(
        speaker.wait_played(Duration::from_secs(5)),
        "the tone plays out"
    );
    assert_eq!(speaker.stop(), SampleClockPosition::new(u64::from(RATE)));

    println!("playing the recording back");
    for frame in &recording {
        speaker
            .push(frame, Duration::from_secs(5))
            .expect("the speaker takes the recording");
    }
    speaker.finish();
    assert!(
        speaker.wait_played(Duration::from_secs(5)),
        "the recording plays out"
    );
    assert_eq!(
        speaker.played(),
        SampleClockPosition::new(100 * FRAME as u64)
    );
    let playout = speaker.stats();
    println!("playout: {playout:?}");
    assert_eq!(playout.stream_errors, 0, "{:?}", playout.last_error);
}
