//! The voice cascade on the testkit mocks (`story:voice-cascade-on-mocks`; topology B of
//! `docs/design/inference.md` 759-778, A-12 at 1148 and the P4 exit at 1162).
//!
//! The testkit's cascade fixture declares topology B under the `offline_strict` profile with
//! egress denied, with exposed ends and no source or sink node: the audio ingress, at the fan-out
//! in front of the voice activity detector and the STT node, and three egresses, the STT node's
//! transcript events, the committed assistant text and the synthesized audio. The test is the
//! application: it sends the synthetic utterance and the end of input into the ingress, and reads
//! each egress until the node's end is gone. It finds the committed span of the scripted
//! question, the committed text of the scripted reply and the frames the mock synthesiser made
//! for that text, and the run Completed after the `stop` with every queue dropped.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_pipeline::Declaration;
use inference_runtime::RunController;
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_testkit::cascade::{
    AUDIO_INGRESS, CascadePayload, Mocks, SPEECH_EGRESS, TEXT_EGRESS, TRANSCRIPT_EGRESS, TurnStore,
    declare,
};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::inference_stt::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame,
};
use inference_testkit::stt::synthetic_audio::{Stretch, synthesize};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep,
};
use inference_testkit::tts::{
    FRAME_SAMPLES, MockConfiguration as TtsConfiguration, MockSynthesiser,
};
use inference_text::{SpanState, TurnState};
use std::num::{NonZeroU16, NonZeroU32};

/// What the user asks: the recogniser's scripted span.
const QUESTION: &str = "how warm is it outside";

/// The mock LLM's answer to the question.
fn reply(question: &str) -> String {
    format!("I cannot know {question}")
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes: the runtime commits to no executor.
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

/// The scripted mocks: the recogniser emits the question and commits it, the LLM answers with
/// [`reply`], the synthesiser streams.
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

/// The synthetic utterance: 300 ms of a 440 Hz tone at half scale, then 800 ms of silence.
fn utterance() -> Vec<AudioFrame> {
    synthesize(&[
        Stretch::Tone {
            frames: 30,
            hertz: 440,
            amplitude: 0.5,
        },
        Stretch::Silence { frames: 80 },
    ])
}

/// What the application sends on the ingress: frames of its own stream, numbered from 1, on the
/// audio frame clock, then the end of input.
struct Microphone {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Microphone {
    fn envelope(
        &mut self,
        kind: &str,
        timestamp: i64,
        payload: CascadePayload,
    ) -> EventEnvelope<CascadePayload> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: AUDIO_INGRESS.to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp,
            timebase: Timebase::AudioFrameTime,
            causation_id: None,
            payload,
        }
    }
}

/// Everything `egress` delivers until the node's end of it is gone.
fn drain(mut egress: Box<dyn InputEndpoint<CascadePayload>>) -> Vec<EventEnvelope<CascadePayload>> {
    let mut received = Vec::new();
    while let Some(envelope) = block_on(egress.recv()) {
        received.push(envelope);
    }
    received
}

/// The stream events among `received`.
fn events(received: &[EventEnvelope<CascadePayload>]) -> Vec<StreamEvent> {
    received
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            CascadePayload::Stream(event) => Some(event.clone()),
            _ => None,
        })
        .collect()
}

/// Whether the last of `received` is the end of input.
fn ends_with_stop(received: &[EventEnvelope<CascadePayload>]) -> bool {
    received
        .last()
        .is_some_and(|envelope| envelope.kind == ControlKind::Stop.as_str())
}

