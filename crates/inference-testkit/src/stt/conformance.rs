//! The backend-neutral checks of a streaming STT adapter, generic over any [`Recogniser`].
//!
//! An adapter crate runs them against its own recogniser, a hosted one over a local fixture
//! server: §23.1 test 1 of `docs/design/inference.md` (line 1105), STT revisions cannot rewrite
//! committed spans, the stream order of `ess/domains/stream.yaml`, and the one usage record of a
//! completed recognition (`ess/domains/usage.yaml`). There is one check per [`Item`]; each starts
//! its own recognitions, drops them before it returns, and answers with what it observed or a
//! [`Failure`] naming the item and the [`Fault`].
//!
//! Every event a check takes passes the same ledger: an envelope's sequence is the one after the
//! envelope before it, from the request's `first_sequence`; at most one span is open at a time;
//! spans commit in the order they were emitted; no hypothesis, revision or second commit of a
//! span follows its commit; nothing follows the usage record. A recogniser that keeps several
//! spans open at once is not held by these checks; none of this repository's does.
//!
//! The contract already refuses a span before the end of input without `output_streaming` and
//! input taken early without `incremental_input`. What it does not refuse is a claim the
//! recogniser never exercises; [`claims_observed`] does, reading the claims from the adapter
//! record only.
//!
//! The audio the checks offer is an [`Input`]: [`synthetic_audio`]'s tone
//! and silence unless the caller passes other frames. A hosted recogniser answers on its own
//! time, so before the end of input an iteration that yields nothing means only that nothing has
//! arrived yet: [`Input::patience`] is how long [`claims_observed`] keeps polling for streaming
//! output before it ends the input, zero for an in-process recogniser. [`Input::drain_bound`] is
//! how long the events after the end of input may take until the iteration ends; a recognition
//! that takes longer fails [`Item::EndOfInput`]. A handle that yields more than [`EVENT_LIMIT`]
//! events for one recognition fails the check that drives it instead of running forever.
//!
//! ```
//! # use inference_stt::inference_core::model::ModelId;
//! # use inference_testkit::stt::{MockConfiguration, MockRecogniser, script};
//! use inference_testkit::stt::conformance::{self, Input};
//! let mut recogniser =
//!     MockRecogniser::new(MockConfiguration::Incremental, ModelId::generate(), script());
//! let observed = conformance::claims_observed(&mut recogniser, &Input::default()).unwrap();
//! assert!(observed.output_streaming && observed.incremental_input);
//! ```

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use inference_stt::inference_audio::AudioFrame;
use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_core::model::Capability;
use inference_stt::inference_core::usage::{Quantity, QuantityKind};
use inference_stt::inference_text::{ReviseSpan, SpanId, TurnId};
use inference_stt::{
    ConcurrencyLimitReached, Recogniser, RecognitionHandle, RecognitionRequest, RevisionRefused,
    TranscriptEvent,
};

use super::synthetic_audio;

/// The most events the checks take from one recognition before they fail it as unbounded.
pub const EVENT_LIMIT: usize = 100_000;

/// The most recognitions [`cancellation`] runs at once. Up to it, the check asks that a start
/// beyond the adapter's declared maximum concurrency be refused; above it, it runs this many and
/// does not.
pub const CONCURRENCY_BOUND: usize = 64;

/// How long the checks sleep between two polls of a handle that has nothing yet.
pub const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The text an external revision of a committed span tries to write.
const REWRITE: &str = "rewritten after commit";

/// The audio the checks offer and how long they wait for a recogniser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    frames: Vec<AudioFrame>,
    /// How long [`claims_observed`] polls before the end of input for the output a claim names,
    /// once every frame is offered. Zero polls once.
    pub patience: Duration,
    /// How long the events after the end of input may take until the iteration ends.
    pub drain_bound: Duration,
}

impl Input {
    /// The input of `frames`, or `None` when there is none, with no patience and a drain bound
    /// of ten seconds.
    #[must_use]
    pub fn new(frames: Vec<AudioFrame>) -> Option<Self> {
        if frames.is_empty() {
            return None;
        }
        Some(Self {
            frames,
            patience: Duration::ZERO,
            drain_bound: Duration::from_secs(10),
        })
    }

    /// This input with another patience.
    #[must_use]
    pub const fn with_patience(mut self, patience: Duration) -> Self {
        self.patience = patience;
        self
    }

    /// This input with another drain bound.
    #[must_use]
    pub const fn with_drain_bound(mut self, drain_bound: Duration) -> Self {
        self.drain_bound = drain_bound;
        self
    }

