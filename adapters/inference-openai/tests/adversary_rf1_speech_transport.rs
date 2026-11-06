//! Adversary pass 1 on the OpenAI speech binding's transport, over hand-made loopback servers
//! that answer outside the happy path: TLS on an `https://` URL, a body cut before its end, a
//! server that never answers, a redirect, and the key kept out of every Debug and error text.

mod speech_support;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use inference_openai::speech::{
    OpenAiSpeech, SpeechConfig, SpeechEndpoint, SpeechError, SpeechVoice,
};
use inference_tts::inference_audio::AudioEncoding;
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use speech_support::{
    FixedKey, FixtureServer, KEY, Script, VOICE, committed, format, tone_bytes, within_deadline,
};

/// A loopback listener whose first accepted connection is handed, with the request read off it,
/// to `answer` on a thread of its own. The receiver yields what `answer` returns.
fn one_shot<T: Send + 'static>(
    answer: impl FnOnce(&mut TcpStream) -> T + Send + 'static,
) -> (String, mpsc::Receiver<T>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let address = listener.local_addr().expect("an address");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("a read timeout");
        read_request(&mut stream);
        let _ = tx.send(answer(&mut stream));
    });
    (format!("http://{address}/v1/audio/speech"), rx)
}

/// Reads one request's head and its `content-length` body off `stream`.
fn read_request(stream: &mut TcpStream) {
    let mut buffer = Vec::new();
    let mut byte = [0_u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => buffer.push(byte[0]),
            _ => return,
        }
    }
    let head = String::from_utf8_lossy(&buffer).to_lowercase();
    let length = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0_u8; length];
    let _ = stream.read_exact(&mut body);
}

fn synthesiser(url: &str, read_timeout: Duration) -> OpenAiSpeech {
    OpenAiSpeech::new(
        SpeechEndpoint::new()
            .with_url(url)
            .with_read_timeout(read_timeout)
            .with_connect_timeout(Duration::from_secs(2)),
        SpeechConfig::new(
            SpeechVoice::BuiltIn(VOICE.to_owned()),
            format(AudioEncoding::PcmMulaw, 8_000),
        ),
        Arc::new(FixedKey(KEY)),
        "openai-api-key",
    )
    .expect("a loopback endpoint")
}

/// Speaks one segment to its end and returns the events, the failure's Debug and the segment's
/// state.
fn speak_one(
    synthesiser: &mut OpenAiSpeech,
) -> (Vec<SynthesisEvent>, Option<String>, Option<SegmentState>) {
    let mut handle = synthesiser.start().expect("a slot");
    let segment = committed(&["Please hold."]).remove(0);
    let id = segment.segment_id();
    handle.offer_segment(segment).unwrap();
    handle.end_utterance().unwrap();
    let events: Vec<SynthesisEvent> = handle.by_ref().collect();
    let failure = handle.failure().map(|error| format!("{error:?}"));
    let state = handle.output().segment(id).map(|segment| segment.state());
    (events, failure, state)
}

#[test]
fn an_https_url_speaks_tls_to_the_server() {
    within_deadline(|| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut first = [0_u8; 1];
            let _ = tx.send(stream.read_exact(&mut first).ok().map(|()| first[0]));
        });
        let mut synthesiser = synthesiser(
            &format!("https://{address}/v1/audio/speech"),
            Duration::from_secs(2),
        );
        let (_, failure, _) = speak_one(&mut synthesiser);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).ok().flatten(),
            Some(0x16),
            "the first byte on an https:// connection is a TLS handshake record"
        );
        let failure = failure.expect("no TLS server answers, so the synthesis fails");
        assert!(
            failure.starts_with("Connect") || failure.starts_with("Stalled"),
            "{failure}"
        );
        assert!(!failure.contains(KEY));
    });
}

#[test]
fn a_chunked_body_cut_before_its_last_chunk_fails_the_synthesis() {
    within_deadline(|| {
        let (url, _rx) = one_shot(|stream| {
            let tone = tone_bytes();
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\n\
                  transfer-encoding: chunked\r\n\r\n",
            );
            let _ = stream.write_all(format!("{:x}\r\n", 4_800).as_bytes());
            let _ = stream.write_all(&tone[..4_800]);
            let _ = stream.write_all(b"\r\n");
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Both);
        });
        let (_, failure, state) = speak_one(&mut synthesiser(&url, Duration::from_secs(5)));
        assert!(
            failure
                .as_deref()
                .is_some_and(|failure| failure.starts_with("Transport")),
            "a body cut at a sample boundary without its last chunk is not the end: {failure:?}"
        );
        assert_ne!(state, Some(SegmentState::Synthesized));
    });
}

