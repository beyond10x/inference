//! Run and session usage totals in the facade (`story:usage-session-totals`).
//!
//! A session over the voice cascade on the testkit mocks, each model node's `usage` port exposed
//! as an egress ([`declare`]), answers one spoken turn: it delivers one
//! `UsageRecorded` per model call, one STT, one LLM turn and one TTS, and its usage view holds a
//! total of its run and of the session equal, kind by kind, to the sum of those records, every
//! unknown quantity counted as unknown. The run's nodes are wrapped by a recorder of the runtime
//! and its ingresses tapped by it ([`LiveSession::record_inputs`]).
//!
//! | case | acceptance |
//! |---|---|
//! | [`a_spoken_turn_delivers_one_usage_record_per_model_call_and_totals_them_kind_by_kind`] | one record per model call, the run and session totals their sum, unknowns kept |
//! | [`a_telemetry_channel_that_is_never_read_drops_telemetry_and_no_usage_record`] | a telemetry channel of capacity 1, never read, reports drops; all three records still arrive |
//! | [`the_recording_lists_the_usage_records_and_a_replay_reporting_other_counts_does_not_diverge`] | the recording lists the three usage envelopes; a replay whose LLM reports other counts reports no divergence |
//! | [`a_session_switching_from_text_to_the_cascade_keeps_one_total_per_run_and_one_over_both`] | a total per run and a session total over both runs |

#![allow(dead_code)]

mod text;

use std::collections::HashMap;
use std::num::{NonZeroU16, NonZeroU32, NonZeroUsize};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use inference_core::endpoint::InputEndpoint;
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_core::usage::{KindTotal, QuantityKind, QuantitySource, UsageRecorded, UsageTotal};
use inference_facade::events::SessionEvent;
use inference_facade::live::{AlreadyRecorded, LiveSession, LiveSessions};
use inference_facade::usage::SessionUsage;
use inference_llm::llm_core::Usage;
use inference_pipeline::{Declaration, InteractionMode, Node, NodeImplementation};
use inference_runtime::RunController;
use inference_runtime::record::{Recorder, Recording};
use inference_runtime::replay::Replay;
use inference_runtime::run::RunState;
use inference_runtime::telemetry::{self, TelemetrySink};
use inference_testkit::cascade::{
    AUDIO_INGRESS, CascadePayload, LLM_USAGE_EGRESS, Mocks, SPEECH_EGRESS, STT_USAGE_EGRESS,
    TEXT_EGRESS, TRANSCRIPT_EGRESS, TTS_USAGE_EGRESS, TurnStore, VoiceCascade, declare,
};
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

use text::{INGRESS, block_on, generator, planned_with_usage, reply};

/// What the user asks by voice: the recogniser's scripted span.
const QUESTION: &str = "how warm is it outside";

/// The policy version both the recorded and the replayed run are driven under.
const POLICY: &str = "usage policy 1";

/// How long a replay waits before each input at most.
const PATIENCE: Duration = Duration::from_secs(10);

/// Every egress of the cascade declared with usage.
const EGRESSES: [&str; 6] = [
    TRANSCRIPT_EGRESS,
    TEXT_EGRESS,
    SPEECH_EGRESS,
    STT_USAGE_EGRESS,
    LLM_USAGE_EGRESS,
    TTS_USAGE_EGRESS,
];

/// The voice cascade's LLM answer.
fn voice_reply(question: &str) -> String {
    format!("I cannot know {question}")
}

/// The cascade's scripted mocks: the recogniser emits the question and commits it, the LLM
/// answers with [`voice_reply`] and reports no usage, the synthesiser streams.
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

/// The cascade's nodes for whichever run asks first, over `mocks`.
fn voice_nodes(
    cascade: &VoiceCascade,
    mocks: &Mocks,
) -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<CascadePayload>> + use<> {
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

/// Speaks the utterance into `session`, closes it by draining and receives every event it
/// delivers until none is left.
fn speak_and_close(
    session: &mut LiveSession<CascadePayload>,
    application: &mut Application,
    received: &mut Vec<SessionEvent>,
) {
    for frame in utterance() {
        block_on(session.send(application.frame(frame))).expect("the ingress takes every frame");
    }
    block_on(session.close()).expect("a Ready session drains");
    while let Some(event) = block_on(session.receive()) {
        received.push(event);
    }
}

/// The usage records among `received`, in the order they were delivered.
fn records(received: &[SessionEvent]) -> Vec<&UsageRecorded> {
    received
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) => Some(recorded),
            _ => None,
        })
        .collect()
}