    /// The frames, in the order they are offered.
    #[must_use]
    pub fn frames(&self) -> &[AudioFrame] {
        &self.frames
    }

    /// The milliseconds of audio of the frames: the samples per channel at each sample rate over
    /// that rate, summed exactly and rounded down once. None above `i64::MAX`.
    #[must_use]
    pub fn audio_ms(&self) -> Option<i64> {
        let mut samples: BTreeMap<u32, u128> = BTreeMap::new();
        for frame in &self.frames {
            *samples.entry(frame.format.sample_rate.get()).or_default() +=
                u128::from(frame.sample_count);
        }
        audio_ms(&samples)
    }
}

impl Default for Input {
    /// [`synthetic_audio::tone_silence_tone_silence`], no patience, a drain bound of ten seconds.
    fn default() -> Self {
        Self::new(synthetic_audio::tone_silence_tone_silence())
            .expect("the synthetic fixture has frames")
    }
}

/// An item of the STT adapter checks, one per check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Item {
    /// [`committed_final`]: no hypothesis, revision or second commit of a span follows its
    /// commit, and an external revision of a committed span is refused and changes nothing.
    CommittedFinal,
    /// [`spans_in_order`]: sequences are contiguous, at most one span is open at a time, and
    /// spans commit in the order they were emitted.
    SpansInOrder,
    /// [`end_of_input`]: the end of input ends the iteration within the drain bound, with every
    /// span committed, and it stays ended.
    EndOfInput,
    /// [`cancellation`]: a dropped handle releases its slot and revives nothing: the next
    /// recognition commits what a clean one of the same input does, and the declared maximum
    /// concurrency is held.
    Cancellation,
    /// [`backend_failure`]: a backend that closes or fails ends the iteration with a usage record
    /// that is not final, and the handle names a typed error.
    BackendFailure,
    /// [`usage_recorded`]: a completed recognition yields exactly one final usage record, its last
    /// event, whose audio milliseconds are all the audio offered, taken or not.
    UsageRecorded,
    /// [`claims_observed`]: `incremental_input` and `output_streaming` are claimed exactly when
    /// they are observed.
    ClaimsObserved,
}

/// What a check observed that the item does not allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// A start was refused while fewer recognitions ran than the adapter declares.
    StartRefused(ConcurrencyLimitReached),
    /// A frame was refused before the end of input.
    OfferRefused { frame: usize },
    /// The end of input was refused when it was first marked.
    EndRefused,
    /// The handle yielded more than [`EVENT_LIMIT`] events for one recognition.
    EventsUnbounded,
    /// An envelope carries `actual` where the sequence after the envelope before it is
    /// `expected`.
    SequenceGap { expected: i64, actual: i64 },
    /// An event names a span no hypothesis of this recognition emitted.
    UnheldSpan { span_id: SpanId },
    /// A hypothesis, revision or commit of a span arrived after the span was committed.
    MovedAfterCommit { span_id: SpanId },
    /// A hypothesis arrived while the span `open` was still open.
    SpansOverlap { open: SpanId, new: SpanId },
    /// The span `committed` was committed while `earlier`, emitted before it, was still open.
    CommittedOutOfOrder { earlier: SpanId, committed: SpanId },
    /// An external revision of a committed span was taken.
    CommittedRevised { span_id: SpanId },
    /// The text of a committed span changed after its commit.
    CommittedTextChanged { span_id: SpanId },
    /// The iteration had not ended `bound` after the end of input.
    DrainUnbounded { bound: Duration },
    /// An event was delivered after the iteration ended once input had ended.
    EventAfterEnd,
    /// Once input had ended and the iteration ended, the span was not committed.
    SpanLeftOpen { span_id: SpanId },
    /// No span was committed over the whole input.
    NoTranscript,
    /// The adapter claims `capability` and the recognition never did what it names.
    ClaimNotObserved { capability: Capability },
    /// The recognition did what `capability` names and the adapter does not claim it.
    ClaimMissing { capability: Capability },
    /// A start beyond the maximum concurrency the adapter declares succeeded.
    ConcurrencyNotHeld { max_concurrency: i64 },
    /// A start after a handle was dropped mid-recognition was refused: its slot was not released.
    SlotNotReleased(ConcurrencyLimitReached),
    /// A fresh recognition delivered an event of a span of the recognition dropped before it.
    StaleOutput { span_id: SpanId },
    /// The fresh recognition after a drop committed other texts than a clean baseline
    /// recognition of the same input.
    TranscriptDiffers {
        baseline: Vec<String>,
        fresh: Vec<String>,
    },
    /// Once the iteration ended, no usage record had been delivered.
    UsageMissing,
    /// A second usage record was delivered.
    UsageRepeated,
    /// An event followed the usage record.
    UsageNotLast,
    /// The usage record carries `actual` of `kind`, none when it carries not exactly one, where
    /// the check expected `expected`; `expected` is none for `local_compute_ms`, of which any one
    /// quantity, measured or unknown, is accepted.
    UsageQuantity {
        kind: QuantityKind,
        expected: Option<Quantity>,
        actual: Option<Quantity>,
    },
    /// The usage record names another adapter or model than the recogniser's adapter record.
    UsageOfAnotherAdapter,
    /// The usage record of a completed recognition is not final.
    UsageNotFinal,
    /// The usage record of a failed backend call is final.
    FailedUsageFinal,
    /// The backend was to fail and the handle names no error.
    NoFailureReported,
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// A check failed: the item it checks, and what it observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub item: Item,
    pub fault: Fault,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} failed: {}", self.item, self.fault)
    }
}

