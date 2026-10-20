//! The backend-neutral checks of the TTS adapter exit criteria, generic over any [`Synthesiser`],
//! with [`MockSink`] as the media sink.
//!
//! An adapter crate runs them against its own synthesiser: §23.1 test 3 of
//! `docs/design/inference.md` (line 1107), the scenarios
//! `inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.QueueForSynthesis` and
//! `inference.stream.OutputSegment/transition/play/by/inference.stream.AcknowledgePlayback/played`,
//! and §23.2 (lines 1114-1117). There is one check per exit [`Item`]; each starts its own
//! syntheses, drops them before it returns, and answers with what it observed or a [`Failure`]
//! naming the item and the [`Fault`].
//!
//! The contract already refuses a frame of a Queued segment without `output_streaming` and input
//! before the end of the utterance without `incremental_input`. What it does not refuse is a claim
//! the synthesiser never exercises; [`claims_observed`] does, reading the claims from the adapter
//! record only.
//!
//! The text the checks speak is an [`Utterance`], [`DEFAULT_SENTENCES`] unless the caller passes
//! another, so a real backend is driven with words. A handle that yields more than
//! [`EVENT_LIMIT`] events for one synthesis fails the check that drives it instead of running
//! forever.
//!
//! ```
//! # use std::num::{NonZeroU16, NonZeroU32};
//! # use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
//! # use inference_tts::inference_core::model::ModelId;
//! # use inference_testkit::tts::{MockConfiguration, MockSynthesiser};
//! use inference_testkit::tts::conformance::{self, Utterance};
//! # let format = AudioFormat {
//! #     encoding: AudioEncoding::PcmS16le,
//! #     sample_rate: NonZeroU32::new(16_000).unwrap(),
//! #     channels: NonZeroU16::new(1).unwrap(),
//! # };
//! let mut synthesiser =
//!     MockSynthesiser::new(MockConfiguration::OutputStreamingOnly, ModelId::generate(), format);
//! let observed = conformance::claims_observed(&mut synthesiser, &Utterance::default()).unwrap();
//! assert!(observed.output_streaming && !observed.incremental_input);
//! ```

use std::error::Error;
use std::fmt;

use inference_tts::inference_audio::AudioFormat;
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_core::model::Capability;
use inference_tts::inference_text::{OutputSegment, SegmentId, SegmentState, SegmentStateConflict};
use inference_tts::{
    ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt, PlaybackRefused,
    SynthesisEvent, SynthesisHandle, Synthesiser,
};

use super::{MockSink, SinkRefused};

/// The two short English sentences the checks speak unless the caller passes others.
pub const DEFAULT_SENTENCES: [&str; 2] =
    ["Hello, this is a short test.", "The weather is fine today."];

/// The most events the checks take from one synthesis before they fail it as unbounded.
pub const EVENT_LIMIT: usize = 100_000;

/// The most syntheses [`cancellation`] runs at once. Up to it, the check asks that a start beyond
/// the adapter's declared maximum concurrency be refused; above it, it runs this many and does
/// not.
pub const CONCURRENCY_BOUND: usize = 64;

/// The text the checks speak: one Committed segment per sentence, offered in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utterance {
    sentences: Vec<String>,
}

impl Utterance {
    /// The utterance of `sentences`, or `None` when there is none or one of them is blank, which
    /// no backend is bound to speak.
    pub fn new<I, T>(sentences: I) -> Option<Self>
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        let sentences: Vec<String> = sentences.into_iter().map(Into::into).collect();
        if sentences.is_empty() || sentences.iter().any(|sentence| sentence.trim().is_empty()) {
            return None;
        }
        Some(Self { sentences })
    }

    /// The sentences, in the order they are offered.
    #[must_use]
    pub fn sentences(&self) -> &[String] {
        &self.sentences
    }

    /// One Committed segment per sentence, emitted in order by one fresh generation.
    fn committed(&self) -> Vec<OutputSegment> {
        let generation = generation();
        self.sentences
            .iter()
            .zip(1..)
            .map(|(sentence, sequence)| commit(tentative(&generation, sequence, sentence)))
            .collect()
    }

    /// The first sentence; an utterance has one.
    fn first(&self) -> &str {
        &self.sentences[0]
    }
}

