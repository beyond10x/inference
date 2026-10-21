//! Cost records and the session cost total, behind the feature `cost` (`story:usage-pricing`).
//!
//! | case | acceptance |
//! |---|---|
//! | [`a_voice_cascade_priced_for_its_llm_only_has_one_cost_record_per_usage_record`] | one cost record per usage record; the LLM's lines priced, every STT and TTS line `price_unknown`; the known subtotal is the sum of the priced lines and there is no complete total |
//! | [`a_text_session_whose_llm_reports_every_token_count_has_a_complete_total`] | all four token counts final and charged: a complete total equal to the known subtotal, in the document's currency, naming its revision and digest |
//! | [`usage_read_before_the_session_is_priced_is_priced_when_it_is`] | records read before any pricing are pending, then priced at `price_with` |
//! | [`a_price_document_in_another_currency_is_refused`] | the session keeps its currency; a second document in it is taken |
//! | the `price_*` cases | [`price`] without a session: unbound adapters, uncharged and unpriceable kinds, compute, unknown models, partial counts, refused counts |
//!
//! The testkit's mock LLM reports no upstream model, and `llm-cost` prices no model without one
//! (`model_unknown`). [`Serving`] stands in for a provider that reports the model it served: it
//! wraps the LLM node and names [`UPSTREAM`] in each usage record the node sends that names none.

#![allow(dead_code)]

mod text;

use std::collections::HashMap;
use std::num::{NonZeroU16, NonZeroU32};
use std::thread;

use inference_core::endpoint::{EndpointFuture, OutputEndpoint, SendError};
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{AdapterId, ModelId};
use inference_core::usage::{
    CostLine, CostRecorded, Quantity, QuantityKind, RecordUsage, UnknownReason, UsageRecord,
    UsageRecorded,
};
use inference_facade::cost::{
    BillingKind, ComputePriceBinding, CostError, CurrencyMismatch, Id, ModelPriceBinding,
    PriceBinding, PriceBook, PriceError, Pricing, price,
};
use inference_facade::events::SessionEvent;
use inference_facade::live::{LiveSession, LiveSessions};
use inference_llm::llm_core::Usage;
use inference_pipeline::{
    Declaration, InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation,
    NodeInstanceId, NodeOutput, PortId,
};
use inference_testkit::cascade::{CascadePayload, Mocks, TurnStore, VoiceCascade, declare};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::inference_stt::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame,
};
use inference_testkit::stt::synthetic_audio::{Stretch, synthesize};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep,
};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use inference_text::{SpanId, TranscriptCommitted};

use text::{INGRESS, block_on, generator, planned_with_usage, reply};

/// The upstream model [`Serving`] reports and the model price is for.
const UPSTREAM: &str = "mock-served";

/// What the user asks by voice: the recogniser's scripted span.
const QUESTION: &str = "how warm is it outside";

