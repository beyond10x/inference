//! `inference.sdk`: `ResolveStack` and its event of `ess/domains/sdk.yaml`, answered by the
//! `inference` crate's [`Stack`].
//!
//! The SDK keeps no store: a stack is a value, resolved from the builder, an environment and stack
//! files. So this module turns each request into those sources and resolves a real [`Stack`]
//! over them: a `builder` binding is [`Stack::bind`], an `environment` one a variable
//! `INFERENCE_<ROLE>_ADAPTER` of the [`Env`] it passes, and a file binding a `[<role>]` table of
//! the file `INFERENCE_STACK` names, of the working directory's `inference.toml` or of the config
//! home's, each written into a temporary directory of the scenario. The profile is the builder's
//! ([`Stack::profile`]). The outcome, the error and the event come from what the resolve returned.
//!
//! The adapter registry is the process's and append-only, so each request registers the adapter
//! names it sends under a prefix of its own, `conform-<n>-`, serving every role with the testkit
//! mocks; a name is registered before the resolve, as an application registers its adapters.
//! The three refusals are declared `external`, and a scenario may force one on the next
//! invocation, which then arranges its cause, never its answer:
//!
//! - `unknown-adapter`: the names are not registered, and the resolve refuses the first;
//! - `role-bound-twice`: the builder binds the `tts` role twice more, and the resolve refuses it;
//! - `locality-denied`: the builder binds `tts` to a synthesiser of an external provider, in
//!   place of any `tts` binding the builder was sent, under `offline_strict` when the sent
//!   profile admits external providers; the resolve binds it, and the answer is the first
//!   binding whose adapter's locality the stack's profile does not admit, as planning over the
//!   stack refuses it.
//!
//! A resolved stack is kept for the scenario, and its `StackResolved` carries a stack identity of
//! this module (`inference.sdk` counts in 8), the stack's profile and the bindings as sent.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ExternalOutcomeControl, ObservedEvent,
    SemanticCommandRequest, SemanticCommandResult, SemanticViewRequest, SemanticViewResult,
    TargetError,
};
use ess_primitives::node::Node;
use inference::core::model::{
    Adapter, AdapterDescriptor as ModelDescriptor, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference::core::policy::ProfileKind;
use inference::pipeline::Catalog;
use inference::text::{AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued};
use inference::tts::{
    ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt, PlaybackRefused,
    SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};
use inference::{
    AdapterDescriptor, Binding, BindingSource, Env, Registry, Role, RoleBoundTwice, Stack,
};
use inference_testkit::duplex::mock::MockSpeechModel;
use inference_testkit::llm::{self, MockGenerator};
use inference_testkit::stt::{self, MockRecogniser};
use inference_testkit::tts::{self, MockSynthesiser};

use super::{Input, Stores, named_node, text_node, unreadable, unsupported};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.sdk";

const RESOLVE_STACK: &str = "inference.sdk.ResolveStack";
const STACK_RESOLVED: &str = "inference.sdk.StackResolved";
const ROLE_BOUND_TWICE: &str = "inference.sdk.RoleBoundTwice";
const ADAPTER_NOT_REGISTERED: &str = "inference.sdk.AdapterNotRegistered";
const LOCALITY_DENIED: &str = "inference.sdk.LocalityDenied";

/// The outcomes of `ResolveStack` the specification declares external.
const EXTERNAL: [&str; 3] = ["role-bound-twice", "unknown-adapter", "locality-denied"];

/// The requests of the whole process, so that no two register the same names.
static REQUESTS: AtomicU64 = AtomicU64::new(0);

/// One binding as a request sends it: `inference.sdk.RoleBinding`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleBinding {
    role: Role,
    adapter: String,
    source: BindingSource,
}

/// A stack this module resolved, and the request that made it.
pub struct Resolved {
    stack_id: String,
    stack: Stack,
    bindings: Vec<RoleBinding>,
}

/// The `inference.sdk` state of one scenario: the stacks it resolved, each one `StackResolved`.
#[derive(Default)]
pub struct State {
    resolved: Vec<Resolved>,
}

impl State {
    /// The stacks resolved so far, one event each.
    pub fn events(&self) -> &[Resolved] {
        &self.resolved
    }
}

pub fn execute_command(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    stores.publish();
    let name = request.command.to_string();
    let forced = stores.forced(&request.command);
    let result = match name.as_str() {
        RESOLVE_STACK => resolve_stack(&mut stores.sdk, forced.as_ref(), request)?,
        _ => return Err(unsupported(&name)),
    };
    stores.publish();
    Ok(result.with_consistency(stores.token()?))
}