impl Default for Utterance {
    /// [`DEFAULT_SENTENCES`].
    fn default() -> Self {
        Self {
            sentences: DEFAULT_SENTENCES
                .iter()
                .map(|&sentence| sentence.to_owned())
                .collect(),
        }
    }
}

/// An exit item of the TTS adapter, one per check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Item {
    /// [`tentative_refused`]: a Tentative segment is refused and none of its audio is emitted.
    TentativeRefused,
    /// [`played_only_by_sink`]: a Committed segment moves Queued, Synthesized, Played, and
    /// reaches Played only on the sink's acknowledgement.
    PlayedOnlyBySink,
    /// [`claims_observed`]: `incremental_input` and `output_streaming` are claimed exactly when
    /// they are observed.
    ClaimsObserved,
    /// [`audio_valid`]: every frame has one format, its data length and a contiguous position.
    AudioValid,
    /// [`cancellation`]: a discarded segment yields no further frame, and a dropped handle
    /// releases its slot and revives nothing.
    Cancellation,
}

/// What a check observed that the exit item does not allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// A start was refused while fewer syntheses ran than the adapter declares.
    StartRefused(ConcurrencyLimitReached),
    /// A Committed segment of the utterance was refused.
    OfferRefused {
        segment_id: SegmentId,
        refusal: OfferRefused,
    },
    /// The end of the utterance was refused when it was first marked.
    UtteranceEndRefused,
    /// The handle yielded more than [`EVENT_LIMIT`] events for one synthesis.
    EventsUnbounded,
    /// An event names a segment the check did not offer to the synthesis.
    UnheldSegment { segment_id: SegmentId },
    /// A Tentative segment was queued.
    TentativeQueued { segment_id: SegmentId },
    /// A Tentative segment was refused, but not as the `wrong-state` outcome naming Tentative.
    TentativeRefusedOtherwise { refusal: OfferRefused },
    /// An event of a segment whose offer was refused was delivered.
    RefusedSegmentSpoken { segment_id: SegmentId },
    /// An offered segment is not Queued in the synthesis's record, in the state carried.
    NotQueued {
        segment_id: SegmentId,
        state: Option<SegmentState>,
    },
    /// The handle did not refuse an acknowledgement of a Queued segment as `wrong-state`.
    QueuedAcknowledged { segment_id: SegmentId },
    /// The segment reached Played before the sink acknowledged it.
    PlayedWithoutSink { segment_id: SegmentId },
    /// Once the utterance was drained, the segment was not Synthesized after exactly one
    /// `SynthesisCompleted`.
    NotSynthesized {
        segment_id: SegmentId,
        state: Option<SegmentState>,
        completions: usize,
    },
    /// The sink's acknowledgement was refused: among other reasons, because the synthesis
    /// emitted frames it never delivered to the sink.
    SinkRefused {
        segment_id: SegmentId,
        refusal: SinkRefused,
    },
    /// After the sink's acknowledgement the record is not Played with the frames given to the
    /// sink, those counted as played and those emitted all the same.
    NotPlayed {
        segment_id: SegmentId,
        state: Option<SegmentState>,
        given: usize,
        played: Option<usize>,
        emitted: Option<usize>,
    },
    /// The adapter claims `capability` and the synthesis never did what it names.
    ClaimNotObserved { capability: Capability },
    /// The synthesis did what `capability` names and the adapter does not claim it.
    ClaimMissing { capability: Capability },
    /// No audio was emitted: of the segment named, or of the whole utterance.
    NoAudio { segment_id: Option<SegmentId> },
    /// The frame at index `frame` of the synthesis has another format than the one expected,
    /// the caller's or else the first frame's.
    FormatDiffers {
        frame: usize,
        expected: AudioFormat,
        actual: AudioFormat,
    },
    /// The frame at index `frame` carries `actual` bytes where its samples make `expected`.
    FrameLength {
        frame: usize,
        expected: u64,
        actual: u64,
    },
    /// The frame at index `frame` carries no sample, so the positions do not increase.
    EmptyFrame { frame: usize },
    /// The frame at index `frame` starts at sample `actual` where the frame before it ended at
    /// `expected`, so the positions are not contiguous.
    PositionNotContiguous {
        frame: usize,
        expected: u64,
        actual: u64,
    },
    /// A segment could not be discarded while Queued.
    DiscardRefused {
        segment_id: SegmentId,
        refusal: DiscardRefused,
    },
    /// An event of a discarded segment was delivered after its discard.
    EventAfterDiscard { segment_id: SegmentId },
    /// A start beyond the maximum concurrency the adapter declares succeeded.
    ConcurrencyNotHeld { max_concurrency: i64 },
    /// A start after a handle was dropped mid-utterance was refused: its slot was not released.
    SlotNotReleased(ConcurrencyLimitReached),
    /// A fresh synthesis, offered nothing, delivered an event of a segment.
    StaleOutput { segment_id: SegmentId },
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// A check failed: the exit item it checks, and what it observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// What [`tentative_refused`] observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TentativeObserved {
    /// The Tentative segment refused.
    pub refused: SegmentId,
    /// The Committed segment of the same text offered after it.
    pub spoken: SegmentId,
    /// The frames emitted of the Committed one.
    pub frames_spoken: usize,
}