/// A price document in `currency`, revision `revision`, with one model price, `mock-llm`, for the
/// testkit mock LLM's binding (serving model `mock`, binding revision `1`) and [`UPSTREAM`], and
/// one compute price, `pod`. Per million tokens: input 2, cached input 0.5, cache-creation
/// input 2.5, output 8; per hour of the pod, 3.6.
fn book(currency: &str, revision: &str) -> PriceBook {
    let document = format!(
        r#"{{
            "format": "llm.prices/1",
            "revision": "{revision}",
            "source": "inference facade tests",
            "as_of_unix_ms": 1792540800000,
            "currency": "{currency}",
            "models": [{{
                "id": "mock-llm",
                "serving_model_id": "mock",
                "binding_revision": "1",
                "upstream_model": "{UPSTREAM}",
                "input": {{"amount": "2", "per_units": 1000000}},
                "cached_input": {{"amount": "0.5", "per_units": 1000000}},
                "cache_creation_input": {{"amount": "2.5", "per_units": 1000000}},
                "output": {{"amount": "8", "per_units": 1000000}}
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

fn id(value: &str) -> Id {
    Id::new(value).expect("a printable identifier")
}

const TOKEN_KINDS: [QuantityKind; 4] = [
    QuantityKind::InputTextTokens,
    QuantityKind::CachedInputTokens,
    QuantityKind::CacheCreationInputTokens,
    QuantityKind::OutputTextTokens,
];

/// The binding of the mock LLM: the four token kinds, charged under `mock-llm`.
fn llm_binding() -> PriceBinding {
    PriceBinding {
        charges: TOKEN_KINDS.to_vec(),
        model: Some(ModelPriceBinding {
            price_id: id("mock-llm"),
            serving_model_id: id("mock"),
            binding_revision: id("1"),
            billing: BillingKind::Metered,
        }),
        compute: None,
    }
}

/// Every count of a turn, final: 1000 input tokens, 200 of them read from the cache and 100
/// written to it, and 500 output tokens.
fn every_count() -> Usage {
    Usage {
        input_tokens: Some(1000),
        output_tokens: Some(500),
        cached_input_tokens: Some(200),
        cache_creation_input_tokens: Some(100),
        reasoning_output_tokens: None,
    }
}

/// The lines of [`every_count`] under `mock-llm`: 700 uncached input tokens at 2, 200 cached at
/// 0.5, 100 written at 2.5 and 500 output at 8, per million.
fn every_count_lines() -> Vec<CostLine> {
    let line = |kind, quantity, amount: &str| CostLine {
        kind,
        quantity: Some(quantity),
        amount: Some(amount.to_owned()),
        unknown: None,
    };
    vec![
        line(QuantityKind::InputTextTokens, 700, "0.0014"),
        line(QuantityKind::OutputTextTokens, 500, "0.004"),
        line(QuantityKind::CachedInputTokens, 200, "0.0001"),
        line(QuantityKind::CacheCreationInputTokens, 100, "0.00025"),
    ]
}

/// Sums exact decimal amounts of at most nine fractional digits, as nanounits; no float.
fn nanos(amount: &str) -> u64 {
    let (whole, fraction) = amount.split_once('.').unwrap_or((amount, ""));
    let fraction = format!("{fraction:0<9}");
    whole.parse::<u64>().expect("digits") * 1_000_000_000 + fraction.parse::<u64>().expect("digits")
}

// --- the stand-in for a provider that reports the model it served ---

/// A node wrapped so that each usage record it sends names [`UPSTREAM`] when it names no
/// upstream model.
struct Serving(Box<dyn NodeImplementation<CascadePayload>>);

struct Naming(Box<dyn OutputEndpoint<CascadePayload>>);

impl OutputEndpoint<CascadePayload> for Naming {
    fn send(
        &self,
        mut envelope: EventEnvelope<CascadePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<CascadePayload>>> {
        if let CascadePayload::Usage(recorded) = &mut envelope.payload {
            recorded
                .upstream_model
                .get_or_insert_with(|| UPSTREAM.to_owned());
        }
        self.0.send(envelope)
    }
}

impl NodeImplementation<CascadePayload> for Serving {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.0.describe(node)
    }

    fn described(&mut self, instance_id: NodeInstanceId) {
        self.0.described(instance_id);
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.0.prepare()
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<CascadePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let endpoints = NodeEndpoints {
            inputs: endpoints.inputs,
            outputs: endpoints
                .outputs
                .into_iter()
                .map(|output| NodeOutput {
                    port_id: output.port_id,
                    endpoint: Box::new(Naming(output.endpoint)),
                })
                .collect(),
        };
        self.0.start(endpoints)
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.0.drain()
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.0.cancel()
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.0.stop()
    }
}

// --- the application's side ---

/// What the application sends, on a stream of its own numbered from 1.
struct Application {
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Application {
    fn new() -> Self {
        Self {
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
            sequence: 0,
        }
    }

    fn envelope(
        &mut self,
        port: &str,
        kind: &str,
        timebase: Timebase,
        timestamp: i64,
        payload: CascadePayload,
    ) -> EventEnvelope<CascadePayload> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: RunId::generate(),
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: port.to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp,
            timebase,
            causation_id: None,
            payload,
        }
    }

    fn text(&mut self, text: &str) -> EventEnvelope<CascadePayload> {
        let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: text.to_owned(),
        });
        let kind = event.name();
        self.envelope(
            INGRESS,
            kind,
            Timebase::WallTime,
            0,
            CascadePayload::Stream(event),
        )
    }

    fn frame(&mut self, frame: AudioFrame) -> EventEnvelope<CascadePayload> {
        let timestamp = i64::try_from(frame.position.get()).expect("within i64");
        self.envelope(
            inference_testkit::cascade::AUDIO_INGRESS,
            "inference.audio.AudioFrame",
            Timebase::AudioFrameTime,
            timestamp,
            CascadePayload::Audio(frame),
        )
    }
}

/// The usage records among `received`, in the order they were delivered.
fn usage_records(received: &[SessionEvent]) -> Vec<UsageRecorded> {
    received
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) => Some(recorded.clone()),
            _ => None,
        })
        .collect()
}