pub fn query_view(
    _stores: &mut Stores,
    request: &SemanticViewRequest,
) -> Result<SemanticViewResult, TargetError> {
    Err(unsupported(&request.view.to_string()))
}

pub fn observe_events(
    stores: &mut Stores,
    request: &EventObservationRequest,
) -> Result<Vec<ObservedEvent>, TargetError> {
    let wanted = request.event.to_string();
    if wanted != STACK_RESOLVED {
        return Err(unsupported(&wanted));
    }
    stores.publish();
    let mut observed = Vec::new();
    for (index, resolved) in stores.sdk.events().iter().enumerate() {
        let position = stores.sdk_position(index)?;
        observed.push(
            stack_resolved(resolved)?
                .in_activity(request.correlation.clone())
                .at(position),
        );
    }
    Ok(observed)
}

/// Accepts a force of one of `ResolveStack`'s external outcomes.
pub fn configure_external_outcome(
    stores: &mut Stores,
    request: &ExternalOutcomeControl,
) -> Result<(), TargetError> {
    let command = request.force.command.to_string();
    if command != RESOLVE_STACK {
        return Err(unsupported(&request.force.to_string()));
    }
    if !EXTERNAL
        .iter()
        .any(|branch| request.force == outcome(branch))
    {
        return Err(unreadable(
            format!("forcing `{}`", request.force),
            "it is not an outcome `ess/domains/sdk.yaml` declares external",
        ));
    }
    stores.force(request.force.clone());
    Ok(())
}

fn role_of(name: &str) -> Option<Role> {
    Role::ALL.into_iter().find(|role| role.as_str() == name)
}

fn source_of(name: &str) -> Option<BindingSource> {
    [
        BindingSource::Builder,
        BindingSource::Environment,
        BindingSource::StackFile,
        BindingSource::WorkingDirectoryFile,
        BindingSource::UserConfigFile,
    ]
    .into_iter()
    .find(|source| source.as_str() == name)
}

/// The `bindings` of a request, each read field by field.
fn role_bindings(node: &Node) -> Result<Vec<RoleBinding>, TargetError> {
    let refuse = |detail: String| unreadable(format!("reading `{RESOLVE_STACK}`"), detail);
    let Node::Seq(items) = node else {
        return Err(refuse(format!("`bindings` is not a list: {node:?}")));
    };
    items
        .iter()
        .map(|item| {
            let Node::Map(fields) = item else {
                return Err(refuse(format!("a binding is not a record: {item:?}")));
            };
            let text = |field: &str| match fields.get(field) {
                Some(Node::Text(text)) => Ok(text.clone()),
                other => Err(refuse(format!(
                    "`bindings.{field}` is not a string: {other:?}"
                ))),
            };
            if let Some(extra) = fields
                .keys()
                .find(|field| !["role", "adapter", "source"].contains(&field.as_str()))
            {
                return Err(refuse(format!("`bindings.{extra}` is undeclared")));
            }
            let role = text("role")?;
            let source = text("source")?;
            Ok(RoleBinding {
                role: role_of(&role).ok_or_else(|| refuse(format!("`{role}` is not a role")))?,
                adapter: text("adapter")?,
                source: source_of(&source)
                    .ok_or_else(|| refuse(format!("`{source}` is not a binding source")))?,
            })
        })
        .collect()
}

fn role_binding_node(binding: &RoleBinding) -> Node {
    Node::Map(BTreeMap::from([
        ("role".to_owned(), text_node(binding.role.as_str())),
        ("adapter".to_owned(), text_node(&binding.adapter)),
        ("source".to_owned(), text_node(binding.source.as_str())),
    ]))
}

/// Registers `name` serving every role with a testkit mock.
fn register(name: &str) -> Result<(), TargetError> {
    let pcm16 = inference::audio::AudioFormat {
        encoding: inference::audio::AudioEncoding::PcmS16le,
        sample_rate: std::num::NonZeroU32::new(24_000).unwrap_or(std::num::NonZeroU32::MIN),
        channels: std::num::NonZeroU16::MIN,
    };
    Registry::register(
        AdapterDescriptor::new(name, Locality::SameProcess)
            .role(Role::Stt, |_| {
                Ok(Binding::stt(MockRecogniser::new(
                    stt::MockConfiguration::Incremental,
                    ModelId::generate(),
                    stt::script(),
                ))
                .into())
            })
            .role(Role::Llm, |_| {
                Ok(Binding::llm(MockGenerator::new(
                    llm::MockConfiguration::StreamingOnly,
                    ModelId::generate(),
                    ["resolved"],
                ))
                .into())
            })
            .role(Role::Tts, move |_| {
                Ok(Binding::tts(MockSynthesiser::new(
                    tts::MockConfiguration::IncrementalStreaming,
                    ModelId::generate(),
                    pcm16,
                ))
                .into())
            })
            .role(Role::Speech, |_| {
                Ok(Binding::speech(MockSpeechModel::native(Vec::new())).into())
            }),
    )
    .map_err(|error| unreadable(format!("registering `{name}`"), error.to_string()))
}

