//! A local adapter of the inference framework for llama.cpp's `llama-server`: an output-streaming
//! text generator over the Chat Completions client of the `llm` repository.
//!
//! `llama-server` serves the Chat Completions wire on a local base URL with no credential. This
//! crate is a binding, a runtime bridge and the declarations of the model and the adapter, not a
//! protocol client: [`binding`] makes the anonymous, self-hosted Chat Completions binding of a
//! [`Config`], `llm`'s `ChatClient` speaks the wire, and [`LlamaServer`] is the
//! [`llm_core::Model`] that [`inference_llm::Generator`] runs generations on. The adapter claims
//! [`Capability::OutputStreaming`] ([`declare_adapter`]): each delta of the server reaches the
//! caller while the generation is Active, and input offered while it is Active is refused, as
//! `inference-llm` refuses input for an adapter that does not claim
//! [`Capability::ConcurrentInputOutput`].
//!
//! # Tool proposals
//!
//! With [`Config::tools`] on, the binding declares tools and a tool choice and the adapter claims
//! [`Capability::ToolCalling`] beside output streaming. The server must then run with `--jinja`,
//! the only mode in which it parses a chat template's calls into structured `tool_calls`. `llm`'s
//! client sends the turn's tools and assembles the streamed fragments of each call; a call never
//! reaches the caller as text. The caller takes it with
//! [`GenerationHandle::take_proposal`](inference_llm::GenerationHandle::take_proposal), proposes
//! it on the generation with the `inference.tools.ProposeToolCall` the `tools` feature's
//! `tools::proposal` makes of it, and answers it with
//! [`GenerationHandle::integrate`](inference_llm::GenerationHandle::integrate): the next turn runs
//! on the same generation. A call whose name never arrived is refused by `llm` as a protocol
//! error, and the generation is cancelled with it.
//!
//! # The runtime bridge
//!
//! The HTTP transport of `llm` must be polled inside a tokio runtime, and its clients refuse a
//! turn polled outside one; neither `inference-llm` nor the graph runtime polls in one.
//! [`LlamaServer`] runs each turn on a thread of its own through `llm`'s `BlockingModel`, which
//! owns a one-worker runtime that drives the I/O; the turn's events cross to the poller of
//! [`llm_core::Model::turn`] as they are decoded, and the poller is woken for each. Dropping the
//! turn, or a sink refusing an event, cancels the running request through a child of the
//! caller's [`llm_core::Cancel`]. The bridge goes when `llm` offers a client that can be polled
//! outside a runtime.
//!
//! # Cancellation and failures
//!
//! Cancelling delivery and stopping computation are distinct. `GenerationHandle::cancel` of
//! `inference-llm` cancels delivery: `GenerationCancelled`, and no later delta, though more of
//! the stream may already have arrived. The same cancel reaches the turn's thread, and `llm`
//! drops the response, which closes the connection; `llama-server` stops a task whose connection
//! is gone. That close is all the adapter can do to stop computation, and the adapter claims no
//! [`Capability::Interruptible`]: whether the server stopped is observed only on the server, in
//! its slot state (`GET /slots`). Dropping the handle does the same.
//!
//! A failed turn cancels the generation with the segments it discarded and keeps `llm`'s error
//! unchanged: a 503 of a server still loading its model is `Transport` and dispatch `Unknown`, a
//! stream that ends without a finish reason or `[DONE]` is `Protocol`, and a server silent past
//! [`Config::limits`] is `Deadline`; each closes the connection. The adapter retries nothing.
//! [`LlamaServer::running_turns`] counts the turns whose thread has not yet let go.
//!
//! # Running the server by hand
//!
//! Tests start no server and download no weights. The live test, ignored and behind the `live`
//! feature, talks to a server started by hand. With a prebuilt `llama-server` release (MIT) and
//! the checkpoint [`CHECKPOINT`] from [`SOURCE`] at revision [`SOURCE_REVISION`], whose SHA-256
//! is [`CHECKSUM`]:
//!
//! ```text
//! llama-server --host 127.0.0.1 --port 8080 -m qwen2.5-1.5b-instruct-q4_k_m.gguf \
//!     --alias qwen2.5-1.5b-instruct -c 4096 -np 1 -ngl 0
//! INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:8080/v1 \
//! INFERENCE_LLAMA_SERVER_MODEL=qwen2.5-1.5b-instruct \
//!     cargo test -p inference-llama-server --features live -- --ignored --nocapture
//! ```
//!
//! `-np` is the number of parallel slots of the server, which [`Config::max_concurrency`]
//! declares; `-c` is the context window, which [`Config::context_window`] declares. For tool
//! proposals, add `--jinja` to the server's arguments and run the live tool test with
//! `--features live,tools --test live_tools`.