/// The total of `records`, added one by one.
fn total_of<'a>(records: impl IntoIterator<Item = &'a UsageRecorded>) -> UsageTotal {
    let mut total = UsageTotal::new();
    for recorded in records {
        total.add(&recorded.quantities, recorded.r#final);
    }
    total
}

/// One session over the cascade that answered one spoken turn.
struct Voiced {
    declaration: Declaration,
    cascade: VoiceCascade,
    mocks: Mocks,
    run_id: RunId,
    received: Vec<SessionEvent>,
    usage: SessionUsage,
    recording: Recording<CascadePayload>,
}

/// Opens a session over the cascade with usage, every node wrapped by a recorder, offering
/// `telemetry` every envelope it takes, and its ingresses tapped; speaks the utterance, closes the
/// session and receives everything it delivers.
fn voice_session(telemetry: Option<Arc<dyn TelemetrySink>>) -> Voiced {
    let mocks = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade plans");
    let mut recorder = Recorder::new(POLICY);
    if let Some(sink) = telemetry {
        recorder = recorder.with_telemetry(sink);
    }
    let sessions = LiveSessions::new();
    let wrapping = recorder.clone();
    let mut nodes = voice_nodes(&cascade, &mocks);
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &cascade.planned,
        InteractionMode::TurnBased,
        move |run_id, node| wrapping.wrap(node, nodes(run_id, node)),
    ))
    .expect("a session opens over the cascade");
    session
        .record_inputs(&recorder)
        .expect("the ingresses of the new run are not tapped yet");
    let run_id = session.run_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut received = Vec::new();
    speak_and_close(&mut session, &mut Application::new(), &mut received);
    let run = drive.join().expect("no panic").expect("the run ends");
    assert_eq!(run.state(), RunState::Completed);
    assert!(
        matches!(received.last(), Some(SessionEvent::SessionClosed(_))),
        "the session closed: {received:#?}"
    );
    let usage = session.usage();
    Voiced {
        declaration,
        cascade,
        mocks,
        run_id,
        received,
        usage,
        recording: recorder.recording(),
    }
}

/// Asserts that `records` holds exactly one record of each model node of `voiced`'s cascade,
/// each placed on its run.
fn one_per_model_call(voiced: &Voiced, records: &[&UsageRecorded]) {
    let ids = voiced.cascade.ids;
    let mut nodes: Vec<Option<NodeId>> = records.iter().map(|recorded| recorded.node_id).collect();
    nodes.sort_unstable();
    let mut expected = vec![Some(ids.stt), Some(ids.llm), Some(ids.tts)];
    expected.sort_unstable();
    assert_eq!(
        nodes, expected,
        "one usage record per model call, one STT, one LLM turn and one TTS: {records:#?}"
    );
    for recorded in records {
        assert_eq!(recorded.run_id, Some(voiced.run_id), "placed on the run");
    }
}

