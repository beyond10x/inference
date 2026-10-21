//! Adversary pass 1 on `story:usage-pricing`: unknown never becomes zero, exact money, totals and
//! the session's refused records.
//!
//! | case | kind |
//! |---|---|
//! | [`a_partial_record_never_prices_its_compute_as_final`] | failing: a non-final `local_compute_ms` is priced as a final count |
//! | [`a_negative_count_among_others_of_its_kind_is_unknown`] | failing: a negative count is netted against a positive one |
//! | [`an_unbound_adapter_with_no_quantity_is_not_a_complete_zero`] | failing: an adapter with no price gives a complete total of `0` |
//! | the other cases | guards |

#![cfg(feature = "cost")]
#![allow(dead_code)]

mod text;

use std::thread;

use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{AdapterId, ModelId};
use inference_core::usage::{
    CostLine, CostRecorded, Quantity, QuantityKind, RecordUsage, UnknownReason, UsageRecord,
    UsageRecorded,
};
use inference_facade::cost::{
    BillingKind, ComputePriceBinding, CostError, Id, ModelPriceBinding, PriceBinding, PriceBook,
    PriceError, Pricing, price,
};
use inference_facade::live::LiveSessions;
use inference_llm::llm_core::Usage;
use inference_pipeline::{InteractionMode, Node, NodeImplementation, PortId};
use inference_testkit::cascade::CascadePayload;
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

use text::{INGRESS, block_on, generator, planned_with_usage, reply};

const UPSTREAM: &str = "served";

/// One model price `m` (serving model `s`, binding revision `1`, upstream [`UPSTREAM`]) with the
/// rates given, and one compute price `pod`, 3.6 per hour.
fn book_with(input: &str, cached: Option<&str>, output: &str) -> PriceBook {
    let cached = cached.map_or(String::new(), |rate| {
        format!(r#""cached_input": {{"amount": "{rate}", "per_units": 1000000}},"#)
    });
    let document = format!(
        r#"{{
            "format": "llm.prices/1",
            "revision": "adv-1",
            "source": "adversary",
            "as_of_unix_ms": 1792540800000,
            "currency": "EUR",
            "models": [{{
                "id": "m",
                "serving_model_id": "s",
                "binding_revision": "1",
                "upstream_model": "{UPSTREAM}",
                "input": {input},
                {cached}
                "cache_creation_input": {{"amount": "2.5", "per_units": 1000000}},
                "output": {output}
            }}],
            "compute": [{{
                "id": "pod",
                "hosting_provider": "local",
                "account": "lab",
                "resource_class": "cpu-pod",
                "rate": {{"amount": "3.6", "per_units": 3600000}}
            }}]
        }}"#
    );
    PriceBook::parse_json(&document).expect("a valid price document")
}

fn book() -> PriceBook {
    book_with(
        r#"{"amount": "2", "per_units": 1000000}"#,
        Some("0.5"),
        r#"{"amount": "8", "per_units": 1000000}"#,
    )
}

fn id(value: &str) -> Id {
    Id::new(value).expect("printable")
}

fn model(revision: &str) -> ModelPriceBinding {
    ModelPriceBinding {
        price_id: id("m"),
        serving_model_id: id("s"),
        binding_revision: id(revision),
        billing: BillingKind::Metered,
    }
}

fn compute() -> ComputePriceBinding {
    ComputePriceBinding {
        price_id: id("pod"),
        hosting_provider: id("local"),
        account: id("lab"),
        resource_class: id("cpu-pod"),
    }
}

fn binding(charges: &[QuantityKind]) -> PriceBinding {
    PriceBinding {
        charges: charges.to_vec(),
        model: Some(model("1")),
        compute: Some(compute()),
    }
}

const TOKENS: [QuantityKind; 4] = [
    QuantityKind::InputTextTokens,
    QuantityKind::CachedInputTokens,
    QuantityKind::CacheCreationInputTokens,
    QuantityKind::OutputTextTokens,
];

fn record(adapter: AdapterId, quantities: Vec<Quantity>, is_final: bool) -> UsageRecorded {
    let mut input = RecordUsage::new(adapter, ModelId::generate(), 0, 10, quantities);
    input.upstream_model = Some(UPSTREAM.to_owned());
    input.r#final = is_final;
    UsageRecord::record(input).1
}

fn reported(kind: QuantityKind, value: i64) -> Quantity {
    Quantity::provider_reported(kind, value)
}

