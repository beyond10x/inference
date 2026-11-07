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
//!
//! `RegisterRecipe` registers a real `Recipe::custom` in the crate's process-wide recipe table,
//! under the sent name scoped as above, in the sent mode with one role-bound node per sent role;
//! a forced `name-taken` registers it once first, and the second registration's refusal is the
//! answer. `InstantiateRecipe` runs `Recipe::instantiate`: over the stack the scenario resolved
//! under the sent identity, or else one binding every role to the mocks, of the recipe the table
//! holds under the sent name, or else `chat`. A forced `role-unbound` instantiates `speak` over a
//! stack binding only `llm`, and a forced `profile-denies` `speak` over an `offline_strict` stack
//! binding `tts` to the external provider's synthesiser below; each answer is the refusal the
//! instantiation returned. An instantiation's identity counts in 9.
//!
//! `OpenInstantiation` runs `Instantiation::open` over a fresh `LiveSessions`, polled on this
//! thread, on the instantiation the module holds under the sent identity, over the stack it was
//! instantiated over; the opened session and its driver are dropped. A forced `concurrency-unclaimed` opens `chat` instantiated
//! over a stack binding `llm` to a generator that claims only output streaming, and the answer
//! is the refusal the open returned. The mocks this module registers claim
//! `concurrent_input_output` for `llm`, as the crate's `mock` registration does.

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
use inference::facade::live::LiveSessions;
use inference::pipeline::{Catalog, InteractionMode, PipelineId};
use inference::text::{AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued};
use inference::tts::{
    ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt, PlaybackRefused,
    SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};
