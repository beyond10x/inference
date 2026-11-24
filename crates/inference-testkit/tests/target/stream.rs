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
use ess_primitives::node::Node;
use inference_core::generation::{
    GenerationStarted, GenerationStateConflict, StartGeneration, StopReason,
};
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

use super::{Input, Stores, integer_node, named_node, text_node, unreadable, unsupported};

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
const REFUSE_GENERATION: &str = "inference.stream.RefuseGeneration";
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
const GENERATION_REFUSED: &str = "inference.stream.GenerationRefused";
const TEXT_DELTA: &str = "inference.stream.TextDelta";
const TEXT_SEGMENT_COMMITTED: &str = "inference.stream.TextSegmentCommitted";
const SEGMENT_QUEUED: &str = "inference.stream.SegmentQueued";
const SYNTHESIS_COMPLETED: &str = "inference.stream.SynthesisCompleted";
const AUDIO_PLAYED: &str = "inference.stream.AudioPlayed";
const SEGMENT_DISCARDED: &str = "inference.stream.SegmentDiscarded";

/// Every event this module observes.
const EVENTS: [&str; 18] = [
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
    GENERATION_REFUSED,
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
            let (generation_id, stop_reason) = completion(request)?;
            let answer = domain.complete_generation(generation_id, stop_reason);
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
        REFUSE_GENERATION => refuse_generation(domain, request)?,
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

/// The fields of `CompleteGeneration`: the generation and, when the request names one, the
/// stop reason.
fn completion(
    request: &SemanticCommandRequest,
) -> Result<(GenerationId, Option<StopReason>), TargetError> {
    let mut input = Input::of(request);
    let generation_id = input.named("generation_id")?;
    let stop_reason = input
        .optional("stop_reason")
        .map(|node| match node {
            Node::Text(text) => serde_json::from_value(serde_json::Value::String(text.clone()))
                .map_err(|error| {
                    unreadable(
                        format!("reading `{COMPLETE_GENERATION}`"),
                        format!("`stop_reason`: {error}: {node:?}"),
                    )
                }),
            other => Err(unreadable(
                format!("reading `{COMPLETE_GENERATION}`"),
                format!("`stop_reason` is not a stop reason: {other:?}"),
            )),
        })
        .transpose()?;
    input.finish()?;
    Ok((generation_id, stop_reason))
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

/// The answer of `RefuseGeneration`: `refused` and its event, `unknown-generation` or
/// `stale-generation`.
fn refuse_generation(
    domain: &mut StreamDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let generation_id = input.named("generation_id")?;
    let text = input.text("text")?;
    input.finish()?;
    Ok(match domain.refuse_generation(generation_id, text) {
        Ok(event) => SemanticCommandResult::took(outcome(REFUSE_GENERATION, "refused")).emitting(
            stream_event(&StreamEvent::GenerationRefused(event))?
                .in_activity(request.correlation.clone()),
        ),
        Err(GenerationMoveError::Unknown(error)) => {
            SemanticCommandResult::took(outcome(REFUSE_GENERATION, "unknown-generation"))
                .with_error(
                    DeclaredErrorValue::new(parse::<ErrorRef>(GENERATION_NOT_FOUND))
                        .with("generation_id", named_node(&error.generation_id)?),
                )
        }
        Err(GenerationMoveError::Conflict(conflict)) => {
            SemanticCommandResult::took(outcome(REFUSE_GENERATION, "stale-generation"))
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
            generation(GENERATION_COMPLETED, event.generation_id)?.with(
                "stop_reason",
                event
                    .stop_reason
                    .as_ref()
                    .map_or(Ok(Node::Null), named_node)?,
            )
        }
        StreamEvent::GenerationSuperseded(event) => {
            generation(GENERATION_SUPERSEDED, event.generation_id)?
        }
        StreamEvent::GenerationCancelled(event) => {
            generation(GENERATION_CANCELLED, event.generation_id)?
        }
        StreamEvent::GenerationRefused(event) => {
            generation(GENERATION_REFUSED, event.generation_id)?
                .with("text", text_node(&event.text))
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