#[test]
fn a_spoken_turn_delivers_one_usage_record_per_model_call_and_totals_them_kind_by_kind() {
    let voiced = voice_session(None);
    let records = records(&voiced.received);
    one_per_model_call(&voiced, &records);

    let expected = total_of(records.iter().copied());
    assert_eq!(voiced.usage.session, expected, "the session total");
    assert_eq!(
        voiced.usage.runs,
        [(voiced.run_id, expected.clone())],
        "the run's total, the session's one run"
    );
    assert_eq!(voiced.usage.run(voiced.run_id), Some(&expected));
    assert_eq!(voiced.usage.session.records(), 3);

    // Kind by kind, with the unknowns of the mocks counted as unknown.
    let session = &voiced.usage.session;
    assert_eq!(
        session.kind(QuantityKind::Requests),
        Some(KindTotal {
            known: 3,
            unknown: 0,
            records: 3,
            partial: 0,
        }),
        "each model call is one request"
    );
    assert_eq!(
        session.kind(QuantityKind::LocalComputeMs),
        Some(KindTotal {
            known: 0,
            unknown: 3,
            records: 3,
            partial: 0,
        }),
        "no mock computes anything of its own: unknown, never zero"
    );
    assert_eq!(
        session.kind(QuantityKind::InputTextTokens),
        Some(KindTotal {
            known: 0,
            unknown: 1,
            records: 1,
            partial: 0,
        }),
        "the mock LLM reports no tokens"
    );
    for (kind, total) in session.kinds() {
        let quantities: Vec<_> = records
            .iter()
            .flat_map(|recorded| &recorded.quantities)
            .filter(|quantity| quantity.kind() == kind)
            .collect();
        let known: i128 = quantities
            .iter()
            .filter_map(|quantity| quantity.value())
            .map(i128::from)
            .sum();
        let unknown = quantities
            .iter()
            .filter(|quantity| quantity.source() == QuantitySource::Unknown)
            .count();
        assert_eq!(total.known, known, "{kind:?}");
        assert_eq!(
            total.unknown,
            u64::try_from(unknown).expect("small"),
            "{kind:?}"
        );
        assert_eq!(total.complete(), unknown == 0, "{kind:?}");
    }
    assert!(
        session
            .kind(QuantityKind::InputAudioMs)
            .is_some_and(|total| total.known > 0 && total.complete()),
        "the STT node measured the audio it heard"
    );
    assert!(!session.complete());
}

#[test]
fn a_telemetry_channel_that_is_never_read_drops_telemetry_and_no_usage_record() {
    let (sender, receiver) = telemetry::channel(NonZeroUsize::MIN);
    let voiced = voice_session(Some(Arc::new(sender.clone())));
    let records = records(&voiced.received);
    one_per_model_call(&voiced, &records);
    assert_eq!(voiced.usage.session, total_of(records.iter().copied()));
    assert!(
        sender.dropped() > 0,
        "the channel of capacity 1, never read, dropped telemetry: accepted {}, dropped {}",
        sender.accepted(),
        sender.dropped()
    );
    drop(receiver);
}

/// Everything `egress` delivers until the node's end of it is gone.
fn drain(mut egress: Box<dyn InputEndpoint<CascadePayload>>) -> Vec<EventEnvelope<CascadePayload>> {
    let mut received = Vec::new();
    while let Some(envelope) = block_on(egress.recv()) {
        received.push(envelope);
    }
    received
}

/// The usage records the recording lists, by the node that made each.
fn recorded_usage(recording: &Recording<CascadePayload>) -> Vec<UsageRecorded> {
    recording
        .usage()
        .into_iter()
        .map(|envelope| match &envelope.payload {
            CascadePayload::Usage(recorded) => {
                assert_eq!(envelope.kind, UsageRecorded::KIND);
                recorded.clone()
            }
            other => panic!("a usage envelope carries a usage record: {other:?}"),
        })
        .collect()
}