use inference::{
    AdapterDescriptor, Binding, BindingSource, Env, Error, ErrorKind, Instantiation, Recipe,
    Registry, Role, RoleBoundTwice, Stack,
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
const REGISTER_RECIPE: &str = "inference.sdk.RegisterRecipe";
const INSTANTIATE_RECIPE: &str = "inference.sdk.InstantiateRecipe";
const OPEN_INSTANTIATION: &str = "inference.sdk.OpenInstantiation";
const INSTANTIATION_OPENED: &str = "inference.sdk.InstantiationOpened";
const CONCURRENCY_UNCLAIMED: &str = "inference.sdk.ConcurrencyUnclaimed";
const RECIPE_REGISTERED: &str = "inference.sdk.RecipeRegistered";
const RECIPE_INSTANTIATED: &str = "inference.sdk.RecipeInstantiated";
const RECIPE_NAME_TAKEN: &str = "inference.sdk.RecipeNameTaken";
const ROLE_UNBOUND: &str = "inference.sdk.RoleUnbound";
const PROFILE_DENIES: &str = "inference.sdk.ProfileDenies";

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

/// A recipe this module registered, under the name the request sent.
pub struct Registered {
    name: String,
    mode: InteractionMode,
    roles: Vec<Role>,
}

/// A recipe this module instantiated over a stack, the instantiation, and the stack when the
/// module made it for the instantiation rather than resolving it for the scenario.
pub struct Instantiated {
    instantiation_id: String,
    recipe: String,
    stack_id: String,
    pipeline_id: PipelineId,
    instantiation: Instantiation,
    owned: Option<Stack>,
}

/// An instantiation this module opened a session over.
pub struct Opened {
    instantiation_id: String,
}

/// One event of `inference.sdk`, in the order the module recorded them.
pub enum Recorded {
    Resolved(Box<Resolved>),
    Registered(Registered),
    Instantiated(Box<Instantiated>),
    Opened(Opened),
}

/// The `inference.sdk` state of one scenario: what it resolved, registered and instantiated,
/// each one event.
#[derive(Default)]
pub struct State {
    recorded: Vec<Recorded>,
}

impl State {
    /// The events recorded so far.
    pub fn events(&self) -> &[Recorded] {
        &self.recorded
    }

    fn count(&self, wanted: fn(&Recorded) -> bool) -> usize {
        self.recorded.iter().filter(|event| wanted(event)).count()
    }

    /// The stack this module resolved under `stack_id`.
    fn stack(&self, stack_id: &str) -> Option<&Stack> {
        self.recorded.iter().find_map(|event| match event {
            Recorded::Resolved(resolved) if resolved.stack_id == stack_id => Some(&resolved.stack),
            _ => None,
        })
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
        REGISTER_RECIPE => register_recipe(&mut stores.sdk, forced.as_ref(), request)?,
        INSTANTIATE_RECIPE => instantiate_recipe(&mut stores.sdk, forced.as_ref(), request)?,
        OPEN_INSTANTIATION => open_instantiation(&mut stores.sdk, forced.as_ref(), request)?,
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
    if ![
        STACK_RESOLVED,
        RECIPE_REGISTERED,
        RECIPE_INSTANTIATED,
        INSTANTIATION_OPENED,
    ]
    .contains(&wanted.as_str())
    {
        return Err(unsupported(&wanted));
    }
    stores.publish();
    let mut observed = Vec::new();
    for (index, recorded) in stores.sdk.events().iter().enumerate() {
        let event = match recorded {
            Recorded::Resolved(resolved) if wanted == STACK_RESOLVED => stack_resolved(resolved)?,
            Recorded::Registered(registered) if wanted == RECIPE_REGISTERED => {
                recipe_registered(registered)?
            }
            Recorded::Instantiated(instantiated) if wanted == RECIPE_INSTANTIATED => {
                recipe_instantiated(instantiated)?
            }
            Recorded::Opened(opened) if wanted == INSTANTIATION_OPENED => {
                instantiation_opened(opened)
            }
            _ => continue,
        };
        let position = stores.sdk_position(index)?;
        observed.push(event.in_activity(request.correlation.clone()).at(position));
    }
    Ok(observed)
}

/// Accepts a force of one of `ResolveStack`'s external outcomes.
pub fn configure_external_outcome(
    stores: &mut Stores,
    request: &ExternalOutcomeControl,
) -> Result<(), TargetError> {
    let command = request.force.command.to_string();
    let external: &[&str] = match command.as_str() {
        RESOLVE_STACK => &EXTERNAL,
        REGISTER_RECIPE => &["name-taken"],
        INSTANTIATE_RECIPE => &["role-unbound", "profile-denies"],
        OPEN_INSTANTIATION => &["concurrency-unclaimed"],
        _ => return Err(unsupported(&request.force.to_string())),
    };
    if !external
        .iter()
        .any(|branch| request.force == recipe_outcome(&command, branch))
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
                    llm::MockConfiguration::StreamingAndConcurrentInput,
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
            let stack_id = format!(
                "00000008-0000-4000-8000-{:012x}",
                state.count(|event| matches!(event, Recorded::Resolved(_))) + 1
            );
            let resolved = Resolved {
                stack_id,
                stack,
                bindings,
            };
            let event = stack_resolved(&resolved)?.in_activity(request.correlation.clone());
            state.recorded.push(Recorded::Resolved(Box::new(resolved)));
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

/// Registers an application recipe under the request's name, scoped to the request, in the
/// request's mode with one role-bound node per sent role. A forced `name-taken` registers the
/// name once first, as an earlier registration of the process holds it, and answers the second.
fn register_recipe(
    state: &mut State,
    forced: Option<&OutcomeRef>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let name = input.text("name")?;
    let mode: InteractionMode = input.named("mode")?;
    let roles = roles_of(input.node("roles")?)?;
    input.finish()?;

    let request_number = REQUESTS.fetch_add(1, Ordering::Relaxed) + 1;
    let scoped = format!("conform-{request_number}-{name}");
    let recipe = roles
        .iter()
        .fold(Recipe::custom(&scoped, mode, |_| Ok(())), |recipe, role| {
            recipe.role(role.as_str(), *role)
        });
    let taken =
        forced.is_some_and(|forced| *forced == recipe_outcome(REGISTER_RECIPE, "name-taken"));
    if taken {
        recipe.clone().register().map_err(|error| {
            unreadable(format!("registering `{scoped}` first"), error.to_string())
        })?;
    }
    match (taken, recipe.register()) {
        (false, Ok(())) => {
            let registered = Registered { name, mode, roles };
            let event = recipe_registered(&registered)?.in_activity(request.correlation.clone());
            state.recorded.push(Recorded::Registered(registered));
            Ok(
                SemanticCommandResult::took(recipe_outcome(REGISTER_RECIPE, "registered"))
                    .emitting(event),
            )
        }
        (true, Err(error))
            if error.kind() == ErrorKind::Layer && error.to_string().contains(&scoped) =>
        {
            Ok(
                SemanticCommandResult::took(recipe_outcome(REGISTER_RECIPE, "name-taken"))
                    .with_error(
                        DeclaredErrorValue::new(parse::<ErrorRef>(RECIPE_NAME_TAKEN))
                            .with("name", text_node(&name)),
                    ),
            )
        }
        (taken, result) => Err(unreadable(
            format!("registering `{scoped}`, name-taken forced: {taken}"),
            format!("the table answered {result:?}"),
        )),
    }
}

/// Instantiates the recipe the request names, or `chat` for a name the recipe table does not
/// hold, over the stack the scenario resolved under the sent identity, or else a stack binding
/// every role to a testkit mock. A forced `role-unbound` instantiates `speak` over a stack
/// binding only `llm`; a forced `profile-denies` instantiates `speak` over an `offline_strict`
/// stack binding `tts` to a synthesiser of an external provider. Each answer is what
/// `Recipe::instantiate` returned.
fn instantiate_recipe(
    state: &mut State,
    forced: Option<&OutcomeRef>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let recipe_name = input.text("recipe")?;
    let stack_id = input.text("stack_id")?;
    input.finish()?;

    let branch = ["role-unbound", "profile-denies"]
        .into_iter()
        .find(|branch| {
            forced.is_some_and(|forced| *forced == recipe_outcome(INSTANTIATE_RECIPE, branch))
        });
    let request_number = REQUESTS.fetch_add(1, Ordering::Relaxed) + 1;
    let mock = format!("conform-{request_number}-mock");
    let resolve = |stack: Stack| {
        stack
            .resolve_with(&Env::new())
            .map_err(|error| unreadable("resolving the instantiation's stack", error.to_string()))
    };
    let (recipe, owned) = match branch {
        Some("role-unbound") => {
            register(&mock)?;
            let stack = Stack::new().bind(Binding::named(Role::Llm, mock, BTreeMap::new()));
            (Recipe::speak(), Some(resolve(stack)?))
        }
        Some(_) => {
            let stack = Stack::new()
                .profile(ProfileKind::OfflineStrict)
                .bind(Binding::tts(Hosted::new()));
            (Recipe::speak(), Some(resolve(stack)?))
        }
        None => {
            let recipe = Recipe::named(&recipe_name).unwrap_or_else(Recipe::chat);
            if state.stack(&stack_id).is_some() {
                (recipe, None)
            } else {
                register(&mock)?;
                let stack = Role::ALL.into_iter().fold(Stack::new(), |stack, role| {
                    stack.bind(Binding::named(role, mock.clone(), BTreeMap::new()))
                });
                (recipe, Some(resolve(stack)?))
            }
        }
    };
    let result = match owned.as_ref().or_else(|| state.stack(&stack_id)) {
        Some(stack) => recipe.instantiate(stack),
        None => {
            return Err(unreadable(
                "finding the instantiation's stack",
                "none is held",
            ));
        }
    };

    match (branch, result) {
        (None, Ok(instantiation)) => {
            let instantiated = Instantiated {
                instantiation_id: format!(
                    "00000009-0000-4000-8000-{:012x}",
                    state.count(|event| matches!(event, Recorded::Instantiated(_))) + 1
                ),
                recipe: recipe_name,
                stack_id,
                pipeline_id: instantiation.pipeline_id(),
                instantiation,
                owned,
            };
            let event =
                recipe_instantiated(&instantiated)?.in_activity(request.correlation.clone());
            state
                .recorded
                .push(Recorded::Instantiated(Box::new(instantiated)));
            Ok(
                SemanticCommandResult::took(recipe_outcome(INSTANTIATE_RECIPE, "instantiated"))
                    .emitting(event),
            )
        }
        (Some("role-unbound"), Err(Error::RoleUnbound { role })) => Ok(
            SemanticCommandResult::took(recipe_outcome(INSTANTIATE_RECIPE, "role-unbound"))
                .with_error(
                    DeclaredErrorValue::new(parse::<ErrorRef>(ROLE_UNBOUND))
                        .with("role", text_node(&role)),
                ),
        ),
        (
            Some("profile-denies"),
            Err(Error::ProfileDenies {
                role,
                adapter,
                locality,
                profile,
                ..
            }),
        ) => Ok(
            SemanticCommandResult::took(recipe_outcome(INSTANTIATE_RECIPE, "profile-denies"))
                .with_error(
                    DeclaredErrorValue::new(parse::<ErrorRef>(PROFILE_DENIES))
                        .with("role", text_node(&role))
                        .with("adapter", text_node(&adapter))
                        .with("locality", named_node(&locality)?)
                        .with("profile", named_node(&profile)?),
                ),
        ),
        (branch, result) => Err(unreadable(
            format!("instantiating `{}` with {branch:?} forced", recipe.name()),
            match result {
                Ok(_) => "the recipe instantiated".to_owned(),
                Err(error) => error.to_string(),
            },
        )),
    }
}

/// Opens a session over the instantiation the module holds under the sent identity, or, with
/// `concurrency-unclaimed` forced, over `chat` instantiated over a stack binding `llm` to a
/// generator that claims only output streaming. Each answer is what `Instantiation::open`
/// returned.
fn open_instantiation(
    state: &mut State,
    forced: Option<&OutcomeRef>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let instantiation_id = input.text("instantiation_id")?;
    input.finish()?;
    let sessions = LiveSessions::new();

    if forced.is_some_and(|forced| {
        *forced == recipe_outcome(OPEN_INSTANTIATION, "concurrency-unclaimed")
    }) {
        let stack = Stack::new()
            .bind(Binding::llm(MockGenerator::new(
                llm::MockConfiguration::StreamingOnly,
                ModelId::generate(),
                ["never sent"],
            )))
            .resolve_with(&Env::new())
            .map_err(|error| unreadable("resolving the open's stack", error.to_string()))?;
        let mut instantiation = Recipe::chat()
            .instantiate(&stack)
            .map_err(|error| unreadable("instantiating `chat`", error.to_string()))?;
        return match block_on(instantiation.open(&stack, &sessions)) {
            Err(Error::ConcurrencyUnclaimed { role, adapter }) => Ok(SemanticCommandResult::took(
                recipe_outcome(OPEN_INSTANTIATION, "concurrency-unclaimed"),
            )
            .with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(CONCURRENCY_UNCLAIMED))
                    .with("role", text_node(&role))
                    .with("adapter", text_node(&adapter)),
            )),
            other => Err(unreadable(
                "opening `chat` over a generator without concurrent input",
                match other {
                    Ok(_) => "the session opened".to_owned(),
                    Err(error) => error.to_string(),
                },
            )),
        };
    }

    let Some(index) = state.recorded.iter().position(|event| {
        matches!(event, Recorded::Instantiated(held) if held.instantiation_id == instantiation_id)
    }) else {
        return Err(unreadable(
            format!("opening the instantiation `{instantiation_id}`"),
            "the module holds no instantiation under it",
        ));
    };
    let (mut instantiation, stack_id, owned) = match state.recorded.get_mut(index) {
        Some(Recorded::Instantiated(held)) => (
            held.instantiation.clone(),
            held.stack_id.clone(),
            held.owned.take(),
        ),
        _ => return Err(unreadable("finding the instantiation", "none is held")),
    };
    let opened = match owned.as_ref().or_else(|| state.stack(&stack_id)) {
        Some(stack) => block_on(instantiation.open(stack, &sessions)),
        None => {
            return Err(unreadable(
                "finding the instantiation's stack",
                "none is held",
            ));
        }
    };
    if let Some(Recorded::Instantiated(held)) = state.recorded.get_mut(index) {
        held.owned = owned;
    }
    match opened {
        Ok(_session) => {
            let opened = Opened { instantiation_id };
            let event = instantiation_opened(&opened).in_activity(request.correlation.clone());
            state.recorded.push(Recorded::Opened(opened));
            Ok(
                SemanticCommandResult::took(recipe_outcome(OPEN_INSTANTIATION, "opened"))
                    .emitting(event),
            )
        }
        Err(error) => Err(unreadable(
            format!("opening the instantiation `{instantiation_id}`"),
            error.to_string(),
        )),
    }
}

fn instantiation_opened(opened: &Opened) -> ObservedEvent {
    ObservedEvent::new(parse::<EventRef>(INSTANTIATION_OPENED))
        .with("instantiation_id", text_node(&opened.instantiation_id))
}

/// Polls `future` on this thread until it completes: the facade commits to no executor.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
    let mut future = std::pin::pin!(future);
    loop {
        if let std::task::Poll::Ready(output) = future
            .as_mut()
            .poll(&mut std::task::Context::from_waker(&waker))
        {
            return output;
        }
        std::thread::park_timeout(std::time::Duration::from_millis(10));
    }
}