fn nanos(amount: &str) -> u128 {
    let (whole, fraction) = amount.split_once('.').unwrap_or((amount, ""));
    assert!(
        fraction.len() <= 9,
        "at most nine fractional digits: {amount}"
    );
    let fraction = format!("{fraction:0<9}");
    whole.parse::<u128>().expect("digits") * 1_000_000_000
        + fraction.parse::<u128>().expect("digits")
}

fn line_of(cost: &CostRecorded, kind: QuantityKind) -> &CostLine {
    cost.lines
        .iter()
        .find(|line| line.kind == kind)
        .unwrap_or_else(|| panic!("a line of {kind:?}: {:?}", cost.lines))
}

/// The totals of `cost` are its lines added up: the known subtotal the sum of every amount, the
/// unknown lines those with no amount or a reason, and a complete total only with none.
fn assert_totals_add_up(cost: &CostRecorded) {
    let sum: u128 = cost
        .lines
        .iter()
        .filter_map(|line| line.amount.as_deref())
        .map(nanos)
        .sum();
    assert_eq!(nanos(&cost.total.known_subtotal), sum, "{cost:#?}");
    let unknown = cost
        .lines
        .iter()
        .filter(|line| line.amount.is_none() || line.unknown.is_some())
        .count();
    assert_eq!(
        cost.total.unknown_lines,
        u64::try_from(unknown).expect("small"),
        "{cost:#?}"
    );
    if unknown == 0 {
        assert_eq!(
            cost.total.complete_total.as_deref(),
            Some(cost.total.known_subtotal.as_str())
        );
    } else {
        assert_eq!(cost.total.complete_total, None, "{cost:#?}");
    }
    for line in &cost.lines {
        if let Some(amount) = &line.amount {
            assert!(
                amount.bytes().all(|b| b.is_ascii_digit() || b == b'.'),
                "an exact decimal: {amount}"
            );
        }
        if line.amount.is_none() {
            assert!(
                line.unknown.is_some(),
                "no amount without a reason: {line:?}"
            );
        }
    }
}

// --- failing ---

#[test]
fn a_partial_record_never_prices_its_compute_as_final() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book()).bind(adapter, binding(&[QuantityKind::LocalComputeMs]));
    let cost = price(
        &pricing,
        &record(
            adapter,
            vec![Quantity::measured(QuantityKind::LocalComputeMs, 1000)],
            false,
        ),
    )
    .expect("priced");

    assert_totals_add_up(&cost);
    assert_eq!(
        cost.total.complete_total, None,
        "a count its source marked partial is a lower bound, never a complete total: {cost:#?}"
    );
    assert_eq!(
        line_of(&cost, QuantityKind::LocalComputeMs).unknown,
        Some(UnknownReason::UsageIncomplete)
    );
}

#[test]
fn a_negative_count_among_others_of_its_kind_is_unknown() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book()).bind(adapter, binding(&[QuantityKind::OutputTextTokens]));
    let cost = price(
        &pricing,
        &record(
            adapter,
            vec![
                reported(QuantityKind::OutputTextTokens, 500),
                reported(QuantityKind::OutputTextTokens, -100),
            ],
            true,
        ),
    )
    .expect("priced");

    let line = line_of(&cost, QuantityKind::OutputTextTokens);
    assert_eq!(
        (line.amount.as_deref(), line.unknown),
        (None, Some(UnknownReason::QuantityUnknown)),
        "a negative count is priced as unknown, not netted against 500: {line:?}"
    );
    assert_eq!(cost.total.complete_total, None);
}

#[test]
fn an_unbound_adapter_with_no_quantity_is_not_a_complete_zero() {
    let cost = price(
        &Pricing::new(book()),
        &record(AdapterId::generate(), Vec::new(), true),
    )
    .expect("priced");

    assert_eq!(
        cost.total.complete_total, None,
        "an adapter with no price is priced as unknown, never zero: {cost:#?}"
    );
}

// --- guards ---

#[test]
fn a_record_of_an_unbound_adapter_has_one_line_per_quantity_even_of_one_kind() {
    let cost = price(
        &Pricing::new(book()),
        &record(
            AdapterId::generate(),
            vec![
                reported(QuantityKind::OutputTextTokens, 5),
                reported(QuantityKind::OutputTextTokens, 7),
                Quantity::unknown(QuantityKind::Requests),
                reported(QuantityKind::InputCharacters, 0),
            ],
            false,
        ),
    )
    .expect("priced");

    assert_eq!(cost.lines.len(), 4, "{:?}", cost.lines);
    for line in &cost.lines {
        assert_eq!(line.amount, None, "never zero: {line:?}");
        assert_eq!(line.unknown, Some(UnknownReason::PriceUnknown));
    }
    assert_eq!(cost.lines[3].quantity, Some(0));
    assert_eq!(cost.total.known_subtotal, "0");
    assert_eq!(cost.total.unknown_lines, 4);
    assert_eq!(cost.total.complete_total, None);
}

