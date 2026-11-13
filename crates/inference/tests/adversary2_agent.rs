//! Adversarial pass 2 of story:sdk-agent: a dropped device run, an application cut of a cascade
//! reply on the devices, and the device feeder's `Error::Closed` path.

#![cfg(feature = "testing")]

use std::collections::BTreeMap;
use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use inference::audio::AudioFrame;
use inference::audio_device::convert::decode;
use inference::audio_device::fake::{FakeDevice, FakeHost, SinkHold};
use inference::core::model::ModelId;
use inference::facade::session::SessionState;
use inference::{Agent, Binding, Event, Role, Stack};
use inference_testkit::duplex::mock::{MockSpeechModel, Reply, ScriptedTurn, pcm, utterance};
use inference_testkit::llm::{MockConfiguration, MockGenerator};
use inference_testkit::mark::{self, MockId};
use inference_testkit::stt::synthetic_audio::{FIXTURE, synthesize};
use tokio::task::JoinHandle;

const LIMIT: Duration = Duration::from_secs(5);

fn turn(text: &str) -> ScriptedTurn {
    ScriptedTurn {
        heard: "hello".to_owned(),
        reply: Reply::Say(text.to_owned()),
    }
}

fn speech(model: MockSpeechModel) -> Stack {
    Stack::new().bind(Binding::speech(model))
}

fn cascade_without(role: Role) -> Stack {
    [Role::Stt, Role::Llm, Role::Tts]
        .into_iter()
        .filter(|bound| *bound != role)
        .fold(Stack::new(), |stack, bound| {
            stack.bind(Binding::named(bound, "mock", BTreeMap::new()))
        })
}

fn cascade() -> Stack {
    cascade_without(Role::Speech)
}

fn name(event: &Event) -> &'static str {
    match event {
        Event::Started { .. } => "Started",
        Event::Heard { final_: true, .. } => "Heard",
        Event::Heard { .. } => "Hypothesis",
        Event::Transcript { .. } => "Transcript",
        Event::Said { .. } => "Said",
        Event::Audio(_) => "Audio",
        Event::ToolCalled { .. } => "ToolCalled",
        Event::Usage(_) => "Usage",
        Event::Interrupted { .. } => "Interrupted",
        Event::Closed { .. } => "Closed",
        Event::Error { .. } => "Error",
        Event::Lagged(_) => "Lagged",
        Event::ConfirmationRequested { .. } => "ConfirmationRequested",
        _ => "other",
    }
}

fn names(events: &[Event]) -> Vec<&'static str> {
    events.iter().map(name).collect()
}

fn count(events: &[Event], event: &str) -> usize {
    names(events).iter().filter(|seen| **seen == event).count()
}

/// Reads `events` until `done` answers what was read, or the limit passes; what was read.
async fn read_while(
    events: &mut inference::Events,
    limit: Duration,
    done: impl Fn(&[Event]) -> bool,
) -> Vec<Event> {
    let mut seen = Vec::new();
    let _ = tokio::time::timeout(limit, async {
        while let Some(event) = events.next().await {
            seen.push(event.expect("no terminal error"));
            if done(&seen) {
                return;
            }
        }
    })
    .await;
    seen
}

async fn read_all(events: &mut inference::Events) -> Vec<Event> {
    read_while(events, LIMIT, |_| false).await
}

fn fake_host(input: u32) -> FakeHost {
    let rate = |hertz| NonZeroU32::new(hertz).expect("a rate");
    FakeHost::new(vec![
        FakeDevice::input("test microphone", rate(input), NonZeroU16::MIN, true),
        FakeDevice::output("test speaker", rate(24_000), NonZeroU16::MIN, true),
    ])
}

fn spawn_run(agent: &Arc<Agent>, host: &FakeHost) -> JoinHandle<inference::Result<()>> {
    let (agent, host) = (Arc::clone(agent), host.clone());
    tokio::spawn(async move { agent.run_on_devices_with(host).await })
}

