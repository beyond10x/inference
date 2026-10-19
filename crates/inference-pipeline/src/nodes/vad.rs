//! The voice activity detector: a node of role `policy` with one input port of audio frames
//! (`inference-audio`) and one output port of Control modality carrying speech activity
//! (`docs/design/inference.md` 466). It decides by a deterministic energy threshold and carries
//! no model and no weights; a learned detector is an adapter, not this node (917).
//!
//! A frame is loud when the root mean square of its samples, at full scale 1, reaches the
//! configured threshold, and quiet otherwise. Speech starts once loud frames have run for the
//! attack time of audio frame time with no quiet frame between them, and its onset is the first
//! sample of that run. Speech ends once quiet frames have run for the hold time, and its offset is
//! the first sample of that run. Times are counted in samples at the sample rate of each frame, so
//! a quiet gap shorter than the hold time leaves speech going, and a loud burst shorter than the
//! attack time starts none.
//!
//! Each change is a [`SpeechActivity`], sent on in an envelope of the kind
//! [`SpeechActivity::kind`] names, read on [`Timebase::AudioFrameTime`] with the sample clock
//! position just past the frame that decided it as its timestamp, and naming that frame's envelope
//! as its cause. The two kinds are the events `inference.stream.SpeechStarted` and
//! `inference.stream.SpeechEnded` of `ess/domains/stream.yaml`, whose `onset` and `offset` the
//! variants carry.
//!
//! On the end of input, a control event of kind `stop`, or once its input edge is closed, as on a
//! drain, speech still going ends: its offset is the first sample of the quiet run going, if one
//! is shorter than the hold time, and otherwise the position just past the last frame; the change
//! is decided just past the last frame and names no cause. On a `stop` the end of input is then
//! passed on. Then the node ends. Cancelled, it sends nothing more. Content other than an audio
//! frame fails the node.

use std::fmt;
use std::future;
use std::sync::Arc;
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFrame, SampleClockPosition};
use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId};
use serde::{Deserialize, Serialize};

use super::{Emitter, Next, Task, Watched, block_on, next, one_in_one_out};
use crate::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation};

/// The configuration of a [`VoiceActivityDetector`]: `threshold` is the root mean square level,
/// at full scale 1, from which a frame is loud; `attack` the loud time that starts speech and
/// `hold` the quiet time that ends it, both in audio frame time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VadConfig {
    pub threshold: f64,
    pub attack: Duration,
    pub hold: Duration,
}

/// `inference.stream.SpeechStarted`: speech started at `onset`, the first sample of the loud run
/// that started it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechStarted {
    pub onset: SampleClockPosition,
}

/// `inference.stream.SpeechEnded`: speech ended at `offset`, the first sample of the quiet run
/// that ended it. When input ended during speech, the first sample of the quiet run going, if one
/// is, and otherwise the position just past the last frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechEnded {
    pub offset: SampleClockPosition,
}

/// A change of speech activity, placed on the sample clock of the audio it was detected in: one
/// of the two speech events of `inference.stream`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechActivity {
    /// `inference.stream.SpeechStarted`.
    Started(SpeechStarted),
    /// `inference.stream.SpeechEnded`.
    Ended(SpeechEnded),
}

impl SpeechActivity {
    /// The `kind` of the envelope of a [`SpeechActivity::Started`].
    pub const STARTED: &'static str = "inference.stream.SpeechStarted";
    /// The `kind` of the envelope of a [`SpeechActivity::Ended`].
    pub const ENDED: &'static str = "inference.stream.SpeechEnded";

    /// The `kind` of the envelope carrying the change: [`SpeechActivity::STARTED`] or
    /// [`SpeechActivity::ENDED`].
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Started(_) => Self::STARTED,
            Self::Ended(_) => Self::ENDED,
        }
    }
}

/// The payload of a run whose edges carry audio frames and speech activity: the detector reads an
/// [`AudioFrame`] out of it and builds a [`SpeechActivity`], or a control event, into it.
pub trait AudioPayload: Send + Sized + 'static {
    /// The audio frame the payload carries.
    ///
    /// # Errors
    ///
    /// The payload, unchanged, when it carries none.
    fn into_audio(self) -> Result<AudioFrame, Self>;

    /// The payload carrying `activity`.
    fn from_activity(activity: SpeechActivity) -> Self;

    /// The payload of the control event `kind`; its envelope's `kind` is [`ControlKind::as_str`].
    fn control(kind: ControlKind) -> Self;
}

