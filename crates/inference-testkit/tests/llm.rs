//! §23.1 test 2 of `docs/design/inference.md`: LLM output streaming and input during output are
//! verified independently, against the three configurations of the mock generator. Output
//! streaming is observed as a `TextDelta` delivered while the generation is still Active, before
//! `GenerationCompleted`; input during output as input offered while the generation is Active
//! being accepted. Each is observed on a generation of its own, so one cannot stand in for the
//! other, and what each configuration was observed to do is what it claims.

#![cfg(feature = "llm")]

use std::collections::HashSet;
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use inference_core::generation::GenerationState;
use inference_core::id::{NodeId, RunId};
use inference_core::model::{Capability, ModelId};
use inference_core::usage::{Quantity, QuantityKind, UsageRecorded};
use inference_llm::llm_core::{
    Cancel, Item, Model, StreamEvent, TurnOutcome, TurnRequest, Usage, VecSink,
};
use inference_llm::{
    CapabilityNotClaimed, ConcurrencyLimitReached, GenerationEvent, GenerationRequest, InputRefused,
};
use inference_testkit::llm::{MockConfiguration, MockGenerator};

const SCRIPT: [&str; 3] = ["Hello", ", ", "world"];
const INPUT: &str = "and the weather?";

fn generator(configuration: MockConfiguration) -> MockGenerator {
    MockGenerator::new(configuration, ModelId::generate(), SCRIPT)
}

fn request() -> GenerationRequest {
    GenerationRequest {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
        turn: TurnRequest::new("mock", vec![Item::user("Greet the user.")]),
    }
}

/// Every event one generation delivered, each with the generation's state when the caller
/// received it.
fn deliveries(configuration: MockConfiguration) -> Vec<(GenerationEvent, GenerationState)> {
    let request = request();
    let mut handle = generator(configuration)
        .start(request.clone())
        .expect("a fresh generator runs nothing else");
    assert_eq!(handle.output().generation().run_id(), request.run_id);
    assert_eq!(handle.output().generation().node_id(), request.node_id);
    let mut seen = Vec::new();
    while let Some(event) = handle.next() {
        seen.push((event, handle.output().generation().state()));
    }
    seen
}

/// Whether a `TextDelta` reached the caller while the generation was Active, before
/// `GenerationCompleted`; panics unless the generation completed.
fn streams(configuration: MockConfiguration) -> bool {
    let seen = deliveries(configuration);
    let completed_at = seen
        .iter()
        .position(|(event, _)| matches!(event, GenerationEvent::Completed(_)))
        .unwrap_or_else(|| panic!("{configuration:?} never completed: {seen:?}"));
    seen[..completed_at].iter().any(|(event, state)| {
        matches!(event, GenerationEvent::TextDelta(_)) && *state == GenerationState::Active
    })
}

/// What happened to input offered to a generation while it was Active, after its first delivered
/// `TextDelta` if one arrived while Active and right after `GenerationStarted` otherwise.
struct Offer {
    answer: Result<(), InputRefused>,
    completed: bool,
    deltas_after: Vec<String>,
}

fn offer_input(configuration: MockConfiguration) -> Offer {
    let mut handle = generator(configuration)
        .start(request())
        .expect("a fresh generator runs nothing else");
    let started = handle.next();
    assert!(
        matches!(started, Some(GenerationEvent::Started(_))),
        "{configuration:?}: the first event is GenerationStarted, got {started:?}"
    );
    let streaming = configuration
        .capabilities()
        .contains(&Capability::OutputStreaming);
    let mut buffered = Vec::new();
    if streaming {
        for event in handle.by_ref() {
            let is_delta = matches!(event, GenerationEvent::TextDelta(_));
            buffered.push(event);
            if is_delta {
                break;
            }
        }
    }
    assert_eq!(
        handle.output().generation().state(),
        GenerationState::Active,
        "{configuration:?}: input is offered while the generation is Active; delivered so far {buffered:?}"
    );
    let answer = handle.offer_input(INPUT.to_owned());
    let mut completed = false;
    let mut deltas_after = Vec::new();
    for event in handle.by_ref() {
        match event {
            GenerationEvent::TextDelta(delta) => deltas_after.push(delta.text),
            GenerationEvent::Completed(_) => completed = true,
            _ => {}
        }
    }
    Offer {
        answer,
        completed,
        deltas_after,
    }
}

