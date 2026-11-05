//! The backend-neutral STT adapter checks of `inference_testkit::stt::conformance`: every check
//! accepts the mock recogniser in both of its configurations, and the failure check a double whose
//! backend fails. Doubles that keep two spans open, claim streaming they do not do, fail without
//! saying why, or record no usage are each rejected with the fault that names it.

#![cfg(feature = "stt")]

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use inference_core::envelope::EventEnvelope;
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::usage::{Quantity, QuantityKind};
use inference_stt::inference_audio::AudioFrame;
use inference_stt::{
    CONTRACT_REVISION, ConcurrencyLimitReached, InputEnded, Recogniser, RecognitionHandle,
    RecognitionRequest, RevisionRefused, TranscriptEvent, TranscriptOutput,
};
use inference_testkit::stt::conformance::{self, Fault, Input, Item, ObservedClaims};
use inference_testkit::stt::{MockConfiguration, MockRecogniser, script};
use inference_text::ReviseSpan;

fn mock(configuration: MockConfiguration) -> MockRecogniser {
    MockRecogniser::new(configuration, ModelId::generate(), script())
}

fn claims(configuration: MockConfiguration, capability: Capability) -> bool {
    configuration.capabilities().contains(&capability)
}

#[test]
fn the_committed_check_accepts_the_mock_and_every_external_revision_is_refused() {
    for configuration in MockConfiguration::ALL {
        let observed = conformance::committed_final(&mut mock(configuration), &Input::default())
            .unwrap_or_else(|failure| panic!("{configuration:?}: {failure}"));
        let texts: Vec<&str> = observed
            .committed
            .iter()
            .map(|(_, text)| text.as_str())
            .collect();
        assert_eq!(texts, ["hello there", "how are you"], "{configuration:?}");
        assert_eq!(observed.revisions_refused, 2, "{configuration:?}");
    }
}

#[test]
fn the_order_check_accepts_the_mock_in_both_configurations() {
    for configuration in MockConfiguration::ALL {
        let observed = conformance::spans_in_order(&mut mock(configuration), &Input::default())
            .unwrap_or_else(|failure| panic!("{configuration:?}: {failure}"));
        assert_eq!(observed.span_events, 6, "{configuration:?}");
        assert_eq!(observed.committed, 2, "{configuration:?}");
        assert_eq!(
            observed.last_sequence, 7,
            "{configuration:?}: six spans and usage"
        );
    }
}

#[test]
fn the_end_check_accepts_the_mock_in_both_configurations() {
    for configuration in MockConfiguration::ALL {
        let observed = conformance::end_of_input(&mut mock(configuration), &Input::default())
            .unwrap_or_else(|failure| panic!("{configuration:?}: {failure}"));
        assert_eq!(observed.committed, 2, "{configuration:?}");
    }
}

#[test]
fn the_cancellation_check_accepts_the_mock_in_both_configurations() {
    for configuration in MockConfiguration::ALL {
        let observed = conformance::cancellation(&mut mock(configuration), &Input::default())
            .unwrap_or_else(|failure| panic!("{configuration:?}: {failure}"));
        assert_eq!(observed.max_concurrency, 1, "{configuration:?}");
        assert_eq!(observed.frames_before_drop, 80, "{configuration:?}");
    }
}

#[test]
fn the_usage_check_accepts_the_mock_and_counts_the_audio_offered() {
    for configuration in MockConfiguration::ALL {
        let observed = conformance::usage_recorded(&mut mock(configuration), &Input::default())
            .unwrap_or_else(|failure| panic!("{configuration:?}: {failure}"));
        assert_eq!(observed.audio_ms, 1_600, "{configuration:?}");
        assert_eq!(
            observed.local_compute,
            Quantity::unknown(QuantityKind::LocalComputeMs),
            "{configuration:?}: the mock computes nothing of its own"
        );
    }
}