#[test]
fn a_charged_kind_the_record_does_not_carry_is_unknown_never_zero() {
    let adapter = AdapterId::generate();
    let charges = [
        QuantityKind::OutputTextTokens,
        QuantityKind::LocalComputeMs,
        QuantityKind::Requests,
        QuantityKind::OutputAudioMs,
    ];
    let pricing = Pricing::new(book()).bind(adapter, binding(&charges));
    let cost = price(
        &pricing,
        &record(
            adapter,
            vec![reported(QuantityKind::InputTextTokens, 9)],
            true,
        ),
    )
    .expect("priced");

    assert_totals_add_up(&cost);
    assert_eq!(cost.lines.len(), charges.len());
    for line in &cost.lines {
        assert_eq!(line.quantity, None, "{line:?}");
        assert_eq!(line.amount, None, "{line:?}");
    }
    assert_eq!(
        line_of(&cost, QuantityKind::OutputTextTokens).unknown,
        Some(UnknownReason::QuantityUnknown)
    );
    assert_eq!(
        line_of(&cost, QuantityKind::LocalComputeMs).unknown,
        Some(UnknownReason::QuantityUnknown)
    );
    assert_eq!(
        line_of(&cost, QuantityKind::Requests).unknown,
        Some(UnknownReason::PriceUnknown)
    );
    assert_eq!(
        line_of(&cost, QuantityKind::OutputAudioMs).unknown,
        Some(UnknownReason::PriceUnknown)
    );
    assert_eq!(cost.total.complete_total, None);
}

#[test]
fn a_rate_the_document_does_not_name_is_rate_unknown() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book_with(
        r#"{"amount": "2", "per_units": 1000000}"#,
        None,
        r#"{"amount": "8", "per_units": 1000000}"#,
    ))
    .bind(adapter, binding(&TOKENS));
    let cost = price(
        &pricing,
        &record(
            adapter,
            vec![
                reported(QuantityKind::InputTextTokens, 1000),
                reported(QuantityKind::CachedInputTokens, 200),
                reported(QuantityKind::CacheCreationInputTokens, 0),
                reported(QuantityKind::OutputTextTokens, 10),
            ],
            true,
        ),
    )
    .expect("priced");

    assert_totals_add_up(&cost);
    let cached = line_of(&cost, QuantityKind::CachedInputTokens);
    assert_eq!(
        (cached.quantity, cached.amount.as_deref(), cached.unknown),
        (Some(200), None, Some(UnknownReason::RateUnknown))
    );
    assert_eq!(
        line_of(&cost, QuantityKind::CacheCreationInputTokens)
            .amount
            .as_deref(),
        Some("0")
    );
    assert_eq!(cost.total.known_subtotal, "0.00168");
    assert_eq!(cost.total.complete_total, None);
}

#[test]
fn a_binding_revision_the_price_is_not_for_is_a_binding_mismatch_on_every_token_line() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book()).bind(
        adapter,
        PriceBinding {
            model: Some(model("2")),
            ..binding(&TOKENS)
        },
    );
    let cost = price(
        &pricing,
        &record(
            adapter,
            TOKENS.iter().map(|kind| reported(*kind, 0)).collect(),
            true,
        ),
    )
    .expect("priced");

    assert_eq!(cost.lines.len(), 4);
    for line in &cost.lines {
        assert_eq!(
            line.amount, None,
            "a zero count under a mismatch is unknown"
        );
        assert_eq!(line.unknown, Some(UnknownReason::BindingMismatch));
    }
    assert_eq!(cost.total.complete_total, None);
}