use std::collections::VecDeque;
use std::fmt;
use std::future;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::thread;

use inference_core::model::{
    Capability, DeclareAdapter, DeclareModel, ExecutionForm, Locality, Modality, ModelId,
};
use inference_llm::{CONTRACT_REVISION, CapabilityNotDeclared, Generator};
use llm_blocking::BlockingModel;
use llm_chat::{
    Account, BaseUrl, Binding, BindingDocument, ChatClient, Endpoint, Provider, ServedModel,
    ServingModel,
};
use llm_core::{
    AuthKind, BillingKind, BoxFuture, Cancel, Capabilities, Dispatch, Error, ErrorCode, Id, Model,
    Protocol, Provenance, StreamEvent, StreamSink, TurnOutcome, TurnRequest,
};
use llm_credentials::AnonymousResolver;
use llm_http::{HttpClient, Limits};

pub use inference_core::model::Adapter;

#[cfg(feature = "tools")]
pub mod tools;

/// The backend an adapter of this crate declares.
pub const BACKEND: &str = "llama.cpp llama-server";

/// The model family of the pinned checkpoint.
pub const FAMILY: &str = "Qwen2.5";

/// The pinned checkpoint: the file the server is started with.
pub const CHECKPOINT: &str = "qwen2.5-1.5b-instruct-q4_k_m.gguf";

/// The SHA-256 of [`CHECKPOINT`], as its source publishes it at [`SOURCE_REVISION`].
pub const CHECKSUM: &str =
    "sha256:6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e";

/// Where [`CHECKPOINT`] is published.
pub const SOURCE: &str = "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF";

/// The revision of [`SOURCE`] the checkpoint, its licence and its checksum were read at.
pub const SOURCE_REVISION: &str = "91cad51170dc346986eccefdc2dd33a9da36ead9";

/// The licence of llama.cpp, the backend's code.
pub const CODE_LICENSE: &str = "MIT";

/// The licence of the checkpoint's weights, as the `LICENSE` file of [`SOURCE`] states it at
/// [`SOURCE_REVISION`].
pub const WEIGHT_LICENSE: &str = "Apache-2.0";

/// How to reach one running `llama-server` and what it serves.
#[derive(Debug, Clone)]
pub struct Config {
    /// The base URL of the server's OpenAI-compatible API, `http://127.0.0.1:8080/v1`.
    pub base_url: String,
    /// The model name the server answers to, its `--alias`; every request names it.
    pub served_model: String,
    /// The context window of the server, its `-c`, in tokens.
    pub context_window: u64,
    /// The most tokens one turn may produce.
    pub max_output_tokens: u64,
    /// The parallel slots of the server, its `-np`: the adapter's maximum concurrency.
    pub max_concurrency: i64,
    /// The bounds of each request: time to the response headers, idle time between chunks and
    /// total time.
    pub limits: Limits,
    /// Whether the server is offered tools: started with `--jinja`, it streams the calls of a
    /// chat template it parses as structured `tool_calls`. Off by default.
    pub tools: bool,
}

impl Config {
    /// A server at `base_url` serving `served_model`, with the server's default context window of
    /// 4096 tokens, at most 1024 tokens per turn, one slot and `llm`'s default HTTP limits.
    #[must_use]
    pub fn new(base_url: impl Into<String>, served_model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            served_model: served_model.into(),
            context_window: 4096,
            max_output_tokens: 1024,
            max_concurrency: 1,
            limits: Limits::default(),
            tools: false,
        }
    }

    /// The capabilities the binding declares: text, streamed, no input into a running turn, and
    /// no optional setting but tools and a tool choice when [`Config::tools`] is on.
    #[must_use]
    pub const fn capabilities(&self) -> Capabilities {
        let mut capabilities = Capabilities::text(self.context_window, self.max_output_tokens);
        capabilities.tools = self.tools;
        capabilities.tool_choice = self.tools;
        capabilities
    }
}

