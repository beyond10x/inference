//! Final adversary pass on the voice cascade: a cancel of a run whose application stopped
//! reading an egress.
//!
//! `RunController::cancel` cancels and then stops each node; the stop waits for the node's
//! exchange to end. The tool nodes of `inference-pipeline` send under `send_unless`, so "a cancel
//! taken while the send waits for room ends the wait". The testkit's fan-out, STT, LLM and TTS
//! nodes send with a plain `send(..).await` and look at their cancel only between envelopes. A
//! cascade whose egress the application does not read fills it (every exposure pauses on
//! overload at 64), the node feeding it waits for room, and the cancel waits for that node.

use std::collections::HashMap;
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::sync::mpsc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::OutputEndpoint;
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_pipeline::Declaration;
use inference_runtime::RunController;
use inference_runtime::run::RunState;
use inference_testkit::cascade::{
    AUDIO_INGRESS, CascadePayload, Mocks, SPEECH_EGRESS, TEXT_EGRESS, TRANSCRIPT_EGRESS, TurnStore,
    declare,
};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stt::inference_stt::inference_audio::{AudioEncoding, AudioFormat};
use inference_testkit::stt::synthetic_audio::{Stretch, synthesize};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep,
};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use std::num::{NonZeroU16, NonZeroU32};

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park();
    }
}

/// A reply of 300 words: the synthesiser makes one frame per word, more than an egress holds.
fn long_reply(_: &str) -> String {
    (0..300)
        .map(|index| format!("word{index}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn mocks(reply: fn(&str) -> String) -> Mocks {
    Mocks {
        recogniser: MockRecogniser::new(
            SttConfiguration::Incremental,
            ModelId::generate(),
            vec![
                Cue {
                    after_frames: 1,
                    step: ScriptStep::Hypothesis("tell me a long story".to_owned()),
                },
                Cue {
                    after_frames: 2,
                    step: ScriptStep::Commit { span: 0 },
                },
            ],
        ),
        generator: MockGenerator::new(
            LlmConfiguration::StreamingAndConcurrentInput,
            ModelId::generate(),
            Vec::<String>::new(),
        ),
        synthesiser: MockSynthesiser::new(
            TtsConfiguration::IncrementalStreaming,
            ModelId::generate(),
            AudioFormat {
                encoding: AudioEncoding::PcmS16le,
                sample_rate: NonZeroU32::new(16_000).expect("16000"),
                channels: NonZeroU16::new(1).expect("1"),
            },
        ),
        reply,
    }
}

fn envelope(
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
    kind: &str,
    timestamp: i64,
    payload: CascadePayload,
) -> EventEnvelope<CascadePayload> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id,
        session_id: None,
        stream_id,
        sequence,
        producer,
        port: AUDIO_INGRESS.to_owned(),
        kind: kind.to_owned(),
        generation_id: None,
        timestamp,
        timebase: Timebase::AudioFrameTime,
        causation_id: None,
        payload,
    }
}

/// A reply of 3 words, whose frames and events fit in every egress.
fn short_reply(_: &str) -> String {
    "it is warm".to_owned()
}

/// The application speaks, holds the egresses without reading them, and cancels the run: the
/// cancel ends the run.
#[test]
fn adversary2_a_cancel_ends_a_cascade_whose_egress_nobody_reads() {
    cancel_unread(long_reply);
}

/// The control: the same run, with a reply that fills no egress, is cancelled in time.
#[test]
fn adversary2_control_a_cancel_ends_a_cascade_whose_egresses_have_room() {
    cancel_unread(short_reply);
}

fn cancel_unread(reply: fn(&str) -> String) {
    let mocks = mocks(reply);
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade validates and plans");
    let run_id = RunId::generate();
    let mut run = cascade.nodes(run_id, mocks, TurnStore::default(), RunLog::new());
    let mut controller = RunController::create_as(
        run_id,
        &cascade.planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &inference_pipeline::Node| {
            run.implementations
                .remove(&node.node_id())
                .expect("an implementation for every node")
        },
    )
    .expect("planned");
    controller.validate().expect("validates");
    block_on(controller.prepare()).expect("prepares");
    let mut ends = controller.exposed();
    let ingress: Box<dyn OutputEndpoint<CascadePayload>> =
        ends.ingress(AUDIO_INGRESS).expect("the audio ingress");
    let held = [
        ends.egress(TRANSCRIPT_EGRESS).expect("transcript"),
        ends.egress(TEXT_EGRESS).expect("assistant text"),
        ends.egress(SPEECH_EGRESS).expect("speech"),
    ];
    block_on(controller.activate()).expect("starts");
    let requests = controller.requests();
    let (ended_tx, ended_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = block_on(controller.drive());
        let _ = ended_tx.send((controller.run().state(), result.is_ok()));
    });

    let (producer, stream_id) = (NodeId::generate(), StreamId::generate());
    let mut sequence = 0;
    let mut last = 0;
    for frame in synthesize(&[
        Stretch::Tone {
            frames: 30,
            hertz: 440,
            amplitude: 0.5,
        },
        Stretch::Silence { frames: 80 },
    ]) {
        sequence += 1;
        let position = i64::try_from(frame.position.get()).expect("within i64");
        last = position + i64::from(frame.sample_count);
        let sent = envelope(
            run_id,
            producer,
            stream_id,
            sequence,
            "inference.audio.AudioFrame",
            position,
            CascadePayload::Audio(frame),
        );
        block_on(ingress.send(sent)).expect("the ingress takes every frame");
    }
    thread::sleep(Duration::from_millis(1500));

    requests.cancel();
    let ended = ended_rx.recv_timeout(Duration::from_secs(10));
    drop(held);
    assert_eq!(
        ended.ok(),
        Some((RunState::Cancelled, true)),
        "the cancel ended the run within 10 s while the application held its egresses \
         (last frame at {last})"
    );
    drop(ingress);
}