#[test]
fn every_line_rounds_up_once_and_the_subtotal_is_the_sum_of_the_rounded_lines() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book_with(
        r#"{"amount": "0.000000001", "per_units": 3}"#,
        Some("0.000000007"),
        r#"{"amount": "0.333333333", "per_units": 7}"#,
    ))
    .bind(adapter, binding(&TOKENS));
    let cost = price(
        &pricing,
        &record(
            adapter,
            vec![
                reported(QuantityKind::InputTextTokens, 4),
                reported(QuantityKind::CachedInputTokens, 1),
                reported(QuantityKind::CacheCreationInputTokens, 2),
                reported(QuantityKind::OutputTextTokens, 1),
            ],
            true,
        ),
    )
    .expect("priced");

    assert_totals_add_up(&cost);
    assert_eq!(
        line_of(&cost, QuantityKind::InputTextTokens)
            .amount
            .as_deref(),
        Some("0.000000001"),
        "one uncached token at a third of a nanounit rounds up to one nanounit, never to zero"
    );
    assert_eq!(
        line_of(&cost, QuantityKind::OutputTextTokens)
            .amount
            .as_deref(),
        Some("0.047619048")
    );
    assert_eq!(cost.total.complete_total.as_deref(), Some("0.04762405"));
}

#[test]
fn a_very_large_count_is_exact_and_one_beyond_the_bound_is_refused_not_zero() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book()).bind(adapter, binding(&[QuantityKind::OutputTextTokens]));
    let exact = price(
        &pricing,
        &record(
            adapter,
            vec![reported(QuantityKind::OutputTextTokens, 1_000_000_000_000)],
            true,
        ),
    )
    .expect("priced");
    assert_eq!(exact.total.complete_total.as_deref(), Some("8000000"));

    let beyond = price(
        &pricing,
        &record(
            adapter,
            vec![reported(QuantityKind::OutputTextTokens, i64::MAX)],
            true,
        ),
    );
    assert_eq!(beyond, Err(PriceError::Cost(CostError::Overflow)));
}

#[test]
fn many_large_lines_add_up_exactly() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book()).bind(adapter, binding(&TOKENS));
    let cost = price(
        &pricing,
        &record(
            adapter,
            vec![
                reported(QuantityKind::InputTextTokens, 999_999_999_999),
                reported(QuantityKind::CachedInputTokens, 333_333_333_333),
                reported(QuantityKind::CacheCreationInputTokens, 111_111_111_111),
                reported(QuantityKind::OutputTextTokens, 777_777_777_777),
            ],
            true,
        ),
    )
    .expect("priced");

    assert_totals_add_up(&cost);
    assert_eq!(cost.total.complete_total.as_deref(), Some("7777777.77777"));
}

// --- a session with a record that cannot be priced ---

fn nodes(
    generator: &MockGenerator,
    usage: PortId,
) -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<CascadePayload>> + use<> {
    let generator = generator.clone();
    move |run_id, _| {
        Box::new(
            ModelNode::<CascadePayload>::carrying(run_id, generator.clone(), reply, RunLog::new())
                .with_usage(usage),
        )
    }
}

fn text(sequence: i64, stream_id: StreamId, producer: NodeId) -> EventEnvelope<CascadePayload> {
    let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
        span_id: SpanId::generate(),
        text: "hello".to_owned(),
    });
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id,
        sequence,
        producer,
        port: INGRESS.to_owned(),
        kind: event.name().to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: CascadePayload::Stream(event),
    }
}

#[test]
fn a_session_with_a_record_it_could_not_price_has_no_complete_total() {
    let generator = generator(Vec::<String>::new()).reporting([Some(Usage {
        input_tokens: Some(10),
        output_tokens: Some(0),
        cached_input_tokens: Some(200),
        cache_creation_input_tokens: Some(0),
        reasoning_output_tokens: None,
    })]);
    let adapter = generator.adapter().adapter_id();
    let (declaration, planned, usage_port) = planned_with_usage(&generator);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        nodes(&generator, usage_port),
    ))
    .expect("a session opens");
    session
        .price_with(Pricing::new(book()).bind(
            adapter,
            PriceBinding {
                charges: vec![
                    QuantityKind::OutputTextTokens,
                    QuantityKind::CachedInputTokens,
                ],
                ..binding(&[])
            },
        ))
        .expect("the first document");
    let drive = thread::spawn(move || block_on(driver.drive()));
    block_on(session.send(text(1, StreamId::generate(), NodeId::generate()))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    block_on(session.close()).expect("drains");
    while block_on(session.receive()).is_some() {}
    drive.join().expect("no panic").expect("the run ends");

    let cost = session.usage().cost;
    assert_eq!(cost.unpriced.len(), 1, "{cost:#?}");
    assert_eq!(
        cost.unpriced[0].error,
        PriceError::Cost(CostError::InvalidUsage)
    );
    assert!(cost.records.is_empty());
    assert_eq!(cost.total.unknown_lines, 0, "no line was priced");
    assert_eq!(
        cost.total.complete_total, None,
        "a record that failed to price keeps the total incomplete"
    );
}
