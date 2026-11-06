//! Adversary pass 1 on the backend-neutral STT checks: one deliberately broken recogniser per
//! item of `inference_testkit::stt::conformance`, each run through the check that names it. A
//! check must go red for its mutant; a mutant a check lets through is a check that cannot fail
//! for the defect it names.

#![cfg(feature = "stt")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use inference_core::envelope::EventEnvelope;
use inference_core::model::{
    Adapter, AdapterDescriptor, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_stt::inference_audio::AudioFrame;
use inference_stt::{
    CONTRACT_REVISION, ConcurrencyLimitReached, InputEnded, Recogniser, RecognitionHandle,
    RecognitionRequest, RevisionRefused, TranscriptEvent, TranscriptOutput,
};
use inference_testkit::stt::conformance::{self, Fault, Input, Item};
use inference_text::ReviseSpan;

/// The one defect a [`Mutant`] carries; `None` is a recogniser every check accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Defect {
    None,
    /// `revise` of a committed span answers `Ok` and delivers nothing.
    AcceptsRevisionOfCommitted,
    /// The first hypothesis is consumed from the output and never delivered.
    SwallowsAnEvent,
    /// Once input has ended, the first iteration with nothing queued yields `None`, and the
    /// usage record follows on the next one.
    EndsEarly,
    /// Once input has ended, every iteration sleeps this long first.
    SlowDrain(Duration),
    /// The text of every span a dropped recognition emitted is replayed, as fresh spans, at the
    /// start of the next recognition of the same recogniser.
    LeaksIntoNext,
    /// Fails after its first span, names why, and never delivers a usage record.
    FailsWithoutUsage,
    /// Takes the frames it is offered and never hands them to the contract's output.
    DropsAudio,
    /// Claims `incremental_input` and takes input only once it has ended.
    TakesLate,
}

struct Mutant {
    adapter: Adapter,
    defect: Defect,
    running: Arc<AtomicUsize>,
    leaked: Arc<Mutex<Vec<String>>>,
}

impl Mutant {
    fn new(defect: Defect) -> Self {
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "adversary mutant".to_owned(),
            contract_revision: CONTRACT_REVISION.to_owned(),
            execution_form: ExecutionForm::InProcess,
            locality: Locality::SameProcess,
            capabilities: vec![Capability::IncrementalInput, Capability::OutputStreaming],
            max_concurrency: 1,
            descriptor: AdapterDescriptor::default(),
        })
        .expect("one is positive");
        Self {
            adapter,
            defect,
            running: Arc::new(AtomicUsize::new(0)),
            leaked: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Recogniser for Mutant {
    type Handle = MutantRecognition;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(
        &mut self,
        request: RecognitionRequest,
    ) -> Result<MutantRecognition, ConcurrencyLimitReached> {
        if self.running.fetch_add(1, Ordering::AcqRel) >= 1 {
            self.running.fetch_sub(1, Ordering::AcqRel);
            return Err(ConcurrencyLimitReached { max_concurrency: 1 });
        }
        let replay = std::mem::take(&mut *self.leaked.lock().unwrap());
        Ok(MutantRecognition {
            output: TranscriptOutput::start(&self.adapter, request),
            defect: self.defect,
            taken: 0,
            spans: 0,
            failed: false,
            swallowed: false,
            ended_early: false,
            replay,
            queue: Vec::new(),
            running: Arc::clone(&self.running),
            leaked: Arc::clone(&self.leaked),
        })
    }
}

struct MutantRecognition {
    output: TranscriptOutput,
    defect: Defect,
    taken: usize,
    spans: usize,
    failed: bool,
    swallowed: bool,
    ended_early: bool,
    replay: Vec<String>,
    queue: Vec<EventEnvelope<TranscriptEvent>>,
    running: Arc<AtomicUsize>,
    leaked: Arc<Mutex<Vec<String>>>,
}

impl Drop for MutantRecognition {
    fn drop(&mut self) {
        if self.defect == Defect::LeaksIntoNext {
            let texts = self
                .output
                .spans()
                .iter()
                .map(|span| span.text().to_owned());
            self.leaked.lock().unwrap().extend(texts);
        }
        self.running.fetch_sub(1, Ordering::AcqRel);
    }
}

impl MutantRecognition {
    fn push(&mut self, envelope: EventEnvelope<TranscriptEvent>) {
        self.queue.insert(0, envelope);
    }

    fn span(&mut self, text: String) {
        let hypothesis = self.output.hypothesis(text).unwrap();
        let span_id = hypothesis.payload.span_id().unwrap();
        if self.defect == Defect::SwallowsAnEvent && !self.swallowed {
            self.swallowed = true;
        } else {
            self.push(hypothesis);
        }
        let commit = self.output.commit(span_id).unwrap();
        self.push(commit);
    }

    fn advance(&mut self) {
        if self.failed {
            return;
        }
        for text in std::mem::take(&mut self.replay) {
            self.span(text);
        }
        let ended = self.output.input_ended();
        if (self.defect != Defect::TakesLate || ended)
            && let Ok(frames) = self.output.take_input()
        {
            self.taken += frames.len();
        }
        while self.spans < self.taken / 40 {
            let text = format!("word {}", self.spans);
            self.span(text);
            self.spans += 1;
            if self.defect == Defect::FailsWithoutUsage {
                self.output.mark_failed();
                self.failed = true;
                return;
            }
        }
        if ended && self.queue.is_empty() {
            if self.defect == Defect::EndsEarly && !self.ended_early {
                self.ended_early = true;
                return;
            }
            if let Ok(usage) = self.output.usage() {
                self.push(usage);
            }
        }
    }
}

impl Iterator for MutantRecognition {
    type Item = EventEnvelope<TranscriptEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Defect::SlowDrain(pause) = self.defect
            && self.output.input_ended()
        {
            std::thread::sleep(pause);
        }
        if self.queue.is_empty() {
            self.advance();
        }
        self.queue.pop()
    }
}

