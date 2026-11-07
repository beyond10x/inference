//! Recipes: named, parameterised topologies that declare, validate and plan a pipeline over a stack
//! (`docs/design/sdk.md` §3.3).
//!
//! A [`Recipe`] is a name, an interaction mode, a declare function and the roles of its
//! role-bound nodes. The five built-ins are [`Recipe::chat`], [`Recipe::transcribe`],
//! [`Recipe::speak`], [`Recipe::voice_cascade`] and [`Recipe::native_voice`]; an application
//! writes its own with [`Recipe::custom`] and [`Recipe::role`], and registers it by name with
//! [`Recipe::register`] for [`Recipe::named`] to find.
//!
//! [`Recipe::declare_into`] declares a recipe into a caller's [`Declaration`] over the caller's
//! [`Catalog`] and profile, which the caller then validates and plans; [`Recipe::declaration`] is
//! the recipe's unbound topology; [`Recipe::instantiate`] declares, validates and plans it over a
//! [`Stack`], refusing a role the stack does not bind with [`Error::RoleUnbound`] and a bound
//! adapter whose locality the stack's profile does not admit with [`Error::ProfileDenies`].
//!
//! [`Instantiation::open`] opens a live session over the planned pipeline, each node of its run
//! built for that run: a role-bound node over its role's binding, which owns the model and shares
//! it across runs, and every other node, a fan-out and the cascade's detector, turn policy, commit
//! gates and speaker, by the recipe. The cascade speaker's handle is
//! `Instantiation::sink` (feature `audio-host`) until the next open.

mod run;
#[cfg(feature = "audio-host")]
#[cfg_attr(
    not(all(test, feature = "testing")),
    expect(
        dead_code,
        reason = "the cascade's speaker is built when an agent opens the instantiation over its route"
    )
)]
mod speaker;
mod topology;

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use inference_core::model::{AdapterId, Capability, Locality};
use inference_core::policy::{
    DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind, ProfileNotFound,
};
use inference_facade::live::{LiveSession, LiveSessions, SessionDriver};
use inference_pipeline::nodes::vad::VadConfig;
use inference_pipeline::{
    AdapterNotFound, BindNode, Catalog, Declaration, InteractionMode, LocalityDenied, PipelineId,
    PipelineSpec, PlanPipelineError, PlannedPipeline,
};

use crate::graph::Builder;
use crate::{Error, Payload, Role, Stack};

/// What every exposure of the five recipes buffers, ingress and egress, under
/// `OverloadPolicy::Pause`: 64 events. It trades latency against buffering; a smaller value
/// bounds how far the application falls behind before the model's output pauses.
/// [`Recipe::exposure_capacity`] sets it for one recipe.
pub const EXPOSURE_CAPACITY: NonZeroUsize = NonZeroUsize::MIN.saturating_add(63);

/// The detector of `voice_cascade` unless [`Recipe::vad`] sets another: the reference cascade's
/// values, a threshold of 0.05 of full scale, 30 ms of attack and 200 ms of hold.
const VAD: VadConfig = VadConfig {
    threshold: 0.05,
    attack: Duration::from_millis(30),
    hold: Duration::from_millis(200),
};

/// How many times `native_voice`'s tool loop may return a call to the model: 4.
pub const MAX_TOOL_ITERATIONS: NonZeroUsize = NonZeroUsize::MIN.saturating_add(3);

/// The names the five built-in recipes hold in the recipe table.
const BUILT_IN: [&str; 5] = [
    "chat",
    "transcribe",
    "speak",
    "voice_cascade",
    "native_voice",
];

/// An application's declare function, over the graph builder.
type Declare = dyn Fn(&mut Builder<'_, '_>) -> Result<(), Error> + Send + Sync;

#[derive(Clone)]
enum Topology {
    Chat,
    Transcribe,
    Speak,
    VoiceCascade,
    NativeVoice,
    Custom(Arc<Declare>),
}

/// A named topology in one interaction mode that needs a set of roles; see the module
/// documentation.
#[derive(Clone)]
pub struct Recipe {
    name: String,
    mode: InteractionMode,
    topology: Topology,
    roles: Vec<(String, Role)>,
    capacity: NonZeroUsize,
    vad: VadConfig,
}

impl fmt::Debug for Recipe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Recipe")
            .field("name", &self.name)
            .field("mode", &self.mode)
            .field("roles", &self.roles)
            .field("capacity", &self.capacity)
            .field("vad", &self.vad)
            .finish_non_exhaustive()
    }
}

