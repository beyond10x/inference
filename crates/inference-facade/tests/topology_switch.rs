//! One session switches between the text-only pipeline and the voice cascade on the mocks with
//! one event vocabulary (`story:session-topology-switch`; A-12 at `docs/design/inference.md`
//! 1148, the P4 exit at 1162), and describe reports what runs.
//!
//! The session opens over topology A, switches to topology B and is fed the synthetic utterance,
//! then switches back. Both topologies run on the cascade's payload, so one session carries them;
//! what the application sends differs by topology, text or audio frames, and what it receives
//! does not: [`until_reply`] is written once against `SessionEvent`, with no branch on topology.

#![allow(dead_code)]

mod text;

use std::collections::HashMap;
use std::num::{NonZeroU16, NonZeroU32};
use std::thread;

use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{Capability, ModelId};
use inference_facade::events::SessionEvent;
use inference_facade::history::HistoryKind;
use inference_facade::live::{LiveSession, LiveSessions};
use inference_facade::session::{SessionState, SwitchPipelineError};
use inference_facade::switch::SwitchError;
use inference_pipeline::{
    Declaration, InteractionMode, Node, NodeImplementation, PipelineNotFound,
};
use inference_runtime::run::RunState;
use inference_testkit::cascade::{AUDIO_INGRESS, CascadePayload, Mocks, TurnStore, declare};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::inference_stt::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame,
};
use inference_testkit::stt::synthetic_audio::{Stretch, synthesize};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep,
};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use inference_text::{SpanId, TranscriptCommitted};

use text::{INGRESS, block_on, generator, planned, reply};

/// What the user asks by voice: the recogniser's scripted span.
const QUESTION: &str = "how warm is it outside";

/// The voice cascade's LLM answer.
fn voice_reply(question: &str) -> String {
    format!("I cannot know {question}")
}

/// The cascade's scripted mocks: the recogniser emits the question and commits it, the LLM
/// answers with [`voice_reply`], the synthesiser streams.
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
        reply: voice_reply,
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

/// The text pipeline's one node on the cascade's payload: the mock model over `generator`.
fn text_nodes(
    generator: &MockGenerator,
) -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<CascadePayload>> {
    let generator = generator.clone();
    move |run_id, _| {
        Box::new(ModelNode::<CascadePayload>::carrying(
            run_id,
            generator.clone(),
            reply,
            RunLog::new(),
        ))
    }
}

/// The cascade's nodes for whichever run asks first, over `mocks`.
fn voice_nodes(
    cascade: &inference_testkit::cascade::VoiceCascade,
    mocks: &Mocks,
) -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<CascadePayload>> {
    let cascade = cascade.clone();
    let mocks = mocks.clone();
    let mut nodes: Option<HashMap<NodeId, Box<dyn NodeImplementation<CascadePayload>>>> = None;
    move |run_id, node| {
        nodes
            .get_or_insert_with(|| {
                cascade
                    .nodes(run_id, mocks.clone(), TurnStore::default(), RunLog::new())
                    .implementations
            })
            .remove(&node.node_id())
            .expect("the cascade has an implementation for every node")
    }
}

/// What the application sends, on a stream of its own numbered from 1.
struct Application {
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Application {
    fn new() -> Self {
        Self {
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
            sequence: 0,
        }
    }

    fn envelope(
        &mut self,
        port: &str,
        kind: &str,
        timebase: Timebase,
        timestamp: i64,
        payload: CascadePayload,
    ) -> EventEnvelope<CascadePayload> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: RunId::generate(),
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: port.to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp,
            timebase,
            causation_id: None,
            payload,
        }
    }

    /// Typed text for the text pipeline's ingress.
    fn text(&mut self, text: &str) -> EventEnvelope<CascadePayload> {
        let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: text.to_owned(),
        });
        let kind = event.name();
        self.envelope(
            INGRESS,
            kind,
            Timebase::WallTime,
            0,
            CascadePayload::Stream(event),
        )
    }

    /// A frame of the utterance for the cascade's audio ingress.
    fn frame(&mut self, frame: AudioFrame) -> EventEnvelope<CascadePayload> {
        let timestamp = i64::try_from(frame.position.get()).expect("within i64");
        self.envelope(
            AUDIO_INGRESS,
            "inference.audio.AudioFrame",
            Timebase::AudioFrameTime,
            timestamp,
            CascadePayload::Audio(frame),
        )
    }
}