impl Error for Failure {}

/// What [`committed_final`] observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedObserved {
    /// The committed spans, in commit order, with their text.
    pub committed: Vec<(SpanId, String)>,
    /// The external revisions refused, one per committed span.
    pub revisions_refused: usize,
}

/// What [`spans_in_order`] observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderObserved {
    /// The span events delivered: hypotheses, revisions and commits.
    pub span_events: usize,
    /// The spans committed.
    pub committed: usize,
    /// The last sequence issued.
    pub last_sequence: i64,
}

/// What [`end_of_input`] observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndObserved {
    /// How long the iteration took to end after the end of input.
    pub drained_in: Duration,
    /// The spans committed.
    pub committed: usize,
}

/// What [`cancellation`] observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancellationObserved {
    /// The maximum concurrency the adapter declares.
    pub max_concurrency: i64,
    /// The frames the dropped recognition was offered before it was dropped.
    pub frames_before_drop: usize,
    /// The spans the fresh recognition committed, as many as the baseline did.
    pub committed: usize,
}

/// What [`backend_failure`] observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureObserved {
    /// The error the handle named, as it displays.
    pub error: String,
    /// The spans committed before the failure.
    pub committed: usize,
}

/// What [`usage_recorded`] observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageObserved {
    /// The `input_audio_ms` of the record, all the audio offered, taken or not. Beyond
    /// `i64::MAX` it reads `i64::MAX`; the record then carried them as unknown.
    pub audio_ms: i64,
    /// The `local_compute_ms` of the record, as the recogniser reported it.
    pub local_compute: Quantity,
}

/// What [`claims_observed`] observed the recogniser do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedClaims {
    /// Frames were taken before the end of input.
    pub incremental_input: bool,
    /// A span event reached the caller before the end of input.
    pub output_streaming: bool,
}

/// Checks [`Item::CommittedFinal`]: the whole input is offered and ended; whenever a commit is
/// delivered, an external revision of that span through [`RecognitionHandle::revise`] is refused,
/// as [`RevisionRefused::CommittedSpanImmutable`] or, once the usage record is made,
/// [`RevisionRefused::RecognitionEnded`], and once the iteration has ended every committed span
/// still reads the text it was committed with. The ledger holds every event to its order.
///
/// # Errors
///
/// A [`Failure`] naming [`Item::CommittedFinal`].
pub fn committed_final<R: Recogniser>(
    recogniser: &mut R,
    input: &Input,
) -> Result<CommittedObserved, Failure> {
    let mut driver = Driver::start(recogniser, Item::CommittedFinal)?;
    let mut committed = Vec::new();
    let mut refused = 0;
    let mut on_event = |driver: &mut Driver<R::Handle>,
                        envelope: &EventEnvelope<TranscriptEvent>|
     -> Result<(), Failure> {
        let TranscriptEvent::Committed(event) = &envelope.payload else {
            return Ok(());
        };
        let text = driver
            .handle
            .output()
            .span(event.span_id)
            .map(|span| span.text().to_owned())
            .unwrap_or_default();
        committed.push((event.span_id, text));
        match driver.handle.revise(ReviseSpan {
            span_id: event.span_id,
            text: REWRITE.to_owned(),
        }) {
            Err(
                RevisionRefused::CommittedSpanImmutable(_) | RevisionRefused::RecognitionEnded(_),
            ) => {
                refused += 1;
                Ok(())
            }
            _ => Err(driver.fail(Fault::CommittedRevised {
                span_id: event.span_id,
            })),
        }
    };
    driver.feed(input, &mut on_event)?;
    driver.end()?;
    driver.drain(input.drain_bound, &mut on_event)?;
    for (span_id, text) in &committed {
        let now = driver
            .handle
            .output()
            .span(*span_id)
            .map(|span| span.text());
        if now != Some(text.as_str()) {
            return Err(driver.fail(Fault::CommittedTextChanged { span_id: *span_id }));
        }
    }
    Ok(CommittedObserved {
        committed,
        revisions_refused: refused,
    })
}