impl Recipe {
    fn built_in(
        name: &str,
        mode: InteractionMode,
        topology: Topology,
        roles: &[(&str, Role)],
    ) -> Self {
        Self {
            name: name.to_owned(),
            mode,
            topology,
            roles: roles
                .iter()
                .map(|(node, role)| ((*node).to_owned(), *role))
                .collect(),
            capacity: EXPOSURE_CAPACITY,
            vad: VAD,
        }
    }

    /// Text in, the `agent` LLM node, text out: ingress `user_input`, egress `assistant_text`.
    #[must_use]
    pub fn chat() -> Self {
        Self::built_in(
            "chat",
            InteractionMode::TurnBased,
            Topology::Chat,
            &[("agent", Role::Llm)],
        )
    }

    /// Audio in, the `stt` node, committed text out: ingress `audio`, egress `transcript`.
    #[must_use]
    pub fn transcribe() -> Self {
        Self::built_in(
            "transcribe",
            InteractionMode::Continuous,
            Topology::Transcribe,
            &[("stt", Role::Stt)],
        )
    }

    /// Text in, the `tts` node, audio out: ingress `text`, egress `audio`.
    #[must_use]
    pub fn speak() -> Self {
        Self::built_in(
            "speak",
            InteractionMode::Batch,
            Topology::Speak,
            &[("tts", Role::Tts)],
        )
    }

    /// The reference cascade of `local-voice-cascade`: microphone in, its VAD, turn policy, `stt`,
    /// gates, `agent` and `tts` nodes and its speaker sink, with the egresses `transcript` and
    /// `assistant_text`.
    #[must_use]
    pub fn voice_cascade() -> Self {
        Self::built_in(
            "voice_cascade",
            InteractionMode::TurnBased,
            Topology::VoiceCascade,
            &[("stt", Role::Stt), ("agent", Role::Llm), ("tts", Role::Tts)],
        )
    }

    /// The native speech model's `speech` node and the runtime's tool `authority` and `executor`
    /// nodes, with no source or sink: ingresses `audio` and `control`, one egress `events`. The
    /// model's `tool_call` output reaches the authority's `proposed` input through a feedback
    /// edge bounded at [`MAX_TOOL_ITERATIONS`].
    #[must_use]
    pub fn native_voice() -> Self {
        Self::built_in(
            "native_voice",
            InteractionMode::Duplex,
            Topology::NativeVoice,
            &[("speech", Role::Speech)],
        )
    }

