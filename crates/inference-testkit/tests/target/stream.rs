//! `inference.stream`: the turn, span, generation and segment commands, views and events of
//! `ess/domains/stream.yaml`, answered by [`StreamDomain`].
//!
//! Each request field is read on its own ([`Input`]); the outcome, the error and the event come
//! from what the store returned. A command naming a turn, span, generation or segment no record
//! carries takes its `wrong-state` outcome, whose error then carries no `current`, since no record
//! holds a state to report; `ReviseSpan` declares `no-such-span` for it and `EmitTextDelta`
//! `unknown-generation`, and those carry the identity sent. `SupersedeGeneration` and
//! `CancelGeneration` publish their generation event only: the discard of the generation's
//! unplayed segments, which the store makes in the same outcome, emits none.
//!
//! `OpenTurn`, `EmitHypothesis` and `StartGeneration` read the runs, and `StartGeneration` the
//! nodes, of the `inference.graph` store ([`GraphDomain`]) through the stream store's
//! [`StreamDomain::open_turn_in`], [`StreamDomain::emit_hypothesis_in`] and
//! [`StreamDomain::start_generation_in`], which decide an identity no record carries before a
//! state. Their `run-not-active` error, `RunStateConflict`, carries the state the run holds, and
//! `EmitHypothesis`'s `turn-not-open` error, `TurnStateConflict`, the turn's.
//!
//! [`StreamDomain::open_turn_in`]: inference_testkit::stream::StreamDomain::open_turn_in
//! [`StreamDomain::emit_hypothesis_in`]: inference_testkit::stream::StreamDomain::emit_hypothesis_in
//! [`StreamDomain::start_generation_in`]: inference_testkit::stream::StreamDomain::start_generation_in
//!
//! A command's consistency token is the target's
//! ([`Stores::token`]), and a read demanding a token the stores have not reached is an error
//! ([`Stores::require`]). An observed event's position is its place in the target's one
//! publication order ([`Stores::stream_position`]).

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ObservedEvent, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError, ViewRow,
};
use inference_core::generation::{GenerationStarted, GenerationStateConflict, StartGeneration};
use inference_core::id::GenerationId;
use inference_runtime::run::{RunNotFound, RunStateConflict};
use inference_testkit::graph::GraphDomain;
use inference_testkit::stream::{
    EmitHypothesisError, EmitTextDeltaError, GenerationMoveError, GenerationsRow, OpenTurnError,
    ReviseSpanError, SegmentMoveError, SegmentsRow, SpanMoveError, SpansRow, StartGenerationError,
    StreamDomain, StreamEvent, TurnMoveError, TurnsRow,
};
use inference_text::{
    EmitHypothesis, EmitTextDelta, OpenTurn, ReviseSpan, SegmentId, SegmentStateConflict, SpanId,
    SpanStateConflict, TurnId, TurnStateConflict,
};

use super::{Input, Stores, integer_node, named_node, text_node, unsupported};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.stream";

const OPEN_TURN: &str = "inference.stream.OpenTurn";
const COMPLETE_TURN: &str = "inference.stream.CompleteTurn";
const EMIT_HYPOTHESIS: &str = "inference.stream.EmitHypothesis";
const REVISE_SPAN: &str = "inference.stream.ReviseSpan";
const COMMIT_SPAN: &str = "inference.stream.CommitSpan";
const START_GENERATION: &str = "inference.stream.StartGeneration";
const COMPLETE_GENERATION: &str = "inference.stream.CompleteGeneration";
const SUPERSEDE_GENERATION: &str = "inference.stream.SupersedeGeneration";
const CANCEL_GENERATION: &str = "inference.stream.CancelGeneration";
const EMIT_TEXT_DELTA: &str = "inference.stream.EmitTextDelta";
const COMMIT_SEGMENT: &str = "inference.stream.CommitSegment";
const QUEUE_FOR_SYNTHESIS: &str = "inference.stream.QueueForSynthesis";
const COMPLETE_SYNTHESIS: &str = "inference.stream.CompleteSynthesis";
const ACKNOWLEDGE_PLAYBACK: &str = "inference.stream.AcknowledgePlayback";
const DISCARD_SEGMENT: &str = "inference.stream.DiscardSegment";

const SPEECH_STARTED: &str = "inference.stream.SpeechStarted";
const SPEECH_ENDED: &str = "inference.stream.SpeechEnded";
const TURN_OPENED: &str = "inference.stream.TurnOpened";
const TURN_COMPLETED: &str = "inference.stream.TurnCompleted";
const TRANSCRIPT_HYPOTHESIS: &str = "inference.stream.TranscriptHypothesis";
const TRANSCRIPT_REVISION: &str = "inference.stream.TranscriptRevision";
const TRANSCRIPT_COMMITTED: &str = "inference.stream.TranscriptCommitted";
const GENERATION_STARTED: &str = "inference.stream.GenerationStarted";
const GENERATION_COMPLETED: &str = "inference.stream.GenerationCompleted";
const GENERATION_SUPERSEDED: &str = "inference.stream.GenerationSuperseded";
const GENERATION_CANCELLED: &str = "inference.stream.GenerationCancelled";
const TEXT_DELTA: &str = "inference.stream.TextDelta";
const TEXT_SEGMENT_COMMITTED: &str = "inference.stream.TextSegmentCommitted";
const SEGMENT_QUEUED: &str = "inference.stream.SegmentQueued";
const SYNTHESIS_COMPLETED: &str = "inference.stream.SynthesisCompleted";
const AUDIO_PLAYED: &str = "inference.stream.AudioPlayed";
const SEGMENT_DISCARDED: &str = "inference.stream.SegmentDiscarded";

/// Every event this module observes.
const EVENTS: [&str; 17] = [
    SPEECH_STARTED,
    SPEECH_ENDED,
    TURN_OPENED,
    TURN_COMPLETED,
    TRANSCRIPT_HYPOTHESIS,
    TRANSCRIPT_REVISION,
    TRANSCRIPT_COMMITTED,
    GENERATION_STARTED,
    GENERATION_COMPLETED,
    GENERATION_SUPERSEDED,
    GENERATION_CANCELLED,
    TEXT_DELTA,
    TEXT_SEGMENT_COMMITTED,
    SEGMENT_QUEUED,
    SYNTHESIS_COMPLETED,
    AUDIO_PLAYED,
    SEGMENT_DISCARDED,
];

const TURN_STATE_CONFLICT: &str = "inference.stream.TurnStateConflict";
const SPAN_STATE_CONFLICT: &str = "inference.stream.SpanStateConflict";
const SPAN_NOT_FOUND: &str = "inference.stream.SpanNotFound";
const COMMITTED_SPAN_IMMUTABLE: &str = "inference.stream.CommittedSpanImmutable";
const GENERATION_STATE_CONFLICT: &str = "inference.stream.GenerationStateConflict";
const SEGMENT_STATE_CONFLICT: &str = "inference.stream.SegmentStateConflict";
const GENERATION_NOT_FOUND: &str = "inference.stream.GenerationNotFound";
const TURN_NOT_FOUND: &str = "inference.stream.TurnNotFound";
const RUN_NOT_FOUND: &str = "inference.graph.RunNotFound";
const RUN_STATE_CONFLICT: &str = "inference.graph.RunStateConflict";
const NODE_NOT_FOUND: &str = "inference.graph.NodeNotFound";

