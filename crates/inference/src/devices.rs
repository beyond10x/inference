//! The audio devices of a handle's run: microphone to session, session to speaker, over an
//! `AudioHost` (feature `audio-host`; `docs/design/sdk.md` §3.2).

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use inference_audio::{AudioFormat, AudioFrame};
use inference_audio_device::{
    AudioHost, CaptureConfig, DeviceError, DeviceSelector, Direction, Microphone, PlayoutConfig,
    PushError, Speaker,
};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use crate::agent::Playback;
use crate::{Agent, Error, Result};

/// What the playback wait allows beyond the audio queued when it starts.
const MARGIN: Duration = Duration::from_millis(250);

/// How long one push waits for room before the playout thread looks for a cut again.
const PATIENCE: Duration = Duration::from_millis(100);

/// How much captured audio the microphone holds before the feeder reads it.
const CAPTURE_BUFFER: Duration = Duration::from_secs(2);

/// How long the microphone thread waits for a frame before it looks for the run's end again.
const READ: Duration = Duration::from_millis(20);

/// What goes down the playout channel: a frame with the interruption count at its sending, or
/// the end of the session's audio.
enum Out {
    Frame {
        frame: AudioFrame,
        interruption: u64,
    },
    End,
}

/// The frames sent by the pump, those the playout thread handed out (pushed or discarded), those
/// it pushed, and the rate of the last frame pushed.
#[derive(Default)]
struct Counts {
    sent: AtomicU64,
    handed: AtomicU64,
    pushed: AtomicU64,
    rate: AtomicU32,
}

/// A device run's playback in the agent's sink slot: a channel to the playout thread, which
/// feeds the speaker.
struct Sink {
    sender: mpsc::Sender<Out>,
    speaker: Arc<Speaker>,
    interruptions: Arc<AtomicU64>,
    counts: Arc<Counts>,
    ended: AtomicBool,
}

