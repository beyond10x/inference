//! Adversary pass 1 on `story:llm-0-8-3`: the usage record of a turn whose stream carries
//! llama-server's `timings` object in shapes the recorded fixtures do not: a reused prompt
//! cache, a `usage` report that leaves a counter out, `timings` that contradict `usage`, a
//! `timings` field in a shape the server does not write, and `timings` on chunks before the last.
//! Every body is derived from a recorded one by the one edit its case names.

mod common;

use common::scripted::{Scripted, TEXT_TIMINGS_ONLY};
use common::{DELTAS, RECORDED, SERVED_MODEL};
use inference_core::id::{NodeId, RunId};
use inference_core::model::{Adapter, Model};
use inference_core::usage::{Quantity, QuantityKind, UsageRecorded};
use inference_llama_server::{Config, LlamaServer, declare_adapter, declare_model};
use inference_llm::llm_core::{ErrorCode, Item, TurnRequest};
use inference_llm::{GenerationEvent, GenerationRequest};

const TOKEN_KINDS: [QuantityKind; 4] = [
    QuantityKind::InputTextTokens,
    QuantityKind::OutputTextTokens,
    QuantityKind::CachedInputTokens,
    QuantityKind::CacheCreationInputTokens,
];

/// `body` with `from`, which occurs in it exactly once, replaced by `to`, kept for the run.
fn edited(body: &[u8], from: &str, to: &str) -> &'static [u8] {
    let text = std::str::from_utf8(body).expect("the fixture is UTF-8");
    assert_eq!(text.matches(from).count(), 1, "`{from}` occurs once");
    Box::leak(text.replacen(from, to, 1).into_bytes().into_boxed_slice())
}

struct Ran {
    events: Vec<GenerationEvent>,
    failure: Option<inference_llm::llm_core::Error>,
}