#[test]
fn the_claims_check_accepts_the_mock_and_observes_what_it_claims() {
    for configuration in MockConfiguration::ALL {
        let observed = conformance::claims_observed(&mut mock(configuration), &Input::default())
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
fn the_failure_check_accepts_a_double_whose_backend_fails_and_names_why() {
    let mut double = Double::new(Behaviour {
        fails: true,
        ..Behaviour::STREAMING
    });
    let observed =
        conformance::backend_failure(&mut double, &Input::default(), DoubleRecognition::failure)
            .unwrap_or_else(|failure| panic!("{failure}"));
    assert_eq!(observed.error, "the double's backend closed");
    assert_eq!(observed.committed, 1, "one span before the failure");
}

#[test]
fn the_failure_check_rejects_a_double_that_does_not_say_why_it_failed() {
    let mut double = Double::new(Behaviour {
        fails: true,
        names_failure: false,
        ..Behaviour::STREAMING
    });
    let failure =
        conformance::backend_failure(&mut double, &Input::default(), DoubleRecognition::failure)
            .expect_err("the double names no error");
    assert_eq!(failure.item, Item::BackendFailure);
    assert_eq!(failure.fault, Fault::NoFailureReported);
}

#[test]
fn the_failure_check_rejects_a_backend_that_did_not_fail() {
    let failure = conformance::backend_failure(
        &mut Double::new(Behaviour::STREAMING),
        &Input::default(),
        DoubleRecognition::failure,
    )
    .expect_err("the double completes");
    assert_eq!(failure.fault, Fault::FailedUsageFinal);
}

#[test]
fn the_order_check_rejects_two_spans_open_at_once() {
    let mut double = Double::new(Behaviour {
        overlaps: true,
        ..Behaviour::STREAMING
    });
    let failure = conformance::spans_in_order(&mut double, &Input::default())
        .expect_err("the double opens a second span before the first commits");
    assert_eq!(failure.item, Item::SpansInOrder);
    assert!(
        matches!(failure.fault, Fault::SpansOverlap { .. }),
        "{failure}"
    );
}

#[test]
fn the_claims_check_rejects_streaming_claimed_and_never_done() {
    let mut double = Double::new(Behaviour {
        holds_output: true,
        ..Behaviour::STREAMING
    });
    let failure = conformance::claims_observed(&mut double, &Input::default())
        .expect_err("the double emits only after the end of input");
    assert_eq!(failure.item, Item::ClaimsObserved);
    assert_eq!(
        failure.fault,
        Fault::ClaimNotObserved {
            capability: Capability::OutputStreaming
        }
    );
}

#[test]
fn the_usage_check_rejects_a_double_that_records_no_usage() {
    let mut double = Double::new(Behaviour {
        records_usage: false,
        ..Behaviour::STREAMING
    });
    let failure = conformance::usage_recorded(&mut double, &Input::default())
        .expect_err("the double records nothing");
    assert_eq!(failure.fault, Fault::UsageMissing);
}

#[test]
fn every_other_check_accepts_the_doubles_that_only_one_check_rejects() {
    let input = Input::default();
    let double = Double::new;
    let holds = Behaviour {
        holds_output: true,
        ..Behaviour::STREAMING
    };
    conformance::committed_final(&mut double(holds), &input).unwrap();
    conformance::spans_in_order(&mut double(holds), &input).unwrap();
    conformance::end_of_input(&mut double(holds), &input).unwrap();
    conformance::cancellation(&mut double(holds), &input).unwrap();
    conformance::usage_recorded(&mut double(holds), &input).unwrap();
    let silent = Behaviour {
        records_usage: false,
        ..Behaviour::STREAMING
    };
    conformance::spans_in_order(&mut double(silent), &input).unwrap();
    conformance::claims_observed(&mut double(silent), &input).unwrap();
}

/// How a [`Double`] behaves: every 40 frames taken it emits a span and commits it, one open at a
/// time, as a streaming recogniser that claims both capabilities.
#[derive(Debug, Clone, Copy)]
struct Behaviour {
    /// Opens a second span before committing the first, then commits both.
    overlaps: bool,
    /// Takes input as it comes and emits nothing until input has ended.
    holds_output: bool,
    /// Fails after its first committed span, marking the call failed.
    fails: bool,
    /// Says why it failed through `DoubleRecognition::failure`.
    names_failure: bool,
    /// Records the usage of the recognition once input has ended.
    records_usage: bool,
}

impl Behaviour {
    const STREAMING: Self = Self {
        overlaps: false,
        holds_output: false,
        fails: false,
        names_failure: true,
        records_usage: true,
    };
}

#[derive(Debug)]
struct Closed;

impl fmt::Display for Closed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the double's backend closed")
    }
}