/// Closes `session` by draining and receives what it still delivers.
fn close(session: &mut LiveSession<CascadePayload>, received: &mut Vec<SessionEvent>) {
    block_on(session.close()).expect("a Ready session drains");
    while let Some(event) = block_on(session.receive()) {
        received.push(event);
    }
}

// --- the voice cascade ---

fn voice_mocks() -> Mocks {
    Mocks {
        recogniser: MockRecogniser::new(
            SttConfiguration::Incremental,
            ModelId::generate(),
            vec![
                Cue {
                    after_frames: 1,
                    step: ScriptStep::Hypothesis(QUESTION.to_owned()),
                },
                Cue {
                    after_frames: 2,
                    step: ScriptStep::Commit { span: 0 },
                },
            ],
        ),
        generator: MockGenerator::new(
            LlmConfiguration::StreamingAndConcurrentInput,
            ModelId::generate(),
            Vec::<String>::new(),
        )
        .reporting([Some(every_count())]),
        synthesiser: MockSynthesiser::new(
            TtsConfiguration::IncrementalStreaming,
            ModelId::generate(),
            AudioFormat {
                encoding: AudioEncoding::PcmS16le,
                sample_rate: NonZeroU32::new(16_000).expect("16000"),
                channels: NonZeroU16::new(1).expect("1"),
            },
        ),
        reply: |question| format!("I cannot know {question}"),
    }
}

/// The cascade's nodes for whichever run asks first, the LLM node wrapped by [`Serving`].
fn voice_nodes(
    cascade: &VoiceCascade,
    mocks: &Mocks,
) -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<CascadePayload>> + use<> {
    let cascade = cascade.clone();
    let mocks = mocks.clone();
    let llm = cascade.ids.llm;
    let mut nodes: Option<HashMap<NodeId, Box<dyn NodeImplementation<CascadePayload>>>> = None;
    move |run_id, node| {
        let implementation = nodes
            .get_or_insert_with(|| {
                cascade
                    .nodes(run_id, mocks.clone(), TurnStore::default(), RunLog::new())
                    .implementations
            })
            .remove(&node.node_id())
            .expect("the cascade has an implementation for every node");
        if node.node_id() == llm {
            Box::new(Serving(implementation))
        } else {
            implementation
        }
    }
}