const TURNS: &str = "inference.stream.Turns";
const SPANS: &str = "inference.stream.Spans";
const GENERATIONS: &str = "inference.stream.Generations";
const SEGMENTS: &str = "inference.stream.Segments";

/// The turn, span, generation and segment stores of one scenario.
pub type State = StreamDomain;

pub fn execute_command(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    stores.publish();
    let graph = &stores.graph;
    let domain = &mut stores.stream;
    let name = request.command.to_string();
    let result = match name.as_str() {
        OPEN_TURN => open_turn(domain, graph, request)?,
        EMIT_HYPOTHESIS => emit_hypothesis(domain, graph, request)?,
        COMPLETE_TURN => {
            let answer = domain.complete_turn(turn_id(request)?);
            turn_moved(answer.map(StreamEvent::TurnCompleted), request)?
        }
        REVISE_SPAN => revise_span(domain, request)?,
        COMMIT_SPAN => {
            let answer = domain.commit_span(span_id(request)?);
            span_moved(answer.map(StreamEvent::TranscriptCommitted), request)?
        }
        START_GENERATION => start_generation(domain, graph, request)?,
        COMPLETE_GENERATION => {
            let answer = domain.complete_generation(generation_id(request)?);
            let answer = answer.map(StreamEvent::GenerationCompleted);
            generation_moved(COMPLETE_GENERATION, "completed", answer, request)?
        }
        SUPERSEDE_GENERATION => {
            let answer = domain.supersede_generation(generation_id(request)?);
            let answer = answer.map(StreamEvent::GenerationSuperseded);
            generation_moved(SUPERSEDE_GENERATION, "superseded", answer, request)?
        }
        CANCEL_GENERATION => {
            let answer = domain.cancel_generation(generation_id(request)?);
            let answer = answer.map(StreamEvent::GenerationCancelled);
            generation_moved(CANCEL_GENERATION, "cancelled", answer, request)?
        }
        EMIT_TEXT_DELTA => emit_text_delta(domain, request)?,
        COMMIT_SEGMENT => {
            let answer = domain.commit_segment(segment_id(request)?);
            let answer = answer.map(StreamEvent::TextSegmentCommitted);
            segment_moved(COMMIT_SEGMENT, "committed", answer, request)?
        }
        QUEUE_FOR_SYNTHESIS => {
            let answer = domain.queue_for_synthesis(segment_id(request)?);
            let answer = answer.map(StreamEvent::SegmentQueued);
            segment_moved(QUEUE_FOR_SYNTHESIS, "queued", answer, request)?
        }
        COMPLETE_SYNTHESIS => {
            let answer = domain.complete_synthesis(segment_id(request)?);
            let answer = answer.map(StreamEvent::SynthesisCompleted);
            segment_moved(COMPLETE_SYNTHESIS, "synthesized", answer, request)?
        }
        ACKNOWLEDGE_PLAYBACK => {
            let answer = domain.acknowledge_playback(segment_id(request)?);
            let answer = answer.map(StreamEvent::AudioPlayed);
            segment_moved(ACKNOWLEDGE_PLAYBACK, "played", answer, request)?
        }
        DISCARD_SEGMENT => {
            let answer = domain.discard_segment(segment_id(request)?);
            let answer = answer.map(StreamEvent::SegmentDiscarded);
            segment_moved(DISCARD_SEGMENT, "discarded", answer, request)?
        }
        _ => return Err(unsupported(&name)),
    };
    stores.publish();
    Ok(result.with_consistency(stores.token()?))
}

fn open_turn(
    domain: &mut StreamDomain,
    graph: &GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let open = OpenTurn {
        run_id: input.named("run_id")?,
    };
    input.finish()?;
    Ok(match domain.open_turn_in(graph, open) {
        Ok(event) => SemanticCommandResult::took(outcome(OPEN_TURN, "opened")).emitting(
            stream_event(&StreamEvent::TurnOpened(event))?.in_activity(request.correlation.clone()),
        ),
        Err(OpenTurnError::UnknownRun(error)) => {
            SemanticCommandResult::took(outcome(OPEN_TURN, "unknown-run"))
                .with_error(run_not_found(error)?)
        }
        Err(OpenTurnError::RunNotActive(conflict)) => {
            SemanticCommandResult::took(outcome(OPEN_TURN, "run-not-active"))
                .with_error(run_conflict(conflict)?)
        }
    })
}

fn emit_hypothesis(
    domain: &mut StreamDomain,
    graph: &GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let emit = EmitHypothesis {
        run_id: input.named("run_id")?,
        turn_id: input.named("turn_id")?,
        stream_id: input.named("stream_id")?,
        sequence: input.integer("sequence")?,
        text: input.text("text")?,
    };
    input.finish()?;
    Ok(match domain.emit_hypothesis_in(graph, emit) {
        Ok(event) => SemanticCommandResult::took(outcome(EMIT_HYPOTHESIS, "emitted")).emitting(
            stream_event(&StreamEvent::TranscriptHypothesis(event))?
                .in_activity(request.correlation.clone()),
        ),
        Err(EmitHypothesisError::UnknownRun(error)) => {
            SemanticCommandResult::took(outcome(EMIT_HYPOTHESIS, "unknown-run"))
                .with_error(run_not_found(error)?)
        }
        Err(EmitHypothesisError::RunNotActive(conflict)) => {
            SemanticCommandResult::took(outcome(EMIT_HYPOTHESIS, "run-not-active"))
                .with_error(run_conflict(conflict)?)
        }
        Err(EmitHypothesisError::UnknownTurn(error)) => {
            SemanticCommandResult::took(outcome(EMIT_HYPOTHESIS, "unknown-turn")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(TURN_NOT_FOUND))
                    .with("turn_id", named_node(&error.turn_id)?),
            )
        }
        Err(EmitHypothesisError::TurnNotOpen(conflict)) => {
            SemanticCommandResult::took(outcome(EMIT_HYPOTHESIS, "turn-not-open"))
                .with_error(turn_conflict(conflict)?)
        }
    })
}

fn start_generation(
    domain: &mut StreamDomain,
    graph: &GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let start = StartGeneration {
        run_id: input.named("run_id")?,
        node_id: input.named("node_id")?,
        epoch: input.integer("epoch")?,
    };
    input.finish()?;
    Ok(match domain.start_generation_in(graph, start) {
        Ok(event) => SemanticCommandResult::took(outcome(START_GENERATION, "started"))
            .emitting(generation_started(&event)?.in_activity(request.correlation.clone())),
        Err(StartGenerationError::UnknownRun(error)) => {
            SemanticCommandResult::took(outcome(START_GENERATION, "unknown-run"))
                .with_error(run_not_found(error)?)
        }
        Err(StartGenerationError::UnknownNode(error)) => {
            SemanticCommandResult::took(outcome(START_GENERATION, "unknown-node")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(NODE_NOT_FOUND))
                    .with("node_id", named_node(&error.node_id)?),
            )
        }
        Err(StartGenerationError::RunNotActive(conflict)) => {
            SemanticCommandResult::took(outcome(START_GENERATION, "run-not-active"))
                .with_error(run_conflict(conflict)?)
        }
    })
}