    /// An application recipe named `name` in `mode`, its topology what `declare` adds to the
    /// builder. Its nodes have no role until [`Recipe::role`] names them.
    #[must_use]
    pub fn custom(
        name: &str,
        mode: InteractionMode,
        declare: impl Fn(&mut Builder<'_, '_>) -> Result<(), Error> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_owned(),
            mode,
            topology: Topology::Custom(Arc::new(declare)),
            roles: Vec::new(),
            capacity: EXPOSURE_CAPACITY,
            vad: VAD,
        }
    }

    /// Makes the node `node_name` role-bound to `role`: declaring binds it to that role's
    /// adapter, and instantiating needs the role bound. A node named twice keeps its place in
    /// the role order and takes the later role.
    #[must_use]
    pub fn role(mut self, node_name: &str, role: Role) -> Self {
        match self.roles.iter_mut().find(|(node, _)| node == node_name) {
            Some(entry) => entry.1 = role,
            None => self.roles.push((node_name.to_owned(), role)),
        }
        self
    }

    /// Sets the capacity of every exposure the recipe declares, [`EXPOSURE_CAPACITY`] by default.
    /// An application recipe's own exposures are its declare function's.
    #[must_use]
    pub const fn exposure_capacity(mut self, capacity: NonZeroUsize) -> Self {
        self.capacity = capacity;
        self
    }

    /// Sets the configuration of the voice activity detector of `voice_cascade`, which starts
    /// and ends the user's turns; by default a threshold of 0.05 of full scale, 30 ms of attack
    /// and 200 ms of hold, the reference cascade's. Every other recipe has no detector and keeps
    /// it unused.
    #[must_use]
    pub const fn vad(mut self, config: VadConfig) -> Self {
        self.vad = config;
        self
    }

    /// The recipe's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The recipe's interaction mode.
    #[must_use]
    pub const fn mode(&self) -> InteractionMode {
        self.mode
    }

    /// The roles of the recipe's role-bound nodes, in role order: a built-in's as its table
    /// lists them, an application recipe's in the order of its [`Recipe::role`] calls.
    #[must_use]
    pub fn roles(&self) -> Vec<Role> {
        self.roles.iter().map(|(_, role)| *role).collect()
    }

    /// Adds the recipe to the process-wide recipe table under its name, for [`Recipe::named`].
    /// The table is append-only.
    ///
    /// # Errors
    ///
    /// [`Error::RecipeNameTaken`] naming the name when a built-in recipe or an earlier
    /// registration holds it; the table is unchanged.
    pub fn register(self) -> Result<(), Error> {
        let mut table = table().lock().unwrap_or_else(PoisonError::into_inner);
        if BUILT_IN.contains(&self.name.as_str()) || table.contains_key(&self.name) {
            return Err(Error::RecipeNameTaken { name: self.name });
        }
        table.insert(self.name.clone(), self);
        Ok(())
    }

    /// The built-in or registered recipe named `name`.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "chat" => Some(Self::chat()),
            "transcribe" => Some(Self::transcribe()),
            "speak" => Some(Self::speak()),
            "voice_cascade" => Some(Self::voice_cascade()),
            "native_voice" => Some(Self::native_voice()),
            _ => table()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(name)
                .cloned(),
        }
    }

    /// The recipe's unbound topology: a fresh declaration holding one pipeline, under a built-in
    /// `offline_strict` profile with egress denied, with no binding and not planned. An
    /// application recipe whose declare function fails leaves what it added before failing.
    #[must_use]
    pub fn declaration(&self) -> Declaration {
        let catalog = Unbound::new();
        let mut declaration = Declaration::new();
        let _ = self.declare(
            &mut declaration,
            &catalog,
            catalog.profile.profile_id(),
            &[],
        );
        declaration
    }

    /// Declares the recipe's nodes, ports, edges and exposures into `declaration` under
    /// `profile_id`, binds each role-bound node to its role's adapter in `adapters`, and gives
    /// each role named in `requires` that node's requirements in place of the recipe's default.
    /// A pair for a role the recipe has no node for is ignored. The caller validates and plans
    /// the returned pipeline over `catalog`; an application recipe's requirements are its
    /// declare function's, and `requires` does not change them.
    ///
    /// # Errors
    ///
    /// [`Error::Layer`], with `declaration` unchanged: when `catalog` holds no profile
    /// `profile_id` (its source [`ProfileNotFound`]), when a role the recipe needs has no pair
    /// (the first in role order, no source), or when `catalog` holds no adapter a needed pair
    /// names (its source [`AdapterNotFound`]). [`Error::Layer`] too when an application recipe's
    /// declare function fails or names a role-bound node it did not add, which leaves what it
    /// declared.
    pub fn declare_into(
        &self,
        declaration: &mut Declaration,
        catalog: &dyn Catalog,
        profile_id: ProfileId,
        adapters: &[(Role, AdapterId)],
        requires: &[(Role, &[Capability])],
    ) -> Result<PipelineId, Error> {
        if catalog.profile(profile_id).is_none() {
            return Err(Error::Layer {
                message: format!(
                    "the profile {profile_id} is not in the catalog: pass the id of a profile the catalog holds"
                ),
                source: Some(Box::new(ProfileNotFound { profile_id })),
            });
        }
        let mut bound = Vec::with_capacity(self.roles.len());
        for (node, role) in &self.roles {
            let Some((_, adapter_id)) = adapters.iter().find(|(given, _)| given == role) else {
                return Err(Error::Layer {
                    message: format!(
                        "the recipe `{}` needs the role {role}, and no adapter is given for it: pass a ({role}, adapter id) pair",
                        self.name
                    ),
                    source: None,
                });
            };
            if catalog.adapter(*adapter_id).is_none() {
                return Err(Error::Layer {
                    message: format!(
                        "the adapter {adapter_id} given for the role {role} is not in the catalog: pass an adapter the catalog holds"
                    ),
                    source: Some(Box::new(AdapterNotFound {
                        adapter_id: *adapter_id,
                    })),
                });
            }
            bound.push((node.as_str(), *adapter_id));
        }
        let pipeline_id = self.declare(declaration, catalog, profile_id, requires)?;
        for (node, adapter_id) in bound {
            let node_id = declaration
                .nodes()
                .find(|declared| declared.pipeline_id() == pipeline_id && declared.name() == node)
                .map(inference_pipeline::Node::node_id)
                .ok_or_else(|| Error::Layer {
                    message: format!(
                        "the recipe `{}` names the node `{node}` role-bound, and its declare function adds no node of that name: add it or name another",
                        self.name
                    ),
                    source: None,
                })?;
            declaration
                .bind_node(
                    BindNode {
                        pipeline_id,
                        node_id,
                        adapter_id,
                    },
                    catalog,
                )
                .map_err(|error| layer(&error.to_string(), error))?;
        }
        Ok(pipeline_id)
    }

    /// Declares, validates and plans the recipe over `stack`: its roles checked against the
    /// stack's bindings, then [`Recipe::declare_into`] over the stack's catalog, profile and
    /// adapters.
    ///
    /// # Errors
    ///
    /// [`Error::RoleUnbound`] naming the first role in role order the stack does not bind;
    /// [`Error::ProfileDenies`] when planning refuses a bound adapter whose locality the stack's
    /// profile does not admit; [`Error::AdapterNotCompiled`] naming `audio-host` for
    /// `voice_cascade` without that feature; [`Error::Layer`] for any other refusal.
    pub fn instantiate(&self, stack: &Stack) -> Result<Instantiation, Error> {
        if let Some(role) = self
            .roles()
            .into_iter()
            .find(|role| stack.adapter(*role).is_none())
        {
            return Err(Error::RoleUnbound {
                role: role.as_str().to_owned(),
            });
        }
        if matches!(self.topology, Topology::VoiceCascade) && !cfg!(feature = "audio-host") {
            return Err(Error::AdapterNotCompiled {
                name: self.name.clone(),
                feature: "audio-host".to_owned(),
            });
        }
        let mut declaration = Declaration::new();
        let pipeline_id = self.declare_into(
            &mut declaration,
            stack,
            stack.profile_id(),
            &stack.adapter_ids(),
            &[],
        )?;
        let validated = declaration.validate(pipeline_id).map_err(|error| {
            layer(
                &format!("the recipe `{}` does not validate", self.name),
                error,
            )
        })?;
        let planned = declaration
            .plan(validated, stack)
            .map_err(|error| match error {
                PlanPipelineError::LocalityDenied(denied) => profile_denies(stack, denied),
                other => layer(&format!("the recipe `{}` does not plan", self.name), other),
            })?;
        Ok(Instantiation {
            recipe: self.clone(),
            pipeline_id,
            declaration,
            planned,
            #[cfg(feature = "audio-host")]
            speaker: None,
            #[cfg(feature = "audio-host")]
            sink: None,
        })
    }

    /// The declare step `declare_into` runs after its checks; it binds nothing.
    fn declare(
        &self,
        declaration: &mut Declaration,
        catalog: &dyn Catalog,
        profile_id: ProfileId,
        requires: &[(Role, &[Capability])],
    ) -> Result<PipelineId, Error> {
        let spec = PipelineSpec {
            name: self.name.clone(),
            schema_revision: "1".to_owned(),
            mode: self.mode,
            required_capabilities: Vec::new(),
        };
        let requirement = |role: Role, default: &[Capability]| -> Vec<Capability> {
            requires
                .iter()
                .find(|(given, _)| *given == role)
                .map_or(default, |(_, capabilities)| capabilities)
                .to_vec()
        };
        let capacity = self.capacity;
        let (pipeline_id, declared) = declaration
            .build(catalog, profile_id, spec, |pipeline| match &self.topology {
                Topology::Chat => {
                    topology::chat(pipeline, capacity, &requirement);
                    Ok(())
                }
                Topology::Transcribe => {
                    topology::transcribe(pipeline, capacity, &requirement);
                    Ok(())
                }
                Topology::Speak => {
                    topology::speak(pipeline, capacity, &requirement);
                    Ok(())
                }
                Topology::VoiceCascade => {
                    topology::voice_cascade(pipeline, capacity, &requirement);
                    Ok(())
                }
                Topology::NativeVoice => {
                    topology::native_voice(pipeline, capacity, &requirement);
                    Ok(())
                }
                Topology::Custom(declare) => declare(pipeline),
            })
            .map_err(|error| layer(&error.to_string(), error))?;
        declared?;
        Ok(pipeline_id)
    }
}