/// Receives the session's events into `received` until the assistant's committed reply, and
/// answers its text: written once, with no branch on the topology the session runs.
fn until_reply(
    session: &mut LiveSession<CascadePayload>,
    received: &mut Vec<SessionEvent>,
) -> String {
    loop {
        let event = block_on(session.receive()).expect("the session delivers until it closes");
        let reply = match &event {
            SessionEvent::TextSegmentCommitted(segment) => Some(segment.text.clone()),
            _ => None,
        };
        received.push(event);
        if let Some(reply) = reply {
            return reply;
        }
    }
}

/// The position in `received` of the first event `matches` accepts.
fn position(received: &[SessionEvent], matches: impl Fn(&SessionEvent) -> bool) -> usize {
    received
        .iter()
        .position(matches)
        .expect("the event was delivered")
}

#[test]
fn one_session_switches_text_to_voice_and_back_with_one_vocabulary_and_one_history() {
    let text_generator = generator(Vec::<String>::new());
    let (text_declaration, text_planned) = planned(&text_generator, &[Capability::OutputStreaming]);
    let mocks = mocks();
    let mut voice_declaration = Declaration::new();
    let cascade = declare(&mut voice_declaration, &mocks).expect("the cascade plans");
    let sessions = LiveSessions::new();
    let mut application = Application::new();
    let mut received = Vec::new();

    // Topology A.
    let (mut session, driver) = block_on(sessions.open(
        &text_declaration,
        &text_planned,
        InteractionMode::TurnBased,
        text_nodes(&text_generator),
    ))
    .expect("a session opens over the text pipeline");
    let session_id = session.session_id();
    let first_run = session.run_id();
    let text_described = session
        .describe(&text_declaration)
        .expect("the text pipeline is described");
    let first = thread::spawn(move || block_on(driver.drive()));
    block_on(session.send(application.text("hello"))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    assert_eq!(until_reply(&mut session, &mut received), "shello");

    // Topology B.
    let driver = block_on(session.switch(
        &voice_declaration,
        &cascade.planned,
        InteractionMode::TurnBased,
        voice_nodes(&cascade, &mocks),
    ))
    .expect("a Ready session switches to the voice cascade");
    let second_run = session.run_id();
    assert_ne!(second_run, first_run);
    let first = first.join().expect("no panic").expect("the text run ends");
    assert_eq!(first.state(), RunState::Completed, "the old run drained");
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Ready)
    );
    let voice_described = session
        .describe(&voice_declaration)
        .expect("the cascade is described");
    let second = thread::spawn(move || block_on(driver.drive()));
    for frame in utterance() {
        block_on(session.send(application.frame(frame))).expect("the ingress takes every frame");
    }
    assert_eq!(
        until_reply(&mut session, &mut received),
        voice_reply(QUESTION)
    );

    // Topology A again.
    let driver = block_on(session.switch(
        &text_declaration,
        &text_planned,
        InteractionMode::TurnBased,
        text_nodes(&text_generator),
    ))
    .expect("the session switches back");
    let third_run = session.run_id();
    let second = second
        .join()
        .expect("no panic")
        .expect("the voice run ends");
    assert_eq!(second.state(), RunState::Completed, "the old run drained");
    let third = thread::spawn(move || block_on(driver.drive()));
    block_on(session.send(application.text("again"))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    assert_eq!(until_reply(&mut session, &mut received), "sagain");
    let back_described = session
        .describe(&text_declaration)
        .expect("the text pipeline is described again");

    block_on(session.close()).expect("drains");
    while let Some(event) = block_on(session.receive()) {
        received.push(event);
    }
    let third = third.join().expect("no panic").expect("the text run ends");
    assert_eq!(third.state(), RunState::Completed);
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Closed)
    );

    // The old run is Completed before the new one is attached, at each switch.
    for (old, new) in [(first_run, second_run), (second_run, third_run)] {
        let completed = position(
            &received,
            |event| matches!(event, SessionEvent::RunCompleted(completed) if completed.run_id == old),
        );
        let attached = position(
            &received,
            |event| matches!(event, SessionEvent::RunAttached(attached) if attached.run_id == new),
        );
        assert!(
            completed < attached,
            "run {old} completed at {completed}, run {new} attached at {attached}"
        );
    }
    assert!(
        received
            .iter()
            .any(|event| matches!(event, SessionEvent::AudioChunk(_))),
        "the voice phase delivered speech in the same vocabulary"
    );

    // The history of all three phases, in order, each entry naming its run.
    assert_eq!(
        sessions
            .history(session_id)
            .iter()
            .map(|entry| (entry.kind(), entry.text().to_owned(), entry.run_id()))
            .collect::<Vec<_>>(),
        [
            (HistoryKind::UserTurn, "hello".to_owned(), first_run),
            (
                HistoryKind::AssistantSegment,
                "shello".to_owned(),
                first_run
            ),
            (HistoryKind::UserTurn, QUESTION.to_owned(), second_run),
            (
                HistoryKind::AssistantSegment,
                voice_reply(QUESTION),
                second_run
            ),
            (HistoryKind::UserTurn, "again".to_owned(), third_run),
            (
                HistoryKind::AssistantSegment,
                "sagain".to_owned(),
                third_run
            ),
        ]
    );

    // describe follows the switch.
    assert_ne!(voice_described.pipeline_id, text_described.pipeline_id);
    assert_ne!(voice_described.name, text_described.name);
    assert_ne!(
        voice_described.effective_capabilities, text_described.effective_capabilities,
        "the voice phase runs with other effective capabilities"
    );
    assert_eq!(back_described, text_described);
    assert_eq!(voice_described.nodes.len(), 7);
    assert_eq!(
        voice_described
            .nodes
            .iter()
            .filter(|node| node.adapter_id.is_some())
            .count(),
        3,
        "the three model nodes are bound"
    );
    assert_eq!(voice_described.exposures.len(), 4);
}