/// A synthesiser of an external provider: it holds a declared adapter and synthesises nothing.
struct Hosted {
    adapter: Adapter,
}

impl Hosted {
    fn new() -> Self {
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "conformance hosted synthesiser".to_owned(),
            contract_revision: inference::tts::CONTRACT_REVISION.to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: Vec::new(),
            max_concurrency: 1,
            descriptor: ModelDescriptor::default(),
        })
        .unwrap_or_else(|_| unreachable!("a concurrency of one is positive"));
        Self { adapter }
    }
}

/// The synthesis of [`Hosted`]: the contract's record, which every operation forwards to.
struct HostedSynthesis {
    output: SynthesisOutput,
}

impl Iterator for HostedSynthesis {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        None
    }
}

impl SynthesisHandle for HostedSynthesis {
    fn output(&self) -> &SynthesisOutput {
        &self.output
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        self.output.offer(segment)
    }

    fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        self.output.end_utterance()
    }

    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        self.output.acknowledge_playback(segment_id, receipt)
    }

    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        self.output.discard(segment_id)
    }
}

impl Synthesiser for Hosted {
    type Handle = HostedSynthesis;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(&mut self) -> Result<HostedSynthesis, ConcurrencyLimitReached> {
        Ok(HostedSynthesis {
            output: SynthesisOutput::start(&self.adapter),
        })
    }
}

fn io_error(what: &str, error: std::io::Error) -> TargetError {
    unreadable(what.to_owned(), error.to_string())
}

