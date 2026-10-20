//! The `offline_strict` profile enforced (`story:cascade-offline-profile`; A-04 of
//! `docs/design/inference.md` at 1140, section 23.2 at 1120 and the profile table at 1046).
//!
//! Planning refuses an adapter whose locality the pipeline's profile does not admit
//! (`inference.graph.PlanPipeline/outcome/locality-denied`). At run time the egress watch polls the
//! sockets of this process while the testkit's voice cascade runs: the cascade completes under
//! it, a node connected to 127.0.0.1 leaves the run alone, and a node that connects a UDP socket to
//! 192.0.2.1 (TEST-NET-1, RFC 5737; connecting a UDP socket sends nothing) fails the run with a
//! reason naming the address. A UDP socket connected to nothing fails the run, naming its local
//! address, when it is bound to the unspecified address, and leaves it alone when it is bound to
//! 127.0.0.1.
//!
//! The watch reads every socket of the process, so the cases that run the cascade hold one lock
//! and never overlap: a socket another case opened would trip this case's watch.

#![cfg(target_os = "linux")]

use std::any::Any;
use std::collections::HashMap;
use std::future::{self, Future};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::num::{NonZeroU16, NonZeroU32, NonZeroUsize};
use std::pin::pin;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, LocalityDenied, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeInstanceId, NodeRole, OverloadPolicy, PipelineSpec,
    PipelineState, PlanPipelineError, PortTraits, modality,
};
use inference_runtime::RunController;
use inference_runtime::egress::{
    EgressWatch, connected_sockets, is_egress, unconnected_datagram_sockets,
};
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_testkit::cascade::{
    AUDIO_INGRESS, CascadePayload, Mocks, SPEECH_EGRESS, TEXT_EGRESS, TRANSCRIPT_EGRESS, TurnStore,
    VoiceCascade, declare,
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
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};

/// What the user asks: the recogniser's scripted span.
const QUESTION: &str = "how warm is it outside";

/// The TEST-NET-1 address the offending node connects to.
const TEST_NET_1: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);

/// How long a case waits for the watch or the run before it gives up.
const PATIENCE: Duration = Duration::from_secs(10);

static WATCHED: Mutex<()> = Mutex::new(());

/// Holds the lock of the cases whose sockets the watch reads.
fn watched() -> MutexGuard<'static, ()> {
    WATCHED.lock().unwrap_or_else(PoisonError::into_inner)
}

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

/// The declared profile and the adapters a plan reads.
struct Known {
    profile: DeploymentProfile,
    adapters: Vec<Adapter>,
}

impl Catalog for Known {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.adapter_id() == adapter_id)
    }
}

#[test]
fn planning_refuses_an_external_provider_under_offline_strict_and_the_pipeline_stays_validated() {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![Locality::SameProcess, Locality::LocalWorker],
    })
    .expect("a row of the table");
    let (hosted, _) = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "hosted".to_owned(),
        contract_revision: "llm/1".to_owned(),
        execution_form: ExecutionForm::HostedService,
        locality: Locality::ExternalProvider,
        capabilities: vec![Capability::OutputStreaming],
        max_concurrency: 1,
    })
    .expect("a positive concurrency");
    let known = Known {
        profile,
        adapters: vec![hosted],
    };
    let mut declaration = Declaration::new();
    let spec = PipelineSpec {
        name: "remote agent".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let (pipeline_id, agent) = declaration
        .build(&known, known.profile.profile_id(), spec, |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let agent = pipeline.node("agent", NodeRole::Model, vec![Capability::OutputStreaming]);
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let asked = pipeline.output::<modality::Text>(&source, "out", PortTraits::default());
            let question = pipeline.input::<modality::Text>(&agent, "in", PortTraits::default());
            let answer = pipeline.output::<modality::Text>(&agent, "out", PortTraits::default());
            let answered = pipeline.input::<modality::Text>(&sink, "in", PortTraits::default());
            pipeline.connect(&asked, &question, NonZeroUsize::MIN, OverloadPolicy::Pause);
            pipeline.connect(&answer, &answered, NonZeroUsize::MIN, OverloadPolicy::Pause);
            pipeline
                .bind(&agent, known.adapters[0].adapter_id())
                .expect("the adapter is declared");
            agent.id()
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("the graph is valid");

    let refused = declaration.plan(validated, &known);

    assert_eq!(
        refused,
        Err(PlanPipelineError::LocalityDenied(LocalityDenied {
            node_id: agent,
            adapter_id: known.adapters[0].adapter_id(),
            locality: Locality::ExternalProvider,
            profile_id: known.profile.profile_id(),
        })),
        "inference.graph.PlanPipeline/outcome/locality-denied"
    );
    let pipeline = declaration.pipeline(pipeline_id).expect("declared");
    assert_eq!(pipeline.state(), PipelineState::Validated, "nothing moved");
    assert_eq!(pipeline.effective_capabilities(), None);
}

#[test]
fn the_watch_runs_for_denied_egress_only() {
    let _watched = watched();
    let mocks = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade validates and plans");
    let started = Started::new(&cascade, &declaration, mocks, None);
    let requests = started.controller.requests();
    for egress in Egress::ALL {
        let watch = EgressWatch::start(egress, requests.clone()).expect("the watch thread starts");
        assert_eq!(
            watch.is_some(),
            egress == Egress::Denied,
            "{egress:?} is watched only when denied"
        );
    }
}

#[test]
fn loopback_and_unspecified_addresses_are_not_egress() {
    for address in [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V4(Ipv4Addr::new(127, 3, 2, 1)),
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped()),
        IpAddr::V6(Ipv4Addr::UNSPECIFIED.to_ipv6_mapped()),
    ] {
        assert!(!is_egress(address), "{address} is not egress");
    }
    for address in [
        IpAddr::V4(TEST_NET_1),
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        IpAddr::V6(TEST_NET_1.to_ipv6_mapped()),
        IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
    ] {
        assert!(is_egress(address), "{address} is egress");
    }
}

