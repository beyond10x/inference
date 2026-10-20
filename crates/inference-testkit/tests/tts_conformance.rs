//! The backend-neutral TTS adapter checks of `inference_testkit::tts::conformance`: every check
//! accepts the mock synthesiser in each of its four configurations, and the claims check rejects
//! two doubles that claim more than they do, naming the capability it did not observe, while each
//! double passes every other check. The usage check rejects a double that records no usage, a
//! record that misstates the characters offered and a second record.

#![cfg(feature = "tts")]

use std::cell::Cell;
use std::collections::VecDeque;
use std::num::{NonZeroU16, NonZeroU32};
use std::rc::Rc;

use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::usage::{Quantity, QuantityKind};
use inference_testkit::tts::conformance::{
    self, DEFAULT_SENTENCES, Failure, Fault, Item, ObservedClaims, Utterance,
};
use inference_testkit::tts::{MockConfiguration, MockSynthesis, MockSynthesiser};
use inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued, SegmentState,
};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::{
    CONTRACT_REVISION, ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt,
    PlaybackRefused, SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};

const SAMPLES: u32 = 80;

fn format() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("16000 is not zero"),
        channels: NonZeroU16::new(1).expect("one is not zero"),
    }
}

fn mock(configuration: MockConfiguration) -> MockSynthesiser {
    MockSynthesiser::new(configuration, ModelId::generate(), format())
}

fn claims(configuration: MockConfiguration, capability: Capability) -> bool {
    configuration.capabilities().contains(&capability)
}

#[test]
fn the_default_utterance_is_two_short_english_sentences_and_an_utterance_is_never_empty() {
    let utterance = Utterance::default();
    assert_eq!(utterance.sentences().len(), 2);
    assert_eq!(utterance.sentences(), DEFAULT_SENTENCES);
    assert!(Utterance::new(Vec::<String>::new()).is_none());
    assert!(Utterance::new(["Fine.", "  "]).is_none());
    let three = Utterance::new(["One.", "Two words.", "Three words here."]).expect("three");
    assert_eq!(three.sentences().len(), 3);
}

#[test]
fn the_tentative_check_accepts_the_mock_in_every_configuration() {
    for configuration in MockConfiguration::ALL {
        let observed =
            conformance::tentative_refused(&mut mock(configuration), &Utterance::default());
        let observed = observed.unwrap_or_else(|failure| panic!("{configuration:?}: {failure}"));
        assert_ne!(observed.refused, observed.spoken, "{configuration:?}");
        assert!(observed.frames_spoken > 0, "{configuration:?}");
    }
}

#[test]
fn the_playback_check_accepts_the_mock_in_every_configuration() {
    for configuration in MockConfiguration::ALL {
        let utterance = Utterance::new(["One.", "Two words.", "Three words here."]).expect("three");
        let observed = conformance::played_only_by_sink(&mut mock(configuration), &utterance)
            .unwrap_or_else(|failure| panic!("{configuration:?}: {failure}"));
        let frames: Vec<usize> = observed.played.iter().map(|&(_, frames)| frames).collect();
        assert_eq!(
            frames,
            vec![1, 2, 3],
            "{configuration:?}: one frame per word"
        );
    }
}

#[test]
fn the_claims_check_accepts_the_mock_in_every_configuration_and_observes_what_it_claims() {
    for configuration in MockConfiguration::ALL {
        let observed =
            conformance::claims_observed(&mut mock(configuration), &Utterance::default())
                .unwrap_or_else(|failure| panic!("{configuration:?}: {failure}"));
        assert_eq!(
            observed,
            ObservedClaims {
                incremental_input: claims(configuration, Capability::IncrementalInput),
                output_streaming: claims(configuration, Capability::OutputStreaming),
            },
            "{configuration:?}"
        );
    }
}

#[test]
fn the_audio_check_accepts_the_mock_in_every_configuration_with_and_without_an_expected_format() {
    for configuration in MockConfiguration::ALL {
        for expected in [None, Some(format())] {
            let observed =
                conformance::audio_valid(&mut mock(configuration), &Utterance::default(), expected)
                    .unwrap_or_else(|failure| panic!("{configuration:?}, {expected:?}: {failure}"));
            assert_eq!(observed.format, format(), "{configuration:?}");
            assert!(observed.frames > 0, "{configuration:?}");
            assert_eq!(
                observed.samples,
                observed.frames as u64 * u64::from(inference_testkit::tts::FRAME_SAMPLES),
                "{configuration:?}"
            );
        }
    }
}