/// Checks [`Item::SpansInOrder`]: the whole input is offered and ended, and every event delivered
/// passes the ledger; at least one span is committed.
///
/// # Errors
///
/// A [`Failure`] naming [`Item::SpansInOrder`].
pub fn spans_in_order<R: Recogniser>(
    recogniser: &mut R,
    input: &Input,
) -> Result<OrderObserved, Failure> {
    let mut driver = Driver::start(recogniser, Item::SpansInOrder)?;
    driver.feed(input, &mut ignore)?;
    driver.end()?;
    driver.drain(input.drain_bound, &mut ignore)?;
    let committed = driver.ledger.committed;
    if committed == 0 {
        return Err(driver.fail(Fault::NoTranscript));
    }
    Ok(OrderObserved {
        span_events: driver.ledger.span_events,
        committed,
        last_sequence: driver.ledger.next_sequence - 1,
    })
}

/// Checks [`Item::EndOfInput`]: the whole input is offered and ended; the iteration ends within
/// [`Input::drain_bound`], every span the recognition emitted is committed, and a further
/// iteration yields nothing. The bound holds while the iteration waits: the drain runs on a
/// thread of its own, and a handle that blocks past the bound fails the check when the bound
/// passes, its thread left to end on its own.
///
/// # Errors
///
/// A [`Failure`] naming [`Item::EndOfInput`].
pub fn end_of_input<R>(recogniser: &mut R, input: &Input) -> Result<EndObserved, Failure>
where
    R: Recogniser,
    R::Handle: Send + 'static,
{
    let mut driver = Driver::start(recogniser, Item::EndOfInput)?;
    driver.feed(input, &mut ignore)?;
    driver.end()?;
    let (mut driver, drained_in) = driver.drain_within(input.drain_bound)?;
    if let Some(&span_id) = driver.ledger.open.first() {
        return Err(driver.fail(Fault::SpanLeftOpen { span_id }));
    }
    if driver.handle.next().is_some() {
        return Err(driver.fail(Fault::EventAfterEnd));
    }
    Ok(EndObserved {
        drained_in,
        committed: driver.ledger.committed,
    })
}

/// Checks [`Item::Cancellation`]: first a clean baseline recognition is offered the whole input,
/// ended and drained, and its committed texts kept. Then as many recognitions as the adapter
/// declares run at once, up to [`CONCURRENCY_BOUND`], and, within that bound, one more start is
/// refused; one of them, offered the first half of the input and stepped once, is dropped
/// mid-recognition; the next start succeeds, and that fresh recognition, offered the whole input
/// and ended, delivers no event of a span of the dropped one, passes the ledger from its own first
/// sequence, ends with its usage record and commits exactly the baseline's texts in the same
/// order: words of the dropped recognition carried into it under fresh span ids fail it. The
/// recogniser must transcribe the same input the same way twice, as an in-process model or a
/// scripted fixture server does. A cancel is the drop: the contract has no other.
///
/// # Errors
///
/// A [`Failure`] naming [`Item::Cancellation`].
pub fn cancellation<R: Recogniser>(
    recogniser: &mut R,
    input: &Input,
) -> Result<CancellationObserved, Failure> {
    let item = Item::Cancellation;
    let fail = |fault| Failure { item, fault };
    let baseline = {
        let mut driver = Driver::start(recogniser, item)?;
        driver.feed(input, &mut ignore)?;
        driver.end()?;
        driver.drain(input.drain_bound, &mut ignore)?;
        if driver.ledger.usage.is_none() {
            return Err(driver.fail(Fault::UsageMissing));
        }
        driver.committed_texts()
    };
    let max_concurrency = recogniser.adapter().max_concurrency();
    let declared = usize::try_from(max_concurrency).unwrap_or(1).max(1);
    let mut running = Vec::with_capacity(declared.min(CONCURRENCY_BOUND));
    while running.len() < declared.min(CONCURRENCY_BOUND) {
        running.push(
            recogniser
                .start(request())
                .map_err(|refusal| fail(Fault::StartRefused(refusal)))?,
        );
    }
    if declared <= CONCURRENCY_BOUND && recogniser.start(request()).is_ok() {
        return Err(fail(Fault::ConcurrencyNotHeld { max_concurrency }));
    }
    let mut dropped = running.pop().expect("at least one recognition runs");
    let half = input.frames.len().div_ceil(2);
    for (index, frame) in input.frames.iter().take(half).enumerate() {
        dropped
            .offer_frame(frame.clone())
            .map_err(|_| fail(Fault::OfferRefused { frame: index }))?;
    }
    let before_drop: Vec<SpanId> = dropped
        .next()
        .and_then(|envelope| envelope.payload.span_id())
        .into_iter()
        .chain(dropped.output().spans().iter().map(|span| span.span_id()))
        .collect();
    drop(dropped);
    let fresh = recogniser
        .start(request())
        .map_err(|refusal| fail(Fault::SlotNotReleased(refusal)))?;
    let mut driver = Driver::new(fresh, item);
    let mut stale = |driver: &mut Driver<R::Handle>,
                     envelope: &EventEnvelope<TranscriptEvent>|
     -> Result<(), Failure> {
        match envelope.payload.span_id() {
            Some(span_id) if before_drop.contains(&span_id) => {
                Err(driver.fail(Fault::StaleOutput { span_id }))
            }
            _ => Ok(()),
        }
    };
    driver.feed(input, &mut stale)?;
    driver.end()?;
    driver.drain(input.drain_bound, &mut stale)?;
    if driver.ledger.usage.is_none() {
        return Err(driver.fail(Fault::UsageMissing));
    }
    let fresh = driver.committed_texts();
    if fresh != baseline {
        return Err(driver.fail(Fault::TranscriptDiffers { baseline, fresh }));
    }
    drop(running);
    Ok(CancellationObserved {
        max_concurrency,
        frames_before_drop: half,
        committed: fresh.len(),
    })
}