/// The anonymous, self-hosted Chat Completions binding of `config`: no credential is resolved and
/// no authorization header is sent. Its model is `config.served_model`, under that name both in
/// the binding and on the wire.
///
/// # Errors
///
/// The `llm` error refusing the base URL or the served model name, or the binding.
pub fn binding(config: &Config) -> Result<Binding, Error> {
    let served = id(&config.served_model)?;
    BindingDocument::new(
        Provider {
            id: id("llama-server")?,
            category: id("self-hosted")?,
        },
        Account {
            id: id("local")?,
            provider_id: id("llama-server")?,
            auth_kind: AuthKind::Anonymous,
            billing_kind: BillingKind::SelfHosted,
            secret_reference_id: None,
            api_key_header: None,
        },
        Endpoint {
            id: id("local-llama-server")?,
            account_id: id("local")?,
            base_url: BaseUrl::new(&config.base_url)?,
        },
        ServedModel {
            id: served.clone(),
            upstream_name: served.clone(),
        },
        ServingModel {
            id: id(&format!("llama-server/{}", config.served_model))?,
            endpoint_id: id("local-llama-server")?,
            model_id: served,
            protocol: Protocol::ChatCompletions,
            capabilities: config.capabilities(),
        },
    )
    .bind()
}

fn id(value: &str) -> Result<Id, Error> {
    Id::new(value).map_err(|_| Error::invalid(format!("`{value}` is not an llm identifier")))
}

/// The `inference.model.DeclareModel` of the pinned checkpoint: its family, file, the two
/// licences, its checksum, and its source and revision as provenance.
#[must_use]
pub fn declare_model() -> DeclareModel {
    DeclareModel {
        family: FAMILY.to_owned(),
        checkpoint: CHECKPOINT.to_owned(),
        code_license: CODE_LICENSE.to_owned(),
        weight_license: WEIGHT_LICENSE.to_owned(),
        checksum: CHECKSUM.to_owned(),
        input_modalities: vec![Modality::Text],
        output_modalities: vec![Modality::Text],
        provenance: SOURCE.to_owned(),
        source_revision: SOURCE_REVISION.to_owned(),
    }
}

/// The `inference.model.DeclareAdapter` of a server configured by `config`, for the model
/// `model_id`: backend [`BACKEND`], a local service on a local worker, claiming
/// [`Capability::OutputStreaming`], and [`Capability::ToolCalling`] beside it when
/// [`Config::tools`] is on, with the server's slots as its maximum concurrency.
#[must_use]
pub fn declare_adapter(model_id: ModelId, config: &Config) -> DeclareAdapter {
    let mut capabilities = vec![Capability::OutputStreaming];
    if config.tools {
        capabilities.push(Capability::ToolCalling);
    }
    DeclareAdapter {
        model_id,
        backend: BACKEND.to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::LocalService,
        locality: Locality::LocalWorker,
        capabilities,
        max_concurrency: config.max_concurrency,
    }
}

/// The [`llm_core::Model`] of one `llama-server`: `llm`'s Chat Completions client on the binding
/// of a [`Config`], each turn run on a thread of its own through `llm`'s `BlockingModel`, so the
/// turn can be polled outside a tokio runtime; see the crate documentation.
pub struct LlamaServer {
    blocking: BlockingModel,
    /// Held by the thread of every turn until it let go of everything else it held.
    turns: Arc<()>,
    /// [`Config::max_output_tokens`]: the output limit of a turn that names none.
    max_output_tokens: u64,
}

impl fmt::Debug for LlamaServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlamaServer")
            .field("provenance", self.blocking.provenance())
            .finish_non_exhaustive()
    }
}

impl LlamaServer {
    /// The model of the server `config` names, its client handed `llm`'s `AnonymousResolver`,
    /// which the anonymous binding never asks for a secret. Nothing is sent before the first turn.
    ///
    /// # Errors
    ///
    /// The `llm` error refusing the binding ([`binding`]), the HTTP client, or the runtime that
    /// drives the turns.
    pub fn new(config: &Config) -> Result<Self, Error> {
        let client = ChatClient::new(
            binding(config)?,
            HttpClient::new(config.limits)?,
            Arc::new(AnonymousResolver),
        );
        Ok(Self {
            blocking: BlockingModel::new(Arc::new(client))?,
            turns: Arc::new(()),
            max_output_tokens: config.max_output_tokens,
        })
    }

    /// The generator of `adapter` over this model.
    ///
    /// # Errors
    ///
    /// [`CapabilityNotDeclared`] when `adapter` claims what the binding does not declare.
    pub fn generator(self, adapter: Adapter) -> Result<Generator, CapabilityNotDeclared> {
        Generator::new(adapter, Arc::new(self))
    }