/// `inference.graph.RunNotFound`, naming the identity sent.
fn run_not_found(error: RunNotFound) -> Result<DeclaredErrorValue, TargetError> {
    Ok(DeclaredErrorValue::new(parse::<ErrorRef>(RUN_NOT_FOUND))
        .with("run_id", named_node(&error.run_id)?))
}

/// `inference.graph.RunStateConflict`, carrying the state the run holds.
fn run_conflict(conflict: RunStateConflict) -> Result<DeclaredErrorValue, TargetError> {
    Ok(
        DeclaredErrorValue::new(parse::<ErrorRef>(RUN_STATE_CONFLICT))
            .with("current", named_node(&conflict.current)?),
    )
}

/// The one field of `CompleteTurn`.
fn turn_id(request: &SemanticCommandRequest) -> Result<TurnId, TargetError> {
    let mut input = Input::of(request);
    let turn_id = input.named("turn_id")?;
    input.finish()?;
    Ok(turn_id)
}

/// The one field of `CommitSpan`.
fn span_id(request: &SemanticCommandRequest) -> Result<SpanId, TargetError> {
    let mut input = Input::of(request);
    let span_id = input.named("span_id")?;
    input.finish()?;
    Ok(span_id)
}

/// The answer of `CompleteTurn`: `completed` and its event, or `wrong-state`.
fn turn_moved(
    answer: Result<StreamEvent, TurnMoveError>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(match answer {
        Ok(event) => SemanticCommandResult::took(outcome(COMPLETE_TURN, "completed"))
            .emitting(stream_event(&event)?.in_activity(request.correlation.clone())),
        Err(TurnMoveError::Unknown(_)) => {
            SemanticCommandResult::took(outcome(COMPLETE_TURN, "wrong-state")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(TURN_STATE_CONFLICT)),
            )
        }
        Err(TurnMoveError::Conflict(conflict)) => {
            SemanticCommandResult::took(outcome(COMPLETE_TURN, "wrong-state"))
                .with_error(turn_conflict(conflict)?)
        }
    })
}

/// The answer of `CommitSpan`: `committed` and its event, or `wrong-state`.
fn span_moved(
    answer: Result<StreamEvent, SpanMoveError>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(match answer {
        Ok(event) => SemanticCommandResult::took(outcome(COMMIT_SPAN, "committed"))
            .emitting(stream_event(&event)?.in_activity(request.correlation.clone())),
        Err(SpanMoveError::Unknown(_)) => {
            SemanticCommandResult::took(outcome(COMMIT_SPAN, "wrong-state")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(SPAN_STATE_CONFLICT)),
            )
        }
        Err(SpanMoveError::Conflict(conflict)) => {
            SemanticCommandResult::took(outcome(COMMIT_SPAN, "wrong-state"))
                .with_error(span_conflict(conflict)?)
        }
    })
}

fn revise_span(
    domain: &mut StreamDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let revise = ReviseSpan {
        span_id: input.named("span_id")?,
        text: input.text("text")?,
    };
    input.finish()?;
    Ok(match domain.revise_span(revise) {
        Ok(event) => SemanticCommandResult::took(outcome(REVISE_SPAN, "revised")).emitting(
            stream_event(&StreamEvent::TranscriptRevision(event))?
                .in_activity(request.correlation.clone()),
        ),
        Err(ReviseSpanError::Immutable(error)) => {
            SemanticCommandResult::took(outcome(REVISE_SPAN, "immutable")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(COMMITTED_SPAN_IMMUTABLE))
                    .with("span_id", named_node(&error.span_id)?),
            )
        }
        Err(ReviseSpanError::NoSuchSpan(error)) => {
            SemanticCommandResult::took(outcome(REVISE_SPAN, "no-such-span")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(SPAN_NOT_FOUND))
                    .with("span_id", named_node(&error.span_id)?),
            )
        }
    })
}

fn turn_conflict(conflict: TurnStateConflict) -> Result<DeclaredErrorValue, TargetError> {
    Ok(
        DeclaredErrorValue::new(parse::<ErrorRef>(TURN_STATE_CONFLICT))
            .with("current", named_node(&conflict.current)?),
    )
}

fn span_conflict(conflict: SpanStateConflict) -> Result<DeclaredErrorValue, TargetError> {
    Ok(
        DeclaredErrorValue::new(parse::<ErrorRef>(SPAN_STATE_CONFLICT))
            .with("current", named_node(&conflict.current)?),
    )
}

/// The one field of a generation command.
fn generation_id(request: &SemanticCommandRequest) -> Result<GenerationId, TargetError> {
    let mut input = Input::of(request);
    let generation_id = input.named("generation_id")?;
    input.finish()?;
    Ok(generation_id)
}

/// The one field of a segment command.
fn segment_id(request: &SemanticCommandRequest) -> Result<SegmentId, TargetError> {
    let mut input = Input::of(request);
    let segment_id = input.named("segment_id")?;
    input.finish()?;
    Ok(segment_id)
}

/// The answer of a generation command: `branch` and its event, or `wrong-state`.
fn generation_moved(
    command: &str,
    branch: &str,
    answer: Result<StreamEvent, GenerationMoveError>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(match answer {
        Ok(event) => SemanticCommandResult::took(outcome(command, branch))
            .emitting(stream_event(&event)?.in_activity(request.correlation.clone())),
        Err(GenerationMoveError::Unknown(_)) => {
            SemanticCommandResult::took(outcome(command, "wrong-state")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(GENERATION_STATE_CONFLICT)),
            )
        }
        Err(GenerationMoveError::Conflict(conflict)) => {
            SemanticCommandResult::took(outcome(command, "wrong-state"))
                .with_error(generation_conflict(conflict)?)
        }
    })
}

/// The answer of a segment command: `branch` and its event, or `wrong-state`.
fn segment_moved(
    command: &str,
    branch: &str,
    answer: Result<StreamEvent, SegmentMoveError>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(match answer {
        Ok(event) => SemanticCommandResult::took(outcome(command, branch))
            .emitting(stream_event(&event)?.in_activity(request.correlation.clone())),
        Err(SegmentMoveError::Unknown { .. }) => {
            SemanticCommandResult::took(outcome(command, "wrong-state")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(SEGMENT_STATE_CONFLICT)),
            )
        }
        Err(SegmentMoveError::Conflict(conflict)) => {
            SemanticCommandResult::took(outcome(command, "wrong-state"))
                .with_error(segment_conflict(conflict)?)
        }
    })
}

fn emit_text_delta(
    domain: &mut StreamDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let emit = EmitTextDelta {
        generation_id: input.named("generation_id")?,
        sequence: input.integer("sequence")?,
        text: input.text("text")?,
    };
    input.finish()?;
    Ok(match domain.emit_text_delta(emit) {
        Ok(event) => SemanticCommandResult::took(outcome(EMIT_TEXT_DELTA, "emitted")).emitting(
            stream_event(&StreamEvent::TextDelta(event))?.in_activity(request.correlation.clone()),
        ),
        Err(EmitTextDeltaError::UnknownGeneration(error)) => {
            SemanticCommandResult::took(outcome(EMIT_TEXT_DELTA, "unknown-generation")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(GENERATION_NOT_FOUND))
                    .with("generation_id", named_node(&error.generation_id)?),
            )
        }
        Err(EmitTextDeltaError::StaleGeneration(conflict)) => {
            SemanticCommandResult::took(outcome(EMIT_TEXT_DELTA, "stale-generation"))
                .with_error(generation_conflict(conflict)?)
        }
    })
}