impl Error for Closed {}

/// A recogniser written against the contract alone, in the shape of a hosted one.
struct Double {
    adapter: Adapter,
    behaviour: Behaviour,
    running: Arc<AtomicUsize>,
}

impl Double {
    fn new(behaviour: Behaviour) -> Self {
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "stt conformance double".to_owned(),
            contract_revision: CONTRACT_REVISION.to_owned(),
            execution_form: ExecutionForm::InProcess,
            locality: Locality::SameProcess,
            capabilities: vec![Capability::IncrementalInput, Capability::OutputStreaming],
            max_concurrency: 1,
        })
        .expect("one is positive");
        Self {
            adapter,
            behaviour,
            running: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl Recogniser for Double {
    type Handle = DoubleRecognition;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(
        &mut self,
        request: RecognitionRequest,
    ) -> Result<DoubleRecognition, ConcurrencyLimitReached> {
        if self.running.fetch_add(1, Ordering::AcqRel) >= 1 {
            self.running.fetch_sub(1, Ordering::AcqRel);
            return Err(ConcurrencyLimitReached { max_concurrency: 1 });
        }
        Ok(DoubleRecognition {
            output: TranscriptOutput::start(&self.adapter, request),
            behaviour: self.behaviour,
            taken: 0,
            spans: 0,
            failed: false,
            delivered: Vec::new(),
            running: Arc::clone(&self.running),
        })
    }
}

struct DoubleRecognition {
    output: TranscriptOutput,
    behaviour: Behaviour,
    taken: usize,
    spans: usize,
    failed: bool,
    delivered: Vec<EventEnvelope<TranscriptEvent>>,
    running: Arc<AtomicUsize>,
}

impl Drop for DoubleRecognition {
    fn drop(&mut self) {
        self.running.fetch_sub(1, Ordering::AcqRel);
    }
}

impl DoubleRecognition {
    fn emit(&mut self) {
        let mut push =
            |envelope: EventEnvelope<TranscriptEvent>| self.delivered.insert(0, envelope);
        let first = self
            .output
            .hypothesis(format!("word {}", self.spans))
            .unwrap();
        let first_id = first.payload.span_id().unwrap();
        push(first);
        if self.behaviour.overlaps {
            let second = self.output.hypothesis("overlapping".to_owned()).unwrap();
            let second_id = second.payload.span_id().unwrap();
            push(second);
            push(self.output.commit(first_id).unwrap());
            push(self.output.commit(second_id).unwrap());
        } else {
            push(self.output.commit(first_id).unwrap());
        }
        self.spans += 1;
    }

    fn advance(&mut self) {
        if self.failed {
            if let Ok(usage) = self.output.usage() {
                self.delivered.insert(0, usage);
            }
            return;
        }
        if let Ok(frames) = self.output.take_input() {
            self.taken += frames.len();
        }
        let ended = self.output.input_ended();
        if self.behaviour.holds_output && !ended {
            return;
        }
        while self.spans < self.taken / 40 {
            self.emit();
            if self.behaviour.fails {
                self.output.mark_failed();
                self.failed = true;
                break;
            }
        }
        if ended
            && self.delivered.is_empty()
            && self.behaviour.records_usage
            && let Ok(usage) = self.output.usage()
        {
            self.delivered.insert(0, usage);
        }
    }
}

impl Iterator for DoubleRecognition {
    type Item = EventEnvelope<TranscriptEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.delivered.is_empty() {
            self.advance();
        }
        self.delivered.pop()
    }
}

impl RecognitionHandle for DoubleRecognition {
    fn output(&self) -> &TranscriptOutput {
        &self.output
    }

    fn offer_frame(&mut self, frame: AudioFrame) -> Result<(), InputEnded> {
        self.output.offer(frame)
    }

    fn end_input(&mut self) -> Result<(), InputEnded> {
        self.output.end_input()
    }

    fn revise(&mut self, input: ReviseSpan) -> Result<(), RevisionRefused> {
        let envelope = self.output.revise(input)?;
        self.delivered.insert(0, envelope);
        Ok(())
    }
}

impl DoubleRecognition {
    /// Why the double's backend failed, as it displays.
    fn failure(&self) -> Option<String> {
        (self.failed && self.behaviour.names_failure).then(|| Closed.to_string())
    }
}