/// What [`played_only_by_sink`] observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackObserved {
    /// Each segment of the utterance, in order, with the frames the sink played of it.
    pub played: Vec<(SegmentId, usize)>,
}

/// What [`claims_observed`] observed the synthesiser do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedClaims {
    /// Audio of a segment arrived before the end of the utterance.
    pub incremental_input: bool,
    /// A frame of a segment arrived while the segment was Queued.
    pub output_streaming: bool,
}

/// What [`audio_valid`] observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioObserved {
    /// The one format of every frame.
    pub format: AudioFormat,
    /// The frames emitted.
    pub frames: usize,
    /// The samples per channel of all of them.
    pub samples: u64,
}

/// What [`cancellation`] observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancellationObserved {
    /// The segments discarded while Queued: the first before any step and, when the synthesiser
    /// emitted a frame of a later one while it was Queued, that one after its first frame.
    pub discarded: Vec<SegmentId>,
    /// The maximum concurrency the adapter declares.
    pub max_concurrency: i64,
}

/// Checks [`Item::TentativeRefused`]: a Tentative segment of the first sentence offered to a
/// synthesis is refused with the `wrong-state` [`SegmentStateConflict`] naming Tentative, the
/// record does not hold it, and once a Committed segment of the same text has been offered and
/// the utterance has ended, no event of the Tentative one is delivered and the Committed one has
/// audio.
///
/// # Errors
///
/// A [`Failure`] naming [`Item::TentativeRefused`].
pub fn tentative_refused<S: Synthesiser>(
    synthesiser: &mut S,
    utterance: &Utterance,
) -> Result<TentativeObserved, Failure> {
    let mut driver = Driver::start(synthesiser, Item::TentativeRefused)?;
    let generation = generation();
    let segment = tentative(&generation, 1, utterance.first());
    let refused = segment.segment_id();
    match driver.handle.offer_segment(segment) {
        Ok(_) => {
            return Err(driver.fail(Fault::TentativeQueued {
                segment_id: refused,
            }));
        }
        Err(OfferRefused::SegmentStateConflict(SegmentStateConflict {
            current: SegmentState::Tentative,
        })) => {}
        Err(refusal) => return Err(driver.fail(Fault::TentativeRefusedOtherwise { refusal })),
    }
    if driver.handle.output().segment(refused).is_some() {
        return Err(driver.fail(Fault::TentativeQueued {
            segment_id: refused,
        }));
    }
    let spoken = driver.offer(commit(tentative(&generation, 2, utterance.first())))?;
    driver.end()?;
    let mut frames_spoken = 0;
    while let Some((event, _)) = driver.next()? {
        let segment_id = event.segment_id();
        if segment_id == refused {
            return Err(driver.fail(Fault::RefusedSegmentSpoken { segment_id }));
        }
        if segment_id != spoken {
            return Err(driver.fail(Fault::UnheldSegment { segment_id }));
        }
        if matches!(event, SynthesisEvent::Audio(_)) {
            frames_spoken += 1;
        }
    }
    if frames_spoken == 0 {
        return Err(driver.fail(Fault::NoAudio {
            segment_id: Some(spoken),
        }));
    }
    Ok(TentativeObserved {
        refused,
        spoken,
        frames_spoken,
    })
}