#[test]
fn a_voice_cascade_priced_for_its_llm_only_has_one_cost_record_per_usage_record() {
    let mocks = voice_mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade plans");
    let llm_adapter = mocks.generator.adapter().adapter_id();
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &cascade.planned,
        InteractionMode::TurnBased,
        voice_nodes(&cascade, &mocks),
    ))
    .expect("a session opens over the cascade");
    session
        .price_with(Pricing::new(book("EUR", "voice-1")).bind(llm_adapter, llm_binding()))
        .expect("the first price document");
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new();
    let mut received = Vec::new();
    for frame in synthesize(&[
        Stretch::Tone {
            frames: 30,
            hertz: 440,
            amplitude: 0.5,
        },
        Stretch::Silence { frames: 80 },
    ]) {
        block_on(session.send(application.frame(frame))).expect("the ingress takes every frame");
    }
    close(&mut session, &mut received);
    drive.join().expect("no panic").expect("the run ends");

    let usage = usage_records(&received);
    assert_eq!(usage.len(), 3, "STT, LLM and TTS: {usage:#?}");
    let cost = session.usage().cost;
    assert_eq!(
        cost.records
            .iter()
            .map(|record| record.usage_record_id)
            .collect::<Vec<_>>(),
        usage
            .iter()
            .map(|record| record.usage_record_id)
            .collect::<Vec<_>>(),
        "one cost record per usage record, in the order read"
    );
    assert!(cost.unpriced.is_empty(), "{:?}", cost.unpriced);
    assert_eq!(cost.pending, 0);

    for (recorded, record) in usage.iter().zip(&cost.records) {
        assert_eq!(record.currency, "EUR");
        assert_eq!(record.price_revision, "voice-1");
        if recorded.node_id == Some(cascade.ids.llm) {
            assert_eq!(
                record.lines,
                every_count_lines(),
                "the LLM's charged kinds, line by line"
            );
            assert_eq!(record.total.known_subtotal, "0.00575");
            assert_eq!(record.total.complete_total.as_deref(), Some("0.00575"));
        } else {
            assert_eq!(
                record.lines.len(),
                recorded.quantities.len(),
                "one line per quantity of an unbound adapter"
            );
            for (line, quantity) in record.lines.iter().zip(&recorded.quantities) {
                assert_eq!(line.kind, quantity.kind());
                assert_eq!(line.quantity, quantity.value());
                assert_eq!(line.amount, None, "never zero");
                assert_eq!(line.unknown, Some(UnknownReason::PriceUnknown));
            }
            assert_eq!(record.total.complete_total, None);
        }
    }
    let priced: u64 = cost
        .records
        .iter()
        .flat_map(|record| &record.lines)
        .filter_map(|line| line.amount.as_deref())
        .map(nanos)
        .sum();
    assert_eq!(nanos(&cost.total.known_subtotal), priced);
    assert_eq!(cost.total.known_subtotal, "0.00575");
    assert_eq!(
        cost.total.unknown_lines,
        u64::try_from(
            usage
                .iter()
                .filter(|recorded| recorded.node_id != Some(cascade.ids.llm))
                .map(|recorded| recorded.quantities.len())
                .sum::<usize>()
        )
        .expect("small")
    );
    assert_eq!(cost.total.complete_total, None);
    assert_eq!(cost.currency.as_deref(), Some("EUR"));
}

// --- the text pipeline ---

/// The text pipeline's one node over `generator`, given its usage port, wrapped by [`Serving`].
fn text_nodes(
    generator: &MockGenerator,
    usage: PortId,
) -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<CascadePayload>> + use<> {
    let generator = generator.clone();
    move |run_id, _| {
        Box::new(Serving(Box::new(
            ModelNode::<CascadePayload>::carrying(run_id, generator.clone(), reply, RunLog::new())
                .with_usage(usage),
        )))
    }
}

/// Opens a text session over `generator`, priced with `before` if any, sends one turn, closes it
/// and receives everything; then prices it with `after` if any.
fn text_session(
    generator: &MockGenerator,
    before: Option<Pricing>,
) -> (LiveSession<CascadePayload>, Vec<SessionEvent>) {
    let (declaration, planned, usage_port) = planned_with_usage(generator);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        text_nodes(generator, usage_port),
    ))
    .expect("a session opens over the text pipeline");
    if let Some(pricing) = before {
        session.price_with(pricing).expect("the first document");
    }
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new();
    let mut received = Vec::new();
    block_on(session.send(application.text("hello"))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    close(&mut session, &mut received);
    drive.join().expect("no panic").expect("the run ends");
    (session, received)
}