#[test]
fn the_utterance_through_the_ingress_comes_out_as_transcript_reply_text_and_speech() {
    let mocks = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade validates and plans");
    let run_id = RunId::generate();
    let turns = TurnStore::default();
    let log = RunLog::new();
    let mut run = cascade.nodes(run_id, mocks, turns.clone(), log.clone());
    let mut controller = RunController::create_as(
        run_id,
        &cascade.planned,
        &declaration,
        std::collections::HashMap::<GenerationId, i64>::new(),
        |node: &inference_pipeline::Node| {
            run.implementations
                .remove(&node.node_id())
                .expect("the fixture has an implementation for every node")
        },
    )
    .expect("the cascade is planned and no edge cancels or supersedes");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    let mut ends = controller.exposed();
    let ingress: Box<dyn OutputEndpoint<CascadePayload>> =
        ends.ingress(AUDIO_INGRESS).expect("the audio ingress");
    let transcript = ends
        .egress(TRANSCRIPT_EGRESS)
        .expect("the transcript egress");
    let text = ends.egress(TEXT_EGRESS).expect("the assistant text egress");
    let speech = ends
        .egress(SPEECH_EGRESS)
        .expect("the synthesized audio egress");
    block_on(controller.activate()).expect("every node starts");
    assert_eq!(controller.run().state(), RunState::Active);
    let queues = controller.queues();
    let exposure_queues = controller.exposure_queues();
    assert_eq!(exposure_queues.len(), 4, "one queue per exposure");
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });

    let mut microphone = Microphone {
        run_id,
        producer: NodeId::generate(),
        stream_id: StreamId::generate(),
        sequence: 0,
    };
    let mut last = 0;
    for frame in utterance() {
        last = i64::try_from(frame.position.get()).expect("within i64")
            + i64::from(frame.sample_count);
        let timestamp = i64::try_from(frame.position.get()).expect("within i64");
        let envelope = microphone.envelope(
            "inference.audio.AudioFrame",
            timestamp,
            CascadePayload::Audio(frame),
        );
        block_on(ingress.send(envelope)).expect("the ingress takes every frame");
    }
    let stop = microphone.envelope(
        ControlKind::Stop.as_str(),
        last,
        CascadePayload::Control(ControlKind::Stop),
    );
    block_on(ingress.send(stop)).expect("the ingress takes the end of input");

    let transcript = drain(transcript);
    let text = drain(text);
    let speech = drain(speech);
    drop(ingress);
    drop(ends);
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    assert_eq!(run.handles.stt.failure(), None);
    assert_eq!(run.handles.llm.failure(), None);
    assert_eq!(run.handles.tts.failure(), None);
    result.expect("the drive loop ends the run");
    assert_eq!(
        controller.run().state(),
        RunState::Completed,
        "the stop on the ingress ended the run's input"
    );
    for node_id in cascade.ids.nodes() {
        assert_eq!(
            controller
                .instance(node_id)
                .map(|instance| instance.state()),
            Some(NodeInstanceState::Stopped),
            "the instance of node {node_id}"
        );
    }
    assert_eq!(controller.refusals(), []);

    // The transcript: the question's span, emitted and committed, under the turn the policy
    // opened and completed.
    let turn_rows = turns.with(|stream| stream.turns_view());
    assert_eq!(turn_rows.len(), 1, "one utterance, one turn");
    assert_eq!(turn_rows[0].state, TurnState::Completed);
    let transcribed = events(&transcript);
    let committed: Vec<&str> = transcribed
        .iter()
        .filter_map(|event| match event {
            StreamEvent::TranscriptCommitted(committed) => Some(committed.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(committed, [QUESTION], "the committed span of the question");
    assert!(matches!(
        transcribed.first(),
        Some(StreamEvent::TranscriptHypothesis(hypothesis))
            if hypothesis.turn_id == turn_rows[0].turn_id && hypothesis.text == QUESTION
    ));
    assert!(ends_with_stop(&transcript));
    let spans = run.handles.stt.spans();
    assert_eq!(
        spans
            .iter()
            .map(|span| (span.turn_id(), span.state()))
            .collect::<Vec<_>>(),
        [(turn_rows[0].turn_id, SpanState::Committed)]
    );

    // The assistant text: the scripted reply, committed.
    let answered = events(&text);
    let replies: Vec<(String, inference_text::SegmentId)> = answered
        .iter()
        .filter_map(|event| match event {
            StreamEvent::TextSegmentCommitted(committed) => {
                Some((committed.text.clone(), committed.segment_id))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        replies
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>(),
        [reply(QUESTION).as_str()]
    );
    assert!(matches!(
        answered.first(),
        Some(StreamEvent::GenerationStarted(_))
    ));
    assert!(matches!(
        answered.last(),
        Some(StreamEvent::GenerationCompleted(_))
    ));
    assert!(ends_with_stop(&text));

    // The speech: the frames the mock synthesised for that text, one per word, then its
    // completion.
    let frames: Vec<&AudioFrame> = speech
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            CascadePayload::Audio(frame) => Some(frame),
            _ => None,
        })
        .collect();
    let words = reply(QUESTION).split_whitespace().count();
    assert_eq!(frames.len(), words, "one frame per word of the reply");
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(
            frame.position.get(),
            index as u64 * u64::from(FRAME_SAMPLES),
            "the frames follow one another on the synthesis's clock"
        );
    }
    let segment = replies[0].1;
    let spoken = events(&speech);
    assert!(spoken.iter().any(
        |event| matches!(event, StreamEvent::SegmentQueued(queued) if queued.segment_id == segment)
    ));
    assert!(spoken.iter().any(
        |event| matches!(event, StreamEvent::SynthesisCompleted(done) if done.segment_id == segment)
    ));
    assert!(ends_with_stop(&speech));

    // Every queue of the run, the exposures' among them, is dropped.
    for queue in &queues {
        assert!(
            queue.is_dropped(),
            "the queue of edge {} is dropped",
            queue.edge_id()
        );
    }
    for queue in &exposure_queues {
        assert!(
            queue.is_dropped(),
            "the queue of exposure {} is dropped",
            queue.exposure_id()
        );
    }
}