impl RecognitionHandle for MutantRecognition {
    fn output(&self) -> &TranscriptOutput {
        &self.output
    }

    fn offer_frame(&mut self, frame: AudioFrame) -> Result<(), InputEnded> {
        if self.defect == Defect::DropsAudio {
            return if self.output.input_ended() {
                Err(InputEnded)
            } else {
                Ok(())
            };
        }
        self.output.offer(frame)
    }

    fn end_input(&mut self) -> Result<(), InputEnded> {
        self.output.end_input()
    }

    fn revise(&mut self, input: ReviseSpan) -> Result<(), RevisionRefused> {
        if self.defect == Defect::AcceptsRevisionOfCommitted {
            return Ok(());
        }
        let envelope = self.output.revise(input)?;
        self.push(envelope);
        Ok(())
    }
}

fn input() -> Input {
    Input::default().with_drain_bound(Duration::from_millis(200))
}

#[test]
fn adversary1_the_unmutated_double_passes_every_check() {
    let make = || Mutant::new(Defect::None);
    conformance::committed_final(&mut make(), &input()).unwrap();
    conformance::spans_in_order(&mut make(), &input()).unwrap();
    conformance::end_of_input(&mut make(), &input()).unwrap();
    conformance::cancellation(&mut make(), &input()).unwrap();
    conformance::usage_recorded(&mut make(), &input()).unwrap();
    conformance::claims_observed(&mut make(), &input()).unwrap();
}

#[test]
fn adversary1_committed_final_rejects_a_handle_that_accepts_a_revision_of_a_commit() {
    let failure = conformance::committed_final(
        &mut Mutant::new(Defect::AcceptsRevisionOfCommitted),
        &input(),
    )
    .expect_err("the mutant takes a revision of a committed span");
    assert_eq!(failure.item, Item::CommittedFinal);
    assert!(
        matches!(failure.fault, Fault::CommittedRevised { .. }),
        "{failure}"
    );
}