impl Playback for Sink {
    fn play(&self, frame: AudioFrame) {
        let interruption = self.interruptions.load(Ordering::Acquire);
        self.counts.sent.fetch_add(1, Ordering::AcqRel);
        if self
            .sender
            .send(Out::Frame {
                frame,
                interruption,
            })
            .is_err()
        {
            self.counts.handed.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn pending(&self) -> bool {
        let handed = self.counts.handed.load(Ordering::Acquire);
        self.counts.sent.load(Ordering::Acquire) > handed || self.speaker.queued() > 0
    }

    fn cut(&self) {
        self.interruptions.fetch_add(1, Ordering::AcqRel);
        let _ = self.speaker.stop();
    }

    /// The frames pushed to the speaker: a frame a cut discarded was never handed out, as the
    /// cascade's playout counts it dropped.
    fn played(&self) -> u64 {
        self.counts.pushed.load(Ordering::Acquire)
    }

    fn end(&self) {
        if !self.ended.swap(true, Ordering::AcqRel) {
            let _ = self.sender.send(Out::End);
        }
    }
}

/// The playout thread: pushes each frame not cut, in order, and on `End` finishes the speaker
/// and fires `drained`.
fn playout(
    frames: &mpsc::Receiver<Out>,
    speaker: &Speaker,
    interruptions: &AtomicU64,
    counts: &Counts,
    drained: oneshot::Sender<()>,
) {
    while let Ok(out) = frames.recv() {
        match out {
            Out::Frame {
                frame,
                interruption,
            } => {
                if push(speaker, &frame, interruptions, interruption) {
                    counts
                        .rate
                        .store(frame.format.sample_rate.get(), Ordering::Release);
                    counts.pushed.fetch_add(1, Ordering::AcqRel);
                }
                counts.handed.fetch_add(1, Ordering::AcqRel);
            }
            Out::End => {
                speaker.finish();
                let _ = drained.send(());
                return;
            }
        }
    }
}

/// Pushes `frame`, sent at `interruption`, until it is queued; false when a cut came first or
/// the speaker refused it.
fn push(
    speaker: &Speaker,
    frame: &AudioFrame,
    interruptions: &AtomicU64,
    interruption: u64,
) -> bool {
    loop {
        if interruptions.load(Ordering::Acquire) > interruption {
            return false;
        }
        match speaker.push(frame, PATIENCE) {
            Ok(()) => return true,
            Err(PushError::Full) => {}
            Err(_) => return false,
        }
    }
}

/// The microphone thread: sends each captured frame to the feeder while the run listens.
fn capture(
    mut microphone: Microphone,
    frames: &tokio_mpsc::UnboundedSender<AudioFrame>,
    listening: &AtomicBool,
) {
    while listening.load(Ordering::Acquire) {
        if let Some(frame) = microphone.next_frame(READ)
            && frames.send(frame).is_err()
        {
            return;
        }
    }
}

/// The microphone's frames: the session's input format when it has one, else 16 kHz PCM16.
fn capture_config(format: Option<AudioFormat>) -> CaptureConfig {
    let mut config = CaptureConfig {
        buffer: CAPTURE_BUFFER,
        ..CaptureConfig::default()
    };
    if let Some(format) = format {
        config.sample_rate = format.sample_rate;
        config.encoding = format.encoding;
    }
    config
}

/// The device of `direction` that did not open, with the devices the host reports.
fn device(host: &impl AudioHost, direction: Direction, error: &DeviceError) -> Error {
    let selector = match direction {
        Direction::Input => "the default input",
        Direction::Output => "the default output",
    };
    Error::Device {
        selector: selector.to_owned(),
        devices: host
            .devices()
            .map(|devices| devices.into_iter().map(|device| device.name).collect())
            .unwrap_or_default(),
        message: error.to_string(),
    }
}

/// Waits, off the runtime, until the speaker played what it holds, at most its queued duration
/// plus [`MARGIN`].
async fn wait_played(speaker: Arc<Speaker>, counts: &Counts) {
    let rate = u64::from(counts.rate.load(Ordering::Acquire).max(1));
    let _ = tokio::task::spawn_blocking(move || {
        let queued = u64::try_from(speaker.queued()).unwrap_or(u64::MAX);
        let micros = queued.saturating_mul(1_000_000) / rate;
        speaker.wait_played(Duration::from_micros(micros) + MARGIN)
    })
    .await;
}

impl Agent {
    /// Runs the agent on `host`'s default microphone and speaker until the session ends or
    /// Ctrl-C. While it runs the agent's audio goes to the speaker only, never onto the event
    /// stream, and a barge-in or [`Agent::interrupt`] cuts what the speaker has not played. It
    /// returns once the last reply was handed to the speaker and played.
    ///
    /// # Errors
    ///
    /// [`Error::Device`] when a device does not open; those of [`Agent::send_audio`] but
    /// [`Error::Closed`], and those of [`Agent::close`].
    pub async fn run_on_devices_with(&self, host: impl AudioHost) -> Result<()> {
        let stop = async {
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
        };
        self.run_until(host, stop).await
    }

    /// [`Agent::run_on_devices_with`] on the default audio host, `CpalHost`.
    ///
    /// # Errors
    ///
    /// Those of [`Agent::run_on_devices_with`].
    #[cfg(feature = "audio-device")]
    pub async fn run_on_devices(&self) -> Result<()> {
        self.run_on_devices_with(inference_audio_device::CpalHost::new())
            .await
    }

    /// Runs the agent on `host` until the session ends or `stop` completes, when it closes the
    /// session; either way it returns once the playout thread handed out the last frame and the
    /// speaker played what it holds.
    pub(crate) async fn run_until(
        &self,
        host: impl AudioHost,
        stop: impl Future + Send,
    ) -> Result<()> {
        let speaker = Speaker::open(&host, &DeviceSelector::Default, PlayoutConfig::default())
            .map_err(|error| device(&host, Direction::Output, &error))?;
        let speaker = Arc::new(speaker);
        let (sender, receiver) = mpsc::channel();
        let (drain, drained) = oneshot::channel();
        let interruptions = Arc::new(AtomicU64::new(0));
        let counts = Arc::new(Counts::default());
        {
            let (speaker, interruptions, counts) = (
                Arc::clone(&speaker),
                Arc::clone(&interruptions),
                Arc::clone(&counts),
            );
            thread::spawn(move || playout(&receiver, &speaker, &interruptions, &counts, drain));
        }
        let sink = Arc::new(Sink {
            sender,
            speaker: Arc::clone(&speaker),
            interruptions,
            counts: Arc::clone(&counts),
            ended: AtomicBool::new(false),
        });
        *self
            .sink_slot()
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&sink) as Arc<dyn Playback>);
        let outcome = self.feed_until(&host, stop).await;
        if outcome.is_ok() {
            sink.end();
            let _ = drained.await;
            wait_played(speaker, &counts).await;
        }
        self.sink_slot()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        outcome
    }