fn resolve_stack(
    state: &mut State,
    forced: Option<&OutcomeRef>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let profile: ProfileKind = input.named("profile")?;
    let bindings = role_bindings(input.node("bindings")?)?;
    input.finish()?;

    let forced_branch = forced.map(|forced| {
        EXTERNAL
            .into_iter()
            .find(|branch| *forced == outcome(branch))
            .unwrap_or_default()
    });
    let request_number = REQUESTS.fetch_add(1, Ordering::Relaxed) + 1;
    let scoped = |adapter: &str| format!("conform-{request_number}-{adapter}");
    if forced_branch != Some("unknown-adapter") {
        let names: BTreeSet<&str> = bindings
            .iter()
            .map(|binding| binding.adapter.as_str())
            .collect();
        for name in names {
            register(&scoped(name))?;
        }
    }

    let root = tempfile::tempdir().map_err(|error| io_error("a scenario directory", error))?;
    let working = root.path().join("work");
    let config = root.path().join("config");
    let stack_file = root.path().join("stack.toml");
    for dir in [&working, &config.join("inference")] {
        std::fs::create_dir_all(dir).map_err(|error| io_error("a scenario directory", error))?;
    }
    let mut env = Env::new().working_dir(&working).config_home(&config);

    let profile = match (forced_branch, profile) {
        (Some("locality-denied"), ProfileKind::HybridOptIn) => ProfileKind::OfflineStrict,
        (_, sent) => sent,
    };
    let mut stack = Stack::new().profile(profile);
    let mut files: BTreeMap<&'static str, String> = BTreeMap::new();
    for binding in &bindings {
        let name = scoped(&binding.adapter);
        let role = binding.role;
        let table = |text: &mut String| {
            let _ = write!(text, "[{role}]\nadapter = \"{name}\"\n");
        };
        match binding.source {
            BindingSource::Builder => {
                if forced_branch == Some("locality-denied") && role == Role::Tts {
                    continue;
                }
                stack = stack.bind(Binding::named(role, name.clone(), BTreeMap::new()));
            }
            BindingSource::Environment => {
                env = env.var(
                    format!("INFERENCE_{}_ADAPTER", role.as_str().to_uppercase()),
                    &name,
                );
            }
            BindingSource::StackFile => table(files.entry("stack").or_default()),
            BindingSource::WorkingDirectoryFile => table(files.entry("working").or_default()),
            BindingSource::UserConfigFile => table(files.entry("user").or_default()),
        }
    }
    for (file, text) in &files {
        let path = match *file {
            "stack" => stack_file.clone(),
            "working" => working.join("inference.toml"),
            _ => config.join("inference").join("inference.toml"),
        };
        std::fs::write(&path, text).map_err(|error| io_error("a stack file", error))?;
    }
    if files.contains_key("stack") {
        env = env.var(
            "INFERENCE_STACK",
            stack_file.to_str().unwrap_or_default().to_owned(),
        );
    }
    match forced_branch {
        Some("role-bound-twice") => {
            stack = stack
                .bind(Binding::tts(Hosted::new()))
                .bind(Binding::tts(Hosted::new()));
        }
        Some("locality-denied") => stack = stack.bind(Binding::tts(Hosted::new())),
        _ => {}
    }

    let resolved = stack.resolve_with(&env);
    drop(root);
    match (forced_branch, resolved) {
        (None, Ok(stack)) => {
            if let Some(unbound) = bindings
                .iter()
                .find(|binding| stack.adapter(binding.role).is_none())
            {
                return Err(unreadable(
                    format!("resolving `{RESOLVE_STACK}`"),
                    format!("the stack left the role {} unbound", unbound.role),
                ));
            }
            let stack_id = format!("00000008-0000-4000-8000-{:012x}", state.resolved.len() + 1);
            let resolved = Resolved {
                stack_id,
                stack,
                bindings,
            };
            let event = stack_resolved(&resolved)?.in_activity(request.correlation.clone());
            state.resolved.push(resolved);
            Ok(SemanticCommandResult::took(outcome("resolved")).emitting(event))
        }
        (Some("locality-denied"), Ok(stack)) => {
            let profile = Catalog::profile(&stack, stack.profile_id()).ok_or_else(|| {
                unreadable("reading the stack's profile", "the stack does not hold it")
            })?;
            let denied = stack
                .adapter_ids()
                .into_iter()
                .find_map(|(role, id)| {
                    let adapter = Catalog::adapter(&stack, id)?;
                    (!profile.admitted().contains(&adapter.locality()))
                        .then(|| (role, adapter.clone()))
                })
                .ok_or_else(|| {
                    unreadable(
                        format!("forcing `{RESOLVE_STACK}/locality-denied`"),
                        "every bound adapter runs where the stack's profile admits it",
                    )
                })?;
            let (role, adapter) = denied;
            Ok(
                SemanticCommandResult::took(outcome("locality-denied")).with_error(
                    DeclaredErrorValue::new(parse::<ErrorRef>(LOCALITY_DENIED))
                        .with("role", text_node(role.as_str()))
                        .with("adapter_id", named_node(&adapter.adapter_id())?)
                        .with("locality", named_node(&adapter.locality())?)
                        .with("profile_id", named_node(&stack.profile_id())?),
                ),
            )
        }
        (Some("unknown-adapter"), Err(error)) => {
            let shown = error.to_string();
            let sent = bindings
                .iter()
                .find(|binding| shown.contains(&scoped(&binding.adapter)))
                .ok_or_else(|| {
                    unreadable(
                        format!("forcing `{RESOLVE_STACK}/unknown-adapter`"),
                        format!("the refusal names no adapter sent: {shown}"),
                    )
                })?;
            Ok(
                SemanticCommandResult::took(outcome("unknown-adapter")).with_error(
                    DeclaredErrorValue::new(parse::<ErrorRef>(ADAPTER_NOT_REGISTERED))
                        .with("adapter", text_node(&sent.adapter)),
                ),
            )
        }
        (Some("role-bound-twice"), Err(error))
            if std::error::Error::source(&error)
                .is_some_and(|source| source.is::<RoleBoundTwice>()) =>
        {
            let role = std::error::Error::source(&error)
                .and_then(|source| source.downcast_ref::<RoleBoundTwice>())
                .map_or(Role::Tts, |refusal| refusal.role);
            Ok(
                SemanticCommandResult::took(outcome("role-bound-twice")).with_error(
                    DeclaredErrorValue::new(parse::<ErrorRef>(ROLE_BOUND_TWICE))
                        .with("role", text_node(role.as_str())),
                ),
            )
        }
        (branch, Ok(_)) => Err(unreadable(
            format!("resolving `{RESOLVE_STACK}`"),
            format!("the stack resolved where {branch:?} was forced"),
        )),
        (branch, Err(error)) => Err(unreadable(
            format!("resolving `{RESOLVE_STACK}` with {branch:?} forced"),
            error.to_string(),
        )),
    }
}

fn stack_resolved(resolved: &Resolved) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(STACK_RESOLVED))
        .with("stack_id", text_node(&resolved.stack_id))
        .with("profile_id", named_node(&resolved.stack.profile_id())?)
        .with(
            "bindings",
            Node::Seq(resolved.bindings.iter().map(role_binding_node).collect()),
        ))
}

fn outcome(branch: &str) -> OutcomeRef {
    OutcomeRef::new(
        parse::<CommandRef>(RESOLVE_STACK),
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