/// Checks [`Item::BackendFailure`] on a recogniser whose backend the caller has made fail, by
/// closing or refusing the connection or answering with an error: the whole input is offered and
/// ended; the iteration ends within [`Input::drain_bound`] and its last event is a usage record
/// that is not final; the handle then names the error, which `failure` reads from it as it
/// displays, none while the handle names none. The adapter's own tests hold the error to its type.
/// Every event passes the ledger.
///
/// # Errors
///
/// A [`Failure`] naming [`Item::BackendFailure`].
pub fn backend_failure<R, F>(
    recogniser: &mut R,
    input: &Input,
    failure: F,
) -> Result<FailureObserved, Failure>
where
    R: Recogniser,
    F: Fn(&R::Handle) -> Option<String>,
{
    let mut driver = Driver::start(recogniser, Item::BackendFailure)?;
    driver.feed(input, &mut ignore)?;
    driver.end()?;
    driver.drain(input.drain_bound, &mut ignore)?;
    let Some(record) = driver.ledger.usage.clone() else {
        return Err(driver.fail(Fault::UsageMissing));
    };
    if record.r#final {
        return Err(driver.fail(Fault::FailedUsageFinal));
    }
    let Some(error) = failure(&driver.handle) else {
        return Err(driver.fail(Fault::NoFailureReported));
    };
    Ok(FailureObserved {
        error,
        committed: driver.ledger.committed,
    })
}

/// Checks [`Item::UsageRecorded`]: the whole input is offered and ended, and once the iteration
/// has ended exactly one event was a usage record, the last. It names the adapter and the model
/// of the recogniser's adapter record and is final; it carries exactly one quantity of each kind
/// it reports: `input_audio_ms`, `measured`, the audio of every frame offered, taken or not,
/// never a zero that stands for audio the record did not count; `requests`, `measured` 1; and
/// `local_compute_ms`,
/// measured or unknown as the recogniser reports it.
///
/// # Errors
///
/// A [`Failure`] naming [`Item::UsageRecorded`].
pub fn usage_recorded<R: Recogniser>(
    recogniser: &mut R,
    input: &Input,
) -> Result<UsageObserved, Failure> {
    let adapter = recogniser.adapter().clone();
    let mut driver = Driver::start(recogniser, Item::UsageRecorded)?;
    driver.feed(input, &mut ignore)?;
    driver.end()?;
    driver.drain(input.drain_bound, &mut ignore)?;
    let Some(record) = driver.ledger.usage.clone() else {
        return Err(driver.fail(Fault::UsageMissing));
    };
    if record.adapter_id != adapter.adapter_id() || record.model_id != adapter.model_id() {
        return Err(driver.fail(Fault::UsageOfAnotherAdapter));
    }
    if !record.r#final {
        return Err(driver.fail(Fault::UsageNotFinal));
    }
    let one = |kind: QuantityKind| {
        let mut found = record
            .quantities
            .iter()
            .filter(|quantity| quantity.kind() == kind);
        match (found.next(), found.next()) {
            (Some(&quantity), None) => Some(quantity),
            _ => None,
        }
    };
    let audio = measured(QuantityKind::InputAudioMs, input.audio_ms());
    for expected in [audio, Quantity::measured(QuantityKind::Requests, 1)] {
        let actual = one(expected.kind());
        if actual != Some(expected) {
            return Err(driver.fail(Fault::UsageQuantity {
                kind: expected.kind(),
                expected: Some(expected),
                actual,
            }));
        }
    }
    let Some(local_compute) = one(QuantityKind::LocalComputeMs) else {
        return Err(driver.fail(Fault::UsageQuantity {
            kind: QuantityKind::LocalComputeMs,
            expected: None,
            actual: None,
        }));
    };
    Ok(UsageObserved {
        audio_ms: audio.value().unwrap_or(i64::MAX),
        local_compute,
    })
}