    /// Feeds `host`'s microphone to the session until the session ends or `stop` completes, when
    /// it closes the session.
    async fn feed_until(&self, host: &impl AudioHost, stop: impl Future + Send) -> Result<()> {
        let config = capture_config(self.input_format());
        let microphone = Microphone::open(host, &DeviceSelector::Default, config)
            .map_err(|error| device(host, Direction::Input, &error))?;
        let (frames, mut captured) = tokio_mpsc::unbounded_channel();
        let listening = Arc::new(AtomicBool::new(true));
        let (gone, left) = oneshot::channel::<()>();
        {
            let listening = Arc::clone(&listening);
            thread::spawn(move || {
                capture(microphone, &frames, &listening);
                drop(gone);
            });
        }
        let feeding = async move {
            while let Some(frame) = captured.recv().await {
                match self.send_audio(frame).await {
                    Ok(()) => {}
                    Err(Error::Closed { .. }) => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        };
        let ended = self.closed();
        tokio::pin!(feeding, ended, stop);
        let mut fed = false;
        let outcome = loop {
            tokio::select! {
                result = &mut feeding, if !fed => {
                    fed = true;
                    if let Err(error) = result {
                        break Err(error);
                    }
                }
                () = &mut ended => break Ok(()),
                _ = &mut stop => break self.close().await,
            }
        };
        listening.store(false, Ordering::Release);
        let _ = left.await;
        outcome
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use futures_util::StreamExt;
    use inference_audio::AudioFrame;
    use inference_audio_device::convert::decode;
    use inference_audio_device::fake::{FakeDevice, FakeHost};
    use inference_testkit::duplex::mock::{
        MockHistory, MockSpeechModel, Reply, ScriptedTurn, pcm, utterance,
    };
    use inference_testkit::mark::{self, MockId};
    use tokio::sync::oneshot;

    use crate::{Agent, Binding, Event, Stack};

    const LIMIT: Duration = Duration::from_secs(5);

    fn turn(text: &str) -> ScriptedTurn {
        ScriptedTurn {
            heard: "hello".to_owned(),
            reply: Reply::Say(text.to_owned()),
        }
    }

    /// A host of a mono microphone and a mono speaker at the duplex mock's 24 kHz.
    fn host() -> FakeHost {
        let rate = NonZeroU32::new(24_000).expect("24 kHz");
        FakeHost::new(vec![
            FakeDevice::input("test microphone", rate, NonZeroU16::MIN, true),
            FakeDevice::output("test speaker", rate, NonZeroU16::MIN, true),
        ])
    }

    async fn agent(model: MockSpeechModel) -> Arc<Agent> {
        let agent = Agent::voice_with(Stack::new().bind(Binding::speech(model)))
            .start()
            .await
            .expect("starts");
        Arc::new(agent)
    }

    /// Feeds `frames` to the host's microphone, once the run has opened it.
    async fn feed(host: &FakeHost, frames: &[AudioFrame]) {
        let started = Instant::now();
        while !host.input_open() {
            assert!(started.elapsed() < LIMIT, "the run opens the microphone");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        for frame in frames {
            assert!(host.feed(&decode(frame).expect("a well-formed frame")));
        }
    }

    async fn completed(history: &MockHistory, count: usize) {
        let started = Instant::now();
        while history.completed() < count {
            assert!(
                started.elapsed() < LIMIT,
                "{} of {count}",
                history.completed()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Every event the agent's stream holds, read after its end.
    async fn events(agent: &Agent) -> Vec<&'static str> {
        let mut events = agent.events();
        let mut names = Vec::new();
        while let Some(event) = tokio::time::timeout(LIMIT, events.next())
            .await
            .expect("the stream ends")
        {
            names.push(match event.expect("no terminal error") {
                Event::Audio(_) => "Audio",
                Event::Closed { .. } => "Closed",
                _ => "other",
            });
        }
        names
    }

    /// The frame indices of `host`'s played frames of reply `reply` of the duplex mock.
    fn played(host: &FakeHost, reply: u32) -> Vec<u32> {
        host.played_frames()
            .iter()
            .filter_map(mark::decode)
            .filter(|marked| marked.mock == MockId::Speech && marked.reply == reply)
            .map(|marked| marked.frame)
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stop_ends_the_run_and_closes_the_open_session() {
        let agent = agent(MockSpeechModel::native(vec![turn("hi")])).await;
        let (stop, stopped) = oneshot::channel::<()>();
        let running = Arc::clone(&agent);
        let mut run = tokio::spawn(async move { running.run_until(host(), stopped).await });
        assert!(
            tokio::time::timeout(Duration::from_millis(300), &mut run)
                .await
                .is_err(),
            "the run waits for the session's end or the stop"
        );
        stop.send(()).expect("the run listens");
        let ran = tokio::time::timeout(LIMIT, run)
            .await
            .expect("the run returns within 5 s")
            .expect("the run's task ends");
        assert!(ran.is_ok(), "{ran:?}");
        tokio::time::timeout(LIMIT, agent.closed())
            .await
            .expect("the session is closed");
        let names = events(&agent).await;
        assert_eq!(names.last(), Some(&"Closed"), "{names:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stop_fired_mid_reply_drains_first() {
        let model = MockSpeechModel::native(vec![turn("one"), turn("two")]);
        let history = model.history();
        let agent = agent(model).await;
        let host = host();
        let hold = host.hold_sink();
        let (stop, stopped) = oneshot::channel::<()>();
        let (running, devices) = (Arc::clone(&agent), host.clone());
        let mut run = tokio::spawn(async move { running.run_until(devices, stopped).await });
        feed(&host, &utterance(pcm(24_000), 0).frames()).await;
        completed(&history, 1).await;
        stop.send(()).expect("the run listens");
        assert!(
            tokio::time::timeout(Duration::from_secs(1), &mut run)
                .await
                .is_err(),
            "the run waits for the held sink"
        );
        drop(hold);
        let ran = tokio::time::timeout(LIMIT, run)
            .await
            .expect("the run returns within 5 s")
            .expect("the run's task ends");
        assert!(ran.is_ok(), "{ran:?}");
        assert_eq!(played(&host, 0), [0, 1]);
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(played(&host, 0), [0, 1]);
        let names = events(&agent).await;
        assert!(!names.contains(&"Audio"), "{names:?}");
        assert_eq!(
            names.iter().filter(|name| **name == "Closed").count(),
            1,
            "{names:?}"
        );
        assert_eq!(names.last(), Some(&"Closed"), "{names:?}");
    }
}