#[test]
fn a_text_session_whose_llm_reports_every_token_count_has_a_complete_total() {
    let generator = generator(Vec::<String>::new()).reporting([Some(every_count())]);
    let book = book("EUR", "text-1");
    let digest = book.digest().as_str().to_owned();
    let pricing = Pricing::new(book).bind(generator.adapter().adapter_id(), llm_binding());
    let (session, received) = text_session(&generator, Some(pricing));

    let usage = usage_records(&received);
    assert_eq!(usage.len(), 1, "one LLM turn: {usage:#?}");
    assert!(usage[0].r#final);
    let cost = session.usage().cost;
    assert_eq!(cost.records.len(), 1);
    let record = &cost.records[0];
    assert_eq!(record.usage_record_id, usage[0].usage_record_id);
    assert_eq!(record.lines, every_count_lines());
    assert_eq!(record.price_digest, digest);

    assert_eq!(cost.currency.as_deref(), Some("EUR"));
    assert_eq!(
        cost.documents
            .iter()
            .map(|document| (document.revision.as_str(), document.digest.as_str()))
            .collect::<Vec<_>>(),
        [("text-1", digest.as_str())],
        "the total names its document's revision and digest"
    );
    assert_eq!(cost.total.known_subtotal, "0.00575");
    assert_eq!(cost.total.unknown_lines, 0);
    assert_eq!(
        cost.total.complete_total.as_deref(),
        Some(cost.total.known_subtotal.as_str()),
        "complete, and equal to the known subtotal"
    );
}

#[test]
fn usage_read_before_the_session_is_priced_is_priced_when_it_is() {
    let generator = generator(Vec::<String>::new()).reporting([Some(every_count())]);
    let (session, received) = text_session(&generator, None);
    let usage = usage_records(&received);
    assert_eq!(usage.len(), 1);

    let unpriced = session.usage().cost;
    assert_eq!(unpriced.pending, 1, "read, waiting for a price document");
    assert!(unpriced.records.is_empty());
    assert_eq!(
        unpriced.total.complete_total, None,
        "a pending record keeps the total incomplete"
    );

    session
        .price_with(
            Pricing::new(book("EUR", "late-1"))
                .bind(generator.adapter().adapter_id(), llm_binding()),
        )
        .expect("the first document");
    let cost = session.usage().cost;
    assert_eq!(cost.pending, 0);
    assert_eq!(cost.records.len(), 1);
    assert_eq!(cost.records[0].usage_record_id, usage[0].usage_record_id);
    assert_eq!(cost.total.complete_total.as_deref(), Some("0.00575"));
}

#[test]
fn a_price_document_in_another_currency_is_refused() {
    let generator = generator(Vec::<String>::new()).reporting([Some(every_count())]);
    let adapter = generator.adapter().adapter_id();
    let (session, _) = text_session(
        &generator,
        Some(Pricing::new(book("EUR", "1")).bind(adapter, llm_binding())),
    );
    let before = session.usage().cost;

    assert_eq!(
        session.price_with(Pricing::new(book("USD", "2")).bind(adapter, llm_binding())),
        Err(CurrencyMismatch {
            session: "EUR".to_owned(),
            document: "USD".to_owned(),
        })
    );
    assert_eq!(session.usage().cost, before, "nothing changed");

    session
        .price_with(Pricing::new(book("EUR", "3")).bind(adapter, llm_binding()))
        .expect("a document in the session's currency");
    let after = session.usage().cost;
    assert_eq!(
        after
            .documents
            .iter()
            .map(|document| document.revision.as_str())
            .collect::<Vec<_>>(),
        ["1", "3"]
    );
    assert_eq!(
        after.records, before.records,
        "a record priced already keeps its cost record"
    );
}

// --- one record, no session ---

fn recorded(
    adapter_id: AdapterId,
    quantities: Vec<Quantity>,
    upstream: Option<&str>,
) -> UsageRecorded {
    let mut input = RecordUsage::new(adapter_id, ModelId::generate(), 0, 10, quantities);
    input.upstream_model = upstream.map(str::to_owned);
    UsageRecord::record(input).1
}

fn tokens() -> Vec<Quantity> {
    vec![
        Quantity::provider_reported(QuantityKind::InputTextTokens, 1000),
        Quantity::provider_reported(QuantityKind::OutputTextTokens, 500),
        Quantity::provider_reported(QuantityKind::CachedInputTokens, 200),
        Quantity::provider_reported(QuantityKind::CacheCreationInputTokens, 100),
        Quantity::measured(QuantityKind::Requests, 1),
        Quantity::unknown(QuantityKind::LocalComputeMs),
    ]
}

fn unknown_line(kind: QuantityKind, quantity: Option<i64>, reason: UnknownReason) -> CostLine {
    CostLine {
        kind,
        quantity,
        amount: None,
        unknown: Some(reason),
    }
}

#[test]
fn price_an_unbound_adapter_gives_one_price_unknown_line_per_quantity() {
    let adapter = AdapterId::generate();
    let record = recorded(adapter, tokens(), Some(UPSTREAM));
    let cost = price(&Pricing::new(book("EUR", "1")), &record).expect("priced");

    assert_eq!(
        cost.lines,
        record
            .quantities
            .iter()
            .map(|quantity| unknown_line(
                quantity.kind(),
                quantity.value(),
                UnknownReason::PriceUnknown
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(cost.total.known_subtotal, "0");
    assert_eq!(cost.total.unknown_lines, 6);
    assert_eq!(cost.total.complete_total, None, "unknown, never zero");
}

#[test]
fn price_charges_only_the_bound_kinds_and_marks_a_kind_without_a_unit_price_unknown() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book("EUR", "1")).bind(
        adapter,
        PriceBinding {
            charges: vec![
                QuantityKind::Requests,
                QuantityKind::OutputTextTokens,
                QuantityKind::InputAudioMs,
            ],
            ..llm_binding()
        },
    );
    let cost = price(&pricing, &recorded(adapter, tokens(), Some(UPSTREAM))).expect("priced");

    assert_eq!(
        cost.lines,
        [
            CostLine {
                kind: QuantityKind::OutputTextTokens,
                quantity: Some(500),
                amount: Some("0.004".to_owned()),
                unknown: None,
            },
            unknown_line(
                QuantityKind::InputAudioMs,
                None,
                UnknownReason::PriceUnknown
            ),
            unknown_line(QuantityKind::Requests, Some(1), UnknownReason::PriceUnknown),
        ],
        "in the order of the kinds; the uncharged token kinds have no line"
    );
    assert_eq!(cost.total.known_subtotal, "0.004");
    assert_eq!(cost.total.unknown_lines, 2);
}

#[test]
fn price_token_kinds_without_a_model_price_and_compute_with_one() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book("EUR", "1")).bind(
        adapter,
        PriceBinding {
            charges: vec![QuantityKind::OutputTextTokens, QuantityKind::LocalComputeMs],
            model: None,
            compute: Some(ComputePriceBinding {
                price_id: id("pod"),
                hosting_provider: id("local"),
                account: id("lab"),
                resource_class: id("cpu-pod"),
            }),
        },
    );
    let mut quantities = tokens();
    quantities[5] = Quantity::measured(QuantityKind::LocalComputeMs, 1500);
    let cost = price(&pricing, &recorded(adapter, quantities, Some(UPSTREAM))).expect("priced");

    assert_eq!(
        cost.lines,
        [
            unknown_line(
                QuantityKind::OutputTextTokens,
                Some(500),
                UnknownReason::PriceUnknown
            ),
            CostLine {
                kind: QuantityKind::LocalComputeMs,
                quantity: Some(1500),
                amount: Some("0.0015".to_owned()),
                unknown: None,
            },
        ]
    );
}

