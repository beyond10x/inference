//! Adversary cases, second pass, on "no struct field keeps the signed URL past the handshake",
//! measured in the memory of the test process instead of by a holder count.
//!
//! Each case builds a signed URL in one buffer of its exact size, without a second copy of it:
//! the test keeps the URL's bytes only under a mask. It then reads its own writable memory
//! through `/proc/self/mem` and counts how often the whole URL (scheme, host, path and query,
//! token included) stands there. While the model holds the URL the count is one, the buffer
//! itself. After the open that dialled, however it ended, the count is zero: the buffer is
//! zeroized and nothing else holds a copy.
//!
//! The whole URL is searched for, not the token alone: the request line tungstenite wrote
//! (path and query, without scheme and host) is a copy the crate documentation lists and does
//! not clear, and the fixture server of this process records the query it was sent.
//!
//! Every token here is synthetic: no provider issued it.
#![cfg(all(feature = "duplex", target_os = "linux"))]

#[path = "support/agent.rs"]
mod fixture;

use std::io::{Read, Seek, SeekFrom};
use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use fixture::{AGENT, AgentServer, Handshake, Script, Step};
use inference_elevenlabs::ElevenLabsAgent;
use inference_elevenlabs::agent::{AgentEndpoint, AgentError, SignedUrl};
use inference_elevenlabs::inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_elevenlabs::inference_duplex::{
    NativeSpeechModel, OpenError, SessionConfig, TurnDetection, TurnTuning, Unsupported,
};

const MASK: u8 = 0xA5;
const CHUNK: usize = 1 << 20;
const PAGE: usize = 4096;
const TOKEN_LENGTH: u8 = 32;