/// Checks [`Item::PlayedOnlyBySink`]: each segment of the utterance is Queued once offered, and
/// the handle refuses an acknowledgement of it then as `wrong-state`; once the utterance has
/// ended and every event has been delivered, each frame given to a [`MockSink`], no segment was
/// ever seen Played, and each is Synthesized after exactly one `SynthesisCompleted`; the sink then
/// acknowledges each with the receipt it makes from the frames it was given, and the record moves
/// to Played with those frames counted as played and equal to the frames emitted.
///
/// # Errors
///
/// A [`Failure`] naming [`Item::PlayedOnlyBySink`].
pub fn played_only_by_sink<S: Synthesiser>(
    synthesiser: &mut S,
    utterance: &Utterance,
) -> Result<PlaybackObserved, Failure> {
    let mut driver = Driver::start(synthesiser, Item::PlayedOnlyBySink)?;
    let mut segments = Vec::new();
    for segment in utterance.committed() {
        segments.push(driver.offer(segment)?);
    }
    for &segment_id in &segments {
        let none = PlaybackReceipt::from_frames(Vec::new())
            .expect("the receipt of no frames speaks no other segment");
        match driver.handle.acknowledge_playback(segment_id, none) {
            Err(PlaybackRefused::SegmentStateConflict(SegmentStateConflict {
                current: SegmentState::Queued,
            })) => {}
            _ => return Err(driver.fail(Fault::QueuedAcknowledged { segment_id })),
        }
    }
    driver.end()?;
    let mut sink = MockSink::new();
    let mut completions = vec![0_usize; segments.len()];
    while let Some((event, state)) = driver.next()? {
        let segment_id = event.segment_id();
        let Some(index) = segments.iter().position(|&id| id == segment_id) else {
            return Err(driver.fail(Fault::UnheldSegment { segment_id }));
        };
        if state == Some(SegmentState::Played) {
            return Err(driver.fail(Fault::PlayedWithoutSink { segment_id }));
        }
        match event {
            SynthesisEvent::Completed(_) => completions[index] += 1,
            SynthesisEvent::Audio(audio) => sink.give(audio),
        }
    }
    for (&segment_id, &completions) in segments.iter().zip(&completions) {
        let state = driver.state(segment_id);
        if state == Some(SegmentState::Played) {
            return Err(driver.fail(Fault::PlayedWithoutSink { segment_id }));
        }
        if state != Some(SegmentState::Synthesized) || completions != 1 {
            return Err(driver.fail(Fault::NotSynthesized {
                segment_id,
                state,
                completions,
            }));
        }
    }
    let mut played = Vec::with_capacity(segments.len());
    for &segment_id in &segments {
        let given = sink.frames_given(segment_id);
        let (snapshot, event) =
            sink.acknowledge(&mut driver.handle, segment_id)
                .map_err(|refusal| {
                    driver.fail(Fault::SinkRefused {
                        segment_id,
                        refusal,
                    })
                })?;
        let output = driver.handle.output();
        let state = driver.state(segment_id);
        let counted = output.frames_played(segment_id);
        let emitted = output.frames_emitted(segment_id);
        if state != Some(SegmentState::Played)
            || snapshot.state() != SegmentState::Played
            || event.segment_id != segment_id
            || counted != Some(given)
            || emitted != Some(given)
        {
            return Err(driver.fail(Fault::NotPlayed {
                segment_id,
                state,
                given,
                played: counted,
                emitted,
            }));
        }
        played.push((segment_id, given));
    }
    Ok(PlaybackObserved { played })
}

