//! Audio in: the query names the first frame's encoding and rate, the EU switch selects the EU
//! host, no empty binary message is ever sent, a `KeepAlive` goes out after silence, and audio the
//! binding cannot describe fails the recognition before anything connects.

mod support;

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::Duration;

use inference_deepgram::{DeepgramError, DeepgramRecogniser, Endpoint, ListenConfig, Region};
use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame};
use inference_stt::{Recogniser, RecognitionHandle};
use support::{FixedKey, FixtureServer, KEY, Script, mulaw, recognise, request, tone};

#[test]
fn mulaw_at_8_khz_is_sent_as_encoding_mulaw_and_sample_rate_8000() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let (handle, _) = recognise(&mut server.recogniser(), mulaw(6));
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    let query = &server.seen()[0].query;
    assert!(
        query.contains("&encoding=mulaw&sample_rate=8000&channels=1&interim_results=true"),
        "{query}"
    );
    assert!(query.starts_with("model=nova-3&language=en&"), "{query}");
    assert!(query.contains("smart_format=false"), "{query}");
    assert_eq!(server.seen()[0].audio_bytes, 6 * 80);
    assert_eq!(server.violations(), Vec::<String>::new());
}

#[test]
fn pcm16_is_sent_as_linear16_at_its_rate() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let (handle, _) = recognise(&mut server.recogniser(), tone(4));
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert!(
        server.seen()[0]
            .query
            .contains("&encoding=linear16&sample_rate=16000&"),
        "{}",
        server.seen()[0].query
    );
}

#[test]
fn the_eu_switch_selects_the_eu_host() {
    let format = mulaw(1)[0].format;
    for endpoint in [Endpoint::eu(), Endpoint::region(Region::Eu)] {
        let recogniser = DeepgramRecogniser::new(
            endpoint,
            ListenConfig::new(),
            Arc::new(FixedKey(KEY)),
            "deepgram-api-key",
        )
        .unwrap();
        let url = recogniser.request_url(format).unwrap();
        assert!(
            url.starts_with("wss://api.eu.deepgram.com/v1/listen?model=nova-3&"),
            "{url}"
        );
        assert!(!url.contains(KEY));
    }
    let default = DeepgramRecogniser::new(
        Endpoint::new(),
        ListenConfig::new(),
        Arc::new(FixedKey(KEY)),
        "deepgram-api-key",
    )
    .unwrap();
    assert!(
        default
            .request_url(format)
            .unwrap()
            .starts_with("wss://api.deepgram.com/v1/listen?")
    );
}

#[test]
fn an_empty_frame_is_never_sent_as_an_empty_binary_message() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let mut frames = tone(3);
    let empty = AudioFrame {
        sample_count: 0,
        data: Vec::new(),
        ..frames[1].clone()
    };
    frames.insert(1, empty.clone());
    frames.push(empty);
    let (handle, events) = recognise(&mut server.recogniser(), frames);
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert!(!events.is_empty());
    assert_eq!(server.seen()[0].audio_messages, 3);
    assert_eq!(server.violations(), Vec::<String>::new());
}

#[test]
fn a_keep_alive_is_sent_after_silence() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let config = ListenConfig {
        keepalive_interval: Duration::from_millis(100),
        ..ListenConfig::new()
    };
    let mut recogniser = server.recogniser_with(config);
    let mut handle = recogniser.start(request()).unwrap();
    handle.offer_frame(tone(1).remove(0)).unwrap();
    let _ = handle.by_ref().count();
    std::thread::sleep(Duration::from_millis(600));
    let _ = handle.by_ref().count();
    let keepalives = server.seen()[0].keepalives;
    assert!(keepalives >= 2, "{keepalives} KeepAlive messages in 600 ms");
    handle.end_input().unwrap();
    let _ = handle.by_ref().count();
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert_eq!(server.violations(), Vec::<String>::new());
}

#[test]
fn no_keep_alive_is_sent_while_audio_flows() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let (handle, _) = recognise(&mut server.recogniser(), tone(20));
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert_eq!(server.seen()[0].keepalives, 0);
}

#[test]
fn audio_the_binding_cannot_describe_fails_before_anything_connects() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let float = AudioFormat {
        encoding: AudioEncoding::PcmF32le,
        sample_rate: NonZeroU32::new(16_000).unwrap(),
        channels: NonZeroU16::MIN,
    };
    let frame = AudioFrame {
        format: float,
        data: vec![0; 160 * 4],
        ..tone(1).remove(0)
    };
    let (handle, events) = recognise(&mut server.recogniser(), vec![frame]);
    assert!(
        matches!(handle.failure(), Some(DeepgramError::UnsupportedAudio(_))),
        "{:?}",
        handle.failure()
    );
    assert_eq!(events.len(), 1, "the usage record alone");
    assert!(server.seen().is_empty(), "nothing connected");
}