#[test]
fn price_a_compute_binding_that_does_not_match_its_price_is_a_binding_mismatch() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book("EUR", "1")).bind(
        adapter,
        PriceBinding {
            charges: vec![QuantityKind::LocalComputeMs],
            model: None,
            compute: Some(ComputePriceBinding {
                price_id: id("pod"),
                hosting_provider: id("local"),
                account: id("another"),
                resource_class: id("cpu-pod"),
            }),
        },
    );
    let quantities = vec![Quantity::measured(QuantityKind::LocalComputeMs, 1500)];
    let cost = price(&pricing, &recorded(adapter, quantities, None)).expect("priced");
    assert_eq!(
        cost.lines,
        [unknown_line(
            QuantityKind::LocalComputeMs,
            Some(1500),
            UnknownReason::BindingMismatch
        )]
    );
}

#[test]
fn price_no_upstream_model_leaves_every_token_line_model_unknown() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book("EUR", "1")).bind(adapter, llm_binding());
    let cost = price(&pricing, &recorded(adapter, tokens(), None)).expect("priced");

    assert_eq!(
        cost.lines,
        [
            unknown_line(
                QuantityKind::InputTextTokens,
                Some(700),
                UnknownReason::ModelUnknown
            ),
            unknown_line(
                QuantityKind::OutputTextTokens,
                Some(500),
                UnknownReason::ModelUnknown
            ),
            unknown_line(
                QuantityKind::CachedInputTokens,
                Some(200),
                UnknownReason::ModelUnknown
            ),
            unknown_line(
                QuantityKind::CacheCreationInputTokens,
                Some(100),
                UnknownReason::ModelUnknown
            ),
        ]
    );
    assert_eq!(cost.total.complete_total, None);
}