async fn opened(host: &FakeHost) {
    let started = Instant::now();
    while !host.input_open() {
        assert!(started.elapsed() < LIMIT, "the run opens the microphone");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn feed(host: &FakeHost, frames: &[AudioFrame]) {
    opened(host).await;
    for frame in frames {
        assert!(host.feed(&decode(frame).expect("a well-formed frame")));
    }
}

/// Feeds `frames` to the host's microphone over and over until the run closes it: its microphone
/// thread does once the feeder stopped and the next captured frame finds no taker, or once the
/// session ended. The close is the signal, not how many frames got in before it: the thread may
/// capture, and the feeder stop, between any two of them, and it may already have.
async fn fed_until_closed(host: &FakeHost, frames: &[AudioFrame]) {
    let started = Instant::now();
    for frame in frames.iter().cycle() {
        if !host.feed(&decode(frame).expect("a well-formed frame")) {
            return;
        }
        assert!(started.elapsed() < LIMIT, "the run closes the microphone");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn waiting_for(hold: &SinkHold, mock: MockId, reply: u32) {
    let started = Instant::now();
    while !hold
        .waiting()
        .iter()
        .filter_map(mark::decode)
        .any(|marked| marked.mock == mock && marked.reply == reply)
    {
        assert!(started.elapsed() < LIMIT, "reply {reply} reaches the sink");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn returns_ok(run: &mut JoinHandle<inference::Result<()>>) {
    let ran = tokio::time::timeout(LIMIT, run)
        .await
        .expect("the run returns within 5 s")
        .expect("the run's task ends");
    assert!(ran.is_ok(), "{ran:?}");
}

fn row_state(agent: &Agent) -> SessionState {
    inference::sessions()
        .sessions_view()
        .into_iter()
        .find(|row| row.session_id == agent.session().session_id())
        .expect("the session's row")
        .state
}

/// `run_on_devices_with` documents that while it runs the agent's audio goes to the speaker
/// only; once its future is dropped (a `select!`, a `timeout`) it no longer runs, so the next
/// reply is `Event::Audio` on the stream (R117, R127: the sink is attached for the run only).
#[tokio::test(flavor = "multi_thread")]
async fn after_a_dropped_device_run_the_reply_audio_is_on_the_stream() {
    let model = MockSpeechModel::native(vec![turn("one"), turn("two")]);
    let agent = Arc::new(
        Agent::voice_with(speech(model))
            .start()
            .await
            .expect("starts"),
    );
    let mut events = agent.events();
    let host = fake_host(24_000);
    let run = spawn_run(&agent, &host);
    opened(&host).await;
    run.abort();
    assert!(run.await.is_err(), "the run's future was dropped");
    for frame in utterance(pcm(24_000), 0).frames() {
        agent.send_audio(frame).await.expect("sent");
    }
    let seen = read_while(&mut events, LIMIT, |seen| count(seen, "Usage") == 2).await;
    assert_eq!(
        count(&seen, "Audio"),
        2,
        "no device run is running, yet the reply's audio did not reach the stream: {:?}; \
         the device took {} frames",
        names(&seen),
        host.played_frames().len()
    );
    agent.close().await.expect("closes");
}

/// A dropped device run lets go of the host's microphone and speaker, so the application can run
/// the agent on them again: `FakeHost` refuses a second stream of a direction as busy.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_device_run_releases_the_microphone_and_the_speaker() {
    let model = MockSpeechModel::native(vec![turn("one")]);
    let agent = Arc::new(
        Agent::voice_with(speech(model))
            .start()
            .await
            .expect("starts"),
    );
    let host = fake_host(24_000);
    let run = spawn_run(&agent, &host);
    opened(&host).await;
    run.abort();
    assert!(run.await.is_err(), "the run's future was dropped");
    let started = Instant::now();
    while (host.input_open() || host.output_open()) && started.elapsed() < Duration::from_secs(2) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !host.input_open() && !host.output_open(),
        "2 s after the dropped run: microphone open {}, speaker open {}",
        host.input_open(),
        host.output_open(),
    );
    agent.close().await.expect("closes");
}

/// `agent.interrupt()` while a cascade reply is held at the device sink: the cut drops the reply's
/// frames at the device (R135); the cascade's speaker then still drains, and the close yields no
/// error.
#[tokio::test(flavor = "multi_thread")]
async fn an_application_cut_of_a_cascade_reply_on_the_devices_closes_without_an_error() {
    let generator = MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        ["one two three four"],
    );
    let agent = Arc::new(
        Agent::from_recipe(inference::Recipe::voice_cascade())
            .stack(cascade_without(Role::Llm))
            .llm(Binding::llm(generator))
            .start()
            .await
            .expect("starts"),
    );
    let mut events = agent.events();
    let host = fake_host(16_000);
    let hold = host.hold_sink();
    let mut run = spawn_run(&agent, &host);
    feed(&host, &synthesize(&FIXTURE[..2])).await;
    waiting_for(&hold, MockId::Synthesiser, 0).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    agent.interrupt().await.expect("interrupts");
    let mut seen = read_while(&mut events, LIMIT, |seen| count(seen, "Interrupted") == 1).await;
    assert_eq!(count(&seen, "Interrupted"), 1, "{:?}", names(&seen));
    drop(hold);
    let closed = tokio::time::timeout(LIMIT, agent.close()).await;
    assert!(
        matches!(closed, Ok(Ok(()))),
        "the close returns within 5 s: {closed:?}"
    );
    returns_ok(&mut run).await;
    seen.extend(read_all(&mut events).await);
    assert_eq!(
        count(&seen, "Error"),
        0,
        "the cut reply's speaker drained without a failure: {seen:#?}"
    );
}

/// R146: once a close took the session out of Ready, the device feeder's `send_audio` answers
/// `Error::Closed` and the feeder stops without an error, the run going on to the drain. A held
/// cascade sink keeps the session draining, so frames fed then do reach the feeder's refused send;
/// they are fed until the run closes the microphone, however many got in before the feeder
/// stopped.
#[tokio::test(flavor = "multi_thread")]
async fn frames_fed_while_a_close_drains_do_not_fail_the_device_run() {
    let agent = Arc::new(
        Agent::from_recipe(inference::Recipe::voice_cascade())
            .stack(cascade())
            .start()
            .await
            .expect("starts"),
    );
    let host = fake_host(16_000);
    let hold = host.hold_sink();
    let mut run = spawn_run(&agent, &host);
    feed(&host, &synthesize(&FIXTURE[..2])).await;
    waiting_for(&hold, MockId::Synthesiser, 0).await;
    let closer = Arc::clone(&agent);
    let closing = tokio::spawn(async move { closer.close().await });
    let started = Instant::now();
    while row_state(&agent) == SessionState::Ready {
        assert!(started.elapsed() < LIMIT, "the close leaves Ready");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    fed_until_closed(&host, &synthesize(&FIXTURE[1..2])).await;
    let early = tokio::time::timeout(Duration::from_secs(1), &mut run).await;
    assert!(early.is_err(), "the run waits for the held sink: {early:?}");
    drop(hold);
    returns_ok(&mut run).await;
    let closed = tokio::time::timeout(LIMIT, closing)
        .await
        .expect("the close returns within 5 s")
        .expect("the close's task ends");
    assert!(closed.is_ok(), "{closed:?}");
}
