//! The cascade's speech node stamps each frame it sends with the name a synthesis gives a frame,
//! `inference.audio.AudioChunk` (`story:audio-envelope-kind-is-the-declared-event`). The unit's own
//! tests observe the kind of the testkit's TTS node and of the device source; no test built without
//! the `device` feature observes the kind the cascade's speech node writes, so a speech node left on
//! the old kind would pass every one of them. This case runs the cascade on the mocks and reads the
//! kind of every frame on the speech egress.

use std::collections::HashMap;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::pin::pin;
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_pipeline::Declaration;
use inference_runtime::RunController;
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

const QUESTION: &str = "how warm is it outside";

fn reply(question: &str) -> String {
    format!("I cannot know {question}")
}

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

fn mocks() -> Mocks {
    Mocks {
        recogniser: MockRecogniser::new(
            SttConfiguration::Incremental,
            ModelId::generate(),
            vec![
                Cue {
                    after_frames: 1,
                    step: ScriptStep::Hypothesis(QUESTION.to_owned()),
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

fn drain(mut egress: Box<dyn InputEndpoint<CascadePayload>>) -> Vec<EventEnvelope<CascadePayload>> {
    let mut received = Vec::new();
    while let Some(envelope) = block_on(egress.recv()) {
        received.push(envelope);
    }
    received
}

/// Runs the cascade on the mocks over one utterance and answers what its speech egress delivered.
fn speech_of_one_utterance() -> Vec<EventEnvelope<CascadePayload>> {
    let mocks = mocks();
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
    .expect("the cascade is planned");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    let mut ends = controller.exposed();
    let ingress: Box<dyn OutputEndpoint<CascadePayload>> =
        ends.ingress(AUDIO_INGRESS).expect("the audio ingress");
    let transcript = ends
        .egress(TRANSCRIPT_EGRESS)
        .expect("the transcript egress");
    let text = ends.egress(TEXT_EGRESS).expect("the text egress");
    let speech = ends.egress(SPEECH_EGRESS).expect("the speech egress");
    block_on(controller.activate()).expect("every node starts");
    let drive = thread::spawn(move || block_on(controller.drive()));

    let producer = NodeId::generate();
    let stream_id = StreamId::generate();
    let mut sequence = 0;
    let mut envelope = |kind: &str, timestamp: i64, payload: CascadePayload| {
        sequence += 1;
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
    };
    let mut last = 0;
    for frame in synthesize(&[
        Stretch::Tone {
            frames: 30,
            hertz: 440,
            amplitude: 0.5,
        },
        Stretch::Silence { frames: 80 },
    ]) {
        let timestamp = i64::try_from(frame.position.get()).expect("within i64");
        last = timestamp + i64::from(frame.sample_count);
        block_on(ingress.send(envelope(
            "inference.audio.AudioChunk",
            timestamp,
            CascadePayload::Audio(frame),
        )))
        .expect("the ingress takes every frame");
    }
    block_on(ingress.send(envelope(
        ControlKind::Stop.as_str(),
        last,
        CascadePayload::Control(ControlKind::Stop),
    )))
    .expect("the ingress takes the end of input");
    let _ = drain(transcript);
    let _ = drain(text);
    let speech = drain(speech);
    drop(ingress);
    drop(ends);
    drive
        .join()
        .expect("the drive loop does not panic")
        .expect("the drive loop ends the run");
    assert_eq!(run.handles.tts.failure(), None);
    speech
}

#[test]
fn the_cascade_speech_node_stamps_every_frame_inference_audio_audio_chunk() {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(speech_of_one_utterance());
    });
    let speech = finished
        .recv_timeout(Duration::from_secs(20))
        .expect("the cascade ends within 20 s");
    let frames: Vec<&str> = speech
        .iter()
        .filter(|envelope| matches!(envelope.payload, CascadePayload::Audio(_)))
        .map(|envelope| envelope.kind.as_str())
        .collect();
    let words = reply(QUESTION).split_whitespace().count();
    assert_eq!(
        frames,
        vec!["inference.audio.AudioChunk"; words],
        "one frame per word, each of the declared event: {speech:?}"
    );
}