/// One recipe declared, validated and planned over one stack into one pipeline.
#[derive(Debug, Clone)]
pub struct Instantiation {
    recipe: Recipe,
    pipeline_id: PipelineId,
    declaration: Declaration,
    planned: PlannedPipeline,
    #[cfg(feature = "audio-host")]
    speaker: Option<speaker::Speaker>,
    #[cfg(feature = "audio-host")]
    sink: Option<inference_audio_device::acknowledging::AcknowledgingHandle>,
}

impl Instantiation {
    /// The name of the recipe instantiated.
    #[must_use]
    pub fn recipe(&self) -> &str {
        &self.recipe.name
    }

    /// The pipeline the recipe declared.
    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }

    /// The declaration holding the pipeline, its nodes, ports, edges, exposures and bindings.
    #[must_use]
    pub const fn declaration(&self) -> &Declaration {
        &self.declaration
    }

    /// The planned pipeline.
    #[must_use]
    pub const fn planned(&self) -> &PlannedPipeline {
        &self.planned
    }

    /// Opens a live session of `sessions` over the planned pipeline in the recipe's interaction
    /// mode, its run's nodes built for that run: each role-bound node over its role's binding in
    /// `stack`, the stack the recipe was instantiated over, and every other node by the recipe.
    /// The handles of the run's nodes, such as [`Instantiation::sink`], are the opened run's
    /// until the next open.
    ///
    /// # Errors
    ///
    /// [`Error::ConcurrencyUnclaimed`] before anything opens when the run would build a model
    /// node, which keeps one generation for the whole run and offers it every input, over an
    /// `llm` binding whose adapter does not claim `concurrent_input_output`: every recipe with
    /// an `llm` role but `voice_cascade`, whose agent node starts a generation per turn.
    /// [`Error::Layer`] when a role-bound node's binding in `stack` is not the adapter the recipe
    /// was planned with, when the recipe builds no implementation for one of its nodes (an
    /// application recipe's nodes no role binds, `transcribe`, whose topology has no turn ports,
    /// the tool nodes of `native_voice`, and `voice_cascade` instantiated without an audio
    /// route), or when the session does not open (its source the facade's `OpenError`); the
    /// refusal of a binding that builds no node, such as a deferred recogniser whose checkpoint
    /// did not open. No session is left open.
    pub async fn open(
        &mut self,
        stack: &Stack,
        sessions: &LiveSessions,
    ) -> Result<(LiveSession<Payload>, SessionDriver<Payload>), Error> {
        #[cfg(feature = "audio-host")]
        {
            self.sink = None;
        }
        self.claims_concurrency(stack)?;
        let mut refused: Option<Error> = None;
        let mut nodes = run::Run::new(self, stack);
        let opened = sessions
            .open(
                &self.declaration,
                &self.planned,
                self.recipe.mode,
                |run_id, node| match nodes.node(run_id, node) {
                    Ok(implementation) => implementation,
                    Err(error) => {
                        let reason = error.to_string();
                        refused.get_or_insert(error);
                        Box::new(run::Refused(reason))
                    }
                },
            )
            .await;
        #[cfg(feature = "audio-host")]
        let sink = nodes.into_sink();
        #[cfg(not(feature = "audio-host"))]
        drop(nodes);
        let opened = match (opened, refused) {
            (Ok(opened), None) => opened,
            (Ok(_), Some(error)) | (Err(_), Some(error)) => return Err(error),
            (Err(error), None) => {
                return Err(layer(
                    &format!("the run of the recipe `{}` did not open", self.recipe.name),
                    error,
                ));
            }
        };
        #[cfg(feature = "audio-host")]
        {
            self.sink = sink;
        }
        Ok(opened)
    }

    /// Refuses an `llm` binding of `stack` whose adapter does not claim
    /// `concurrent_input_output` when the run would build a model node over it.
    fn claims_concurrency(&self, stack: &Stack) -> Result<(), Error> {
        if matches!(self.recipe.topology, Topology::VoiceCascade) {
            return Ok(());
        }
        for role in self.recipe.roles() {
            if role != Role::Llm {
                continue;
            }
            let Some(binding) = stack.adapter(role) else {
                continue;
            };
            let adapter = binding.adapter();
            if !adapter
                .capabilities()
                .contains(&Capability::ConcurrentInputOutput)
            {
                return Err(Error::ConcurrencyUnclaimed {
                    role: role.as_str().to_owned(),
                    adapter: binding
                        .name()
                        .map_or_else(|| adapter.adapter_id().to_string(), str::to_owned),
                });
            }
        }
        Ok(())
    }

    /// The handle of the speaker sink of the run [`Instantiation::open`] opened last: the
    /// reference cascade's acknowledging sink, playing through the route `voice_cascade` was
    /// instantiated with. None before a run opened and for every other recipe.
    #[cfg(feature = "audio-host")]
    #[must_use]
    pub const fn sink(
        &self,
    ) -> Option<&inference_audio_device::acknowledging::AcknowledgingHandle> {
        self.sink.as_ref()
    }
}