#[test]
fn the_audio_check_rejects_an_expected_format_the_synthesiser_does_not_emit() {
    let other = AudioFormat {
        encoding: AudioEncoding::PcmF32le,
        ..format()
    };
    for configuration in MockConfiguration::ALL {
        let failure =
            conformance::audio_valid(&mut mock(configuration), &Utterance::default(), Some(other))
                .expect_err("the mock emits PCM s16le");
        assert_eq!(failure.item, Item::AudioValid, "{configuration:?}");
        assert_eq!(
            failure.fault,
            Fault::FormatDiffers {
                frame: 0,
                expected: other,
                actual: format(),
            },
            "{configuration:?}"
        );
    }
}

#[test]
fn the_cancellation_check_accepts_the_mock_in_every_configuration() {
    for configuration in MockConfiguration::ALL {
        let observed = conformance::cancellation(&mut mock(configuration), &Utterance::default())
            .unwrap_or_else(|failure| panic!("{configuration:?}: {failure}"));
        assert_eq!(observed.max_concurrency, 1, "{configuration:?}");
        let expected = if claims(configuration, Capability::OutputStreaming) {
            2
        } else {
            1
        };
        assert_eq!(
            observed.discarded.len(),
            expected,
            "{configuration:?}: a streaming synthesiser is also cancelled mid-segment: {observed:?}"
        );
    }
}

#[test]
fn the_usage_check_accepts_the_mock_in_every_configuration_and_observes_what_it_offered_and_emitted()
 {
    let characters: usize = DEFAULT_SENTENCES
        .iter()
        .map(|sentence| sentence.chars().count())
        .sum();
    for configuration in MockConfiguration::ALL {
        let observed = conformance::usage_recorded(&mut mock(configuration), &Utterance::default())
            .unwrap_or_else(|failure| panic!("{configuration:?}: {failure}"));
        assert_eq!(
            observed.characters,
            i64::try_from(characters).expect("short"),
            "{configuration:?}"
        );
        let samples = observed.frames as u64 * u64::from(inference_testkit::tts::FRAME_SAMPLES);
        assert_eq!(
            observed.audio_ms,
            i64::try_from(samples * 1_000 / 16_000).expect("short"),
            "{configuration:?}"
        );
        assert!(observed.frames > 0, "{configuration:?}");
        assert_eq!(
            observed.local_compute,
            Quantity::unknown(QuantityKind::LocalComputeMs),
            "{configuration:?}: the mock computes nothing of its own"
        );
    }
}

#[test]
fn the_usage_check_rejects_a_double_that_records_no_usage() {
    let mut double = Double::new(
        MockConfiguration::Neither,
        Behaviour {
            records_usage: false,
            ..Behaviour::NEITHER
        },
    );
    let failure = conformance::usage_recorded(&mut double, &Utterance::default())
        .expect_err("the double records no usage");
    assert_eq!(
        failure,
        Failure {
            item: Item::UsageRecorded,
            fault: Fault::UsageMissing,
        }
    );
}

#[test]
fn the_usage_check_rejects_a_record_that_misstates_the_characters_and_a_second_record() {
    let characters: usize = DEFAULT_SENTENCES
        .iter()
        .map(|sentence| sentence.chars().count())
        .sum();
    let characters = i64::try_from(characters).expect("short");
    let mut misstating = Forging::new(Forgery::Characters(characters + 1));
    let failure = conformance::usage_recorded(&mut misstating, &Utterance::default())
        .expect_err("the record misstates the characters");
    assert_eq!(
        failure,
        Failure {
            item: Item::UsageRecorded,
            fault: Fault::UsageQuantity {
                kind: QuantityKind::InputCharacters,
                expected: Some(Quantity::measured(
                    QuantityKind::InputCharacters,
                    characters
                )),
                actual: Some(Quantity::measured(
                    QuantityKind::InputCharacters,
                    characters + 1
                )),
            },
        }
    );

    let mut twice = Forging::new(Forgery::Twice);
    let failure = conformance::usage_recorded(&mut twice, &Utterance::default())
        .expect_err("the synthesis records its usage twice");
    assert_eq!(
        failure,
        Failure {
            item: Item::UsageRecorded,
            fault: Fault::UsageRepeated,
        }
    );
}

/// How a [`Forging`] synthesis falsifies the usage record of the mock it wraps.
#[derive(Debug, Clone, Copy)]
enum Forgery {
    /// Delivers the record with `input_characters` replaced by this many, `measured`.
    Characters(i64),
    /// Delivers the record twice.
    Twice,
}

/// The mock synthesiser behind a handle that falsifies its usage record, as an implementation
/// that builds its own [`SynthesisEvent::Usage`] could.
struct Forging {
    inner: MockSynthesiser,
    forgery: Forgery,
}