fn generation_conflict(
    conflict: GenerationStateConflict,
) -> Result<DeclaredErrorValue, TargetError> {
    Ok(
        DeclaredErrorValue::new(parse::<ErrorRef>(GENERATION_STATE_CONFLICT))
            .with("current", named_node(&conflict.current)?),
    )
}

fn segment_conflict(conflict: SegmentStateConflict) -> Result<DeclaredErrorValue, TargetError> {
    Ok(
        DeclaredErrorValue::new(parse::<ErrorRef>(SEGMENT_STATE_CONFLICT))
            .with("current", named_node(&conflict.current)?),
    )
}

/// An event that carries the `segment_id` alone.
fn segment_event(event: &str, segment_id: SegmentId) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(event)).with("segment_id", named_node(&segment_id)?))
}

pub fn query_view(
    stores: &mut Stores,
    request: &SemanticViewRequest,
) -> Result<SemanticViewResult, TargetError> {
    let domain = &stores.stream;
    let name = request.view.to_string();
    if ![TURNS, SPANS, GENERATIONS, SEGMENTS].contains(&name.as_str()) {
        return Err(unsupported(&name));
    }
    stores.require(&name, &request.consistency)?;
    let rows = match name.as_str() {
        TURNS => domain
            .turns_view()
            .iter()
            .map(turns_row)
            .collect::<Result<Vec<_>, _>>()?,
        SPANS => domain
            .spans_view()
            .iter()
            .map(spans_row)
            .collect::<Result<Vec<_>, _>>()?,
        GENERATIONS => domain
            .generations_view()
            .iter()
            .map(generations_row)
            .collect::<Result<Vec<_>, _>>()?,
        _ => domain
            .segments_view()
            .iter()
            .map(segments_row)
            .collect::<Result<Vec<_>, _>>()?,
    };
    Ok(SemanticViewResult::of(rows))
}

pub fn observe_events(
    stores: &mut Stores,
    request: &EventObservationRequest,
) -> Result<Vec<ObservedEvent>, TargetError> {
    let wanted = request.event.to_string();
    if !EVENTS.contains(&wanted.as_str()) {
        return Err(unsupported(&wanted));
    }
    stores.publish();
    let mut observed = Vec::new();
    for (index, event) in stores.stream.events().iter().enumerate() {
        let event = stream_event(event)?;
        if event.event == request.event {
            let position = stores.stream_position(index)?;
            observed.push(event.in_activity(request.correlation.clone()).at(position));
        }
    }
    Ok(observed)
}

/// An event of the store as the suite observes it, every declared field carried.
fn stream_event(event: &StreamEvent) -> Result<ObservedEvent, TargetError> {
    let generation = |name: &str, generation_id: GenerationId| {
        Ok::<_, TargetError>(
            ObservedEvent::new(parse::<EventRef>(name))
                .with("generation_id", named_node(&generation_id)?),
        )
    };
    Ok(match event {
        StreamEvent::SpeechStarted(event) => ObservedEvent::new(parse::<EventRef>(SPEECH_STARTED))
            .with("onset", position_node(event.onset.get())),
        StreamEvent::SpeechEnded(event) => ObservedEvent::new(parse::<EventRef>(SPEECH_ENDED))
            .with("offset", position_node(event.offset.get())),
        StreamEvent::TurnOpened(event) => ObservedEvent::new(parse::<EventRef>(TURN_OPENED))
            .with("turn_id", named_node(&event.turn_id)?)
            .with("run_id", named_node(&event.run_id)?),
        StreamEvent::TurnCompleted(event) => ObservedEvent::new(parse::<EventRef>(TURN_COMPLETED))
            .with("turn_id", named_node(&event.turn_id)?),
        StreamEvent::TranscriptHypothesis(event) => {
            ObservedEvent::new(parse::<EventRef>(TRANSCRIPT_HYPOTHESIS))
                .with("span_id", named_node(&event.span_id)?)
                .with("turn_id", named_node(&event.turn_id)?)
                .with("sequence", integer_node(event.sequence))
                .with("text", text_node(&event.text))
        }
        StreamEvent::TranscriptRevision(event) => {
            ObservedEvent::new(parse::<EventRef>(TRANSCRIPT_REVISION))
                .with("span_id", named_node(&event.span_id)?)
                .with("text", text_node(&event.text))
        }
        StreamEvent::TranscriptCommitted(event) => {
            ObservedEvent::new(parse::<EventRef>(TRANSCRIPT_COMMITTED))
                .with("span_id", named_node(&event.span_id)?)
                .with("text", text_node(&event.text))
        }
        StreamEvent::GenerationStarted(event) => generation_started(event)?,
        StreamEvent::GenerationCompleted(event) => {
            generation(GENERATION_COMPLETED, event.generation_id)?
        }
        StreamEvent::GenerationSuperseded(event) => {
            generation(GENERATION_SUPERSEDED, event.generation_id)?
        }
        StreamEvent::GenerationCancelled(event) => {
            generation(GENERATION_CANCELLED, event.generation_id)?
        }
        StreamEvent::TextDelta(event) => ObservedEvent::new(parse::<EventRef>(TEXT_DELTA))
            .with("segment_id", named_node(&event.segment_id)?)
            .with("generation_id", named_node(&event.generation_id)?)
            .with("sequence", integer_node(event.sequence))
            .with("text", text_node(&event.text)),
        StreamEvent::TextSegmentCommitted(event) => {
            ObservedEvent::new(parse::<EventRef>(TEXT_SEGMENT_COMMITTED))
                .with("segment_id", named_node(&event.segment_id)?)
                .with("text", text_node(&event.text))
        }
        StreamEvent::SegmentQueued(event) => segment_event(SEGMENT_QUEUED, event.segment_id)?,
        StreamEvent::SynthesisCompleted(event) => {
            segment_event(SYNTHESIS_COMPLETED, event.segment_id)?
        }
        StreamEvent::AudioPlayed(event) => segment_event(AUDIO_PLAYED, event.segment_id)?,
        StreamEvent::SegmentDiscarded(event) => segment_event(SEGMENT_DISCARDED, event.segment_id)?,
    })
}

/// A sample clock position as the Integer it is declared over; a position never exceeds `i64::MAX`.
fn position_node(position: u64) -> ess_primitives::node::Node {
    integer_node(i64::try_from(position).unwrap_or(i64::MAX))
}

fn generation_started(event: &GenerationStarted) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(GENERATION_STARTED))
        .with("generation_id", named_node(&event.generation_id)?)
        .with("run_id", named_node(&event.run_id)?)
        .with("epoch", integer_node(event.epoch)))
}