/// The samples `time` takes at `sample_rate` hertz, rounded up.
fn samples_in(time: Duration, sample_rate: u32) -> u128 {
    const NANOS_PER_SECOND: u128 = 1_000_000_000;
    (time.as_nanos() * u128::from(sample_rate)).div_ceil(NANOS_PER_SECOND)
}

/// The root mean square of the samples of `frame`, at full scale 1; 0 for a frame without one.
fn level(frame: &AudioFrame) -> f64 {
    let squares: Vec<f64> = match frame.format.encoding {
        AudioEncoding::PcmS16le => frame
            .data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|bytes| f64::from(i16::from_le_bytes(*bytes)) / 32_768.0)
            .map(|sample| sample * sample)
            .collect(),
        AudioEncoding::PcmF32le => frame
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f64::from(f32::from_le_bytes(*bytes)))
            .map(|sample| sample * sample)
            .collect(),
    };
    if squares.is_empty() {
        return 0.0;
    }
    let count = u32::try_from(squares.len()).map_or(f64::from(u32::MAX), f64::from);
    (squares.iter().sum::<f64>() / count).sqrt()
}

/// A run of frames of one loudness: the position of its first sample and the samples it holds.
#[derive(Debug, Clone, Copy)]
struct Run {
    start: SampleClockPosition,
    samples: u128,
}

/// The energy detector a [`VoiceActivityDetector`] runs over its frames.
#[derive(Debug)]
struct Detector {
    config: VadConfig,
    speaking: bool,
    /// The run of frames of the loudness that would change the state, if one is going.
    run: Option<Run>,
    /// The position just past the last frame.
    past_last: Option<SampleClockPosition>,
}

impl Detector {
    const fn new(config: VadConfig) -> Self {
        Self {
            config,
            speaking: false,
            run: None,
            past_last: None,
        }
    }

    /// The change `frame` decides, if any.
    fn frame(&mut self, frame: &AudioFrame) -> Option<SpeechActivity> {
        self.past_last = Some(past(frame));
        let loud = level(frame) >= self.config.threshold;
        if loud != self.speaking {
            let run = self.run.get_or_insert(Run {
                start: frame.position,
                samples: 0,
            });
            run.samples += u128::from(frame.sample_count);
            let (run, needed) = (
                *run,
                if loud {
                    self.config.attack
                } else {
                    self.config.hold
                },
            );
            if run.samples >= samples_in(needed, frame.format.sample_rate.get()) {
                self.speaking = loud;
                self.run = None;
                return Some(if loud {
                    SpeechActivity::Started(SpeechStarted { onset: run.start })
                } else {
                    SpeechActivity::Ended(SpeechEnded { offset: run.start })
                });
            }
        } else {
            self.run = None;
        }
        None
    }

    /// The change the end of input decides: speech still going ends, at the first sample of the
    /// quiet run going, or else just past the last frame.
    fn end(&mut self) -> Option<SpeechActivity> {
        if !self.speaking {
            return None;
        }
        self.speaking = false;
        let offset = self.run.take().map(|run| run.start).or(self.past_last)?;
        Some(SpeechActivity::Ended(SpeechEnded { offset }))
    }
}

/// The position just past `frame`, saturating at the largest position.
fn past(frame: &AudioFrame) -> SampleClockPosition {
    SampleClockPosition::new(frame.position.get() + u64::from(frame.sample_count))
        .unwrap_or(SampleClockPosition::MAX)
}

/// What the detector was asked to do and what its exchange came to.
#[derive(Debug, Default)]
struct VadState {
    cancelled: bool,
    failure: Option<String>,
}

impl fmt::Debug for Watched<VadState> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

/// The voice activity detector node; see the module documentation.
#[derive(Debug)]
pub struct VoiceActivityDetector {
    run_id: RunId,
    config: VadConfig,
    node_id: Option<NodeId>,
    state: Arc<Watched<VadState>>,
    task: Task,
}