    /// The turns of this model whose thread still runs. A turn that ended, failed or was
    /// cancelled, or whose future was dropped, leaves the count once its thread has handed over
    /// its end and let go of its connection and of everything it shared with the poller; a
    /// count that stays above zero after every generation ended is a leaked task.
    #[must_use]
    pub fn running_turns(&self) -> usize {
        Arc::strong_count(&self.turns) - 1
    }
}

impl Model for LlamaServer {
    fn provenance(&self) -> &Provenance {
        self.blocking.provenance()
    }

    fn capabilities(&self) -> &Capabilities {
        self.blocking.capabilities()
    }

    /// Runs the turn on a thread of its own and hands each event to `sink` as the thread decodes
    /// it. The request is sent once, when the turn is first polled. A turn that names no output
    /// limit is sent with [`Config::max_output_tokens`], so the server stops there instead of at
    /// the end of its context. A refusal of `sink`, or dropping the turn, cancels the request
    /// still running.
    fn turn<'a>(
        &'a self,
        request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, Error>> {
        Box::pin(async move {
            let cancel = cancel.child_token();
            let _cancel_on_drop = cancel.clone().drop_guard();
            let shared = Arc::new(Shared::default());
            let mut request = request.clone();
            request.max_output_tokens = request.max_output_tokens.or(Some(self.max_output_tokens));
            spawn_turn(
                self.blocking.fork(),
                request,
                cancel,
                Arc::clone(&shared),
                Arc::clone(&self.turns),
            )?;
            loop {
                match future::poll_fn(|cx| shared.poll_step(cx)).await {
                    Step::Event(event) => sink.emit(event).await?,
                    Step::Ended(ended) => return *ended,
                }
            }
        })
    }
}

/// Starts the thread that runs one turn of `blocking` on `request`, handing its events and its
/// end to `shared`, and holding `running` until it let go of everything else.
fn spawn_turn(
    blocking: BlockingModel,
    request: TurnRequest,
    cancel: Cancel,
    shared: Arc<Shared>,
    running: Arc<()>,
) -> Result<(), Error> {
    let ending = Ending(Some(Arc::clone(&shared)));
    thread::Builder::new()
        .name("llama-server-turn".to_owned())
        .spawn(move || {
            let mut sink = |event| {
                shared.hand(|handed| handed.events.push_back(event));
                Ok(())
            };
            let ended = blocking.turn(&request, &mut sink, &cancel);
            ending.end(ended);
            drop((blocking, request, cancel, shared));
            drop(running);
        })
        .map(drop)
        .map_err(|error| {
            Error::new(
                ErrorCode::Unavailable,
                format!("the thread of a turn could not start: {error}"),
            )
        })
}

/// Hands the end of a turn to its poller once: the turn's result, or, when its thread unwinds
/// before one, a failure saying so, so the poller is never left waiting.
struct Ending(Option<Arc<Shared>>);

impl Ending {
    fn end(mut self, ended: Result<TurnOutcome, Error>) {
        if let Some(shared) = self.0.take() {
            shared.hand(|handed| handed.ended = Some(ended));
        }
    }
}

impl Drop for Ending {
    fn drop(&mut self) {
        if let Some(shared) = self.0.take() {
            let unwound = Error::new(
                ErrorCode::Unavailable,
                "the thread of the turn ended without a result",
            )
            .with_dispatch(Dispatch::Unknown);
            shared.hand(|handed| handed.ended = Some(Err(unwound)));
        }
    }
}

/// What the thread of one turn handed over and the poller has not yet taken.
#[derive(Default)]
struct Shared(Mutex<Handed>);

#[derive(Default)]
struct Handed {
    events: VecDeque<StreamEvent>,
    ended: Option<Result<TurnOutcome, Error>>,
    waker: Option<Waker>,
}

/// The next thing the poller of a turn takes.
enum Step {
    Event(StreamEvent),
    Ended(Box<Result<TurnOutcome, Error>>),
}

impl Shared {
    fn hand(&self, update: impl FnOnce(&mut Handed)) {
        let waker = {
            let mut handed = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            update(&mut handed);
            handed.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll_step(&self, cx: &mut Context<'_>) -> Poll<Step> {
        let mut handed = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(event) = handed.events.pop_front() {
            return Poll::Ready(Step::Event(event));
        }
        if let Some(ended) = handed.ended.take() {
            return Poll::Ready(Step::Ended(Box::new(ended)));
        }
        handed.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}