/// Checks [`Item::ClaimsObserved`]: the frames are offered one at a time, every event delivered
/// after each offer taken before the next; then, for up to [`Input::patience`], the handle is
/// polled until each claimed capability has been observed; then input ends and the rest is
/// taken. At least one span is committed. Frames are taken before the end of input exactly when
/// the adapter claims [`Capability::IncrementalInput`], and a span event reaches the caller
/// before it exactly when it claims [`Capability::OutputStreaming`]. The claims are read from
/// [`Recogniser::adapter`] only.
///
/// # Errors
///
/// A [`Failure`] naming [`Item::ClaimsObserved`]; a claim not observed is
/// [`Fault::ClaimNotObserved`] naming the capability, `incremental_input` checked first.
pub fn claims_observed<R: Recogniser>(
    recogniser: &mut R,
    input: &Input,
) -> Result<ObservedClaims, Failure> {
    let claimed = recogniser.adapter().capabilities().to_vec();
    let mut driver = Driver::start(recogniser, Item::ClaimsObserved)?;
    let mut streamed = false;
    let mut before_end = |_: &mut Driver<R::Handle>,
                          envelope: &EventEnvelope<TranscriptEvent>|
     -> Result<(), Failure> {
        streamed |= envelope.payload.span_id().is_some();
        Ok(())
    };
    driver.feed(input, &mut before_end)?;
    let deadline = Instant::now() + input.patience;
    loop {
        let observed = (
            driver.handle.output().frames_taken() > 0,
            driver.ledger.span_events > 0,
        );
        let satisfied = (!claimed.contains(&Capability::IncrementalInput) || observed.0)
            && (!claimed.contains(&Capability::OutputStreaming) || observed.1);
        if satisfied || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
        driver.poll(&mut before_end)?;
    }
    let observed = ObservedClaims {
        incremental_input: driver.handle.output().frames_taken() > 0,
        output_streaming: streamed,
    };
    driver.end()?;
    driver.drain(input.drain_bound, &mut ignore)?;
    if driver.ledger.committed == 0 {
        return Err(driver.fail(Fault::NoTranscript));
    }
    for (capability, seen) in [
        (Capability::IncrementalInput, observed.incremental_input),
        (Capability::OutputStreaming, observed.output_streaming),
    ] {
        match (claimed.contains(&capability), seen) {
            (true, false) => return Err(driver.fail(Fault::ClaimNotObserved { capability })),
            (false, true) => return Err(driver.fail(Fault::ClaimMissing { capability })),
            _ => {}
        }
    }
    Ok(observed)
}

/// What every check holds every delivered event to.
#[derive(Debug)]
struct Ledger {
    next_sequence: i64,
    /// The spans emitted and not committed, in emission order.
    open: Vec<SpanId>,
    committed_spans: Vec<SpanId>,
    committed: usize,
    span_events: usize,
    usage: Option<inference_stt::inference_core::usage::UsageRecorded>,
}

impl Ledger {
    const fn new(first_sequence: i64) -> Self {
        Self {
            next_sequence: first_sequence,
            open: Vec::new(),
            committed_spans: Vec::new(),
            committed: 0,
            span_events: 0,
            usage: None,
        }
    }