#[test]
fn adversary1_spans_in_order_rejects_a_handle_that_swallows_an_event() {
    let failure = conformance::spans_in_order(&mut Mutant::new(Defect::SwallowsAnEvent), &input())
        .expect_err("the mutant never delivers its first hypothesis");
    assert_eq!(failure.item, Item::SpansInOrder);
    assert!(
        matches!(failure.fault, Fault::SequenceGap { .. }),
        "{failure}"
    );
}

#[test]
fn adversary1_end_of_input_rejects_an_iteration_that_ends_before_the_usage_record() {
    let failure = conformance::end_of_input(&mut Mutant::new(Defect::EndsEarly), &input())
        .expect_err("the mutant yields None and then its usage record");
    assert_eq!(failure.item, Item::EndOfInput);
    assert_eq!(failure.fault, Fault::EventAfterEnd);
}

#[test]
fn adversary1_end_of_input_rejects_a_drain_slower_than_its_bound() {
    let failure = conformance::end_of_input(
        &mut Mutant::new(Defect::SlowDrain(Duration::from_millis(150))),
        &input(),
    )
    .expect_err("each iteration after the end of input sleeps 150 ms");
    assert_eq!(failure.item, Item::EndOfInput);
    assert!(
        matches!(failure.fault, Fault::DrainUnbounded { .. }),
        "{failure}"
    );
}

/// The drain bound is measured after the drain returns, so a handle that blocks holds the check
/// for as long as it blocks: the check cannot fail a hang, only report lateness once it is over.
#[test]
fn adversary1_end_of_input_answers_within_its_drain_bound_when_the_handle_blocks() {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = conformance::end_of_input(
            &mut Mutant::new(Defect::SlowDrain(Duration::from_secs(4))),
            &input(),
        );
        let _ = sender.send(outcome.map(|_| ()));
    });
    let answered = receiver.recv_timeout(Duration::from_secs(2));
    assert!(
        answered.is_ok(),
        "the check had not answered 2 s after a drain bound of 200 ms: a blocking handle holds it"
    );
}

/// A recogniser that replays the transcript of a dropped recognition into the next one, as a
/// shared connection or a shared result queue would, under span ids the fresh recognition issued.
#[test]
fn adversary1_cancellation_rejects_a_fresh_recognition_that_carries_the_dropped_ones_text() {
    let mut mutant = Mutant::new(Defect::LeaksIntoNext);
    let outcome = conformance::cancellation(&mut mutant, &input());
    assert!(
        outcome.is_err(),
        "the fresh recognition committed the dropped recognition's words first and the check passed: {outcome:?}"
    );
}

#[test]
fn adversary1_backend_failure_rejects_a_failure_with_no_usage_record() {
    let failure = conformance::backend_failure(
        &mut Mutant::new(Defect::FailsWithoutUsage),
        &input(),
        |handle| handle.failed.then(|| "closed".to_owned()),
    )
    .expect_err("the mutant fails and records nothing");
    assert_eq!(failure.item, Item::BackendFailure);
    assert_eq!(failure.fault, Fault::UsageMissing);
}

#[test]
fn adversary1_usage_recorded_rejects_a_record_that_counts_no_audio() {
    let failure = conformance::usage_recorded(&mut Mutant::new(Defect::DropsAudio), &input())
        .expect_err("the mutant drops every frame before the contract counts it");
    assert_eq!(failure.item, Item::UsageRecorded);
    assert!(
        matches!(
            failure.fault,
            Fault::UsageQuantity {
                kind: inference_core::usage::QuantityKind::InputAudioMs,
                ..
            }
        ),
        "{failure}"
    );
}

#[test]
fn adversary1_claims_observed_rejects_incremental_input_claimed_and_never_done() {
    let failure = conformance::claims_observed(&mut Mutant::new(Defect::TakesLate), &input())
        .expect_err("the mutant takes its input only after the end");
    assert_eq!(failure.item, Item::ClaimsObserved);
    assert_eq!(
        failure.fault,
        Fault::ClaimNotObserved {
            capability: Capability::IncrementalInput
        }
    );
}
