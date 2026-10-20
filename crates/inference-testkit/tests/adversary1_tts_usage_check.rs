//! Adversary pass 1 on `story:usage-tts`: the backend-neutral usage check
//! (`conformance::usage_recorded`) rejects, with the fault it names, a synthesis whose record is
//! not its last event, names another model, is not final, misstates the audio, counts more than
//! one request, or carries no local compute. Each is a record an implementation that builds its
//! own `SynthesisEvent::Usage` could deliver; the double here rewrites the mock's.

#![cfg(feature = "tts")]

use std::collections::VecDeque;
use std::num::{NonZeroU16, NonZeroU32};

use inference_core::model::{Adapter, ModelId};
use inference_core::usage::{Quantity, QuantityKind};
use inference_testkit::tts::conformance::{self, Failure, Fault, Item, Utterance};
use inference_testkit::tts::{MockConfiguration, MockSynthesis, MockSynthesiser};
use inference_text::{AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::{
    ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt, PlaybackRefused,
    SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};

fn format() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("not zero"),
        channels: NonZeroU16::new(1).expect("not zero"),
    }
}

#[derive(Debug, Clone, Copy)]
enum Forgery {
    /// Delivers the record before the event that preceded it.
    NotLast,
    /// Names a model other than the adapter's.
    AnotherModel,
    /// Marks the record not final.
    NotFinal,
    /// Adds 1 ms to `output_audio_ms`.
    AudioPlusOne,
    /// Counts two requests.
    TwoRequests,
    /// Leaves `local_compute_ms` out.
    NoLocalCompute,
}

struct Forging {
    inner: MockSynthesiser,
    forgery: Forgery,
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
            queue: VecDeque::new(),
        })
    }
}

struct ForgingSynthesis {
    inner: MockSynthesis,
    forgery: Forgery,
    queue: VecDeque<SynthesisEvent>,
}

fn replace(quantities: &mut Vec<Quantity>, kind: QuantityKind, value: Option<i64>) {
    quantities.retain(|quantity| quantity.kind() != kind);
    if let Some(value) = value {
        quantities.push(Quantity::measured(kind, value));
    }
}

impl Iterator for ForgingSynthesis {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        if self.queue.is_empty() {
            self.queue.extend(self.inner.by_ref());
            if let Some(SynthesisEvent::Usage(recorded)) = self.queue.back_mut() {
                match self.forgery {
                    Forgery::NotLast => {}
                    Forgery::AnotherModel => recorded.model_id = ModelId::generate(),
                    Forgery::NotFinal => recorded.r#final = false,
                    Forgery::AudioPlusOne => {
                        let ms = recorded
                            .quantities
                            .iter()
                            .find(|quantity| quantity.kind() == QuantityKind::OutputAudioMs)
                            .and_then(Quantity::value)
                            .expect("measured");
                        replace(
                            &mut recorded.quantities,
                            QuantityKind::OutputAudioMs,
                            Some(ms + 1),
                        );
                    }
                    Forgery::TwoRequests => {
                        replace(&mut recorded.quantities, QuantityKind::Requests, Some(2));
                    }
                    Forgery::NoLocalCompute => {
                        replace(&mut recorded.quantities, QuantityKind::LocalComputeMs, None);
                    }
                }
                if matches!(self.forgery, Forgery::NotLast) {
                    let last = self.queue.len() - 1;
                    self.queue.swap(last - 1, last);
                }
            }
        }
        self.queue.pop_front()
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

fn fault_of(forgery: Forgery) -> Fault {
    let mut forging = Forging {
        inner: MockSynthesiser::new(MockConfiguration::Neither, ModelId::generate(), format()),
        forgery,
    };
    let Failure { item, fault } = conformance::usage_recorded(&mut forging, &Utterance::default())
        .expect_err("the forged record is rejected");
    assert_eq!(item, Item::UsageRecorded, "{forgery:?}");
    fault
}

#[test]
fn a_record_followed_by_a_frame_is_rejected_as_not_last() {
    let fault = fault_of(Forgery::NotLast);
    assert!(
        matches!(
            fault,
            Fault::UsageNotLast {
                segment_id: Some(_)
            }
        ),
        "{fault:?}"
    );
}

#[test]
fn a_record_of_another_model_is_rejected() {
    assert_eq!(
        fault_of(Forgery::AnotherModel),
        Fault::UsageOfAnotherAdapter
    );
}

#[test]
fn a_record_of_a_completed_utterance_that_is_not_final_is_rejected() {
    assert_eq!(fault_of(Forgery::NotFinal), Fault::UsageNotFinal);
}

#[test]
fn a_record_that_misstates_the_audio_is_rejected() {
    assert!(
        matches!(
            fault_of(Forgery::AudioPlusOne),
            Fault::UsageQuantity {
                kind: QuantityKind::OutputAudioMs,
                ..
            }
        ),
        "the audio delivered is the check's measure"
    );
}

#[test]
fn a_record_of_two_requests_is_rejected() {
    assert_eq!(
        fault_of(Forgery::TwoRequests),
        Fault::UsageQuantity {
            kind: QuantityKind::Requests,
            expected: Some(Quantity::measured(QuantityKind::Requests, 1)),
            actual: Some(Quantity::measured(QuantityKind::Requests, 2)),
        }
    );
}

#[test]
fn a_record_without_local_compute_is_rejected() {
    assert_eq!(
        fault_of(Forgery::NoLocalCompute),
        Fault::UsageQuantity {
            kind: QuantityKind::LocalComputeMs,
            expected: None,
            actual: None,
        }
    );
}