#[test]
fn the_voice_cascade_completes_under_the_egress_watch() {
    let _watched = watched();
    let mocks = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade validates and plans");
    let started = Started::new(&cascade, &declaration, mocks, None);
    let watch = EgressWatch::start(Egress::Denied, started.controller.requests())
        .expect("the watch thread starts")
        .expect("a denied egress is watched");

    let finished = started.converse(&watch);

    finished.assert_completed(&cascade);
    assert!(
        watch.polls() >= 2,
        "the watch polled while the run was Active"
    );
    assert_eq!(watch.tripped(), None);
}

#[test]
fn a_node_connected_to_loopback_leaves_the_run_unaffected() {
    let _watched = watched();
    let mocks = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade validates and plans");
    let started = Started::new(&cascade, &declaration, mocks, Some(loopback));
    let watch = EgressWatch::start(Egress::Denied, started.controller.requests())
        .expect("the watch thread starts")
        .expect("a denied egress is watched");
    let opened = started.opened();
    assert!(!opened.is_empty(), "the node connected its sockets");
    let seen = connected_sockets().expect("the sockets of this process are readable");
    for address in &opened {
        assert!(
            seen.contains(address),
            "the watch's reading sees {address} among {seen:?}"
        );
    }

    let finished = started.converse(&watch);

    finished.assert_completed(&cascade);
    assert_eq!(watch.tripped(), None, "loopback is not egress");
}

#[test]
fn a_node_connecting_to_test_net_1_fails_the_run_naming_the_address() {
    let _watched = watched();
    let remote = SocketAddr::from((TEST_NET_1, 9));
    failed_by_the_watch(test_net_1, |opened| {
        assert_eq!(opened, [remote], "the node connected its socket");
        remote
    });
}

#[test]
fn a_node_holding_an_unconnected_udp_socket_not_bound_to_loopback_fails_the_run_naming_it() {
    let _watched = watched();
    failed_by_the_watch(unconnected, |opened| {
        let [local] = opened[..] else {
            panic!("the node bound one socket: {opened:?}");
        };
        assert!(
            local.ip().is_unspecified(),
            "{local} is bound to any address"
        );
        assert!(
            unconnected_datagram_sockets()
                .expect("the sockets of this process are readable")
                .contains(&local),
            "the watch's reading sees {local}"
        );
        local
    });
}