impl VoiceActivityDetector {
    /// A detector of the run `run_id` deciding by `config`.
    #[must_use]
    pub fn new(run_id: RunId, config: VadConfig) -> Self {
        Self {
            run_id,
            config,
            node_id: None,
            state: Arc::new(Watched::new(VadState::default())),
            task: Task::new(),
        }
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.state
            .read(|state| state.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

/// One started exchange of a [`VoiceActivityDetector`].
struct Exchanging<P> {
    detector: Detector,
    state: Arc<Watched<VadState>>,
    input: Box<dyn InputEndpoint<P>>,
    output: Box<dyn OutputEndpoint<P>>,
    emitter: Emitter,
}

impl<P: AudioPayload> Exchanging<P> {
    /// Sends `envelope` on, unless the detector was cancelled: it then sends nothing more.
    async fn send(&self, envelope: EventEnvelope<P>) -> Result<(), String> {
        if self.state.read(|state| state.cancelled) {
            return Ok(());
        }
        self.output
            .send(envelope)
            .await
            .map_err(|refused| refused.to_string())
    }

    /// Sends `activity` on, decided at `decided`, the position just past the frame of `cause`.
    async fn activity(
        &mut self,
        activity: SpeechActivity,
        decided: SampleClockPosition,
        cause: Option<EventId>,
    ) -> Result<(), String> {
        let mut envelope = self
            .emitter
            .envelope(activity.kind(), None, P::from_activity(activity));
        envelope.timebase = Timebase::AudioFrameTime;
        envelope.timestamp = i64::try_from(decided.get()).unwrap_or(i64::MAX);
        envelope.causation_id = cause;
        self.send(envelope).await
    }

    /// The end of input, a `stop` or a closed input edge: speech still going ends, decided just
    /// past the last frame, with no cause.
    async fn end_of_input(&mut self) -> Result<(), String> {
        let past_last = self.detector.past_last;
        match (self.detector.end(), past_last) {
            (Some(activity), Some(decided)) => self.activity(activity, decided, None).await,
            _ => Ok(()),
        }
    }

    async fn exchange(&mut self) -> Result<(), String> {
        loop {
            let state = Arc::clone(&self.state);
            let next = next(self.input.as_mut(), &state, |state| {
                state.cancelled.then_some(())
            })
            .await;
            match next {
                Next::Signal(()) => return Ok(()),
                Next::Closed => return self.end_of_input().await,
                Next::Envelope(envelope) => match ControlKind::of(&envelope.kind) {
                    Some(ControlKind::Stop) => {
                        self.end_of_input().await?;
                        let stop = ControlKind::Stop;
                        let end = self.emitter.envelope(stop.as_str(), None, P::control(stop));
                        return self.send(end).await;
                    }
                    Some(_) => {}
                    None => {
                        let frame = envelope.payload.into_audio().map_err(|_| {
                            format!(
                                "the voice activity detector takes audio frames; an event of kind \
                                 {} arrived",
                                envelope.kind
                            )
                        })?;
                        if let Some(activity) = self.detector.frame(&frame) {
                            self.activity(activity, past(&frame), Some(envelope.event_id))
                                .await?;
                        }
                    }
                },
            }
        }
    }

    fn run(mut self) {
        let ended = block_on(self.exchange());
        self.state.update(|state| state.failure = ended.err());
    }
}

impl<P: AudioPayload> NodeImplementation<P> for VoiceActivityDetector {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let config = self.config;
        let prepared = if config.threshold.is_finite() && config.threshold > 0.0 {
            Ok(())
        } else {
            Err(NodeError::new(format!(
                "the energy threshold of a voice activity detector is a level above 0; it is {}",
                config.threshold
            )))
        };
        Box::pin(future::ready(prepared))
    }

    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = one_in_one_out(self.node_id, endpoints).map(|(node_id, input, output)| {
            let exchanging = Exchanging {
                detector: Detector::new(self.config),
                state: Arc::clone(&self.state),
                input: input.endpoint,
                emitter: Emitter::new(self.run_id, node_id, output.port_id.to_string()),
                output: output.endpoint,
            };
            self.task.spawn(move || exchanging.run());
        });
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(async move {
            self.task.finished().await.map_err(NodeError::new)?;
            self.failure()
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.state.update(|state| state.cancelled = true);
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(async move {
            let _already_reported_by_drain = self.task.finished().await;
        })
    }
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};
    use std::time::Duration;

    use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};

    use super::{Detector, SpeechActivity, SpeechEnded, SpeechStarted, VadConfig};

    /// Samples per frame: 10 ms at 1 kHz.
    const FRAME: u32 = 10;

    const CONFIG: VadConfig = VadConfig {
        threshold: 0.1,
        attack: Duration::from_millis(30),
        hold: Duration::from_millis(50),
    };

    fn at(sample: u64) -> SampleClockPosition {
        SampleClockPosition::new(sample).expect("small")
    }

    /// Frames of 1 kHz mono audio in `encoding`, one per entry of `loud`, loud ones at half scale.
    fn frames(encoding: AudioEncoding, loud: &[bool]) -> Vec<AudioFrame> {
        let format = AudioFormat {
            encoding,
            sample_rate: NonZeroU32::new(1_000).expect("1000"),
            channels: NonZeroU16::new(1).expect("1"),
        };
        loud.iter()
            .zip(0_u64..)
            .map(|(&loud, index)| {
                let level = if loud { 0.5 } else { 0.0 };
                let sample: Vec<u8> = match encoding {
                    AudioEncoding::PcmS16le => {
                        (16_384_i16 * i16::from(loud)).to_le_bytes().to_vec()
                    }
                    AudioEncoding::PcmF32le => (level as f32).to_le_bytes().to_vec(),
                };
                AudioFrame {
                    format,
                    position: at(index * u64::from(FRAME)),
                    sample_count: FRAME,
                    data: sample.repeat(FRAME as usize),
                }
            })
            .collect()
    }

    /// What the detector decides over `frames`, each change with the index of the frame that
    /// decided it, and what the end of input decides.
    fn decide(frames: &[AudioFrame]) -> (Vec<(usize, SpeechActivity)>, Option<SpeechActivity>) {
        let mut detector = Detector::new(CONFIG);
        let changes = frames
            .iter()
            .enumerate()
            .filter_map(|(index, frame)| detector.frame(frame).map(|change| (index, change)))
            .collect();
        (changes, detector.end())
    }

    #[test]
    fn speech_starts_after_the_attack_time_and_ends_after_the_hold_time() {
        let mut loud = vec![true; 5];
        loud.extend([false; 6]);
        for encoding in AudioEncoding::ALL {
            assert_eq!(
                decide(&frames(encoding, &loud)),
                (
                    vec![
                        (2, SpeechActivity::Started(SpeechStarted { onset: at(0) })),
                        (9, SpeechActivity::Ended(SpeechEnded { offset: at(50) })),
                    ],
                    None
                ),
                "{encoding:?}"
            );
        }
    }

    #[test]
    fn each_kind_is_an_event_the_stream_domain_declares() {
        let stream = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../ess/domains/stream.yaml"
        ));
        let declared: Vec<&str> = stream
            .lines()
            .filter_map(|line| line.strip_prefix("  - name: "))
            .collect();
        for activity in [
            SpeechActivity::Started(SpeechStarted { onset: at(0) }),
            SpeechActivity::Ended(SpeechEnded { offset: at(0) }),
        ] {
            assert!(declared.contains(&activity.kind()), "{}", activity.kind());
        }
    }

    #[test]
    fn a_burst_shorter_than_the_attack_time_starts_no_speech() {
        let loud = [true, true, false, true, true, false, false];
        assert_eq!(
            decide(&frames(AudioEncoding::PcmS16le, &loud)),
            (Vec::new(), None)
        );
    }

    #[test]
    fn a_gap_shorter_than_the_hold_time_leaves_speech_going() {
        let loud = [true, true, true, false, false, false, false, true, false];
        assert_eq!(
            decide(&frames(AudioEncoding::PcmS16le, &loud)),
            (
                vec![(2, SpeechActivity::Started(SpeechStarted { onset: at(0) }))],
                Some(SpeechActivity::Ended(SpeechEnded { offset: at(80) }))
            )
        );
    }

    #[test]
    fn the_end_of_input_during_speech_ends_it_just_past_the_last_frame() {
        let loud = [false, true, true, true, true];
        assert_eq!(
            decide(&frames(AudioEncoding::PcmS16le, &loud)),
            (
                vec![(3, SpeechActivity::Started(SpeechStarted { onset: at(10) }))],
                Some(SpeechActivity::Ended(SpeechEnded { offset: at(50) }))
            )
        );
    }
}