/// The `roles` of a request, each a role's name.
fn roles_of(node: &Node) -> Result<Vec<Role>, TargetError> {
    let refuse = |detail: String| unreadable(format!("reading `{REGISTER_RECIPE}`"), detail);
    let Node::Seq(items) = node else {
        return Err(refuse(format!("`roles` is not a list: {node:?}")));
    };
    items
        .iter()
        .map(|item| match item {
            Node::Text(name) => {
                role_of(name).ok_or_else(|| refuse(format!("`{name}` is not a role")))
            }
            other => Err(refuse(format!("a role is not a string: {other:?}"))),
        })
        .collect()
}

fn recipe_registered(registered: &Registered) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(RECIPE_REGISTERED))
        .with("name", text_node(&registered.name))
        .with("mode", named_node(&registered.mode)?)
        .with(
            "roles",
            Node::Seq(
                registered
                    .roles
                    .iter()
                    .map(|role| text_node(role.as_str()))
                    .collect(),
            ),
        ))
}

fn recipe_instantiated(instantiated: &Instantiated) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(RECIPE_INSTANTIATED))
        .with(
            "instantiation_id",
            text_node(&instantiated.instantiation_id),
        )
        .with("recipe", text_node(&instantiated.recipe))
        .with("stack_id", text_node(&instantiated.stack_id))
        .with("pipeline_id", named_node(&instantiated.pipeline_id)?))
}

fn recipe_outcome(command: &str, branch: &str) -> OutcomeRef {
    OutcomeRef::new(
        parse::<CommandRef>(command),
        branch
            .parse()
            .unwrap_or_else(|error| panic!("`{branch}` is an outcome name: {error}")),
    )
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