/// Runs the cascade with its `split` node opening sockets by `open` under the egress watch, and
/// checks the watch fails the run naming the address `named` answers from what the node opened,
/// winding every node down.
fn failed_by_the_watch(open: Open, named: impl FnOnce(Vec<SocketAddr>) -> SocketAddr) {
    let mocks = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade validates and plans");
    let started = Started::new(&cascade, &declaration, mocks, Some(open));
    let address = named(started.opened());
    let requests = started.controller.requests();
    let watch = EgressWatch::start(Egress::Denied, requests.clone())
        .expect("the watch thread starts")
        .expect("a denied egress is watched");

    let Started {
        mut controller,
        ends,
        ..
    } = started;
    let (sent, ended) = mpsc::channel();
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        let _ = sent.send(());
        (controller, result)
    });
    let in_time = ended.recv_timeout(PATIENCE).is_ok();
    if !in_time {
        requests.cancel();
    }
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    drop(ends);

    assert!(in_time, "the watch ended the run within {PATIENCE:?}");
    result.expect("the loop carried out the requested failure");
    assert_eq!(controller.run().state(), RunState::Failed);
    let failure = controller
        .run()
        .failure()
        .expect("a Failed run has a reason");
    assert_eq!(failure.node_id, None, "a socket names no node");
    assert!(
        failure.detail.contains(&address.to_string()),
        "the reason names {address}: {failure:?}"
    );
    assert_eq!(watch.tripped(), Some(address));
    for node_id in cascade.ids.nodes() {
        assert_eq!(
            controller
                .instance(node_id)
                .map(|instance| instance.state()),
            Some(NodeInstanceState::Stopped),
            "the instance of node {node_id} was wound down"
        );
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

/// What a connecting node holds open until it stops, and the addresses the case checks: the
/// remote address of each connection, or the local address of a socket connected to nothing.
struct Opened {
    held: Vec<Box<dyn Any + Send>>,
    remotes: Vec<SocketAddr>,
}

/// Opens the sockets a node holds while it runs.
type Open = fn() -> io::Result<Opened>;

/// A TCP connection to a listener on 127.0.0.1, a UDP socket connected to 127.0.0.1:9, and a UDP
/// socket bound to 127.0.0.1 and connected to nothing, which has sent a datagram to the other.
fn loopback() -> io::Result<Opened> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let client = TcpStream::connect(listener.local_addr()?)?;
    let (server, _) = listener.accept()?;
    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let target = SocketAddr::from((Ipv4Addr::LOCALHOST, 9));
    udp.connect(target)?;
    let unconnected = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    unconnected.send_to(b"x", udp.local_addr()?)?;
    Ok(Opened {
        remotes: vec![client.peer_addr()?, server.peer_addr()?, target],
        held: vec![
            Box::new(listener),
            Box::new(client),
            Box::new(server),
            Box::new(udp),
            Box::new(unconnected),
        ],
    })
}

/// A UDP socket bound to the unspecified address and connected to nothing, which has sent a
/// datagram to a loopback socket of its own: it could have sent one anywhere, unseen. Its local
/// address is the one the node names.
fn unconnected() -> io::Result<Opened> {
    let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let sender = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    sender.send_to(b"x", receiver.local_addr()?)?;
    Ok(Opened {
        remotes: vec![sender.local_addr()?],
        held: vec![Box::new(receiver), Box::new(sender)],
    })
}

/// A UDP socket connected to 192.0.2.1:9; connecting it sends nothing.
fn test_net_1() -> io::Result<Opened> {
    let udp = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    let target = SocketAddr::from((TEST_NET_1, 9));
    udp.connect(target)?;
    Ok(Opened {
        remotes: vec![target],
        held: vec![Box::new(udp)],
    })
}

/// A node of the cascade that also opens sockets when it starts and holds them until it stops.
struct Connecting {
    inner: Box<dyn NodeImplementation<CascadePayload>>,
    open: Open,
    held: Vec<Box<dyn Any + Send>>,
    remotes: Arc<Mutex<Vec<SocketAddr>>>,
}

impl NodeImplementation<CascadePayload> for Connecting {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.inner.describe(node)
    }

    fn described(&mut self, instance_id: NodeInstanceId) {
        self.inner.described(instance_id);
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.prepare()
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<CascadePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        match (self.open)() {
            Ok(opened) => {
                self.held = opened.held;
                *self.remotes.lock().unwrap_or_else(PoisonError::into_inner) = opened.remotes;
                self.inner.start(endpoints)
            }
            Err(error) => Box::pin(future::ready(Err(NodeError::new(error.to_string())))),
        }
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.drain()
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.inner.cancel()
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.held.clear();
        self.inner.stop()
    }
}

/// A run of the cascade, Active, its `split` node wrapped in a [`Connecting`] node when an
/// [`Open`] is given.
struct Started {
    run_id: RunId,
    controller: RunController<CascadePayload>,
    ends: inference_runtime::ExposedEnds<CascadePayload>,
    remotes: Arc<Mutex<Vec<SocketAddr>>>,
}