fn pcm(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        voice: String::new(),
        instructions: String::new(),
        input_format: pcm(16_000),
        output_format: pcm(16_000),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

fn endpoint() -> AgentEndpoint {
    AgentEndpoint::new()
        .with_connect_timeout(Duration::from_secs(3))
        .with_metadata_timeout(Duration::from_secs(3))
        .with_read_timeout(Duration::from_secs(3))
}

/// A signed URL at `base`, and its bytes under [`MASK`]. The URL is written once, into a buffer
/// of its exact size, its token one character at a time: no other buffer of the process holds
/// the URL, and none holds the token.
fn signed(base: &str, seed: u8) -> (SignedUrl, Vec<u8>) {
    let path = "/v1/convai/conversation?agent_id=";
    let key = "&token=";
    let length = base.len() + path.len() + AGENT.len() + key.len() + usize::from(TOKEN_LENGTH);
    let mut url = String::with_capacity(length);
    let buffer = url.as_ptr();
    url.push_str(base);
    url.push_str(path);
    url.push_str(AGENT);
    url.push_str(key);
    for index in 0..TOKEN_LENGTH {
        url.push(char::from(b'a' + (index * 7 + seed) % 26));
    }
    assert_eq!(url.len(), length);
    assert_eq!(url.as_ptr(), buffer, "the URL moved while it was written");
    let masked = url.bytes().map(|byte| byte ^ MASK).collect();
    let signed = SignedUrl::new(url).expect("a signed URL the binding takes");
    (signed, masked)
}

/// Reads the writable memory of this process.
struct Memory {
    buffer: Vec<u8>,
}

impl Memory {
    /// Made before the URL is, so that making it leaves no copy of anything behind.
    fn new() -> Self {
        Self {
            buffer: vec![0; CHUNK + PAGE],
        }
    }

    /// How often the bytes `masked` stands for occur in the writable memory of this process,
    /// the buffer of this reader aside. The buffer is cleared before this returns.
    fn occurrences(&mut self, masked: &[u8]) -> usize {
        assert!(masked.len() > 1 && masked.len() <= PAGE);
        let maps = std::fs::read_to_string("/proc/self/maps").expect("the mappings");
        let mut memory = std::fs::File::open("/proc/self/mem").expect("the memory");
        let own_start = self.buffer.as_ptr().addr();
        let own_end = own_start + self.buffer.len();
        let mut found = 0;
        for line in maps.lines() {
            let mut fields = line.split_whitespace();
            let (Some(range), Some(permissions)) = (fields.next(), fields.next()) else {
                continue;
            };
            if !permissions.starts_with("rw") {
                continue;
            }
            let Some((start, end)) = range.split_once('-') else {
                continue;
            };
            let (Ok(start), Ok(end)) = (
                usize::from_str_radix(start, 16),
                usize::from_str_radix(end, 16),
            ) else {
                continue;
            };
            for (from, to) in [(start, end.min(own_start)), (start.max(own_end), end)] {
                if from < to {
                    found += self.count(&mut memory, masked, from, to);
                }
            }
        }
        self.buffer.fill(0);
        found
    }

    /// The occurrences in the addresses `from..to`, read a chunk at a time; the end of each
    /// chunk is kept in front of the next, so an occurrence across two chunks is counted.
    fn count(
        &mut self,
        memory: &mut std::fs::File,
        masked: &[u8],
        from: usize,
        to: usize,
    ) -> usize {
        let first = masked[0] ^ MASK;
        let mut found = 0;
        let mut kept = 0;
        let mut at = from;
        while at < to {
            let want = CHUNK.min(to - at);
            let read = memory
                .seek(SeekFrom::Start(at as u64))
                .and_then(|_| memory.read(&mut self.buffer[kept..kept + want]));
            let read = match read {
                Ok(read) if read > 0 => read,
                _ => {
                    kept = 0;
                    at += PAGE.min(to - at);
                    continue;
                }
            };
            let filled = kept + read;
            found += self.buffer[..filled]
                .windows(masked.len())
                .filter(|window| {
                    window[0] == first
                        && window
                            .iter()
                            .zip(masked)
                            .all(|(byte, masked)| byte ^ MASK == *masked)
                })
                .count();
            let keep = (masked.len() - 1).min(filled);
            self.buffer.copy_within(filled - keep..filled, 0);
            kept = keep;
            at += read;
        }
        found
    }
}

fn stalling() -> AgentServer {
    let mut script = Script::new(Vec::new());
    script.handshake = Handshake::Stall;
    AgentServer::start(script)
}

/// The control: the reader finds the buffer of a model that has not dialled, still finds it
/// after an open refused before it dials, and finds nothing once the model is dropped.
#[test]
fn the_url_of_a_model_that_has_not_dialled_stands_in_memory_once_until_the_model_drops() {
    let mut memory = Memory::new();
    let server = AgentServer::start(Script::new(Vec::new()));
    let (signed, masked) = signed(&server.url, 1);
    assert_eq!(memory.occurrences(&masked), 1, "the URL as it was built");
    let mut model = ElevenLabsAgent::signed(signed, endpoint());
    assert_eq!(memory.occurrences(&masked), 1, "the URL in the model");

    let refused = model.open(SessionConfig {
        auto_response: false,
        ..config()
    });
    assert!(matches!(refused, Err(OpenError::Unsupported(_))));
    assert_eq!(server.connections(), 0);
    assert_eq!(
        memory.occurrences(&masked),
        1,
        "the URL after an open refused before it dials"
    );

    drop(model);
    assert_eq!(memory.occurrences(&masked), 0, "the URL after the model");
}

#[test]
fn no_memory_holds_the_url_while_the_session_it_opened_lives() {
    let mut memory = Memory::new();
    let server = AgentServer::start(Script::new(vec![Step::Pause(Duration::from_secs(10))]));
    let (signed, masked) = signed(&server.url, 2);
    let mut model = ElevenLabsAgent::signed(signed, endpoint());
    assert_eq!(memory.occurrences(&masked), 1);

    let session = model.open(config()).expect("the session opens");
    let seen = server.wait(0, Duration::from_secs(1), |seen| !seen.query.is_empty());
    assert!(
        seen.query.contains("&token="),
        "no token reached the server"
    );
    assert_eq!(
        memory.occurrences(&masked),
        0,
        "the URL stands in memory while the session lives"
    );
    drop(session);
    assert_eq!(memory.occurrences(&masked), 0);
}

#[test]
fn no_memory_holds_the_url_after_a_connection_that_could_not_be_made() {
    let mut memory = Memory::new();
    let closed_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        format!("ws://{}", listener.local_addr().expect("an address"))
    };
    let (signed, masked) = signed(&closed_port, 3);
    let mut model = ElevenLabsAgent::signed(signed, endpoint());
    assert_eq!(memory.occurrences(&masked), 1);

    model.open(config()).expect_err("nothing listens there");
    assert!(matches!(
        model.open_error(),
        Some(AgentError::Connect { status: None, .. })
    ));
    assert_eq!(memory.occurrences(&masked), 0);
}

