//! Adversary pass 1 of `story:example-and-testkit-findings`, finding N32: the story plans to
//! refuse "a rate outside the recogniser's supported set before allocating", and its acceptance
//! is that an absurd rate is refused with a typed error and no allocation. The unit bounds the
//! rate from above only. A first frame at 1 Hz is taken as the stream's rate, and a later frame
//! at 1 Hz into a stream at `MAX_STREAM_RATE` is upsampled by the recogniser's resampler 384 000
//! times, so one 48 000-sample frame becomes about 18 billion samples before anything refuses it.

#![cfg(feature = "stt")]

use inference_sherpa_onnx::stt::{MAX_STREAM_RATE, UnsupportedRate, stream_rate};

#[test]
fn a_rate_of_one_hertz_is_refused_as_a_streams_rate() {
    assert!(stream_rate(MAX_STREAM_RATE).is_ok());
    assert_eq!(
        stream_rate(1),
        Err(UnsupportedRate { rate: 1 }),
        "a 1 Hz frame is as absurd as a u32::MAX Hz one, and is taken as the stream's rate"
    );
}
