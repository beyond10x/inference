//! Adversary pass 1 on `story:llm-0-8-3`: the resolver the adapter's client is handed, and the
//! Tokio runtime `llm` 0.8.3's clients require.
//!
//! The adapter's binding names no secret, so the resolver is never asked and swapping its own
//! refusing resolver for `llm`'s `AnonymousResolver` changes nothing the adapter sends. A
//! credentialed binding handed that resolver stops as `unauthorized` before sending. The client
//! polled outside a runtime is refused, and the adapter's model polled inside or outside one
//! still runs its turn.

mod common;

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use common::{DELTAS, Fixture, SERVED_MODEL};
use inference_llama_server::{Config, LlamaServer, binding};
use llm_blocking::BlockingModel;
use llm_chat::ChatClient;
use llm_core::{
    AuthKind, Cancel, Dispatch, ErrorCode, Item, Model as _, StopReason, StreamEvent, TurnRequest,
    VecSink,
};
use llm_credentials::{AnonymousResolver, SecretRef};
use llm_http::HttpClient;

const PROMPT: &str = "Say hello in three words.";

fn request() -> TurnRequest {
    TurnRequest::new(SERVED_MODEL, vec![Item::user(PROMPT)])
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` to its end on the calling thread, in no Tokio runtime.
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        thread::park();
    }
}

fn texts(events: &[StreamEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_adapters_binding_names_no_secret_whatever_its_config() {
    for tools in [false, true] {
        let mut config = Config::new("http://127.0.0.1:8080/v1", SERVED_MODEL);
        config.tools = tools;
        let bound = binding(&config).expect("a binding");
        let account = &bound.declaration().account;
        assert_eq!(account.auth_kind, AuthKind::Anonymous, "tools {tools}");
        assert!(account.secret_reference_id.is_none(), "tools {tools}");
        assert!(account.api_key_header.is_none(), "tools {tools}");
    }
}

#[test]
fn a_credentialed_binding_handed_the_anonymous_resolver_stops_unauthorized_before_sending() {
    let fixture = Fixture::replaying();
    let config = Config::new(fixture.base_url(), SERVED_MODEL);
    let mut document = binding(&config).expect("a binding").declaration().clone();
    document.account.auth_kind = AuthKind::Bearer;
    document.account.secret_reference_id =
        Some(SecretRef::new("llama-server-key").expect("a reference"));
    let credentialed = document.bind().expect("a credentialed binding");
    let client = ChatClient::new(
        credentialed,
        HttpClient::new(config.limits).expect("an HTTP client"),
        Arc::new(AnonymousResolver),
    );
    let blocking = BlockingModel::new(Arc::new(client)).expect("a runtime");
    let mut events = Vec::new();
    let mut sink = |event| {
        events.push(event);
        Ok(())
    };
    let error = blocking
        .turn(&request(), &mut sink, &Cancel::new())
        .expect_err("a credential the resolver refuses");
    assert_eq!(error.code, ErrorCode::Unauthorized, "{error:?}");
    assert_eq!(error.dispatch, Dispatch::NotSent, "{error:?}");
    assert!(!error.retriable, "{error:?}");
    assert!(events.is_empty(), "{events:?}");
    assert!(fixture.requests().is_empty(), "{:?}", fixture.requests());
}

#[test]
fn the_adapters_client_polled_outside_a_runtime_is_refused_before_sending() {
    let fixture = Fixture::replaying();
    let config = Config::new(fixture.base_url(), SERVED_MODEL);
    let client = ChatClient::new(
        binding(&config).expect("a binding"),
        HttpClient::new(config.limits).expect("an HTTP client"),
        Arc::new(AnonymousResolver),
    );
    let mut sink = VecSink::new(64, 64 * 1024);
    let error = block_on(client.turn(&request(), &mut sink, &Cancel::new()))
        .expect_err("no Tokio runtime on this thread");
    assert_eq!(error.code, ErrorCode::Unsupported, "{error:?}");
    assert_eq!(error.dispatch, Dispatch::NotSent, "{error:?}");
    assert!(!error.retriable, "{error:?}");
    assert!(fixture.requests().is_empty(), "{:?}", fixture.requests());
}

#[test]
fn the_adapters_model_polled_outside_a_runtime_runs_its_turn() {
    let fixture = Fixture::replaying();
    let model = LlamaServer::new(&Config::new(fixture.base_url(), SERVED_MODEL)).expect("a model");
    let mut sink = VecSink::new(64, 64 * 1024);
    let outcome = block_on(model.turn(&request(), &mut sink, &Cancel::new())).expect("the turn");
    assert_eq!(outcome.stop_reason, StopReason::EndTurn, "{outcome:?}");
    assert_eq!(texts(sink.events()), DELTAS);
    assert_eq!(fixture.requests().len(), 1);
}

/// `BlockingModel` polls the model it wraps inside its own Tokio runtime, so wrapping the
/// adapter's model in one polls `LlamaServer::turn` inside a runtime.
#[test]
fn the_adapters_model_polled_inside_a_runtime_runs_its_turn() {
    let fixture = Fixture::replaying();
    let model = LlamaServer::new(&Config::new(fixture.base_url(), SERVED_MODEL)).expect("a model");
    let outer = BlockingModel::new(Arc::new(model)).expect("a runtime");
    let mut events = Vec::new();
    let mut sink = |event| {
        events.push(event);
        Ok(())
    };
    let outcome = outer
        .turn(&request(), &mut sink, &Cancel::new())
        .expect("the turn");
    assert_eq!(outcome.stop_reason, StopReason::EndTurn, "{outcome:?}");
    assert_eq!(texts(&events), DELTAS);
    assert_eq!(fixture.requests().len(), 1);
}