impl Forging {
    fn new(forgery: Forgery) -> Self {
        Self {
            inner: mock(MockConfiguration::Neither),
            forgery,
        }
    }
}

impl Synthesiser for Forging {
    type Handle = ForgingSynthesis;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn start(&mut self) -> Result<ForgingSynthesis, ConcurrencyLimitReached> {
        Ok(ForgingSynthesis {
            inner: self.inner.start()?,
            forgery: self.forgery,
            again: None,
        })
    }
}

struct ForgingSynthesis {
    inner: MockSynthesis,
    forgery: Forgery,
    again: Option<SynthesisEvent>,
}

impl Iterator for ForgingSynthesis {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        if let Some(event) = self.again.take() {
            return Some(event);
        }
        match self.inner.next()? {
            SynthesisEvent::Usage(mut recorded) => {
                match self.forgery {
                    Forgery::Characters(characters) => {
                        for quantity in &mut recorded.quantities {
                            if quantity.kind() == QuantityKind::InputCharacters {
                                *quantity =
                                    Quantity::measured(QuantityKind::InputCharacters, characters);
                            }
                        }
                    }
                    Forgery::Twice => {
                        self.again = Some(SynthesisEvent::Usage(recorded.clone()));
                    }
                }
                Some(SynthesisEvent::Usage(recorded))
            }
            event => Some(event),
        }
    }
}

impl SynthesisHandle for ForgingSynthesis {
    fn output(&self) -> &SynthesisOutput {
        self.inner.output()
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        self.inner.offer_segment(segment)
    }

    fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        self.inner.end_utterance()
    }

    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        self.inner.acknowledge_playback(segment_id, receipt)
    }

    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        self.inner.discard(segment_id)
    }
}

#[test]
fn the_claims_check_rejects_a_double_that_claims_output_streaming_but_emits_audio_only_at_completion()
 {
    let mut double = Double::new(MockConfiguration::OutputStreamingOnly, Behaviour::NEITHER);
    let failure = conformance::claims_observed(&mut double, &Utterance::default())
        .expect_err("the double never streams");
    assert_eq!(
        failure,
        Failure {
            item: Item::ClaimsObserved,
            fault: Fault::ClaimNotObserved {
                capability: Capability::OutputStreaming,
            },
        }
    );
    assert!(
        failure.to_string().contains("OutputStreaming"),
        "the message names the capability: {failure}"
    );
}

#[test]
fn the_claims_check_rejects_a_double_that_claims_incremental_input_but_takes_nothing_before_the_end()
 {
    let mut double = Double::new(MockConfiguration::IncrementalInputOnly, Behaviour::NEITHER);
    let failure = conformance::claims_observed(&mut double, &Utterance::default())
        .expect_err("the double takes nothing early");
    assert_eq!(
        failure,
        Failure {
            item: Item::ClaimsObserved,
            fault: Fault::ClaimNotObserved {
                capability: Capability::IncrementalInput,
            },
        }
    );
    assert!(
        failure.to_string().contains("IncrementalInput"),
        "the message names the capability: {failure}"
    );
}

#[test]
fn the_double_is_accepted_when_it_claims_what_it_does() {
    for configuration in MockConfiguration::ALL {
        let behaviour = Behaviour {
            takes_early: claims(configuration, Capability::IncrementalInput),
            streams: claims(configuration, Capability::OutputStreaming),
            records_usage: true,
        };
        let mut double = Double::new(configuration, behaviour);
        conformance::claims_observed(&mut double, &Utterance::default())
            .unwrap_or_else(|failure| panic!("{configuration:?}: {failure}"));
    }
}

#[test]
fn each_lying_double_passes_every_other_check() {
    for claimed in [
        MockConfiguration::OutputStreamingOnly,
        MockConfiguration::IncrementalInputOnly,
    ] {
        let utterance = Utterance::default();
        let mut double = Double::new(claimed, Behaviour::NEITHER);
        conformance::tentative_refused(&mut double, &utterance)
            .unwrap_or_else(|failure| panic!("{claimed:?}: {failure}"));
        conformance::played_only_by_sink(&mut double, &utterance)
            .unwrap_or_else(|failure| panic!("{claimed:?}: {failure}"));
        conformance::audio_valid(&mut double, &utterance, Some(format()))
            .unwrap_or_else(|failure| panic!("{claimed:?}: {failure}"));
        conformance::cancellation(&mut double, &utterance)
            .unwrap_or_else(|failure| panic!("{claimed:?}: {failure}"));
        conformance::usage_recorded(&mut double, &utterance)
            .unwrap_or_else(|failure| panic!("{claimed:?}: {failure}"));
    }
}