/// Checks [`Item::ClaimsObserved`]: the sentences are offered one at a time, every event
/// delivered after each offer taken before the next, then the utterance ends and the rest is
/// taken. Every segment has audio; audio of a segment arrives before the end of the utterance
/// exactly when the adapter claims [`Capability::IncrementalInput`], and a frame of a segment
/// arrives while its record is Queued exactly when it claims [`Capability::OutputStreaming`].
/// The claims are read from [`Synthesiser::adapter`] only.
///
/// # Errors
///
/// A [`Failure`] naming [`Item::ClaimsObserved`]; a claim not observed is
/// [`Fault::ClaimNotObserved`] naming the capability, `incremental_input` checked first.
pub fn claims_observed<S: Synthesiser>(
    synthesiser: &mut S,
    utterance: &Utterance,
) -> Result<ObservedClaims, Failure> {
    let claimed = synthesiser.adapter().capabilities().to_vec();
    let mut driver = Driver::start(synthesiser, Item::ClaimsObserved)?;
    let mut segments = Vec::new();
    let mut with_audio = Vec::new();
    let mut observed = ObservedClaims {
        incremental_input: false,
        output_streaming: false,
    };
    let mut observe = |driver: &Driver<S::Handle>,
                       segments: &[SegmentId],
                       event: &SynthesisEvent,
                       state: Option<SegmentState>,
                       before_end: bool| {
        let segment_id = event.segment_id();
        if !segments.contains(&segment_id) {
            return Err(driver.fail(Fault::UnheldSegment { segment_id }));
        }
        if matches!(event, SynthesisEvent::Audio(_)) {
            with_audio.push(segment_id);
            observed.incremental_input |= before_end;
            observed.output_streaming |= state == Some(SegmentState::Queued);
        }
        Ok(())
    };
    for segment in utterance.committed() {
        segments.push(driver.offer(segment)?);
        while let Some((event, state)) = driver.next()? {
            observe(&driver, &segments, &event, state, true)?;
        }
    }
    driver.end()?;
    while let Some((event, state)) = driver.next()? {
        observe(&driver, &segments, &event, state, false)?;
    }
    if let Some(&segment_id) = segments.iter().find(|id| !with_audio.contains(id)) {
        return Err(driver.fail(Fault::NoAudio {
            segment_id: Some(segment_id),
        }));
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

/// Checks [`Item::AudioValid`]: the whole utterance is offered and ended, and every frame of the
/// synthesis, in the order delivered, has one format, `expected` when the caller passes it and
/// otherwise the first frame's; data of [`AudioFormat::data_len`] of its sample count; at least
/// one sample; and a position on the synthesis sample clock where the frame before it ended. At
/// least one frame is emitted.
///
/// # Errors
///
/// A [`Failure`] naming [`Item::AudioValid`], the frame at fault by its index in the synthesis.
pub fn audio_valid<S: Synthesiser>(
    synthesiser: &mut S,
    utterance: &Utterance,
    expected: Option<AudioFormat>,
) -> Result<AudioObserved, Failure> {
    let mut driver = Driver::start(synthesiser, Item::AudioValid)?;
    let mut segments = Vec::new();
    for segment in utterance.committed() {
        segments.push(driver.offer(segment)?);
    }
    driver.end()?;
    let mut format = expected;
    let mut frames = 0;
    let mut samples = 0_u64;
    let mut next_position = None;
    while let Some((event, _)) = driver.next()? {
        let segment_id = event.segment_id();
        if !segments.contains(&segment_id) {
            return Err(driver.fail(Fault::UnheldSegment { segment_id }));
        }
        let SynthesisEvent::Audio(audio) = event else {
            continue;
        };
        let frame = audio.frame();
        let index = frames;
        let expected = *format.get_or_insert(frame.format);
        if frame.format != expected {
            return Err(driver.fail(Fault::FormatDiffers {
                frame: index,
                expected,
                actual: frame.format,
            }));
        }
        let length = frame.format.data_len(frame.sample_count);
        let actual = frame.data.len() as u64;
        if actual != length {
            return Err(driver.fail(Fault::FrameLength {
                frame: index,
                expected: length,
                actual,
            }));
        }
        if frame.sample_count == 0 {
            return Err(driver.fail(Fault::EmptyFrame { frame: index }));
        }
        let position = frame.position.get();
        if let Some(expected) = next_position
            && position != expected
        {
            return Err(driver.fail(Fault::PositionNotContiguous {
                frame: index,
                expected,
                actual: position,
            }));
        }
        next_position = Some(position + u64::from(frame.sample_count));
        samples += u64::from(frame.sample_count);
        frames += 1;
    }
    match format {
        Some(format) if frames > 0 => Ok(AudioObserved {
            format,
            frames,
            samples,
        }),
        _ => Err(driver.fail(Fault::NoAudio { segment_id: None })),
    }
}

/// Checks [`Item::Cancellation`], in two parts.
///
/// A discarded segment yields no further frame: the whole utterance is offered, the first segment
/// is discarded while Queued before any step, and the utterance ends; when a frame of another
/// segment then arrives while it is Queued, that one is discarded too. No event of a discarded
/// segment is delivered after its discard.
///
/// A dropped handle releases its slot and revives nothing: as many syntheses as the adapter
/// declares run at once, up to [`CONCURRENCY_BOUND`], and, within that bound, one more start is
/// refused; one of them, offered the first sentence and stepped once, is dropped mid-utterance;
/// the next start succeeds, and that fresh synthesis, offered nothing and ended, delivers no
/// event.
///
/// # Errors
///
/// A [`Failure`] naming [`Item::Cancellation`].
pub fn cancellation<S: Synthesiser>(
    synthesiser: &mut S,
    utterance: &Utterance,
) -> Result<CancellationObserved, Failure> {
    let item = Item::Cancellation;
    let fail = |fault| Failure { item, fault };
    let max_concurrency = synthesiser.adapter().max_concurrency();
    let mut discarded = Vec::new();
    {
        let mut driver = Driver::start(synthesiser, item)?;
        let mut segments = Vec::new();
        for segment in utterance.committed() {
            segments.push(driver.offer(segment)?);
        }
        driver.discard(segments[0])?;
        discarded.push(segments[0]);
        driver.end()?;
        while let Some((event, state)) = driver.next()? {
            let segment_id = event.segment_id();
            if !segments.contains(&segment_id) {
                return Err(driver.fail(Fault::UnheldSegment { segment_id }));
            }
            if discarded.contains(&segment_id) {
                return Err(driver.fail(Fault::EventAfterDiscard { segment_id }));
            }
            if discarded.len() == 1
                && matches!(event, SynthesisEvent::Audio(_))
                && state == Some(SegmentState::Queued)
            {
                driver.discard(segment_id)?;
                discarded.push(segment_id);
            }
        }
    }

    let declared = usize::try_from(max_concurrency).unwrap_or(1).max(1);
    let mut running = Vec::with_capacity(declared.min(CONCURRENCY_BOUND));
    while running.len() < declared.min(CONCURRENCY_BOUND) {
        running.push(
            synthesiser
                .start()
                .map_err(|refusal| fail(Fault::StartRefused(refusal)))?,
        );
    }
    if declared <= CONCURRENCY_BOUND && synthesiser.start().is_ok() {
        return Err(fail(Fault::ConcurrencyNotHeld { max_concurrency }));
    }
    let mut dropped = running.pop().expect("at least one synthesis runs");
    let segment = commit(tentative(&generation(), 1, utterance.first()));
    let segment_id = segment.segment_id();
    dropped.offer_segment(segment).map_err(|refusal| {
        fail(Fault::OfferRefused {
            segment_id,
            refusal,
        })
    })?;
    let _ = dropped.next();
    drop(dropped);
    let fresh = synthesiser
        .start()
        .map_err(|refusal| fail(Fault::SlotNotReleased(refusal)))?;
    let mut driver = Driver {
        handle: fresh,
        item,
        events: 0,
    };
    driver.end()?;
    if let Some((event, _)) = driver.next()? {
        return Err(driver.fail(Fault::StaleOutput {
            segment_id: event.segment_id(),
        }));
    }
    drop(running);
    Ok(CancellationObserved {
        discarded,
        max_concurrency,
    })
}

/// One synthesis a check drives, counting the events it delivered.
struct Driver<H> {
    handle: H,
    item: Item,
    events: usize,
}

impl<H: SynthesisHandle> Driver<H> {
    fn start<S: Synthesiser<Handle = H>>(synthesiser: &mut S, item: Item) -> Result<Self, Failure> {
        let handle = synthesiser.start().map_err(|refusal| Failure {
            item,
            fault: Fault::StartRefused(refusal),
        })?;
        Ok(Self {
            handle,
            item,
            events: 0,
        })
    }

    const fn fail(&self, fault: Fault) -> Failure {
        Failure {
            item: self.item,
            fault,
        }
    }

    /// The state the synthesis's record holds the segment in.
    fn state(&self, segment_id: SegmentId) -> Option<SegmentState> {
        self.handle
            .output()
            .segment(segment_id)
            .map(OutputSegment::state)
    }

    /// The next event delivered, with the state of its segment when it was received.
    fn next(&mut self) -> Result<Option<(SynthesisEvent, Option<SegmentState>)>, Failure> {
        let Some(event) = self.handle.next() else {
            return Ok(None);
        };
        self.events += 1;
        if self.events > EVENT_LIMIT {
            return Err(self.fail(Fault::EventsUnbounded));
        }
        let state = self.state(event.segment_id());
        Ok(Some((event, state)))
    }

    /// Offers a Committed `segment` and requires it Queued in the record.
    fn offer(&mut self, segment: OutputSegment) -> Result<SegmentId, Failure> {
        let segment_id = segment.segment_id();
        self.handle.offer_segment(segment).map_err(|refusal| {
            self.fail(Fault::OfferRefused {
                segment_id,
                refusal,
            })
        })?;
        let state = self.state(segment_id);
        if state != Some(SegmentState::Queued) {
            return Err(self.fail(Fault::NotQueued { segment_id, state }));
        }
        Ok(segment_id)
    }

    fn discard(&mut self, segment_id: SegmentId) -> Result<(), Failure> {
        self.handle
            .discard(segment_id)
            .map(|_| ())
            .map_err(|refusal| {
                self.fail(Fault::DiscardRefused {
                    segment_id,
                    refusal,
                })
            })
    }

    fn end(&mut self) -> Result<(), Failure> {
        self.handle
            .end_utterance()
            .map_err(|_| self.fail(Fault::UtteranceEndRefused))
    }
}

/// A fresh Active generation of a fresh run and node.
fn generation() -> Generation {
    Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0
}

/// The Tentative segment of `text` at `sequence` of `generation`.
fn tentative(generation: &Generation, sequence: i64, text: &str) -> OutputSegment {
    OutputSegment::emit(generation, sequence, text.to_owned())
        .expect("a fresh generation is Active")
        .0
}

/// `segment`, committed.
fn commit(segment: OutputSegment) -> OutputSegment {
    segment.commit().expect("a Tentative segment commits").0
}
