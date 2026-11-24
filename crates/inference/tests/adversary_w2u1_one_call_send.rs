#![cfg(feature = "testing")]
//! "Handles are `Send`" (`docs/design/sdk.md` §8) is worth what the futures of their methods are:
//! a future that is not `Send` cannot be given to `tokio::spawn` on the multi-thread runtime
//! `#[inference::main]` starts. This file compiles only when the future of every one-call
//! function and of every data method of `Chat`, `Transcriber` and `Voice` is `Send`.

use std::num::{NonZeroU16, NonZeroU32};

use inference::audio::{AudioEncoding, AudioFormat};
use inference::audio_device::fake::FakeHost;
use inference::core::model::ModelId;
use inference::{Binding, Chat, Stack, Transcriber, Voice};
use inference_testkit::llm::{self, MockGenerator};
use inference_testkit::stt::{self, MockRecogniser};
use inference_testkit::tts::{self, MockSynthesiser};

fn sendable<T: Send>(_: &T) {}

#[test]
fn the_futures_of_the_one_call_functions_and_of_the_handles_are_send() {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("a rate"),
        channels: NonZeroU16::MIN,
    };
    let stack = Stack::new()
        .bind(Binding::llm(MockGenerator::new(
            llm::MockConfiguration::StreamingAndConcurrentInput,
            ModelId::generate(),
            ["hello"],
        )))
        .bind(Binding::stt(MockRecogniser::new(
            stt::MockConfiguration::Incremental,
            ModelId::generate(),
            stt::script(),
        )))
        .bind(Binding::tts(MockSynthesiser::new(
            tts::MockConfiguration::IncrementalStreaming,
            ModelId::generate(),
            format,
        )));
    let host = FakeHost::new(Vec::new());

    sendable(&inference::chat("a prompt"));
    sendable(&inference::transcribe("a.wav"));
    sendable(&inference::chat_with(&stack, "a prompt"));
    sendable(&inference::transcribe_with(&stack, "a.wav"));
    sendable(&inference::speak_with(&stack, "a text", host.clone()));
    sendable(&inference::talk_with(&stack, host.clone()));

    let mut chat = Chat::new_with(&stack).expect("plans");
    sendable(&chat.send("a prompt"));
    sendable(&chat.stream("a prompt"));
    sendable(&chat.close());

    let mut transcriber = Transcriber::new_with(&stack).expect("plans");
    sendable(&transcriber.transcribe_frames(Vec::new()));
    sendable(&transcriber.transcribe_file("a.wav"));

    let mut voice = Voice::new_with(&stack).expect("plans");
    sendable(&voice.synthesise("a text"));
    sendable(&voice.play("a text", host));
}