/// What a [`Double`] does, whatever its adapter claims.
#[derive(Debug, Clone, Copy)]
struct Behaviour {
    /// Takes segments before the utterance is complete, when the contract lets it.
    takes_early: bool,
    /// Emits frames while a segment is Queued, when the contract lets it.
    streams: bool,
    /// Delivers the synthesis's usage record once it has nothing else left, when the contract
    /// lets it.
    records_usage: bool,
}

impl Behaviour {
    const NEITHER: Self = Self {
        takes_early: false,
        streams: false,
        records_usage: true,
    };
}

/// A synthesiser whose adapter claims one configuration and which behaves as `behaviour` says,
/// speaking each segment as one frame of [`SAMPLES`] samples of silence per word, with a maximum
/// concurrency of one. Its synthesis is built from its own adapter record, so the contract lets
/// it do everything it claims; it chooses not to.
struct Double {
    adapter: Adapter,
    behaviour: Behaviour,
    running: Rc<Cell<usize>>,
}

impl Double {
    fn new(claimed: MockConfiguration, behaviour: Behaviour) -> Self {
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "conformance double".to_owned(),
            contract_revision: CONTRACT_REVISION.to_owned(),
            execution_form: ExecutionForm::InProcess,
            locality: Locality::SameProcess,
            capabilities: claimed.capabilities(),
            max_concurrency: 1,
        })
        .expect("a maximum concurrency of one is positive");
        Self {
            adapter,
            behaviour,
            running: Rc::new(Cell::new(0)),
        }
    }
}

impl Synthesiser for Double {
    type Handle = DoubleSynthesis;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(&mut self) -> Result<DoubleSynthesis, ConcurrencyLimitReached> {
        if self.running.get() >= 1 {
            return Err(ConcurrencyLimitReached { max_concurrency: 1 });
        }
        self.running.set(self.running.get() + 1);
        Ok(DoubleSynthesis {
            output: SynthesisOutput::start(&self.adapter),
            behaviour: self.behaviour,
            next_sample: 0,
            pending: VecDeque::new(),
            delivered: VecDeque::new(),
            running: Rc::clone(&self.running),
        })
    }
}

struct DoubleSynthesis {
    output: SynthesisOutput,
    behaviour: Behaviour,
    next_sample: u64,
    pending: VecDeque<(SegmentId, usize)>,
    delivered: VecDeque<SynthesisEvent>,
    running: Rc<Cell<usize>>,
}

impl Drop for DoubleSynthesis {
    fn drop(&mut self) {
        self.running.set(self.running.get() - 1);
    }
}

impl DoubleSynthesis {
    fn silence(&mut self) -> AudioFrame {
        let frame = AudioFrame {
            format: format(),
            position: SampleClockPosition::new(self.next_sample).expect("small"),
            sample_count: SAMPLES,
            data: vec![0; usize::try_from(format().data_len(SAMPLES)).expect("small")],
        };
        self.next_sample += u64::from(SAMPLES);
        frame
    }
}

impl Iterator for DoubleSynthesis {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        loop {
            if let Some(event) = self.delivered.pop_front() {
                return Some(event);
            }
            if (self.behaviour.takes_early || self.output.utterance_ended())
                && let Ok(taken) = self.output.take_input()
            {
                self.pending.extend(taken.iter().map(|segment| {
                    (
                        segment.segment_id(),
                        segment.text().split_whitespace().count().max(1),
                    )
                }));
            }
            let Some(&(segment_id, frames_left)) = self.pending.front() else {
                return if self.behaviour.records_usage {
                    self.output.usage().ok()
                } else {
                    None
                };
            };
            if self.output.segment(segment_id).map(OutputSegment::state)
                != Some(SegmentState::Queued)
            {
                self.pending.pop_front();
                continue;
            }
            if self.behaviour.streams && frames_left > 0 {
                let frame = self.silence();
                let event = self
                    .output
                    .audio(segment_id, frame)
                    .expect("the double claims output streaming");
                if let Some(job) = self.pending.front_mut() {
                    job.1 -= 1;
                }
                return Some(event);
            }
            let frames = (0..frames_left).map(|_| self.silence()).collect();
            let events = self
                .output
                .complete(segment_id, frames)
                .expect("a taken Queued segment completes");
            self.pending.pop_front();
            self.delivered.extend(events);
        }
    }
}

impl SynthesisHandle for DoubleSynthesis {
    fn output(&self) -> &SynthesisOutput {
        &self.output
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        self.output.offer(segment)
    }

    fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        self.output.end_utterance()
    }

    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        self.output.acknowledge_playback(segment_id, receipt)
    }

    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        let event = self.output.discard(segment_id)?;
        self.delivered
            .retain(|event| event.segment_id() != Some(segment_id));
        Ok(event)
    }
}