#[test]
fn a_content_length_body_cut_short_fails_the_synthesis() {
    within_deadline(|| {
        let (url, _rx) = one_shot(|stream| {
            let tone = tone_bytes();
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\n\
                     content-length: {}\r\n\r\n",
                    tone.len()
                )
                .as_bytes(),
            );
            let _ = stream.write_all(&tone[..4_800]);
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Both);
        });
        let (_, failure, state) = speak_one(&mut synthesiser(&url, Duration::from_secs(5)));
        assert!(
            failure
                .as_deref()
                .is_some_and(|failure| failure.starts_with("Transport")),
            "{failure:?}"
        );
        assert_ne!(state, Some(SegmentState::Synthesized));
    });
}

#[test]
fn a_server_that_never_answers_stalls_within_the_read_timeout() {
    within_deadline(|| {
        let (url, _rx) = one_shot(|stream| {
            let mut sink = [0_u8; 64];
            let _ = stream.read(&mut sink);
        });
        let started = Instant::now();
        let (events, failure, _) = speak_one(&mut synthesiser(&url, Duration::from_secs(1)));
        let waited = started.elapsed();
        assert!(
            failure
                .as_deref()
                .is_some_and(|failure| failure.starts_with("Stalled")),
            "{failure:?}"
        );
        assert!(waited < Duration::from_secs(4), "waited {waited:?}");
        assert!(matches!(events.last(), Some(SynthesisEvent::Usage(_))));
    });
}

#[test]
fn a_redirect_is_not_followed() {
    within_deadline(|| {
        let elsewhere = FixtureServer::start(Script::Serve);
        let location = elsewhere.url.clone();
        let (url, _rx) = one_shot(move |stream| {
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 307 Temporary Redirect\r\nlocation: {location}\r\n\
                     content-length: 0\r\n\r\n"
                )
                .as_bytes(),
            );
            let _ = stream.flush();
        });
        let mut synthesiser = synthesiser(&url, Duration::from_secs(5));
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Please hold."]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let _: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(
            matches!(handle.failure(), Some(SpeechError::Status(307))),
            "{:?}",
            handle.failure()
        );
        assert!(elsewhere.seen().is_empty(), "the redirect was followed");
    });
}

#[test]
fn the_key_is_in_no_debug_or_error_text() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Refuse);
        let mut synthesiser = server.synthesiser(format(AudioEncoding::PcmMulaw, 8_000));
        let debug = format!("{synthesiser:?}");
        assert!(!debug.contains(KEY), "{debug}");
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Hello."]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let _: Vec<SynthesisEvent> = handle.by_ref().collect();
        let failure = handle.failure().expect("a refused key fails");
        for text in [
            format!("{handle:?}"),
            format!("{failure:?}"),
            failure.to_string(),
        ] {
            assert!(!text.contains(KEY), "{text}");
            assert!(!text.contains("Bearer"), "{text}");
        }
    });
}

/// Set in the child process of [`a_proxy_in_the_environment_is_not_used`].
const PROXY_CHILD: &str = "ADVERSARY_RF1_SPEECH_PROXY_CHILD";

/// The environment names a proxy for every scheme; the binding goes to the server directly. The
/// proxy variables are set on a child run of this test binary, since the parent's environment is
/// shared by every test in it.
#[test]
fn a_proxy_in_the_environment_is_not_used() {
    if std::env::var_os(PROXY_CHILD).is_some() {
        within_deadline(|| {
            let server = FixtureServer::start(Script::Serve);
            let mut synthesiser = server.synthesiser(format(AudioEncoding::PcmMulaw, 8_000));
            let (_, failure, state) = speak_one(&mut synthesiser);
            assert_eq!(failure, None);
            assert_eq!(state, Some(SegmentState::Synthesized));
            assert_eq!(server.seen().len(), 1, "the request reached the server");
        });
        return;
    }
    within_deadline(|| {
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        proxy.set_nonblocking(true).unwrap();
        let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "a_proxy_in_the_environment_is_not_used",
                "--test-threads",
                "1",
            ])
            .env(PROXY_CHILD, "1")
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .env_remove("REQUEST_METHOD");
        for name in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            command.env(name, &proxy_url);
        }
        let output = command.output().expect("the child test runs");
        assert!(
            output.status.success(),
            "the child failed: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "the child ran no test: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            proxy.accept().is_err(),
            "the binding connected to the proxy named in the environment"
        );
    });
}