fn turns_row(row: &TurnsRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("turn_id".to_owned(), named_node(&row.turn_id)?),
        ("run_id".to_owned(), named_node(&row.run_id)?),
        ("state".to_owned(), named_node(&row.state)?),
    ]))
}

fn spans_row(row: &SpansRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("span_id".to_owned(), named_node(&row.span_id)?),
        ("run_id".to_owned(), named_node(&row.run_id)?),
        ("turn_id".to_owned(), named_node(&row.turn_id)?),
        ("stream_id".to_owned(), named_node(&row.stream_id)?),
        ("sequence".to_owned(), integer_node(row.sequence)),
        ("text".to_owned(), text_node(&row.text)),
        ("state".to_owned(), named_node(&row.state)?),
    ]))
}

fn generations_row(row: &GenerationsRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("generation_id".to_owned(), named_node(&row.generation_id)?),
        ("run_id".to_owned(), named_node(&row.run_id)?),
        ("node_id".to_owned(), named_node(&row.node_id)?),
        ("epoch".to_owned(), integer_node(row.epoch)),
        ("state".to_owned(), named_node(&row.state)?),
    ]))
}

fn segments_row(row: &SegmentsRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("segment_id".to_owned(), named_node(&row.segment_id)?),
        ("generation_id".to_owned(), named_node(&row.generation_id)?),
        ("run_id".to_owned(), named_node(&row.run_id)?),
        ("sequence".to_owned(), integer_node(row.sequence)),
        ("text".to_owned(), text_node(&row.text)),
        ("state".to_owned(), named_node(&row.state)?),
    ]))
}

fn outcome(command: &str, branch: &str) -> OutcomeRef {
    OutcomeRef::new(
        parse::<CommandRef>(command),
        branch
            .parse()
            .unwrap_or_else(|error| panic!("`{branch}` is an outcome name: {error}")),
    )
}