/// The process-wide table of application recipes, keyed by name.
fn table() -> &'static Mutex<BTreeMap<String, Recipe>> {
    static TABLE: OnceLock<Mutex<BTreeMap<String, Recipe>>> = OnceLock::new();
    TABLE.get_or_init(Mutex::default)
}

fn layer(message: &str, source: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Layer {
        message: message.to_owned(),
        source: Some(Box::new(source)),
    }
}

/// Planning's locality refusal in the SDK's terms: the role whose binding carries the denied
/// adapter, the adapter's registered name, its locality and the stack's profile kind.
fn profile_denies(stack: &Stack, denied: LocalityDenied) -> Error {
    let role = stack
        .adapter_ids()
        .into_iter()
        .find_map(|(role, adapter_id)| (adapter_id == denied.adapter_id).then_some(role))
        .unwrap_or(Role::Speech);
    let adapter = stack
        .adapter(role)
        .and_then(crate::StackBinding::name)
        .map_or_else(|| denied.adapter_id.to_string(), str::to_owned);
    let profile = Catalog::profile(stack, stack.profile_id())
        .map_or(ProfileKind::OfflineStrict, DeploymentProfile::kind);
    Error::ProfileDenies {
        role: role.as_str().to_owned(),
        adapter,
        locality: denied.locality,
        profile,
        source: denied,
    }
}