    fn take(&mut self, envelope: &EventEnvelope<TranscriptEvent>) -> Result<(), Fault> {
        if envelope.sequence != self.next_sequence {
            return Err(Fault::SequenceGap {
                expected: self.next_sequence,
                actual: envelope.sequence,
            });
        }
        self.next_sequence = self.next_sequence.saturating_add(1);
        if self.usage.is_some() {
            return Err(match envelope.payload {
                TranscriptEvent::Usage(_) => Fault::UsageRepeated,
                _ => Fault::UsageNotLast,
            });
        }
        let after_commit = |span_id| {
            if self.committed_spans.contains(&span_id) {
                Err(Fault::MovedAfterCommit { span_id })
            } else {
                Ok(())
            }
        };
        match &envelope.payload {
            TranscriptEvent::Usage(record) => {
                self.usage = Some(record.clone());
                return Ok(());
            }
            TranscriptEvent::Hypothesis(event) => {
                after_commit(event.span_id)?;
                if let Some(&open) = self.open.first() {
                    return Err(Fault::SpansOverlap {
                        open,
                        new: event.span_id,
                    });
                }
                self.open.push(event.span_id);
            }
            TranscriptEvent::Revision(event) => {
                after_commit(event.span_id)?;
                if !self.open.contains(&event.span_id) {
                    return Err(Fault::UnheldSpan {
                        span_id: event.span_id,
                    });
                }
            }
            TranscriptEvent::Committed(event) => {
                after_commit(event.span_id)?;
                match self.open.first() {
                    None => {
                        return Err(Fault::UnheldSpan {
                            span_id: event.span_id,
                        });
                    }
                    Some(&earlier) if earlier != event.span_id => {
                        return Err(if self.open.contains(&event.span_id) {
                            Fault::CommittedOutOfOrder {
                                earlier,
                                committed: event.span_id,
                            }
                        } else {
                            Fault::UnheldSpan {
                                span_id: event.span_id,
                            }
                        });
                    }
                    Some(_) => {}
                }
                self.open.remove(0);
                self.committed_spans.push(event.span_id);
                self.committed += 1;
            }
        }
        self.span_events += 1;
        Ok(())
    }
}

/// What a check does with each event it takes, after the ledger has passed it.
type OnEvent<'a, H> =
    dyn FnMut(&mut Driver<H>, &EventEnvelope<TranscriptEvent>) -> Result<(), Failure> + 'a;

fn ignore<H>(_: &mut Driver<H>, _: &EventEnvelope<TranscriptEvent>) -> Result<(), Failure> {
    Ok(())
}

/// One recognition a check drives, counting the events it delivered.
struct Driver<H> {
    handle: H,
    item: Item,
    events: usize,
    ledger: Ledger,
}

impl<H: RecognitionHandle> Driver<H> {
    fn start<R: Recogniser<Handle = H>>(recogniser: &mut R, item: Item) -> Result<Self, Failure> {
        let handle = recogniser.start(request()).map_err(|refusal| Failure {
            item,
            fault: Fault::StartRefused(refusal),
        })?;
        Ok(Self::new(handle, item))
    }

    fn new(handle: H, item: Item) -> Self {
        let first = handle.output().request().first_sequence();
        Self {
            handle,
            item,
            events: 0,
            ledger: Ledger::new(first),
        }
    }

    const fn fail(&self, fault: Fault) -> Failure {
        Failure {
            item: self.item,
            fault,
        }
    }

    /// The next event delivered, held to the ledger.
    fn next(&mut self) -> Result<Option<EventEnvelope<TranscriptEvent>>, Failure> {
        let Some(envelope) = self.handle.next() else {
            return Ok(None);
        };
        self.events += 1;
        if self.events > EVENT_LIMIT {
            return Err(self.fail(Fault::EventsUnbounded));
        }
        self.ledger
            .take(&envelope)
            .map_err(|fault| self.fail(fault))?;
        Ok(Some(envelope))
    }

    /// Takes every event delivered now, handing each to `on_event`.
    fn poll(&mut self, on_event: &mut OnEvent<'_, H>) -> Result<(), Failure> {
        while let Some(envelope) = self.next()? {
            on_event(self, &envelope)?;
        }
        Ok(())
    }

    /// Offers every frame of `input`, polling after each.
    fn feed(&mut self, input: &Input, on_event: &mut OnEvent<'_, H>) -> Result<(), Failure> {
        for (index, frame) in input.frames.iter().enumerate() {
            self.handle
                .offer_frame(frame.clone())
                .map_err(|_| self.fail(Fault::OfferRefused { frame: index }))?;
            self.poll(on_event)?;
        }
        Ok(())
    }

    fn end(&mut self) -> Result<(), Failure> {
        self.handle
            .end_input()
            .map_err(|_| self.fail(Fault::EndRefused))
    }

    /// Takes every event after the end of input until the iteration ends, failing it when that
    /// took longer than `bound`. The bound is read once the iteration has ended; only
    /// [`Driver::drain_within`] holds it while the iteration waits.
    fn drain(&mut self, bound: Duration, on_event: &mut OnEvent<'_, H>) -> Result<(), Failure> {
        let started = Instant::now();
        self.poll(on_event)?;
        if started.elapsed() > bound {
            return Err(self.fail(Fault::DrainUnbounded { bound }));
        }
        Ok(())
    }