#[test]
fn price_partial_counts_are_a_lower_bound_and_an_unknown_count_stays_unknown() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book("EUR", "1")).bind(adapter, llm_binding());
    let mut quantities = tokens();
    quantities[2] = Quantity::unknown(QuantityKind::CachedInputTokens);
    let mut input = RecordUsage::new(adapter, ModelId::generate(), 0, 10, quantities);
    input.upstream_model = Some(UPSTREAM.to_owned());
    input.r#final = false;
    let cost = price(&pricing, &UsageRecord::record(input).1).expect("priced");

    assert_eq!(
        cost.lines,
        [
            unknown_line(
                QuantityKind::InputTextTokens,
                None,
                UnknownReason::QuantityUnknown
            ),
            CostLine {
                kind: QuantityKind::OutputTextTokens,
                quantity: Some(500),
                amount: Some("0.004".to_owned()),
                unknown: Some(UnknownReason::UsageIncomplete),
            },
            unknown_line(
                QuantityKind::CachedInputTokens,
                None,
                UnknownReason::QuantityUnknown
            ),
            CostLine {
                kind: QuantityKind::CacheCreationInputTokens,
                quantity: Some(100),
                amount: Some("0.00025".to_owned()),
                unknown: Some(UnknownReason::UsageIncomplete),
            },
        ]
    );
    assert_eq!(cost.total.known_subtotal, "0.00425", "a lower bound");
    assert_eq!(cost.total.unknown_lines, 4);
    assert_eq!(cost.total.complete_total, None);
}

#[test]
fn price_refuses_cache_counts_beyond_the_input_tokens() {
    let adapter = AdapterId::generate();
    let pricing = Pricing::new(book("EUR", "1")).bind(adapter, llm_binding());
    let mut quantities = tokens();
    quantities[0] = Quantity::provider_reported(QuantityKind::InputTextTokens, 10);
    assert_eq!(
        price(&pricing, &recorded(adapter, quantities, Some(UPSTREAM))),
        Err(PriceError::Cost(CostError::InvalidUsage))
    );
}

#[test]
fn price_names_the_document_and_records_a_new_identity_each_time() {
    let adapter = AdapterId::generate();
    let book = book("EUR", "named-1");
    let digest = book.digest().as_str().to_owned();
    let pricing = Pricing::new(book).bind(adapter, llm_binding());
    let record = recorded(adapter, tokens(), Some(UPSTREAM));
    let first: CostRecorded = price(&pricing, &record).expect("priced");
    let second = price(&pricing, &record).expect("priced");

    assert_eq!(first.usage_record_id, record.usage_record_id);
    assert_eq!(first.currency, "EUR");
    assert_eq!(first.price_revision, "named-1");
    assert_eq!(first.price_digest, digest);
    assert!(first.price_digest.starts_with("prices-sha256:"));
    assert_ne!(first.cost_record_id, second.cost_record_id);
    assert_eq!(first.lines, second.lines);
}