#[test]
fn no_memory_holds_the_url_after_a_refused_upgrade() {
    let mut memory = Memory::new();
    let server = {
        let mut script = Script::new(Vec::new());
        script.handshake = Handshake::Refuse(403);
        AgentServer::start(script)
    };
    let (signed, masked) = signed(&server.url, 4);
    let mut model = ElevenLabsAgent::signed(signed, endpoint());
    assert_eq!(memory.occurrences(&masked), 1);

    model.open(config()).expect_err("the upgrade is refused");
    assert!(matches!(
        model.open_error(),
        Some(AgentError::Connect {
            status: Some(403),
            ..
        })
    ));
    assert_eq!(memory.occurrences(&masked), 0);
}

#[test]
fn no_memory_holds_the_url_after_a_handshake_that_timed_out() {
    let mut memory = Memory::new();
    let server = stalling();
    let (signed, masked) = signed(&server.url, 5);
    let mut model = ElevenLabsAgent::signed(
        signed,
        endpoint().with_connect_timeout(Duration::from_millis(200)),
    );
    assert_eq!(memory.occurrences(&masked), 1);

    model
        .open(config())
        .expect_err("no handshake within the connect timeout");
    assert!(matches!(
        model.open_error(),
        Some(AgentError::Connect { status: None, .. })
    ));
    assert_eq!(server.connections(), 1, "the open that timed out dialled");
    assert_eq!(memory.occurrences(&masked), 0);
}

#[test]
fn no_memory_holds_the_url_after_a_handshake_that_was_cancelled() {
    let mut memory = Memory::new();
    let server = stalling();
    let (signed, masked) = signed(&server.url, 6);
    let mut model = ElevenLabsAgent::signed(signed, endpoint());
    assert_eq!(memory.occurrences(&masked), 1);

    let canceller = model.canceller();
    let cancelling = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        canceller.cancel();
    });
    model.open(config()).expect_err("the open is cancelled");
    cancelling.join().unwrap();
    assert_eq!(model.open_error(), Some(&AgentError::Cancelled));
    assert_eq!(server.connections(), 1, "the cancelled open dialled");
    assert_eq!(memory.occurrences(&masked), 0);
}

/// The crate documentation: the URI of the handshake request is the URL's own buffer, not a
/// copy of it. While the handshake waits for the server, the URL stands in memory once.
#[test]
fn the_url_stands_in_memory_once_while_its_handshake_waits_and_not_after() {
    let mut memory = Memory::new();
    let server = stalling();
    let (signed, masked) = signed(&server.url, 8);
    let mut model = ElevenLabsAgent::signed(signed, endpoint());
    assert_eq!(memory.occurrences(&masked), 1);

    let canceller = model.canceller();
    let waiting = {
        let masked = masked.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let during = memory.occurrences(&masked);
            canceller.cancel();
            (memory, during)
        })
    };
    model.open(config()).expect_err("the open is cancelled");
    let (mut memory, during) = waiting.join().unwrap();
    assert_eq!(model.open_error(), Some(&AgentError::Cancelled));
    assert_eq!(server.connections(), 1, "the cancelled open dialled");
    assert_eq!(during, 1, "the URL while its handshake waited");
    assert_eq!(memory.occurrences(&masked), 0);
}

#[test]
fn no_memory_holds_the_url_after_an_open_the_agent_refused_for_its_formats() {
    let mut memory = Memory::new();
    let server = {
        let mut script = Script::new(Vec::new());
        script.metadata = Some(("pcm_16000", "pcm_24000"));
        AgentServer::start(script)
    };
    let (signed, masked) = signed(&server.url, 7);
    let mut model = ElevenLabsAgent::signed(signed, endpoint());
    assert_eq!(memory.occurrences(&masked), 1);

    let refused = model.open(config());
    assert_eq!(
        refused.err(),
        Some(OpenError::Unsupported(Unsupported::OutputFormat(pcm(
            16_000
        ))))
    );
    assert_eq!(memory.occurrences(&masked), 0);
}