impl Ran {
    fn texts(&self) -> Vec<&str> {
        self.events
            .iter()
            .filter_map(|event| match event {
                GenerationEvent::TextDelta(delta) => Some(delta.text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn record(&self) -> &UsageRecorded {
        let records: Vec<&UsageRecorded> = self
            .events
            .iter()
            .filter_map(|event| match event {
                GenerationEvent::Usage(recorded) => Some(recorded),
                _ => None,
            })
            .collect();
        let [record] = records.as_slice() else {
            panic!("one turn, one record: {records:?}")
        };
        record
    }

    fn tokens(&self) -> [Quantity; 4] {
        let record = self.record();
        TOKEN_KINDS.map(|kind| {
            record
                .quantities
                .iter()
                .find(|quantity| quantity.kind() == kind)
                .copied()
                .unwrap_or_else(|| panic!("no {kind:?} quantity: {record:?}"))
        })
    }

    fn completed(&self) -> bool {
        matches!(self.events.last(), Some(GenerationEvent::Completed(_)))
    }

    fn cancelled(&self) -> bool {
        matches!(self.events.last(), Some(GenerationEvent::Cancelled(..)))
    }
}

/// One generation against a server answering its one request with `body`.
fn run(body: &'static [u8]) -> Ran {
    let fixture = Scripted::new(vec![body]);
    let config = Config::new(fixture.base_url(), SERVED_MODEL);
    let (model, _) = Model::declare(declare_model());
    let (adapter, _) =
        Adapter::declare(declare_adapter(model.model_id(), &config)).expect("one slot");
    let mut generator = LlamaServer::new(&config)
        .expect("a model")
        .generator(adapter)
        .expect("the binding declares what the adapter claims");
    let mut handle = generator
        .start(GenerationRequest {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
            turn: TurnRequest::new(SERVED_MODEL, vec![Item::user("Say hello in three words.")]),
        })
        .expect("no generation runs yet");
    let events: Vec<GenerationEvent> = handle.by_ref().collect();
    let failure = handle.failure().cloned();
    assert_eq!(fixture.requests().len(), 1);
    Ran { events, failure }
}

fn reported(kind: QuantityKind, value: i64) -> Quantity {
    Quantity::provider_reported(kind, value)
}

/// The existing timings-only case has `cache_n` 0, so it cannot tell `prompt_n` from
/// `prompt_n + cache_n` as the input. With a reused prefix the input is their sum.
#[test]
fn timings_alone_with_a_reused_prompt_report_the_processed_plus_reused_tokens_as_input() {
    let ran = run(edited(
        TEXT_TIMINGS_ONLY,
        r#""cache_n":0,"prompt_n":35"#,
        r#""cache_n":14,"prompt_n":21"#,
    ));
    assert!(ran.completed(), "{:?}", ran.events);
    assert!(ran.record().r#final);
    assert_eq!(
        ran.tokens(),
        [
            reported(QuantityKind::InputTextTokens, 35),
            reported(QuantityKind::OutputTextTokens, 4),
            reported(QuantityKind::CachedInputTokens, 14),
            Quantity::unknown(QuantityKind::CacheCreationInputTokens),
        ]
    );
}

/// A `usage` report without `prompt_tokens_details` leaves the cached count to `timings`.
#[test]
fn a_usage_report_without_a_cached_count_takes_it_from_the_timings() {
    let ran = run(edited(
        RECORDED,
        r#","prompt_tokens_details":{"cached_tokens":0}"#,
        "",
    ));
    assert!(ran.completed(), "{:?}", ran.events);
    assert_eq!(
        ran.tokens(),
        [
            reported(QuantityKind::InputTextTokens, 35),
            reported(QuantityKind::OutputTextTokens, 4),
            reported(QuantityKind::CachedInputTokens, 0),
            Quantity::unknown(QuantityKind::CacheCreationInputTokens),
        ]
    );
}

/// `usage` says 4 completion tokens, `timings` 5: the turn fails after its deltas reached the
/// caller, and the record of the failed turn reports no token count and is not final.
#[test]
fn timings_contradicting_usage_fail_the_turn_and_the_record_reports_no_count() {
    let ran = run(edited(RECORDED, r#""predicted_n":4"#, r#""predicted_n":5"#));
    assert!(ran.cancelled(), "{:?}", ran.events);
    assert_eq!(ran.texts(), DELTAS, "the deltas reached the caller first");
    let failure = ran.failure.as_ref().expect("the turn failed");
    assert_eq!(failure.code, ErrorCode::Protocol, "{failure:?}");
    assert!(
        failure.message.contains("timings contradict"),
        "{failure:?}"
    );
    assert!(!ran.record().r#final);
    assert_eq!(ran.tokens(), TOKEN_KINDS.map(Quantity::unknown));
}

/// A documented `timings` field in a shape llama-server does not write (`null` for a duration)
/// fails a turn that 0.8.0 completed, after its deltas reached the caller.
#[test]
fn a_timings_duration_written_as_null_fails_the_turn_after_its_deltas() {
    let ran = run(edited(
        TEXT_TIMINGS_ONLY,
        r#""prompt_ms":258.373"#,
        r#""prompt_ms":null"#,
    ));
    assert!(ran.cancelled(), "{:?}", ran.events);
    assert_eq!(ran.texts(), DELTAS);
    let failure = ran.failure.as_ref().expect("the turn failed");
    assert_eq!(failure.code, ErrorCode::Protocol, "{failure:?}");
    assert!(failure.message.contains("timings.prompt_ms"), "{failure:?}");
    assert!(
        !failure.message.contains("null"),
        "never the value: {failure:?}"
    );
    assert_eq!(ran.tokens(), TOKEN_KINDS.map(Quantity::unknown));
}

const MID_TIMINGS: &str = r#","timings":{"cache_n":0,"prompt_n":35,"prompt_ms":258.373,"prompt_per_token_ms":7.382085714285714,"prompt_per_second":135.46307083170456,"predicted_n":2,"predicted_ms":76.0,"predicted_per_token_ms":38.0,"predicted_per_second":26.3}}"#;

const THERE_CHUNK_END: &str = r#"{"content":" there"}}],"created":1792537179,"id":"chatcmpl-Q89GHL94V2WixIjr6hLQEuYmzP54BQyK","model":"qwen2.5-1.5b-instruct","system_fingerprint":"b11752-7879491cf","object":"chat.completion.chunk"}"#;

fn there_chunk_with_mid_timings() -> String {
    let end = THERE_CHUNK_END.strip_suffix('}').expect("a JSON object");
    format!("{end}{MID_TIMINGS}")
}

/// Running timings on a chunk before the last (as with per-token timings), then the final ones
/// on the finish chunk: the record reads the final counts.
#[test]
fn running_timings_before_the_final_ones_report_the_final_counts() {
    let ran = run(edited(
        TEXT_TIMINGS_ONLY,
        THERE_CHUNK_END,
        &there_chunk_with_mid_timings(),
    ));
    assert!(ran.completed(), "{:?}", ran.events);
    assert_eq!(
        ran.tokens(),
        [
            reported(QuantityKind::InputTextTokens, 35),
            reported(QuantityKind::OutputTextTokens, 4),
            reported(QuantityKind::CachedInputTokens, 0),
            Quantity::unknown(QuantityKind::CacheCreationInputTokens),
        ]
    );
}