#[test]
fn output_streaming_is_verified_by_a_delta_delivered_before_completion() {
    assert!(
        streams(MockConfiguration::StreamingOnly),
        "claiming output_streaming, a TextDelta reaches the caller before GenerationCompleted: {:?}",
        deliveries(MockConfiguration::StreamingOnly)
    );

    let seen = deliveries(MockConfiguration::Neither);
    assert!(
        !streams(MockConfiguration::Neither),
        "claiming neither, no TextDelta reaches the caller before GenerationCompleted: {seen:?}"
    );
    let texts: Vec<&str> = seen
        .iter()
        .filter_map(|(event, _)| match event {
            GenerationEvent::TextDelta(delta) => Some(delta.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        [SCRIPT.concat()],
        "claiming neither, the whole script arrives as one segment at completion"
    );
}

#[test]
fn concurrent_input_output_is_verified_by_input_accepted_while_active() {
    let accepted = offer_input(MockConfiguration::StreamingAndConcurrentInput);
    assert_eq!(
        accepted.answer,
        Ok(()),
        "claiming concurrent_input_output, input offered while Active is accepted"
    );
    assert!(
        accepted.deltas_after.iter().any(|text| text == INPUT),
        "the accepted input is reflected in a later delta: {:?}",
        accepted.deltas_after
    );
    assert!(accepted.completed, "the generation still completes");

    let refused = offer_input(MockConfiguration::StreamingOnly);
    assert_eq!(
        refused.answer,
        Err(InputRefused::CapabilityNotClaimed(CapabilityNotClaimed {
            capability: Capability::ConcurrentInputOutput,
        })),
        "claiming output_streaming only, input offered while Active is refused naming the missing capability"
    );
    assert!(
        refused.completed,
        "the refused generation still completes its output"
    );
    assert!(
        !refused.deltas_after.iter().any(|text| text.contains(INPUT)),
        "refused input reaches no delta: {:?}",
        refused.deltas_after
    );
}

#[test]
fn each_configuration_claims_exactly_what_it_was_observed_to_do() {
    for configuration in MockConfiguration::ALL {
        let mut observed = HashSet::new();
        if streams(configuration) {
            observed.insert(Capability::OutputStreaming);
        }
        let offer = offer_input(configuration);
        assert!(offer.completed, "{configuration:?} completes");
        if offer.answer.is_ok() && offer.deltas_after.iter().any(|text| text == INPUT) {
            observed.insert(Capability::ConcurrentInputOutput);
        }
        let generator = generator(configuration);
        let claimed: HashSet<Capability> =
            generator.adapter().capabilities().iter().copied().collect();
        assert_eq!(
            claimed, observed,
            "{configuration:?} claims what it was observed to do"
        );
    }
}

#[test]
fn each_configuration_model_declares_its_streaming_claim_and_no_input_during_output() {
    for configuration in MockConfiguration::ALL {
        let generator = generator(configuration);
        let declared = generator.generator().model().capabilities();
        assert_eq!(
            (declared.output_streaming, declared.input_during_output),
            (
                generator
                    .adapter()
                    .capabilities()
                    .contains(&Capability::OutputStreaming),
                false
            ),
            "{configuration:?}: the model streams as claimed and, as every llm protocol, takes no \
             input during output"
        );
        assert_eq!(declared, &configuration.declared());
    }
}

/// The outcome of one turn of `model` on `request`, and the text deltas it showed.
fn one_turn(model: &dyn Model, request: &TurnRequest) -> (TurnOutcome, Vec<String>) {
    let mut sink = VecSink::new(64, 1 << 16);
    let cancel = Cancel::new();
    let outcome = {
        let mut turn = pin!(model.turn(request, &mut sink, &cancel));
        match turn.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(outcome) => outcome.expect("the mock answers"),
            Poll::Pending => panic!("the mock answers without waiting"),
        }
    };
    let deltas = sink
        .events()
        .iter()
        .filter_map(|event| match event {
            StreamEvent::TextDelta { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    (outcome, deltas)
}

#[test]
fn a_mock_model_that_does_not_stream_shows_no_delta_and_returns_its_text_in_the_outcome() {
    let request = TurnRequest::new("mock", vec![Item::user("Greet the user.")]);
    for configuration in MockConfiguration::ALL {
        let generator = generator(configuration);
        let model = generator.generator().model();
        let (outcome, deltas) = one_turn(model, &request);
        let shown: Vec<String> = if model.capabilities().output_streaming {
            SCRIPT.iter().map(|piece| (*piece).to_owned()).collect()
        } else {
            Vec::new()
        };
        assert_eq!(deltas, shown, "{configuration:?}: the deltas shown");
        assert_eq!(
            outcome.items,
            vec![Item::assistant(SCRIPT.concat())],
            "{configuration:?}: the turn's text is in its outcome"
        );
    }
}

/// How many generations of `generator` run at once, starting until a start is refused or `bound`
/// run.
fn observed_concurrency(generator: &mut MockGenerator, bound: usize) -> usize {
    let mut running = Vec::new();
    while running.len() < bound {
        match generator.start(request()) {
            Ok(handle) => running.push(handle),
            Err(_) => break,
        }
    }
    running.len()
}

#[test]
fn a_second_concurrent_generation_is_refused_with_the_concurrency_limit() {
    for configuration in MockConfiguration::ALL {
        let mut generator = generator(configuration);
        let first = generator
            .start(request())
            .expect("a fresh generator runs nothing else");
        assert_eq!(
            generator.start(request()).map(|_| ()),
            Err(ConcurrencyLimitReached { max_concurrency: 1 }),
            "{configuration:?}: a second generation while the first runs is refused"
        );
        let mut clone = generator.clone();
        assert_eq!(
            clone.start(request()).map(|_| ()),
            Err(ConcurrencyLimitReached { max_concurrency: 1 }),
            "{configuration:?}: a clone is the same adapter and shares its limit"
        );
        drop(first);
        assert!(
            generator.start(request()).is_ok(),
            "{configuration:?}: once the first generation ends, another starts"
        );
    }
}

/// The quantity of `kind` in `recorded`; panics unless it carries exactly one.
fn quantity(recorded: &UsageRecorded, kind: QuantityKind) -> Quantity {
    let found: Vec<&Quantity> = recorded
        .quantities
        .iter()
        .filter(|quantity| quantity.kind() == kind)
        .collect();
    assert_eq!(found.len(), 1, "one {kind:?} quantity: {recorded:?}");
    *found[0]
}

const TOKEN_KINDS: [QuantityKind; 4] = [
    QuantityKind::InputTextTokens,
    QuantityKind::OutputTextTokens,
    QuantityKind::CachedInputTokens,
    QuantityKind::CacheCreationInputTokens,
];

#[test]
fn a_generation_of_two_turns_records_the_reported_counts_and_the_missing_ones_as_unknown() {
    let reported = Usage {
        input_tokens: Some(12),
        output_tokens: Some(3),
        cached_input_tokens: Some(4),
        ..Usage::default()
    };
    let mut generator =
        generator(MockConfiguration::StreamingAndConcurrentInput).reporting([Some(reported), None]);
    let adapter = generator.adapter().clone();
    let mut handle = generator
        .start(request())
        .expect("a fresh generator runs nothing else");
    let mut events = Vec::new();
    for event in handle.by_ref() {
        let ended = matches!(event, GenerationEvent::Usage(_));
        events.push(event);
        if ended {
            break;
        }
    }
    handle
        .offer_input(INPUT.to_owned())
        .expect("the configuration claims concurrent input");
    events.extend(handle.by_ref());

    let records: Vec<(usize, UsageRecorded)> = events
        .iter()
        .enumerate()
        .filter_map(|(at, event)| match event {
            GenerationEvent::Usage(recorded) => Some((at, recorded.clone())),
            _ => None,
        })
        .collect();
    let [(first_at, first), (second_at, second)] = records.as_slice() else {
        panic!("one record per turn: {events:?}")
    };
    let texts: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(at, event)| matches!(event, GenerationEvent::TextDelta(_)).then_some(at))
        .collect();
    assert!(
        texts[..SCRIPT.len()].iter().all(|at| at < first_at)
            && texts[SCRIPT.len()..]
                .iter()
                .all(|at| first_at < at && at < second_at),
        "each record follows the events of its turn: {events:?}"
    );
    assert!(matches!(events.last(), Some(GenerationEvent::Completed(_))));

    assert_eq!(
        TOKEN_KINDS.map(|kind| quantity(first, kind)),
        [
            Quantity::provider_reported(QuantityKind::InputTextTokens, 12),
            Quantity::provider_reported(QuantityKind::OutputTextTokens, 3),
            Quantity::provider_reported(QuantityKind::CachedInputTokens, 4),
            Quantity::unknown(QuantityKind::CacheCreationInputTokens),
        ]
    );
    assert_eq!(
        TOKEN_KINDS.map(|kind| quantity(second, kind)),
        TOKEN_KINDS.map(Quantity::unknown),
        "the turn reporting no usage counts every token unknown"
    );
    for record in [first, second] {
        assert_eq!(
            quantity(record, QuantityKind::Requests),
            Quantity::measured(QuantityKind::Requests, 1)
        );
        assert_eq!(record.adapter_id, adapter.adapter_id());
        assert_eq!(record.model_id, adapter.model_id());
        assert!(record.r#final);
    }
}

/// `story:llm-usage-on-cancel`: a generation on the mock superseded, or cancelled, during its
/// second turn delivers two usage records, the first final and the second, of the turn it cut,
/// not final, carrying the counts the model reported for it.
#[test]
fn a_generation_cut_during_its_second_turn_delivers_two_usage_records_the_second_not_final() {
    let first = Usage {
        input_tokens: Some(12),
        output_tokens: Some(3),
        ..Usage::default()
    };
    let cut = Usage {
        input_tokens: Some(20),
        output_tokens: Some(1),
        ..Usage::default()
    };
    for supersede in [true, false] {
        let mut generator = generator(MockConfiguration::StreamingAndConcurrentInput)
            .reporting([Some(first.clone()), Some(cut.clone())])
            .holding(1);
        let mut handle = generator
            .start(request())
            .expect("a fresh generator runs nothing else");
        let mut events = Vec::new();
        for event in handle.by_ref() {
            let ended = matches!(event, GenerationEvent::Usage(_));
            events.push(event);
            if ended {
                break;
            }
        }
        handle
            .offer_input(INPUT.to_owned())
            .expect("the configuration claims concurrent input");
        let mut cx = Context::from_waker(Waker::noop());
        while let Poll::Ready(Some(event)) = handle.poll_event(&mut cx) {
            events.push(event);
        }
        assert!(
            matches!(events.last(), Some(GenerationEvent::SegmentCommitted(_))),
            "the second turn streamed its text and still runs: {events:?}"
        );
        assert_eq!(
            handle.output().generation().state(),
            GenerationState::Active
        );
        if supersede {
            handle.supersede().expect("Active supersedes");
        } else {
            handle.cancel().expect("Active cancels");
        }
        let after: Vec<GenerationEvent> = handle.by_ref().collect();
        assert!(
            after
                .iter()
                .all(|event| matches!(event, GenerationEvent::Usage(_))),
            "supersede {supersede}: nothing but usage after the cut: {after:?}"
        );
        events.extend(after);

        let records: Vec<&UsageRecorded> = events
            .iter()
            .filter_map(|event| match event {
                GenerationEvent::Usage(recorded) => Some(recorded),
                _ => None,
            })
            .collect();
        let [ended, partial] = records.as_slice() else {
            panic!("supersede {supersede}: two usage records: {events:?}")
        };
        assert!(ended.r#final, "the first turn ended");
        assert!(!partial.r#final, "the cut turn did not end: {partial:?}");
        assert_eq!(
            TOKEN_KINDS.map(|kind| quantity(partial, kind)),
            [
                Quantity::provider_reported(QuantityKind::InputTextTokens, 20),
                Quantity::provider_reported(QuantityKind::OutputTextTokens, 1),
                Quantity::unknown(QuantityKind::CachedInputTokens),
                Quantity::unknown(QuantityKind::CacheCreationInputTokens),
            ]
        );
        assert_eq!(
            quantity(partial, QuantityKind::Requests),
            Quantity::measured(QuantityKind::Requests, 1)
        );
        assert!(partial.started_at_ms <= partial.ended_at_ms);
    }
}

/// A turn the mock holds and whose usage it never reported is still a model call: cut, it yields
/// one record of one request, every token count unknown, not final.
#[test]
fn a_cut_turn_whose_usage_was_never_reported_counts_one_request_of_unknown_tokens() {
    let mut generator = generator(MockConfiguration::StreamingOnly).holding(0);
    let mut handle = generator
        .start(request())
        .expect("a fresh generator runs nothing else");
    let mut cx = Context::from_waker(Waker::noop());
    let mut events = Vec::new();
    while let Poll::Ready(Some(event)) = handle.poll_event(&mut cx) {
        events.push(event);
    }
    assert!(
        events
            .iter()
            .any(|event| matches!(event, GenerationEvent::TextDelta(_))),
        "{events:?}"
    );
    handle.supersede().expect("Active supersedes");
    let after: Vec<GenerationEvent> = handle.by_ref().collect();
    let [GenerationEvent::Usage(recorded)] = after.as_slice() else {
        panic!("one record and no text: {after:?}")
    };
    assert!(!recorded.r#final, "its counts are unknown");
    assert_eq!(
        quantity(recorded, QuantityKind::Requests),
        Quantity::measured(QuantityKind::Requests, 1)
    );
    assert_eq!(
        TOKEN_KINDS.map(|kind| quantity(recorded, kind)),
        TOKEN_KINDS.map(Quantity::unknown)
    );
}

#[test]
fn the_mock_reports_no_usage_by_default() {
    for configuration in MockConfiguration::ALL {
        let records: Vec<UsageRecorded> = deliveries(configuration)
            .into_iter()
            .filter_map(|(event, _)| match event {
                GenerationEvent::Usage(recorded) => Some(recorded),
                _ => None,
            })
            .collect();
        let [record] = records.as_slice() else {
            panic!("{configuration:?}: one turn, one record: {records:?}")
        };
        assert_eq!(
            TOKEN_KINDS.map(|kind| quantity(record, kind)),
            TOKEN_KINDS.map(Quantity::unknown),
            "{configuration:?}"
        );
    }
}

#[test]
fn each_configuration_runs_at_most_the_concurrency_it_declares() {
    for configuration in MockConfiguration::ALL {
        let mut generator = generator(configuration);
        let declared = usize::try_from(generator.adapter().max_concurrency())
            .expect("a declared concurrency is positive");
        assert_eq!(
            observed_concurrency(&mut generator, declared + 3),
            declared,
            "{configuration:?}: the declared maximum concurrency is the observed one"
        );
    }
}