/// Parses a name this module writes as a literal; a malformed one is a defect here.
fn parse<T>(name: &str) -> T
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    name.parse()
        .unwrap_or_else(|error| panic!("`{name}` is a well-formed name: {error}"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ess_conformance::scenario::{CommandRef, EventRef, ViewRef};
    use ess_conformance::target::{
        Deadline, EventObservationRequest, SemanticCommandRequest, SemanticCommandResult,
        SemanticViewRequest, TargetError,
    };
    use ess_primitives::consistency::QueryConsistency;
    use ess_primitives::facts::Number;
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;
    use ess_primitives::time::Timestamp;
    use inference_core::generation::StartGeneration;
    use inference_core::id::{NodeId, RunId, StreamId};
    use inference_core::policy::{DeclareProfile, Egress, ProfileKind};
    use inference_pipeline::{
        AddNode, DeclarePipeline, InteractionMode, NodeRole, PlanPipeline, ValidatePipeline,
    };
    use inference_testkit::graph::GraphCatalog;
    use inference_text::{EmitHypothesis, OpenTurn};
    use uuid::Uuid;

    use super::super::Stores;
    use super::{execute_command, observe_events, query_view};

    const UNKNOWN: &str = "00000000-0000-4000-8000-6eced0bc8ba0";

    fn correlation() -> CorrelationId {
        CorrelationId::new("stream").expect("a correlation id")
    }

    fn text(value: &str) -> Node {
        Node::Text(value.to_owned())
    }

    fn command(name: &str, input: &[(&str, Node)]) -> SemanticCommandRequest {
        SemanticCommandRequest {
            command: name.parse::<CommandRef>().expect("a command name"),
            actor: None,
            caller: None,
            input: input
                .iter()
                .map(|(field, value)| ((*field).to_owned(), value.clone()))
                .collect(),
            correlation: correlation(),
        }
    }

    fn run(stores: &mut Stores, name: &str, input: &[(&str, Node)]) -> SemanticCommandResult {
        execute_command(stores, &command(name, input))
            .unwrap_or_else(|error| panic!("`{name}` answers: {error}"))
    }

    fn branch(result: &SemanticCommandResult) -> String {
        result
            .outcome
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    fn rows(stores: &mut Stores, name: &str) -> Vec<BTreeMap<String, Node>> {
        query_view(
            stores,
            &SemanticViewRequest {
                view: name.parse::<ViewRef>().expect("a view name"),
                params: BTreeMap::new(),
                consistency: QueryConsistency::Current,
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .unwrap_or_else(|error| panic!("`{name}` reads: {error}"))
        .rows
    }

    /// A generation in the store, started past the run refusals, which these cases do not
    /// exercise; returns its identity as the runner captures it.
    fn started(stores: &mut Stores) -> Node {
        let event = stores.stream.start_generation(StartGeneration {
            run_id: RunId::from(Uuid::from_u128(0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d)),
            node_id: NodeId::from(Uuid::from_u128(0x1a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d)),
            epoch: 1,
        });
        text(&event.generation_id.to_string())
    }

    fn emitted(stores: &mut Stores, generation: &Node, sequence: i64) -> Node {
        let result = run(
            stores,
            "inference.stream.EmitTextDelta",
            &[
                ("generation_id", generation.clone()),
                ("sequence", Node::Number(Number::from(sequence))),
                ("text", text("hello")),
            ],
        );
        assert_eq!(
            branch(&result),
            "inference.stream.EmitTextDelta/emitted",
            "{result:?}"
        );
        result.direct_events[0].payload["segment_id"].clone()
    }

    #[test]
    fn the_eight_wrong_state_commands_answer_an_unknown_identity_with_no_current() {
        let cases = [
            (
                "inference.stream.CompleteGeneration",
                "generation_id",
                "GenerationStateConflict",
            ),
            (
                "inference.stream.SupersedeGeneration",
                "generation_id",
                "GenerationStateConflict",
            ),
            (
                "inference.stream.CancelGeneration",
                "generation_id",
                "GenerationStateConflict",
            ),
            (
                "inference.stream.CommitSegment",
                "segment_id",
                "SegmentStateConflict",
            ),
            (
                "inference.stream.QueueForSynthesis",
                "segment_id",
                "SegmentStateConflict",
            ),
            (
                "inference.stream.CompleteSynthesis",
                "segment_id",
                "SegmentStateConflict",
            ),
            (
                "inference.stream.AcknowledgePlayback",
                "segment_id",
                "SegmentStateConflict",
            ),
            (
                "inference.stream.DiscardSegment",
                "segment_id",
                "SegmentStateConflict",
            ),
        ];
        for (name, field, error) in cases {
            let mut stores = Stores::default();
            let result = run(&mut stores, name, &[(field, text(UNKNOWN))]);
            assert_eq!(branch(&result), format!("{name}/wrong-state"));
            let carried = result.error.expect("a declared error");
            assert_eq!(
                carried.error.to_string(),
                format!("inference.stream.{error}")
            );
            assert!(
                carried.fields.is_empty(),
                "{name}: no record holds a state to report: {:?}",
                carried.fields
            );
            assert!(result.direct_events.is_empty(), "{name} published an event");
        }
    }

    #[test]
    fn a_conflict_carries_the_state_the_record_holds() {
        let mut stores = Stores::default();
        let generation = started(&mut stores);
        let segment = emitted(&mut stores, &generation, 1);

        let refused = run(
            &mut stores,
            "inference.stream.QueueForSynthesis",
            &[("segment_id", segment.clone())],
        );
        assert_eq!(
            branch(&refused),
            "inference.stream.QueueForSynthesis/wrong-state"
        );
        assert_eq!(
            refused.error.expect("a declared error").fields["current"],
            text("Tentative")
        );

        let completed = run(
            &mut stores,
            "inference.stream.CompleteGeneration",
            &[("generation_id", generation.clone())],
        );
        assert_eq!(
            branch(&completed),
            "inference.stream.CompleteGeneration/completed"
        );
        let again = run(
            &mut stores,
            "inference.stream.CompleteGeneration",
            &[("generation_id", generation)],
        );
        assert_eq!(
            again.error.expect("a declared error").fields["current"],
            text("Completed")
        );
    }

    #[test]
    fn supersede_through_the_target_discards_the_unplayed_segments_and_the_views_show_it() {
        let mut stores = Stores::default();
        let generation = started(&mut stores);
        let tentative = emitted(&mut stores, &generation, 1);
        let played = emitted(&mut stores, &generation, 2);
        for name in [
            "inference.stream.CommitSegment",
            "inference.stream.QueueForSynthesis",
            "inference.stream.CompleteSynthesis",
            "inference.stream.AcknowledgePlayback",
        ] {
            let result = run(&mut stores, name, &[("segment_id", played.clone())]);
            assert!(result.error.is_none(), "{name}: {result:?}");
        }

        let result = run(
            &mut stores,
            "inference.stream.SupersedeGeneration",
            &[("generation_id", generation.clone())],
        );
        assert_eq!(
            branch(&result),
            "inference.stream.SupersedeGeneration/superseded"
        );
        let published: Vec<String> = result
            .direct_events
            .iter()
            .map(|event| event.event.to_string())
            .collect();
        assert_eq!(published, ["inference.stream.GenerationSuperseded"]);

        let generations = rows(&mut stores, "inference.stream.Generations");
        assert_eq!(generations.len(), 1);
        assert_eq!(generations[0]["generation_id"], generation);
        assert_eq!(generations[0]["state"], text("Superseded"));
        assert_eq!(generations[0]["epoch"], Node::Number(Number::from(1_i64)));

        let segments = rows(&mut stores, "inference.stream.Segments");
        let state_of = |segment: &Node| {
            segments
                .iter()
                .find(|row| &row["segment_id"] == segment)
                .expect("a row")["state"]
                .clone()
        };
        assert_eq!(state_of(&tentative), text("Discarded"));
        assert_eq!(state_of(&played), text("Played"));

        let stale = run(
            &mut stores,
            "inference.stream.EmitTextDelta",
            &[
                ("generation_id", generation),
                ("sequence", Node::Number(Number::from(3_i64))),
                ("text", text("stale")),
            ],
        );
        assert_eq!(
            branch(&stale),
            "inference.stream.EmitTextDelta/stale-generation"
        );
        assert_eq!(
            stale.error.expect("a declared error").error.to_string(),
            "inference.stream.GenerationStateConflict"
        );
    }

    #[test]
    fn a_delta_into_an_unknown_generation_is_refused_with_its_identity() {
        let mut stores = Stores::default();
        let result = run(
            &mut stores,
            "inference.stream.EmitTextDelta",
            &[
                ("generation_id", text(UNKNOWN)),
                ("sequence", Node::Number(Number::from(1_i64))),
                ("text", text("lost")),
            ],
        );
        assert_eq!(
            branch(&result),
            "inference.stream.EmitTextDelta/unknown-generation"
        );
        let carried = result.error.expect("a declared error");
        assert_eq!(
            carried.error.to_string(),
            "inference.stream.GenerationNotFound"
        );
        assert_eq!(carried.fields["generation_id"], text(UNKNOWN));
    }

    /// Two runs of one Planned pipeline holding one node, arranged in the graph store: the first
    /// Created, the second Active. Returns the two run identities and the node's.
    fn runs(stores: &mut Stores) -> (Node, Node, Node) {
        let profile_id = stores
            .policy
            .declare_profile(DeclareProfile {
                kind: ProfileKind::OfflineStrict,
                egress: Egress::Denied,
                admitted: Vec::new(),
            })
            .expect("a row of the table")
            .profile_id;
        let catalog = GraphCatalog {
            policy: &stores.policy,
            model: &stores.model,
        };
        let graph = &mut stores.graph;
        let pipeline_id = graph
            .declare_pipeline(
                DeclarePipeline {
                    name: "p".to_owned(),
                    schema_revision: "1".to_owned(),
                    mode: InteractionMode::TurnBased,
                    profile_id,
                    required_capabilities: Vec::new(),
                },
                &catalog,
            )
            .expect("the profile is declared")
            .pipeline_id;
        let node_id = graph
            .add_node(AddNode {
                pipeline_id,
                name: "llm".to_owned(),
                role: NodeRole::Model,
                requires: Vec::new(),
            })
            .expect("the pipeline is declared")
            .node_id;
        graph
            .validate_pipeline(ValidatePipeline { pipeline_id })
            .expect("a lone node is valid");
        graph
            .plan_pipeline(PlanPipeline { pipeline_id }, &catalog)
            .expect("nothing is required");
        let created = graph.create_run(pipeline_id).expect("planned").run_id;
        let active = graph.create_run(pipeline_id).expect("planned").run_id;
        graph.validate_run(active).expect("Created");
        graph.prepare_run(active).expect("Validating");
        graph.activate_run(active).expect("Preparing");
        let node = |id: String| text(&id);
        (
            node(created.to_string()),
            node(active.to_string()),
            node(node_id.to_string()),
        )
    }

    #[test]
    fn start_generation_refuses_an_unknown_run_then_node_then_a_run_that_is_not_active() {
        let mut stores = Stores::default();
        let (created, active, node_id) = runs(&mut stores);
        let start = |stores: &mut Stores, run_id: &Node, node_id: &Node| {
            run(
                stores,
                "inference.stream.StartGeneration",
                &[
                    ("run_id", run_id.clone()),
                    ("node_id", node_id.clone()),
                    ("epoch", Node::Number(Number::from(1_i64))),
                ],
            )
        };

        let unknown_run = start(&mut stores, &text(UNKNOWN), &text(UNKNOWN));
        assert_eq!(
            branch(&unknown_run),
            "inference.stream.StartGeneration/unknown-run"
        );
        let carried = unknown_run.error.expect("a declared error");
        assert_eq!(carried.error.to_string(), "inference.graph.RunNotFound");
        assert_eq!(
            carried.fields,
            BTreeMap::from([("run_id".to_owned(), text(UNKNOWN))])
        );

        let unknown_node = start(&mut stores, &created, &text(UNKNOWN));
        assert_eq!(
            branch(&unknown_node),
            "inference.stream.StartGeneration/unknown-node",
            "an unknown node is refused before the Created run's state"
        );
        let carried = unknown_node.error.expect("a declared error");
        assert_eq!(carried.error.to_string(), "inference.graph.NodeNotFound");
        assert_eq!(
            carried.fields,
            BTreeMap::from([("node_id".to_owned(), text(UNKNOWN))])
        );

        let not_active = start(&mut stores, &created, &node_id);
        assert_eq!(
            branch(&not_active),
            "inference.stream.StartGeneration/run-not-active"
        );
        let carried = not_active.error.expect("a declared error");
        assert_eq!(
            carried.error.to_string(),
            "inference.graph.RunStateConflict"
        );
        assert_eq!(
            carried.fields,
            BTreeMap::from([("current".to_owned(), text("Created"))])
        );
        assert!(stores.stream.generations().is_empty());
        assert!(stores.stream.events().is_empty());

        let started = start(&mut stores, &active, &node_id);
        assert_eq!(branch(&started), "inference.stream.StartGeneration/started");
        let generations = rows(&mut stores, "inference.stream.Generations");
        assert_eq!(generations.len(), 1);
        assert_eq!(generations[0]["run_id"], active);
        assert_eq!(generations[0]["node_id"], node_id);
    }

    #[test]
    fn an_undeclared_or_missing_field_is_an_error_and_moves_nothing() {
        let mut stores = Stores::default();
        let generation = started(&mut stores);
        let extra = execute_command(
            &mut stores,
            &command(
                "inference.stream.CompleteGeneration",
                &[("generation_id", generation), ("reason", text("x"))],
            ),
        );
        assert!(
            matches!(&extra, Err(TargetError::Unavailable { .. })),
            "{extra:?}"
        );
        let missing = execute_command(&mut stores, &command("inference.stream.CommitSegment", &[]));
        assert!(
            matches!(&missing, Err(TargetError::Unavailable { .. })),
            "{missing:?}"
        );
        assert_eq!(
            rows(&mut stores, "inference.stream.Generations")[0]["state"],
            text("Active")
        );
    }

    #[test]
    fn observed_events_are_the_store_events_in_emission_order() {
        let mut stores = Stores::default();
        let generation = started(&mut stores);
        let segment = emitted(&mut stores, &generation, 5);
        let observe = |stores: &mut Stores, name: &str| {
            observe_events(
                stores,
                &EventObservationRequest {
                    event: name.parse::<EventRef>().expect("an event name"),
                    correlation: correlation(),
                    deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
                },
            )
            .unwrap_or_else(|error| panic!("`{name}` is observed: {error}"))
        };

        let started_events = observe(&mut stores, "inference.stream.GenerationStarted");
        assert_eq!(started_events.len(), 1);
        assert_eq!(started_events[0].sequence, Some(1));
        assert_eq!(started_events[0].payload["generation_id"], generation);

        let deltas = observe(&mut stores, "inference.stream.TextDelta");
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].sequence, Some(2));
        assert_eq!(deltas[0].payload["segment_id"], segment);
        assert_eq!(
            deltas[0].payload["sequence"],
            Node::Number(Number::from(5_i64))
        );
        assert_eq!(deltas[0].payload["text"], text("hello"));

        assert!(observe(&mut stores, "inference.stream.AudioPlayed").is_empty());
    }

    fn observed(stores: &mut Stores, name: &str) -> Vec<ess_conformance::target::ObservedEvent> {
        observe_events(
            stores,
            &EventObservationRequest {
                event: name.parse::<EventRef>().expect("an event name"),
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .unwrap_or_else(|error| panic!("`{name}` is observed: {error}"))
    }

    /// An Open turn in the store, opened past the run refusals, which these cases do not
    /// exercise; returns its identity as the runner captures it.
    fn turn(stores: &mut Stores) -> Node {
        let event = stores.stream.open_turn(OpenTurn {
            run_id: RunId::from(Uuid::from_u128(0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d)),
        });
        text(&event.turn_id.to_string())
    }

    /// A Hypothesis span of `turn`, emitted past the run refusals, which these cases do not
    /// exercise.
    fn span(stores: &mut Stores, turn: &Node, sequence: i64) -> Node {
        let Node::Text(turn_id) = turn else {
            panic!("a turn identity is a string: {turn:?}")
        };
        let event = stores
            .stream
            .emit_hypothesis(EmitHypothesis {
                run_id: RunId::from(Uuid::from_u128(0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d)),
                turn_id: turn_id.parse::<Uuid>().expect("a Uuid").into(),
                stream_id: StreamId::from(Uuid::from_u128(
                    0x2a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d,
                )),
                sequence,
                text: "hello wor".to_owned(),
            })
            .expect("an Open turn admits a span");
        text(&event.span_id.to_string())
    }

    #[test]
    fn complete_turn_and_commit_span_answer_an_unknown_identity_with_no_current() {
        for (name, field, error) in [
            (
                "inference.stream.CompleteTurn",
                "turn_id",
                "inference.stream.TurnStateConflict",
            ),
            (
                "inference.stream.CommitSpan",
                "span_id",
                "inference.stream.SpanStateConflict",
            ),
        ] {
            let mut stores = Stores::default();
            let result = run(&mut stores, name, &[(field, text(UNKNOWN))]);
            assert_eq!(branch(&result), format!("{name}/wrong-state"));
            let carried = result.error.expect("a declared error");
            assert_eq!(carried.error.to_string(), error);
            assert!(
                carried.fields.is_empty(),
                "{name}: no record holds a state to report: {:?}",
                carried.fields
            );
            assert!(result.direct_events.is_empty(), "{name} published an event");
        }
    }

    #[test]
    fn revise_span_answers_an_unknown_span_with_its_identity() {
        let mut stores = Stores::default();
        let result = run(
            &mut stores,
            "inference.stream.ReviseSpan",
            &[("span_id", text(UNKNOWN)), ("text", text("lost"))],
        );
        assert_eq!(branch(&result), "inference.stream.ReviseSpan/no-such-span");
        let carried = result.error.expect("a declared error");
        assert_eq!(carried.error.to_string(), "inference.stream.SpanNotFound");
        assert_eq!(carried.fields["span_id"], text(UNKNOWN));
        assert!(result.direct_events.is_empty());
    }

    #[test]
    fn open_turn_and_emit_hypothesis_read_the_run_store_and_refuse_an_identity_before_a_state() {
        let mut stores = Stores::default();
        let (created, active, _) = runs(&mut stores);
        let open = |stores: &mut Stores, run_id: &Node| {
            run(
                stores,
                "inference.stream.OpenTurn",
                &[("run_id", run_id.clone())],
            )
        };
        let unknown = open(&mut stores, &text(UNKNOWN));
        assert_eq!(branch(&unknown), "inference.stream.OpenTurn/unknown-run");
        assert_eq!(
            unknown.error.expect("a declared error").fields,
            BTreeMap::from([("run_id".to_owned(), text(UNKNOWN))])
        );
        let not_active = open(&mut stores, &created);
        assert_eq!(
            branch(&not_active),
            "inference.stream.OpenTurn/run-not-active"
        );
        let carried = not_active.error.expect("a declared error");
        assert_eq!(
            carried.error.to_string(),
            "inference.graph.RunStateConflict"
        );
        assert_eq!(
            carried.fields,
            BTreeMap::from([("current".to_owned(), text("Created"))])
        );
        assert!(stores.stream.turns().is_empty());
        let opened = open(&mut stores, &active);
        assert_eq!(branch(&opened), "inference.stream.OpenTurn/opened");
        let turn_id = opened.direct_events[0].payload["turn_id"].clone();
        assert_eq!(opened.direct_events[0].payload["run_id"], active);
        let completed = open(&mut stores, &active).direct_events[0].payload["turn_id"].clone();
        run(
            &mut stores,
            "inference.stream.CompleteTurn",
            &[("turn_id", completed.clone())],
        );

        let emit = |stores: &mut Stores, run_id: &Node, turn_id: &Node| {
            run(
                stores,
                "inference.stream.EmitHypothesis",
                &[
                    ("run_id", run_id.clone()),
                    ("turn_id", turn_id.clone()),
                    ("stream_id", text(UNKNOWN)),
                    ("sequence", Node::Number(Number::from(1_i64))),
                    ("text", text("hello")),
                ],
            )
        };
        for (run_id, turn_id, outcome, error) in [
            (
                text(UNKNOWN),
                text(UNKNOWN),
                "unknown-run",
                "inference.graph.RunNotFound",
            ),
            (
                created.clone(),
                text(UNKNOWN),
                "unknown-turn",
                "inference.stream.TurnNotFound",
            ),
            (
                created.clone(),
                completed.clone(),
                "run-not-active",
                "inference.graph.RunStateConflict",
            ),
            (
                active.clone(),
                completed.clone(),
                "turn-not-open",
                "inference.stream.TurnStateConflict",
            ),
        ] {
            let refused = emit(&mut stores, &run_id, &turn_id);
            assert_eq!(
                branch(&refused),
                format!("inference.stream.EmitHypothesis/{outcome}")
            );
            assert_eq!(
                refused.error.expect("a declared error").error.to_string(),
                error
            );
            assert!(refused.direct_events.is_empty());
        }
        assert!(stores.stream.spans().is_empty());

        let emitted = emit(&mut stores, &active, &turn_id);
        assert_eq!(branch(&emitted), "inference.stream.EmitHypothesis/emitted");
        let spans = rows(&mut stores, "inference.stream.Spans");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0]["run_id"], active);
        assert_eq!(spans[0]["turn_id"], turn_id);
    }

    #[test]
    fn a_committed_span_refuses_a_revision_through_the_target_and_its_text_is_unchanged() {
        let mut stores = Stores::default();
        let turn = turn(&mut stores);
        let span = span(&mut stores, &turn, 3);

        let revised = run(
            &mut stores,
            "inference.stream.ReviseSpan",
            &[("span_id", span.clone()), ("text", text("hello world"))],
        );
        assert_eq!(branch(&revised), "inference.stream.ReviseSpan/revised");
        assert_eq!(
            revised.direct_events[0].event.to_string(),
            "inference.stream.TranscriptRevision"
        );
        assert_eq!(revised.direct_events[0].payload["span_id"], span);
        assert_eq!(
            revised.direct_events[0].payload["text"],
            text("hello world")
        );

        let committed = run(
            &mut stores,
            "inference.stream.CommitSpan",
            &[("span_id", span.clone())],
        );
        assert_eq!(branch(&committed), "inference.stream.CommitSpan/committed");
        assert_eq!(
            committed.direct_events[0].payload["text"],
            text("hello world")
        );

        let refused = run(
            &mut stores,
            "inference.stream.ReviseSpan",
            &[("span_id", span.clone()), ("text", text("goodbye"))],
        );
        assert_eq!(branch(&refused), "inference.stream.ReviseSpan/immutable");
        let carried = refused.error.expect("a declared error");
        assert_eq!(
            carried.error.to_string(),
            "inference.stream.CommittedSpanImmutable"
        );
        assert_eq!(carried.fields["span_id"], span);
        assert!(refused.direct_events.is_empty());

        let again = run(
            &mut stores,
            "inference.stream.CommitSpan",
            &[("span_id", span.clone())],
        );
        assert_eq!(branch(&again), "inference.stream.CommitSpan/wrong-state");
        assert_eq!(
            again.error.expect("a declared error").fields["current"],
            text("Committed")
        );

        let spans = rows(&mut stores, "inference.stream.Spans");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0]["span_id"], span);
        assert_eq!(spans[0]["turn_id"], turn);
        assert_eq!(
            spans[0]["text"],
            text("hello world"),
            "the text is unchanged"
        );
        assert_eq!(spans[0]["state"], text("Committed"));
        assert_eq!(spans[0]["sequence"], Node::Number(Number::from(3_i64)));
    }

    #[test]
    fn complete_turn_completes_once_and_leaves_its_spans_as_they_are() {
        let mut stores = Stores::default();
        let turn = turn(&mut stores);
        let span = span(&mut stores, &turn, 1);

        let completed = run(
            &mut stores,
            "inference.stream.CompleteTurn",
            &[("turn_id", turn.clone())],
        );
        assert_eq!(
            branch(&completed),
            "inference.stream.CompleteTurn/completed"
        );
        assert_eq!(
            completed.direct_events[0].event.to_string(),
            "inference.stream.TurnCompleted"
        );
        assert_eq!(completed.direct_events[0].payload["turn_id"], turn);

        let again = run(
            &mut stores,
            "inference.stream.CompleteTurn",
            &[("turn_id", turn.clone())],
        );
        assert_eq!(branch(&again), "inference.stream.CompleteTurn/wrong-state");
        assert_eq!(
            again.error.expect("a declared error").fields["current"],
            text("Completed")
        );

        let turns = rows(&mut stores, "inference.stream.Turns");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0]["turn_id"], turn);
        assert_eq!(turns[0]["state"], text("Completed"));
        let spans = rows(&mut stores, "inference.stream.Spans");
        assert_eq!(spans[0]["span_id"], span);
        assert_eq!(spans[0]["state"], text("Hypothesis"));
    }

    #[test]
    fn the_turn_and_span_events_are_observed_with_every_declared_field() {
        let mut stores = Stores::default();
        let turn = turn(&mut stores);
        let span = span(&mut stores, &turn, 2);
        run(
            &mut stores,
            "inference.stream.CommitSpan",
            &[("span_id", span.clone())],
        );

        let opened = observed(&mut stores, "inference.stream.TurnOpened");
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].sequence, Some(1));
        assert_eq!(opened[0].payload["turn_id"], turn);
        assert!(opened[0].payload.contains_key("run_id"));

        let hypotheses = observed(&mut stores, "inference.stream.TranscriptHypothesis");
        assert_eq!(hypotheses.len(), 1);
        assert_eq!(hypotheses[0].sequence, Some(2));
        assert_eq!(hypotheses[0].payload["span_id"], span);
        assert_eq!(hypotheses[0].payload["turn_id"], turn);
        assert_eq!(
            hypotheses[0].payload["sequence"],
            Node::Number(Number::from(2_i64))
        );
        assert_eq!(hypotheses[0].payload["text"], text("hello wor"));

        let committed = observed(&mut stores, "inference.stream.TranscriptCommitted");
        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].sequence, Some(3));
        assert_eq!(committed[0].payload["text"], text("hello wor"));

        assert!(observed(&mut stores, "inference.stream.TranscriptRevision").is_empty());
        assert!(observed(&mut stores, "inference.stream.TurnCompleted").is_empty());
    }
}