#[test]
fn the_recording_lists_the_usage_records_and_a_replay_reporting_other_counts_does_not_diverge() {
    let voiced = voice_session(None);
    let ids = voiced.cascade.ids;

    // The recording lists the three usage envelopes, the records the session delivered.
    let listed = recorded_usage(&voiced.recording);
    let mut listed_ids: Vec<_> = listed.iter().map(|r| r.usage_record_id).collect();
    let mut delivered_ids: Vec<_> = records(&voiced.received)
        .iter()
        .map(|r| r.usage_record_id)
        .collect();
    listed_ids.sort_unstable();
    delivered_ids.sort_unstable();
    assert_eq!(listed.len(), 3, "{listed:#?}");
    assert_eq!(listed_ids, delivered_ids);
    let recorded_llm = listed
        .iter()
        .find(|recorded| recorded.node_id == Some(ids.llm))
        .expect("the LLM node's record is listed");

    // A replay whose LLM reports other counts.
    let reported = Usage {
        input_tokens: Some(11),
        output_tokens: Some(5),
        ..Usage::default()
    };
    let mocks = Mocks {
        generator: voiced.mocks.generator.clone().reporting([Some(reported)]),
        ..voiced.mocks.clone()
    };
    let recorder = Recorder::new(POLICY);
    let run_id = RunId::generate();
    let mut implementations = voiced
        .cascade
        .nodes(run_id, mocks, TurnStore::default(), RunLog::new())
        .implementations;
    let mut controller = RunController::create_as(
        run_id,
        &voiced.cascade.planned,
        &voiced.declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            let implementation = implementations
                .remove(&node.node_id())
                .expect("every node of the cascade has an implementation");
            recorder.wrap(node, implementation)
        },
    )
    .expect("the cascade is planned");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    let mut ends = controller.exposed();
    block_on(controller.activate()).expect("every node starts");
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    let replay = Replay::new(&voiced.recording, &recorder);
    let fed = replay.feed(run_id, &mut ends, |_| {}, PATIENCE);
    assert_eq!(
        (fed.inputs, fed.refused, fed.missing, fed.gates_timed_out),
        (voiced.recording.inputs().count(), 0, 0, 0),
        "every recorded input, the frames and the session's end of input, was fed and taken"
    );
    for name in EGRESSES {
        drain(ends.egress(name).expect("the cascade exposes the egress"));
    }
    drop(ends);
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    result.expect("the drive loop ends the run");
    assert_eq!(controller.run().state(), RunState::Completed);

    let report = replay.report();
    assert!(
        !report.recorded.is_empty(),
        "the comparison is not vacuous: {report:#?}"
    );
    assert_eq!(report.divergence, None, "{report:#?}");
    let replayed = recorded_usage(&recorder.recording());
    assert_eq!(replayed.len(), 3, "{replayed:#?}");
    let replayed_llm = replayed
        .iter()
        .find(|recorded| recorded.node_id == Some(ids.llm))
        .expect("the replayed LLM node's record is listed");
    let tokens = |recorded: &UsageRecorded| {
        recorded
            .quantities
            .iter()
            .find(|quantity| quantity.kind() == QuantityKind::InputTextTokens)
            .map(|quantity| quantity.value())
    };
    assert_eq!(tokens(recorded_llm), Some(None), "recorded: unknown");
    assert_eq!(
        tokens(replayed_llm),
        Some(Some(11)),
        "replayed: other counts"
    );
}

/// The text pipeline's one node on the cascade's payload, given its usage port `usage`: the mock
/// model over `generator`.
fn text_nodes(
    generator: &MockGenerator,
    usage: inference_pipeline::PortId,
) -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<CascadePayload>> + use<> {
    let generator = generator.clone();
    move |run_id, _| {
        Box::new(
            ModelNode::<CascadePayload>::carrying(run_id, generator.clone(), reply, RunLog::new())
                .with_usage(usage),
        )
    }
}