/// The catalog of [`Recipe::declaration`]: one `offline_strict` profile with egress denied and no
/// adapter.
struct Unbound {
    profile: DeploymentProfile,
}

impl Unbound {
    fn new() -> Self {
        let profile = DeploymentProfile::declare(DeclareProfile {
            kind: ProfileKind::OfflineStrict,
            egress: Egress::Denied,
            admitted: vec![Locality::SameProcess],
        })
        .map_or_else(
            |_| unreachable!("offline_strict admitting the same process is a row of the table"),
            |(profile, _)| profile,
        );
        Self { profile }
    }
}

impl Catalog for Unbound {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, _: AdapterId) -> Option<&inference_core::model::Adapter> {
        None
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use std::collections::BTreeMap;

    use inference_pipeline::{
        Delivery, Essentiality, NodeRole, OverloadPolicy, PortTraits, modality,
    };

    use super::{EXPOSURE_CAPACITY, InteractionMode, Recipe};
    use crate::{Binding, Env, Role, Stack};

    #[test]
    fn a_custom_recipe_instantiates_over_its_one_bound_role() {
        let stack = Stack::new()
            .bind(Binding::named(Role::Llm, "mock", BTreeMap::new()))
            .resolve_with(&Env::new())
            .expect("the stack resolves");
        let instantiation = Recipe::custom("ask", InteractionMode::TurnBased, |pipeline| {
            let text = PortTraits {
                essentiality: Essentiality::Essential,
                delivery: Delivery::Reliable,
                incremental: true,
                end_marker_required: true,
                provisional_allowed: false,
            };
            let agent = pipeline.node(
                "agent",
                NodeRole::Model,
                vec![inference_core::model::Capability::OutputStreaming],
            );
            let input = pipeline.input::<modality::Text>(&agent, "user_input", text);
            let output = pipeline.output::<modality::Text>(&agent, "committed_text", text);
            pipeline.expose(
                &input,
                "user_input",
                EXPOSURE_CAPACITY,
                OverloadPolicy::Pause,
            );
            pipeline.expose(
                &output,
                "assistant_text",
                EXPOSURE_CAPACITY,
                OverloadPolicy::Pause,
            );
            Ok(())
        })
        .role("agent", Role::Llm)
        .instantiate(&stack)
        .expect("the custom recipe plans");
        let bound: Vec<_> = instantiation
            .declaration()
            .bindings()
            .map(|binding| {
                instantiation
                    .declaration()
                    .node(binding.node_id())
                    .map(|node| node.name().to_owned())
            })
            .collect();
        assert_eq!(bound, [Some("agent".to_owned())]);
    }
}