    /// The texts of the committed spans, in commit order.
    fn committed_texts(&self) -> Vec<String> {
        self.ledger
            .committed_spans
            .iter()
            .map(|&span_id| {
                self.handle
                    .output()
                    .span(span_id)
                    .map(|span| span.text().to_owned())
                    .unwrap_or_default()
            })
            .collect()
    }
}

impl<H: RecognitionHandle + Send + 'static> Driver<H> {
    /// Takes every event after the end of input until the iteration ends, on a thread of its
    /// own, and fails when it has not ended within `bound`, while it still waits: the thread is
    /// then left to end on its own. Answers the driver back and how long the drain took.
    fn drain_within(self, bound: Duration) -> Result<(Self, Duration), Failure> {
        let item = self.item;
        let started = Instant::now();
        let (sender, receiver) = mpsc::channel();
        let drain = std::thread::spawn(move || {
            let mut driver = self;
            let outcome = driver.poll(&mut ignore);
            let _ = sender.send((driver, outcome));
        });
        match receiver.recv_timeout(bound) {
            Ok((driver, outcome)) => {
                outcome?;
                let drained_in = started.elapsed();
                if drained_in > bound {
                    return Err(driver.fail(Fault::DrainUnbounded { bound }));
                }
                Ok((driver, drained_in))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Err(Failure {
                item,
                fault: Fault::DrainUnbounded { bound },
            }),
            Err(mpsc::RecvTimeoutError::Disconnected) => match drain.join() {
                Err(panic) => std::panic::resume_unwind(panic),
                Ok(()) => unreachable!("the drain thread answers before it ends"),
            },
        }
    }
}

/// A fresh request on a fresh stream, run, turn and node.
fn request() -> RecognitionRequest {
    RecognitionRequest::new(
        RunId::generate(),
        TurnId::generate(),
        StreamId::generate(),
        NodeId::generate(),
        "text_out",
    )
}

/// `count` of `kind`, measured; unknown when there is none.
fn measured(kind: QuantityKind, count: Option<i64>) -> Quantity {
    count.map_or(Quantity::unknown(kind), |count| {
        Quantity::measured(kind, count)
    })
}

/// The milliseconds of `samples`, samples per channel by sample rate: each rate's samples over
/// that rate, summed exactly and rounded down once; should the sum not fit `u128`, each rate's
/// milliseconds rounded down on their own and summed. None above `i64::MAX`.
fn audio_ms(samples: &BTreeMap<u32, u128>) -> Option<i64> {
    let rates = samples
        .keys()
        .try_fold(1_u128, |common, &rate| lcm(common, u128::from(rate)));
    let exact = rates.and_then(|common| {
        samples
            .iter()
            .try_fold(0_u128, |sum, (&rate, &count)| {
                sum.checked_add(
                    count
                        .checked_mul(1_000)?
                        .checked_mul(common / u128::from(rate))?,
                )
            })
            .map(|sum| sum / common)
    });
    let ms = exact.unwrap_or_else(|| {
        samples
            .iter()
            .map(|(&rate, &count)| count.saturating_mul(1_000) / u128::from(rate))
            .fold(0, u128::saturating_add)
    });
    i64::try_from(ms).ok()
}

/// The least common multiple of `a` and `b`, both above 0; none when it does not fit `u128`.
fn lcm(a: u128, b: u128) -> Option<u128> {
    let (mut x, mut y) = (a, b);
    while y != 0 {
        (x, y) = (y, x % y);
    }
    a.checked_mul(b / x)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default input is the synthetic tone and silence, 1.6 s of 16 kHz audio, and an input
    /// is never empty.
    #[test]
    fn the_default_input_is_the_synthetic_fixture_and_an_input_is_never_empty() {
        let input = Input::default();
        assert_eq!(input.frames().len(), 160);
        assert_eq!(input.audio_ms(), Some(1_600));
        assert!(Input::new(Vec::new()).is_none());
    }

    /// Milliseconds above `i64::MAX` are expected unknown, never clamped.
    #[test]
    fn milliseconds_above_i64_max_are_none() {
        let largest = u128::try_from(i64::MAX).expect("i64::MAX fits a u128");
        assert_eq!(audio_ms(&BTreeMap::from([(1_000, largest + 1)])), None);
        assert_eq!(
            audio_ms(&BTreeMap::from([(1_000, largest)])),
            Some(i64::MAX)
        );
    }
}