#[test]
fn a_session_switching_from_text_to_the_cascade_keeps_one_total_per_run_and_one_over_both() {
    let text_generator = generator(Vec::<String>::new());
    let (text_declaration, text_planned, usage_port) = planned_with_usage(&text_generator);
    let mocks = mocks();
    let mut voice_declaration = Declaration::new();
    let cascade = declare(&mut voice_declaration, &mocks).expect("the cascade plans");
    let sessions = LiveSessions::new();
    let mut application = Application::new();
    let mut received = Vec::new();

    let (mut session, driver) = block_on(sessions.open(
        &text_declaration,
        &text_planned,
        InteractionMode::TurnBased,
        text_nodes(&text_generator, usage_port),
    ))
    .expect("a session opens over the text pipeline");
    let text_run = session.run_id();
    let text_drive = thread::spawn(move || block_on(driver.drive()));
    block_on(session.send(application.text("hello"))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    while !received
        .iter()
        .any(|event| matches!(event, SessionEvent::TextSegmentCommitted(_)))
    {
        received.push(block_on(session.receive()).expect("the session delivers the reply"));
    }

    let driver = block_on(session.switch(
        &voice_declaration,
        &cascade.planned,
        InteractionMode::TurnBased,
        voice_nodes(&cascade, &mocks),
    ))
    .expect("a Ready session switches to the cascade");
    let voice_run = session.run_id();
    let text_ended = text_drive
        .join()
        .expect("no panic")
        .expect("the text run ends");
    assert_eq!(text_ended.state(), RunState::Completed);
    let voice_drive = thread::spawn(move || block_on(driver.drive()));
    speak_and_close(&mut session, &mut application, &mut received);
    let voice_ended = voice_drive
        .join()
        .expect("no panic")
        .expect("the voice run ends");
    assert_eq!(voice_ended.state(), RunState::Completed);

    let records = records(&received);
    let of_run = |run_id: RunId| {
        records
            .iter()
            .copied()
            .filter(move |recorded| recorded.run_id == Some(run_id))
    };
    assert_eq!(of_run(text_run).count(), 1, "one LLM turn: {records:#?}");
    assert_eq!(of_run(voice_run).count(), 3, "STT, LLM, TTS: {records:#?}");
    assert_eq!(records.len(), 4);

    let usage = session.usage();
    assert_eq!(
        usage
            .runs
            .iter()
            .map(|(run_id, _)| *run_id)
            .collect::<Vec<_>>(),
        [text_run, voice_run],
        "one total per run the session held, in the order it held them"
    );
    assert_eq!(usage.run(text_run), Some(&total_of(of_run(text_run))));
    assert_eq!(usage.run(voice_run), Some(&total_of(of_run(voice_run))));
    assert_eq!(
        usage.session,
        total_of(records.iter().copied()),
        "the session total spans both runs"
    );
    assert_eq!(usage.session.records(), 4);
    assert_eq!(
        usage
            .session
            .kind(QuantityKind::InputTextTokens)
            .map(|total| total.unknown),
        Some(2),
        "both LLM turns' tokens unknown, kept apart from zero"
    );
}

/// The identities of the inputs `recorder` recorded, in merge order.
fn inputs(recorder: &Recorder<CascadePayload>) -> Vec<EventId> {
    recorder
        .recording()
        .inputs()
        .filter_map(|recorded| recorded.envelope().map(|envelope| envelope.event_id))
        .collect()
}

#[test]
fn a_second_tap_of_a_runs_ingresses_is_refused_so_every_input_is_recorded_once() {
    let first_generator = generator(Vec::<String>::new());
    let (first_declaration, first_planned, first_usage) = planned_with_usage(&first_generator);
    let second_generator = generator(Vec::<String>::new());
    let (second_declaration, second_planned, second_usage) = planned_with_usage(&second_generator);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &first_declaration,
        &first_planned,
        InteractionMode::TurnBased,
        text_nodes(&first_generator, first_usage),
    ))
    .expect("a session opens over the text pipeline");
    let first_run = session.run_id();
    let first_drive = thread::spawn(move || block_on(driver.drive()));

    let recorder = Recorder::new(POLICY);
    session
        .record_inputs(&recorder)
        .expect("the first tap of the run's ingresses");
    assert_eq!(
        session.record_inputs(&recorder),
        Err(AlreadyRecorded { run_id: first_run }),
        "a second tap by the same recorder is refused"
    );
    assert_eq!(
        session.record_inputs(&Recorder::new(POLICY)),
        Err(AlreadyRecorded { run_id: first_run }),
        "and by another"
    );

    let mut application = Application::new();
    let mut received = Vec::new();
    block_on(session.send(application.text("hello"))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    while !received
        .iter()
        .any(|event| matches!(event, SessionEvent::TextSegmentCommitted(_)))
    {
        received.push(block_on(session.receive()).expect("the session delivers the reply"));
    }

    // A switch gives the session a run whose ingresses are not tapped yet.
    let driver = block_on(session.switch(
        &second_declaration,
        &second_planned,
        InteractionMode::TurnBased,
        text_nodes(&second_generator, second_usage),
    ))
    .expect("a Ready session switches");
    let second_run = session.run_id();
    first_drive
        .join()
        .expect("no panic")
        .expect("the first run ends");
    let second_drive = thread::spawn(move || block_on(driver.drive()));
    let second_recorder = Recorder::new(POLICY);
    session
        .record_inputs(&second_recorder)
        .expect("the new run's ingresses are tapped once");
    assert_eq!(
        session.record_inputs(&second_recorder),
        Err(AlreadyRecorded { run_id: second_run })
    );
    block_on(session.close()).expect("drains");
    while block_on(session.receive()).is_some() {}
    second_drive
        .join()
        .expect("no panic")
        .expect("the second run ends");

    let first = inputs(&recorder);
    assert_eq!(
        first.len(),
        3,
        "the text, the commit and the end of input the switch sent, each once: {first:?}"
    );
    let mut unique = first.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), first.len(), "no input is recorded twice");
    assert_eq!(
        inputs(&second_recorder).len(),
        1,
        "the end of input the close sent, once"
    );
}