#[test]
fn a_switch_to_a_pipeline_the_declaration_does_not_hold_is_refused_and_moves_nothing() {
    let text_generator = generator(Vec::<String>::new());
    let (text_declaration, text_planned) = planned(&text_generator, &[Capability::OutputStreaming]);
    let mocks = mocks();
    let mut voice_declaration = Declaration::new();
    let cascade = declare(&mut voice_declaration, &mocks).expect("the cascade plans");
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &text_declaration,
        &text_planned,
        InteractionMode::TurnBased,
        text_nodes(&text_generator),
    ))
    .expect("opens");
    let session_id = session.session_id();
    let run_id = session.run_id();
    let drive = thread::spawn(move || block_on(driver.drive()));

    let refused = block_on(session.switch(
        &text_declaration,
        &cascade.planned,
        InteractionMode::TurnBased,
        voice_nodes(&cascade, &mocks),
    ));
    assert!(
        matches!(
            refused,
            Err(SwitchError::Refused(SwitchPipelineError::UnknownPipeline(PipelineNotFound {
                pipeline_id
            }))) if pipeline_id == cascade.pipeline_id
        ),
        "the cascade is not in the text declaration"
    );
    let after = sessions.session(session_id).expect("stored");
    assert_eq!(after.state(), SessionState::Ready);
    assert_eq!(after.run_id(), Some(run_id), "the session keeps its run");
    assert_eq!(after.pipeline_id(), text_planned.pipeline_id());

    let refused = block_on(session.switch(
        &voice_declaration,
        &cascade.planned,
        InteractionMode::Duplex,
        voice_nodes(&cascade, &mocks),
    ));
    assert!(
        matches!(
            refused,
            Err(SwitchError::Refused(SwitchPipelineError::ModeMismatch(_)))
        ),
        "the session's mode is the pipeline's"
    );
    assert_eq!(
        sessions.session(session_id).map(|s| s.run_id()),
        Some(Some(run_id))
    );

    let mut application = Application::new();
    block_on(session.send(application.text("still here"))).expect("still Ready");
    block_on(session.close()).expect("drains");
    while block_on(session.receive()).is_some() {}
    let run = drive.join().expect("no panic").expect("the run ends");
    assert_eq!(run.state(), RunState::Completed);
}