impl Started {
    fn new(
        cascade: &VoiceCascade,
        declaration: &Declaration,
        mocks: Mocks,
        open: Option<Open>,
    ) -> Self {
        let run_id = RunId::generate();
        let mut run = cascade.nodes(run_id, mocks, TurnStore::default(), RunLog::new());
        let remotes = Arc::new(Mutex::new(Vec::new()));
        if let Some(open) = open {
            let inner = run
                .implementations
                .remove(&cascade.ids.split)
                .expect("the fixture implements split");
            run.implementations.insert(
                cascade.ids.split,
                Box::new(Connecting {
                    inner,
                    open,
                    held: Vec::new(),
                    remotes: Arc::clone(&remotes),
                }),
            );
        }
        let mut controller = RunController::create_as(
            run_id,
            &cascade.planned,
            declaration,
            HashMap::<GenerationId, i64>::new(),
            |node: &Node| {
                run.implementations
                    .remove(&node.node_id())
                    .expect("the fixture has an implementation for every node")
            },
        )
        .expect("the cascade is planned and no edge cancels or supersedes");
        controller.validate().expect("a Created run validates");
        block_on(controller.prepare()).expect("every node prepares");
        let ends = controller.exposed();
        block_on(controller.activate()).expect("every node starts");
        assert_eq!(controller.run().state(), RunState::Active);
        Self {
            run_id,
            controller,
            ends,
            remotes,
        }
    }

    /// The remote addresses the wrapped node connected to.
    fn opened(&self) -> Vec<SocketAddr> {
        self.remotes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Drives the run while the application sends the utterance and the end of input, once
    /// `watch` has polled twice, and reads every egress to its end.
    fn converse(self, watch: &EgressWatch) -> Finished {
        let Self {
            run_id,
            mut controller,
            mut ends,
            ..
        } = self;
        let ingress: Box<dyn OutputEndpoint<CascadePayload>> =
            ends.ingress(AUDIO_INGRESS).expect("the audio ingress");
        let transcript = ends
            .egress(TRANSCRIPT_EGRESS)
            .expect("the transcript egress");
        let text = ends.egress(TEXT_EGRESS).expect("the assistant text egress");
        let speech = ends
            .egress(SPEECH_EGRESS)
            .expect("the synthesized audio egress");
        let drive = thread::spawn(move || {
            let result = block_on(controller.drive());
            (controller, result)
        });
        let deadline = Instant::now() + PATIENCE;
        while watch.polls() < 2 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }

        let mut sequence = 0;
        let stream_id = StreamId::generate();
        let producer = NodeId::generate();
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
        for frame in utterance() {
            let timestamp = i64::try_from(frame.position.get()).expect("within i64");
            last = timestamp + i64::from(frame.sample_count);
            let sent = envelope(
                "inference.audio.AudioFrame",
                timestamp,
                CascadePayload::Audio(frame),
            );
            block_on(ingress.send(sent)).expect("the ingress takes every frame");
        }
        let stop = envelope(
            ControlKind::Stop.as_str(),
            last,
            CascadePayload::Control(ControlKind::Stop),
        );
        block_on(ingress.send(stop)).expect("the ingress takes the end of input");
        let _ = read(transcript);
        let text = read(text);
        let _ = read(speech);
        drop(ingress);
        drop(ends);
        let (controller, result) = drive.join().expect("the drive loop does not panic");
        result.expect("the drive loop ends the run");
        Finished { controller, text }
    }
}

/// A run the application conversed with, and what its assistant text egress delivered.
struct Finished {
    controller: RunController<CascadePayload>,
    text: Vec<EventEnvelope<CascadePayload>>,
}

impl Finished {
    fn assert_completed(&self, cascade: &VoiceCascade) {
        assert_eq!(
            self.controller.run().state(),
            RunState::Completed,
            "the run completed: {:?}",
            self.controller.run().failure()
        );
        for node_id in cascade.ids.nodes() {
            assert_eq!(
                self.controller
                    .instance(node_id)
                    .map(|instance| instance.state()),
                Some(NodeInstanceState::Stopped),
                "the instance of node {node_id}"
            );
        }
        let replies: Vec<&str> = self
            .text
            .iter()
            .filter_map(|envelope| match &envelope.payload {
                CascadePayload::Stream(StreamEvent::TextSegmentCommitted(committed)) => {
                    Some(committed.text.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(replies, [reply(QUESTION).as_str()], "the scripted reply");
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

/// Everything `egress` delivers until the node's end of it is gone.
fn read(mut egress: Box<dyn InputEndpoint<CascadePayload>>) -> Vec<EventEnvelope<CascadePayload>> {
    let mut received = Vec::new();
    while let Some(envelope) = block_on(egress.recv()) {
        received.push(envelope);
    }
    received
}
