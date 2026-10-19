//! `inference.stream`: the generation and segment commands, views and events of
//! `ess/domains/stream.yaml`, answered by [`StreamDomain`].
//!
//! Each request field is read on its own ([`Input`]); the outcome, the error and the event come
//! from what the store returned. A command naming a generation or segment no record carries takes
//! its `wrong-state` outcome, whose error then carries no `current`, since no record holds a
//! state to report; `EmitTextDelta` is the one command that declares `unknown-generation` for it.
//! `SupersedeGeneration` and `CancelGeneration` publish their generation event only: the discard
//! of the generation's unplayed segments, which the store makes in the same outcome, emits none.
//!
//! `StartGeneration` is [`TargetError::Unsupported`]: its refusals read the run and node stores of
//! `inference.graph`, which do not exist yet, and a run is not faked. The span and turn commands
//! and views are unsupported until their stores land. `ess/SKIPPED.md` names the story for each.
//!
//! A command's consistency token is the target's
//! ([`Stores::token`]), and a read demanding a token the stores have not reached is an error
//! ([`Stores::require`]).

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ObservedEvent, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError, ViewRow,
};
use inference_core::generation::{GenerationStarted, GenerationStateConflict};
use inference_core::id::GenerationId;
use inference_testkit::stream::{
    EmitTextDeltaError, GenerationMoveError, GenerationsRow, SegmentMoveError, SegmentsRow,
    StreamDomain, StreamEvent,
};
use inference_text::{EmitTextDelta, SegmentId, SegmentStateConflict};

use super::{Input, Stores, integer_node, named_node, text_node, unsupported};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.stream";

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
const EVENTS: [&str; 10] = [
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

const GENERATION_STATE_CONFLICT: &str = "inference.stream.GenerationStateConflict";
const SEGMENT_STATE_CONFLICT: &str = "inference.stream.SegmentStateConflict";
const GENERATION_NOT_FOUND: &str = "inference.stream.GenerationNotFound";

const GENERATIONS: &str = "inference.stream.Generations";
const SEGMENTS: &str = "inference.stream.Segments";

/// The generation and segment stores of one scenario.
pub type State = StreamDomain;

pub fn execute_command(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let domain = &mut stores.stream;
    let name = request.command.to_string();
    let result = match name.as_str() {
        START_GENERATION => {
            return Err(TargetError::unsupported(
                format!("`{name}`"),
                "its refusals read the inference.graph run and node stores, which do not exist \
                 yet; ess/SKIPPED.md names the story that adds them",
            ));
        }
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
    Ok(result.with_consistency(stores.token()?))
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
    if name != GENERATIONS && name != SEGMENTS {
        return Err(unsupported(&name));
    }
    stores.require(&name, &request.consistency)?;
    let rows = if name == GENERATIONS {
        domain
            .generations_view()
            .iter()
            .map(generations_row)
            .collect::<Result<Vec<_>, _>>()?
    } else {
        domain
            .segments_view()
            .iter()
            .map(segments_row)
            .collect::<Result<Vec<_>, _>>()?
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
    let mut observed = Vec::new();
    for (position, event) in stores.stream.events().iter().enumerate() {
        let event = stream_event(event)?;
        if event.event == request.event {
            let sequence = u64::try_from(position + 1).unwrap_or(u64::MAX);
            observed.push(event.in_activity(request.correlation.clone()).at(sequence));
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

fn generation_started(event: &GenerationStarted) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(GENERATION_STARTED))
        .with("generation_id", named_node(&event.generation_id)?)
        .with("run_id", named_node(&event.run_id)?)
        .with("epoch", integer_node(event.epoch)))
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
    use inference_core::id::{NodeId, RunId};
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

    /// A generation in the store, started as `story:run-lifecycle`'s `StartGeneration` will start
    /// one once a run store decides its refusals; returns its identity as the runner captures it.
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

    #[test]
    fn start_generation_is_unsupported_until_a_run_store_decides_its_refusals() {
        let mut stores = Stores::default();
        let answer = execute_command(
            &mut stores,
            &command(
                "inference.stream.StartGeneration",
                &[
                    ("run_id", text(UNKNOWN)),
                    ("node_id", text(UNKNOWN)),
                    ("epoch", Node::Number(Number::from(1_i64))),
                ],
            ),
        );
        assert!(
            matches!(&answer, Err(error) if error.is_unsupported()),
            "{answer:?}"
        );
        assert!(stores.stream.generations().is_empty());
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
}
