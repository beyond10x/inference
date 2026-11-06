//! The endpoint streams 24 kHz 16-bit PCM; the binding gives frames at the rate and in the
//! encoding asked for, resampled by `inference-audio`'s anti-aliased resampler and encoded by its
//! G.711 codec. A fixture run asking for μ-law at 8 kHz receives μ-law frames at 8 kHz that
//! decode to the tone the server sent, at its time.

mod speech_support;

use inference_openai::speech::{OpenAiSpeech, SpeechError};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat, decode_alaw, decode_mulaw};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use speech_support::{FixtureServer, Script, committed, format, tone, tone_bytes, within_deadline};

/// The frames of one synthesis of `texts`, in order, and whether it failed.
fn speak(synthesiser: &mut OpenAiSpeech, texts: &[&str]) -> (Vec<SynthesisEvent>, Option<String>) {
    let mut handle = synthesiser.start().unwrap();
    for segment in committed(texts) {
        handle.offer_segment(segment).unwrap();
    }
    handle.end_utterance().unwrap();
    let events: Vec<SynthesisEvent> = handle.by_ref().collect();
    let failure = handle.failure().map(ToString::to_string);
    (events, failure)
}

/// The data of every audio frame, concatenated, each frame checked against `expected`.
fn audio(events: &[SynthesisEvent], expected: AudioFormat) -> Vec<u8> {
    let mut data = Vec::new();
    let mut next = 0_u64;
    for event in events {
        if let SynthesisEvent::Audio(audio) = event {
            let frame = audio.frame();
            assert_eq!(frame.format, expected);
            assert_eq!(
                frame.data.len() as u64,
                expected.data_len(frame.sample_count)
            );
            assert_eq!(frame.position.get(), next, "frames are contiguous");
            next += u64::from(frame.sample_count);
            data.extend_from_slice(&frame.data);
        }
    }
    data
}

/// The root mean square of `actual - expected` over the root mean square of `expected`, away
/// from the first and last `edge` samples.
fn relative_error(actual: &[i16], expected: &[i16], edge: usize) -> f64 {
    assert_eq!(actual.len(), expected.len());
    let range = edge..expected.len() - edge;
    let (mut error, mut power) = (0.0_f64, 0.0_f64);
    for index in range {
        let difference = f64::from(actual[index]) - f64::from(expected[index]);
        error += difference * difference;
        power += f64::from(expected[index]) * f64::from(expected[index]);
    }
    (error / power).sqrt()
}

#[test]
fn a_run_asking_for_mulaw_8k_receives_mulaw_8k_frames_of_the_tone() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let wanted = format(AudioEncoding::PcmMulaw, 8_000);
        let (events, failure) = speak(&mut server.synthesiser(wanted), &["Please hold."]);
        assert_eq!(failure, None);
        let data = audio(&events, wanted);
        assert_eq!(data.len(), 1_600, "200 ms at 8 kHz, one byte a sample");
        let decoded: Vec<i16> = data.iter().map(|&code| decode_mulaw(code)).collect();
        let error = relative_error(&decoded, &tone(8_000), 40);
        assert!(
            error < 0.1,
            "the μ-law audio is not the tone in time: {error}"
        );
        assert!(matches!(events.last(), Some(SynthesisEvent::Usage(_))));
        assert_eq!(server.violations(), Vec::<String>::new());
    });
}

#[test]
fn a_run_asking_for_alaw_8k_receives_alaw_8k_frames_of_the_tone() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let wanted = format(AudioEncoding::PcmAlaw, 8_000);
        let (events, failure) = speak(&mut server.synthesiser(wanted), &["Please hold."]);
        assert_eq!(failure, None);
        let data = audio(&events, wanted);
        let decoded: Vec<i16> = data.iter().map(|&code| decode_alaw(code)).collect();
        let error = relative_error(&decoded, &tone(8_000), 40);
        assert!(
            error < 0.1,
            "the A-law audio is not the tone in time: {error}"
        );
    });
}

#[test]
fn a_run_asking_for_pcm_16k_receives_pcm_16k_frames_of_the_tone() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let wanted = format(AudioEncoding::PcmS16le, 16_000);
        let (events, failure) = speak(&mut server.synthesiser(wanted), &["Please hold."]);
        assert_eq!(failure, None);
        let data = audio(&events, wanted);
        let samples: Vec<i16> = data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair))
            .collect();
        let error = relative_error(&samples, &tone(16_000), 80);
        assert!(
            error < 0.05,
            "the 16 kHz audio is not the tone in time: {error}"
        );
    });
}

#[test]
fn a_run_asking_for_pcm_24k_receives_the_bytes_the_server_sent() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let wanted = format(AudioEncoding::PcmS16le, 24_000);
        let (events, failure) = speak(&mut server.synthesiser(wanted), &["One.", "Two."]);
        assert_eq!(failure, None);
        let data = audio(&events, wanted);
        assert_eq!(data, [tone_bytes(), tone_bytes()].concat());
    });
}

#[test]
fn a_body_ending_in_half_a_sample_fails_the_synthesis() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::HalfSample);
        let wanted = format(AudioEncoding::PcmMulaw, 8_000);
        let mut synthesiser = server.synthesiser(wanted);
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Cut."]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let events: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(
            matches!(handle.failure(), Some(SpeechError::Protocol(_))),
            "{:?}",
            handle.failure()
        );
        let Some(SynthesisEvent::Usage(usage)) = events.last() else {
            panic!("no usage record last: {events:?}");
        };
        assert!(!usage.r#final, "a failed call's record is not final");
    });
}
