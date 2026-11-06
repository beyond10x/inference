//! Adversary pass 1 on the OpenAI speech binding's audio path: the resampler and the G.711
//! encoder must give the same frames however the body is cut, each segment on its own filter
//! with no sample lost or repeated at a segment's edge; the character limit at its exact bound
//! and in code points; and a synthesis iterated on a current-thread runtime's own thread.

mod speech_support;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use inference_openai::speech::{
    MAX_INPUT_CHARACTERS, OpenAiSpeech, SpeechConfig, SpeechEndpoint, SpeechVoice,
};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use speech_support::{
    FixedKey, FixtureServer, KEY, Script, VOICE, committed, format, tone_bytes, within_deadline,
};

/// A loopback server answering every request on every connection with the tone, its body cut
/// into chunks whose sizes cycle through `sizes`, each written and flushed on its own.
fn chunking_server(sizes: &'static [usize]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let address = listener.local_addr().expect("an address");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            std::thread::spawn(move || serve(stream, sizes));
        }
    });
    format!("http://{address}/v1/audio/speech")
}

fn serve(mut stream: TcpStream, sizes: &'static [usize]) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let tone = tone_bytes();
    loop {
        if !read_request(&mut stream) {
            return;
        }
        let head = b"HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\n\
                     transfer-encoding: chunked\r\n\r\n";
        if stream.write_all(head).is_err() {
            return;
        }
        let mut start = 0;
        let mut index = 0;
        while start < tone.len() {
            let size = sizes[index % sizes.len()].min(tone.len() - start);
            index += 1;
            let mut chunk = format!("{size:x}\r\n").into_bytes();
            chunk.extend_from_slice(&tone[start..start + size]);
            chunk.extend_from_slice(b"\r\n");
            if stream
                .write_all(&chunk)
                .and_then(|()| stream.flush())
                .is_err()
            {
                return;
            }
            if sizes.len() > 1 && index % 16 == 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
            start += size;
        }
        if stream.write_all(b"0\r\n\r\n").is_err() {
            return;
        }
    }
}

fn read_request(stream: &mut TcpStream) -> bool {
    let mut buffer = Vec::new();
    let mut byte = [0_u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => buffer.push(byte[0]),
            _ => return false,
        }
    }
    let head = String::from_utf8_lossy(&buffer).to_lowercase();
    let length = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).is_ok()
}

fn synthesiser(url: &str, output: AudioFormat) -> OpenAiSpeech {
    OpenAiSpeech::new(
        SpeechEndpoint::new()
            .with_url(url)
            .with_read_timeout(Duration::from_secs(5))
            .with_connect_timeout(Duration::from_secs(2)),
        SpeechConfig::new(SpeechVoice::BuiltIn(VOICE.to_owned()), output),
        Arc::new(FixedKey(KEY)),
        "openai-api-key",
    )
    .expect("a loopback endpoint")
}

/// Every segment's audio data in order, the frames checked contiguous on one clock.
fn speak(synthesiser: &mut OpenAiSpeech, texts: &[&str]) -> Vec<Vec<u8>> {
    let mut handle = synthesiser.start().expect("a slot");
    let segments = committed(texts);
    let ids: Vec<_> = segments
        .iter()
        .map(|segment| segment.segment_id())
        .collect();
    for segment in segments {
        handle.offer_segment(segment).unwrap();
    }
    handle.end_utterance().unwrap();
    let mut data = vec![Vec::new(); ids.len()];
    let mut next = 0_u64;
    for event in handle.by_ref() {
        if let SynthesisEvent::Audio(audio) = event {
            let frame = audio.frame();
            assert_eq!(frame.position.get(), next, "frames are contiguous");
            next += u64::from(frame.sample_count);
            let at = ids.iter().position(|id| *id == audio.segment_id()).unwrap();
            data[at].extend_from_slice(&frame.data);
        }
    }
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    data
}

const ONE_CHUNK: &[usize] = &[usize::MAX];
const RAGGED: &[usize] = &[1, 3, 2, 5, 7, 1, 1, 4, 9, 6];

#[test]
fn ragged_chunks_give_the_same_frames_as_one_chunk_at_every_rate() {
    within_deadline(|| {
        let whole = chunking_server(ONE_CHUNK);
        let ragged = chunking_server(RAGGED);
        for (encoding, rate, samples) in [
            (AudioEncoding::PcmMulaw, 8_000, 1_600_usize),
            (AudioEncoding::PcmAlaw, 8_000, 1_600),
            (AudioEncoding::PcmS16le, 16_000, 3_200 * 2),
            (AudioEncoding::PcmS16le, 48_000, 9_600 * 2),
        ] {
            let wanted = format(encoding, rate);
            let one = speak(&mut synthesiser(&whole, wanted), &["Please hold."]);
            let cut = speak(&mut synthesiser(&ragged, wanted), &["Please hold."]);
            assert_eq!(one[0].len(), samples, "{encoding:?} at {rate}: whole body");
            assert_eq!(
                one, cut,
                "{encoding:?} at {rate}: a body cut into ragged chunks is not the same audio"
            );
        }
    });
}

#[test]
fn each_segment_is_resampled_on_its_own_filter_and_loses_no_sample() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let wanted = format(AudioEncoding::PcmMulaw, 8_000);
        let alone = speak(&mut server.synthesiser(wanted), &["One."]);
        let three = speak(&mut server.synthesiser(wanted), &["One.", "Two.", "Three."]);
        assert_eq!(three.len(), 3);
        for segment in &three {
            assert_eq!(
                segment, &alone[0],
                "a segment's audio depends on the segment before it"
            );
        }
    });
}

#[test]
fn exactly_the_limit_is_sent_and_the_limit_counts_code_points_not_utf16_units() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let wanted = format(AudioEncoding::PcmMulaw, 8_000);
        let at_limit = "a".repeat(MAX_INPUT_CHARACTERS);
        let astral = "\u{1F600}".repeat(MAX_INPUT_CHARACTERS);
        let spoken = speak(&mut server.synthesiser(wanted), &[&at_limit, &astral]);
        assert_eq!(spoken.len(), 2);
        assert!(spoken.iter().all(|data| !data.is_empty()));
        assert_eq!(server.inputs(), [at_limit, astral]);
        assert_eq!(server.violations(), Vec::<String>::new());
    });
}

#[test]
fn a_synthesis_iterated_on_a_current_thread_runtime_speaks_and_drops_cleanly() {
    within_deadline(|| {
        let server = FixtureServer::scripted(vec![Script::Serve, Script::Stall]);
        let wanted = format(AudioEncoding::PcmMulaw, 8_000);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let spoken = runtime.block_on(async {
            let mut synthesiser = server.synthesiser(wanted);
            let mut handle = synthesiser.start().unwrap();
            let mut segments = committed(&["Spoken whole.", "Cut off."]).into_iter();
            handle.offer_segment(segments.next().unwrap()).unwrap();
            let mut samples = 0_u64;
            for event in handle.by_ref() {
                if let SynthesisEvent::Audio(audio) = event {
                    samples += u64::from(audio.frame().sample_count);
                }
            }
            let cut = segments.next().unwrap();
            let cut_id = cut.segment_id();
            handle.offer_segment(cut).unwrap();
            assert!(matches!(handle.next(), Some(SynthesisEvent::Audio(_))));
            handle.discard(cut_id).unwrap();
            assert!(handle.failure().is_none(), "{:?}", handle.failure());
            drop(handle);
            samples
        });
        assert_eq!(spoken, 1_600);
        assert_eq!(server.next_client_close(Duration::from_secs(5)), Some(1));
    });
}
