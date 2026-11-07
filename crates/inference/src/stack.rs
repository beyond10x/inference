//! The stack: one adapter per role under one deployment profile, resolved from the builder, the
//! environment and the stack files, in that order (`docs/design/sdk.md` §4).
//!
//! A role is bound by the first source that names it: [`Stack::bind`] (the builder), then
//! `INFERENCE_<ROLE>_ADAPTER` with its keys `INFERENCE_<ROLE>_<KEY>`, then the `[<role>]` table of
//! the file `INFERENCE_STACK` names, of `./inference.toml` and of
//! `$XDG_CONFIG_HOME/inference/inference.toml`. The top-level `profile` key has the same sources
//! and order, with [`Stack::profile`] as its builder; with none, the stack declares
//! `offline_strict`. Every source but the builder is read from an [`Env`], so a test passes its
//! own and never touches the process environment.
//!
//! Resolving builds each bound adapter through its registered constructor
//! ([`Registry`](crate::Registry)) and opens nothing: no credential is read and no connection is
//! made until a session or a call runs.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFormat};
use inference_core::model::{Adapter, AdapterId, Locality};
use inference_core::policy::{
    DeclareProfile, DeploymentProfile, ProfileId, ProfileKind, TableRule,
};
use inference_duplex::{NativeSpeechModel, SessionConfig, TurnDetection, TurnTuning};
use inference_llm::Generator;
use inference_pipeline::Catalog;
use inference_stt::Recogniser;
use inference_tts::Synthesiser;

use crate::error::Error;
use crate::registry;

/// A role a stack binds to one adapter: `inference.sdk.Role`. Its lowercase name is the `<role>`
/// of `INFERENCE_<ROLE>_*` and of a stack file's `[<role>]` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Speech to text: a [`Recogniser`].
    Stt,
    /// A text model: a [`Generator`].
    Llm,
    /// Text to speech: a [`Synthesiser`].
    Tts,
    /// A native speech model: a [`NativeSpeechModel`].
    Speech,
}

impl Role {
    /// Every role, in the order a stack lists its bindings.
    pub const ALL: [Self; 4] = [Self::Stt, Self::Llm, Self::Tts, Self::Speech];

    /// The lowercase name: `stt`, `llm`, `tts` or `speech`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stt => "stt",
            Self::Llm => "llm",
            Self::Tts => "tts",
            Self::Speech => "speech",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Stt => 0,
            Self::Llm => 1,
            Self::Tts => 2,
            Self::Speech => 3,
        }
    }

    /// `INFERENCE_<ROLE>_`, the prefix of the role's variables.
    fn prefix(self) -> String {
        format!("INFERENCE_{}_", self.as_str().to_uppercase())
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a binding or the profile came from: `inference.sdk.BindingSource`, first found wins in
/// this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindingSource {
    /// [`Stack::bind`] or [`Stack::profile`].
    Builder,
    /// `INFERENCE_<ROLE>_ADAPTER` or `INFERENCE_PROFILE`.
    Environment,
    /// The file `INFERENCE_STACK` names.
    StackFile,
    /// `./inference.toml`.
    WorkingDirectoryFile,
    /// `$XDG_CONFIG_HOME/inference/inference.toml`.
    UserConfigFile,
}

impl BindingSource {
    /// The name `ess/domains/sdk.yaml` gives it: `builder`, `environment`, `stack_file`,
    /// `working_directory_file` or `user_config_file`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builder => "builder",
            Self::Environment => "environment",
            Self::StackFile => "stack_file",
            Self::WorkingDirectoryFile => "working_directory_file",
            Self::UserConfigFile => "user_config_file",
        }
    }
}

impl fmt::Display for BindingSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `inference.sdk.RoleBoundTwice`: the builder bound `role` twice; the stack resolved nothing.
/// It is the `source()` of the [`Error::Layer`] resolving returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoleBoundTwice {
    pub role: Role,
}

impl fmt::Display for RoleBoundTwice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the role {} is bound twice by the builder; bind each role once",
            self.role
        )
    }
}

impl std::error::Error for RoleBoundTwice {}

/// `inference.sdk.AdapterNotRegistered`: a binding names `adapter`, which no registration of this
/// build holds and no built-in carries; the stack resolved nothing. It is the `source()` of the
/// [`Error::Layer`] resolving returns for a name the builder or a variable gave; a stack file's
/// is an [`Error::StackFile`] naming the file and key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterNotRegistered {
    pub adapter: String,
}

impl fmt::Display for AdapterNotRegistered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no adapter is registered under the name \"{}\"; enable the feature of a built-in \
             adapter or register it with Registry::register",
            self.adapter
        )
    }
}

impl std::error::Error for AdapterNotRegistered {}

/// The role in the type: a zero-sized marker per role, so a [`Bound`] of one role is refused
/// where another's is taken.
pub mod role {
    mod sealed {
        pub trait Sealed {}
    }

    /// A role marker; implemented by [`Stt`], [`Llm`], [`Tts`] and [`Speech`] only.
    pub trait Kind: sealed::Sealed {
        /// The role this marker stands for.
        const ROLE: super::Role;
    }

    /// The `stt` role.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Stt;

    /// The `llm` role.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Llm;

    /// The `tts` role.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Tts;

    /// The `speech` role.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Speech;

    impl sealed::Sealed for Stt {}
    impl sealed::Sealed for Llm {}
    impl sealed::Sealed for Tts {}
    impl sealed::Sealed for Speech {}

    impl Kind for Stt {
        const ROLE: super::Role = super::Role::Stt;
    }

    impl Kind for Llm {
        const ROLE: super::Role = super::Role::Llm;
    }

    impl Kind for Tts {
        const ROLE: super::Role = super::Role::Tts;
    }

    impl Kind for Speech {
        const ROLE: super::Role = super::Role::Speech;
    }
}

/// A recogniser bound to the `stt` role.
pub(crate) trait BoundRecogniser: Send {
    fn adapter(&self) -> &Adapter;

    /// Opens what the recogniser reads before its first recognition: a local checkpoint a
    /// registration deferred so that resolving reads nothing. A recogniser given open does
    /// nothing; one already open does nothing again.
    #[allow(
        dead_code,
        reason = "the session that starts a bound recogniser opens it; resolving never does"
    )]
    fn open(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

impl<T: Recogniser + Send + 'static> BoundRecogniser for T {
    fn adapter(&self) -> &Adapter {
        Recogniser::adapter(self)
    }
}

/// A synthesiser bound to the `tts` role.
pub(crate) trait BoundSynthesiser: Send {
    fn adapter(&self) -> &Adapter;
}

impl<T: Synthesiser + Send + 'static> BoundSynthesiser for T {
    fn adapter(&self) -> &Adapter {
        Synthesiser::adapter(self)
    }
}

/// A native speech model bound to the `speech` role.
pub(crate) trait BoundSpeechModel: Send {
    fn adapter(&self) -> &Adapter;
}

impl<T: NativeSpeechModel + Send + 'static> BoundSpeechModel for T {
    fn adapter(&self) -> &Adapter {
        NativeSpeechModel::adapter(self)
    }
}

/// A typed adapter instance, one variant per role.
pub(crate) enum Value {
    Stt(Box<dyn BoundRecogniser>),
    Llm(Box<Generator>),
    Tts(Box<dyn BoundSynthesiser>),
    Speech(Box<dyn BoundSpeechModel>),
}

impl Value {
    const fn role(&self) -> Role {
        match self {
            Self::Stt(_) => Role::Stt,
            Self::Llm(_) => Role::Llm,
            Self::Tts(_) => Role::Tts,
            Self::Speech(_) => Role::Speech,
        }
    }

    fn adapter(&self) -> &Adapter {
        match self {
            Self::Stt(value) => value.adapter(),
            Self::Llm(value) => value.adapter(),
            Self::Tts(value) => value.adapter(),
            Self::Speech(value) => value.adapter(),
        }
    }
}

enum Target {
    Named {
        adapter: String,
        keys: BTreeMap<String, String>,
    },
    Value(Value),
}

/// One role bound to one adapter, as the builder gives it: by registered name with its role keys
/// ([`Binding::named`]), or as a typed adapter instance ([`Binding::stt`], [`Binding::llm`],
/// [`Binding::tts`], [`Binding::speech`]). [`Stack::bind`] takes it; the role is the binding's.
pub struct Binding {
    role: Role,
    target: Target,
    model: Option<String>,
}

impl fmt::Debug for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("Binding");
        debug.field("role", &self.role);
        match &self.target {
            Target::Named { adapter, keys } => {
                debug.field("adapter", adapter).field("keys", keys);
            }
            Target::Value(value) => {
                debug.field("adapter_id", &value.adapter().adapter_id());
            }
        }
        debug.field("model", &self.model).finish()
    }
}

impl Binding {
    /// The role `role` bound to the adapter registered as `adapter`, its constructor given `keys`.
    #[must_use]
    pub fn named(role: Role, adapter: impl Into<String>, keys: BTreeMap<String, String>) -> Self {
        Self {
            role,
            target: Target::Named {
                adapter: adapter.into(),
                keys,
            },
            model: None,
        }
    }

    /// The `stt` role bound to `recogniser`.
    #[must_use]
    pub fn stt(recogniser: impl Recogniser + Send + 'static) -> Bound<role::Stt> {
        Bound::of(Value::Stt(Box::new(recogniser)))
    }

    /// The `stt` role bound to a recogniser a registration declared without opening it; its
    /// [`BoundRecogniser::open`] reads the checkpoint when a session starts it.
    #[cfg_attr(
        not(any(feature = "sherpa-onnx", feature = "whispercpp")),
        allow(
            dead_code,
            reason = "only the local recognisers' registrations defer an open"
        )
    )]
    pub(crate) fn deferred_stt(recogniser: impl BoundRecogniser + 'static) -> Bound<role::Stt> {
        Bound::of(Value::Stt(Box::new(recogniser)))
    }

    /// The `llm` role bound to `generator`, such as an adapter's `Generator` or a testkit
    /// `MockGenerator`.
    #[must_use]
    pub fn llm(generator: impl Into<Generator>) -> Bound<role::Llm> {
        Bound::of(Value::Llm(Box::new(generator.into())))
    }

    /// The `tts` role bound to `synthesiser`.
    #[must_use]
    pub fn tts(synthesiser: impl Synthesiser + Send + 'static) -> Bound<role::Tts> {
        Bound::of(Value::Tts(Box::new(synthesiser)))
    }

    /// The `speech` role bound to `model`; it opens with [`default_session_config`].
    #[must_use]
    pub fn speech(model: impl NativeSpeechModel + Send + 'static) -> Bound<role::Speech> {
        Bound::of(Value::Speech(Box::new(model)))
    }

    /// The binding with the serving model name `name`, the role key `model`: the name a price
    /// document's `serving_model_id` is matched against.
    #[must_use]
    pub fn model(mut self, name: impl Into<String>) -> Self {
        self.model = Some(name.into());
        self
    }

    /// The role this binding binds.
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }
}

/// A [`Binding`] whose role is in its type, `R` one of the [`role`] markers.
pub struct Bound<R> {
    binding: Binding,
    role: PhantomData<fn() -> R>,
}

impl<R> fmt::Debug for Bound<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Bound").field(&self.binding).finish()
    }
}

impl<R: role::Kind> Bound<R> {
    fn of(value: Value) -> Self {
        debug_assert_eq!(value.role(), R::ROLE);
        Self {
            binding: Binding {
                role: R::ROLE,
                target: Target::Value(value),
                model: None,
            },
            role: PhantomData,
        }
    }

    /// The binding with the serving model name `name`, as [`Binding::model`].
    #[must_use]
    pub fn model(mut self, name: impl Into<String>) -> Self {
        self.binding = self.binding.model(name);
        self
    }
}

impl<R: role::Kind> From<Bound<R>> for Binding {
    fn from(bound: Bound<R>) -> Self {
        bound.binding
    }
}

/// What a stack reads besides the builder: variables, the working directory `./inference.toml`
/// is read from and the config home of `inference/inference.toml`. [`Env::new`] holds none of
/// them; [`Env::process`] takes the process's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Env {
    vars: BTreeMap<String, String>,
    working_dir: Option<PathBuf>,
    config_home: Option<PathBuf>,
}

impl Env {
    /// No variables, no working directory and no config home.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The process's variables (those that are Unicode), its current directory, and
    /// `$XDG_CONFIG_HOME`, or `$HOME/.config` when that is unset or empty.
    #[must_use]
    pub fn process() -> Self {
        let vars: BTreeMap<String, String> = std::env::vars_os()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .collect();
        let config_home = vars
            .get("XDG_CONFIG_HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                vars.get("HOME")
                    .filter(|home| !home.is_empty())
                    .map(|home| Path::new(home).join(".config"))
            });
        Self {
            working_dir: std::env::current_dir().ok(),
            config_home,
            vars,
        }
    }

    /// This environment with the variable `name` set to `value`.
    #[must_use]
    pub fn var(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.vars.insert(name.into(), value.into());
        self
    }

    /// This environment with `path` as its working directory.
    #[must_use]
    pub fn working_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(path.into());
        self
    }

    /// This environment with `path` as its config home.
    #[must_use]
    pub fn config_home(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_home = Some(path.into());
        self
    }

    /// The variable `name`, if set.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    /// The variable `name`, if set and not empty.
    fn present(&self, name: &str) -> Option<&str> {
        self.get(name).filter(|value| !value.trim().is_empty())
    }
}

/// One role of a resolved stack: the adapter bound to it and where that came from.
pub struct StackBinding {
    role: Role,
    name: Option<String>,
    source: BindingSource,
    model: Option<String>,
    value: Value,
}

impl fmt::Debug for StackBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StackBinding")
            .field("role", &self.role)
            .field("name", &self.name)
            .field("source", &self.source)
            .field("model", &self.model)
            .field("adapter_id", &self.adapter().adapter_id())
            .finish()
    }
}

impl StackBinding {
    /// The role it binds.
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }

    /// The registered name of the adapter; none for a typed value the builder bound.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Where the binding came from.
    #[must_use]
    pub const fn source(&self) -> BindingSource {
        self.source
    }

    /// The adapter record the bound instance was declared with.
    #[must_use]
    pub fn adapter(&self) -> &Adapter {
        self.value.adapter()
    }

    /// The serving model name: the role key `model`, else the provider model string the adapter
    /// is configured with, else none.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The generator of an `llm` binding; none for another role.
    #[must_use]
    pub fn generator(&self) -> Option<&Generator> {
        match &self.value {
            Value::Llm(generator) => Some(generator.as_ref()),
            _ => None,
        }
    }
}

/// The base session of a speech adapter: voice `marin`, PCM16 mono at 24 kHz in and out, server
/// voice activity detection with the provider's tuning, `auto_response` on, `auto_interrupt`
/// off, an idle limit of 30 s, no instructions and no tools. A speech adapter bound as a value
/// opens with it; a registered one with its registration's.
#[must_use]
pub fn default_session_config() -> SessionConfig {
    let pcm16 = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: std::num::NonZeroU32::new(24_000).unwrap_or(std::num::NonZeroU32::MIN),
        channels: std::num::NonZeroU16::MIN,
    };
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: String::new(),
        input_format: pcm16,
        output_format: pcm16,
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: false,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(30),
    }
}

/// `inference.sdk.Stack`: one adapter per role under one deployment profile.
pub struct Stack {
    pending: Vec<Binding>,
    bindings: [Option<StackBinding>; 4],
    profile: DeploymentProfile,
    profile_source: Option<BindingSource>,
    session_config: Option<SessionConfig>,
}

impl fmt::Debug for Stack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stack")
            .field("pending", &self.pending)
            .field("bindings", &self.bindings)
            .field("profile", &self.profile)
            .field("profile_source", &self.profile_source)
            .field("session_config", &self.session_config)
            .finish()
    }
}

impl Default for Stack {
    fn default() -> Self {
        Self::new()
    }
}

/// The profile of `kind`, with the table's egress, admitting every locality the table allows.
fn declare_profile(kind: ProfileKind) -> DeploymentProfile {
    let egress = kind.egress();
    let admitted = Locality::ALL
        .into_iter()
        .filter(|locality| TableRule::broken_by(kind, egress, &[*locality]).is_none())
        .collect();
    DeploymentProfile::declare(DeclareProfile {
        kind,
        egress,
        admitted,
    })
    .map(|(profile, _)| profile)
    .unwrap_or_else(|_| unreachable!("a row the table allows is declared"))
}

fn profile_kind(name: &str) -> Option<ProfileKind> {
    match name.trim() {
        "offline_strict" => Some(ProfileKind::OfflineStrict),
        "self_hosted_lan" => Some(ProfileKind::SelfHostedLan),
        "self_hosted_cluster" => Some(ProfileKind::SelfHostedCluster),
        "hybrid_opt_in" => Some(ProfileKind::HybridOptIn),
        _ => None,
    }
}

const PROFILE_NAMES: &str = "offline_strict, self_hosted_lan, self_hosted_cluster or hybrid_opt_in";

/// Where a named binding was read: the builder, the environment, or a stack file.
#[derive(Clone, Copy)]
enum Origin<'a> {
    Builder,
    Environment,
    File(&'a StackFile),
}

impl Origin<'_> {
    const fn source(self) -> BindingSource {
        match self {
            Self::Builder => BindingSource::Builder,
            Self::Environment => BindingSource::Environment,
            Self::File(file) => file.source,
        }
    }

    /// A refusal of the value at `key` of the role `role`: [`Error::StackFile`] naming the file
    /// and `<role>.<key>`, else [`Error::Layer`] naming the variable or the builder.
    fn refuse(self, role: Role, key: &str, message: &str) -> Error {
        match self {
            Self::File(file) => Error::StackFile {
                path: file.path.clone(),
                key: format!("{role}.{key}"),
                message: message.to_owned(),
                source: None,
            },
            Self::Environment => Error::Layer {
                message: format!(
                    "the variable {}{}: {message}",
                    role.prefix(),
                    key.to_uppercase()
                ),
                source: None,
            },
            Self::Builder => Error::Layer {
                message: format!("the builder's binding of the role {role}, key {key}: {message}"),
                source: None,
            },
        }
    }
}

/// The adapter a role table names and the rest of its keys.
type RoleTable = (String, BTreeMap<String, String>);

/// One stack file read: its path, the source it stands for and its top-level table.
struct StackFile {
    path: PathBuf,
    source: BindingSource,
    table: toml::Table,
}

impl StackFile {
    /// Reads `path`; `None` when it is absent and `required` is false.
    fn read(path: PathBuf, source: BindingSource, required: bool) -> Result<Option<Self>, Error> {
        if !required && !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|error| Error::StackFile {
            path: path.clone(),
            key: if source == BindingSource::StackFile {
                "INFERENCE_STACK".to_owned()
            } else {
                "(the file)".to_owned()
            },
            message: format!("the file cannot be read: {error}"),
            source: Some(Box::new(error)),
        })?;
        let table = text
            .parse::<toml::Table>()
            .map_err(|error| Error::StackFile {
                path: path.clone(),
                key: "(the document)".to_owned(),
                message: format!("not TOML: {}", error.message()),
                source: None,
            })?;
        Ok(Some(Self {
            path,
            source,
            table,
        }))
    }

    /// The top-level `profile`, if the file has one.
    fn profile(&self) -> Result<Option<ProfileKind>, Error> {
        let Some(value) = self.table.get("profile") else {
            return Ok(None);
        };
        let refuse = |message: String| Error::StackFile {
            path: self.path.clone(),
            key: "profile".to_owned(),
            message,
            source: None,
        };
        let name = value
            .as_str()
            .ok_or_else(|| refuse(format!("expected a string, one of {PROFILE_NAMES}")))?;
        profile_kind(name)
            .map(Some)
            .ok_or_else(|| refuse(format!("\"{name}\" is not {PROFILE_NAMES}")))
    }

    /// The adapter and the keys of the role's table, if the file has one.
    fn role(&self, role: Role) -> Result<Option<RoleTable>, Error> {
        let Some(value) = self.table.get(role.as_str()) else {
            return Ok(None);
        };
        let origin = Origin::File(self);
        let refuse = |message: &str| Error::StackFile {
            path: self.path.clone(),
            key: role.as_str().to_owned(),
            message: message.to_owned(),
            source: None,
        };
        let table = value.as_table().ok_or_else(|| refuse("expected a table"))?;
        let mut adapter = None;
        let mut keys = BTreeMap::new();
        for (key, value) in table {
            let text = match value {
                toml::Value::String(text) => text.clone(),
                toml::Value::Integer(number) => number.to_string(),
                toml::Value::Float(number) => number.to_string(),
                toml::Value::Boolean(flag) => flag.to_string(),
                _ => {
                    return Err(origin.refuse(role, key, "expected a string, number or boolean"));
                }
            };
            if key == "adapter" {
                adapter = Some(text);
            } else {
                keys.insert(key.clone(), text);
            }
        }
        let adapter = adapter.ok_or_else(|| {
            origin.refuse(
                role,
                "adapter",
                "missing; name the adapter this role is bound to",
            )
        })?;
        Ok(Some((adapter, keys)))
    }
}

/// `${NAME}` in `value` replaced by the variable `NAME` of `env`.
fn expand(value: &str, env: &Env) -> Result<String, String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("\"{value}\" opens ${{ without closing it"))?;
        let name = &after[..end];
        let expanded = env
            .get(name)
            .ok_or_else(|| format!("names the variable {name}, which is not set"))?;
        out.push_str(expanded);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

impl Stack {
    /// An empty stack: no role bound, the default `offline_strict` profile.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            bindings: [None, None, None, None],
            profile: declare_profile(ProfileKind::OfflineStrict),
            profile_source: None,
            session_config: None,
        }
    }

    /// The stack resolved from the process: `Stack::new().resolve_with(&Env::process())`.
    ///
    /// # Errors
    ///
    /// As [`Stack::resolve_with`].
    pub fn from_env() -> Result<Self, Error> {
        Stack::new().resolve_with(&Env::process())
    }

    /// Binds the role `binding` carries, recorded as [`BindingSource::Builder`] when the stack
    /// resolves; the builder comes before every other source.
    #[must_use]
    pub fn bind(mut self, binding: impl Into<Binding>) -> Self {
        self.pending.push(binding.into());
        self
    }

    /// The stack under the profile `kind`, recorded as [`BindingSource::Builder`].
    #[must_use]
    pub fn profile(mut self, kind: ProfileKind) -> Self {
        self.profile = declare_profile(kind);
        self.profile_source = Some(BindingSource::Builder);
        self
    }

    /// [`Stack::resolve_with`] over the process's [`Env::process`].
    ///
    /// # Errors
    ///
    /// As [`Stack::resolve_with`].
    pub fn resolve(self) -> Result<Self, Error> {
        self.resolve_with(&Env::process())
    }

    /// Resolves the stack: the builder's bindings first, then, for each role still unbound, the
    /// first of `INFERENCE_<ROLE>_ADAPTER`, the file `INFERENCE_STACK` names, `./inference.toml`
    /// and `$XDG_CONFIG_HOME/inference/inference.toml` that names it, all read from `env`; the
    /// profile likewise. A role no source names stays unbound.
    ///
    /// A builder binding replaces a binding an earlier resolve read from another source, so
    /// [`Stack::bind`] onto [`Stack::from_env`] wins; only a second builder binding of one role
    /// is refused. A stack file is read only while a role or the profile is still unsettled by
    /// the sources above it, so a broken lower file does not fail a stack those sources settle.
    ///
    /// # Errors
    ///
    /// [`Error::Layer`] for a role the builder binds twice, for an adapter no registration
    /// holds (named by the builder or a variable), for a constructor's binding of another role
    /// or by name, and for a variable a key names that `env` does not set;
    /// [`Error::StackFile`] for a file that cannot be read, is not TOML or has a wrong value,
    /// an unregistered adapter among them, and for a relative `INFERENCE_STACK` when `env` has
    /// no working directory; [`Error::AdapterNotCompiled`] for a built-in adapter whose feature
    /// is off; and whatever a constructor returns.
    pub fn resolve_with(mut self, env: &Env) -> Result<Self, Error> {
        for binding in std::mem::take(&mut self.pending) {
            let role = binding.role;
            if self.bindings[role.index()]
                .as_ref()
                .is_some_and(|bound| bound.source == BindingSource::Builder)
            {
                let refusal = RoleBoundTwice { role };
                return Err(Error::Layer {
                    message: refusal.to_string(),
                    source: Some(Box::new(refusal)),
                });
            }
            let resolved = match binding.target {
                Target::Value(value) => (
                    StackBinding {
                        role,
                        name: None,
                        source: BindingSource::Builder,
                        model: binding.model,
                        value,
                    },
                    (role == Role::Speech).then(default_session_config),
                ),
                Target::Named { adapter, keys } => {
                    let mut keys = keys;
                    if let Some(model) = binding.model {
                        keys.insert("model".to_owned(), model);
                    }
                    Self::named(role, &adapter, keys, Origin::Builder, env)?
                }
            };
            self.settle(resolved);
        }

        if self.profile_source.is_none()
            && let Some(name) = env.present("INFERENCE_PROFILE")
        {
            let kind = profile_kind(name).ok_or_else(|| Error::Layer {
                message: format!(
                    "the variable INFERENCE_PROFILE: \"{name}\" is not {PROFILE_NAMES}"
                ),
                source: None,
            })?;
            self.profile = declare_profile(kind);
            self.profile_source = Some(BindingSource::Environment);
        }

        for role in Role::ALL {
            if self.bindings[role.index()].is_some() {
                continue;
            }
            if let Some(adapter) = env.present(&format!("{}ADAPTER", role.prefix())) {
                let keys = Self::variables(role, env)?;
                let adapter = adapter.to_owned();
                let resolved = Self::named(role, &adapter, keys, Origin::Environment, env)?;
                self.settle(resolved);
            }
        }

        for (path, source, required) in Self::file_paths(env) {
            if self.settled() {
                break;
            }
            let Some(file) = StackFile::read(path?, source, required)? else {
                continue;
            };
            if self.profile_source.is_none()
                && let Some(kind) = file.profile()?
            {
                self.profile = declare_profile(kind);
                self.profile_source = Some(file.source);
            }
            for role in Role::ALL {
                if self.bindings[role.index()].is_some() {
                    continue;
                }
                if let Some((adapter, keys)) = file.role(role)? {
                    let origin = Origin::File(&file);
                    let keys = keys
                        .into_iter()
                        .map(|(key, value)| {
                            expand(&value, env)
                                .map(|value| (key.clone(), value))
                                .map_err(|message| origin.refuse(role, &key, &message))
                        })
                        .collect::<Result<_, _>>()?;
                    let resolved = Self::named(role, &adapter, keys, origin, env)?;
                    self.settle(resolved);
                }
            }
        }
        Ok(self)
    }

    /// Whether every role and the profile are bound, so no lower source is read.
    fn settled(&self) -> bool {
        self.profile_source.is_some() && self.bindings.iter().all(Option::is_some)
    }

    /// The stack files `env` names, in their order of precedence, each with its source and
    /// whether it must exist. A relative `INFERENCE_STACK` is joined to `env`'s working
    /// directory; with none, its path is a refusal, never the process's current directory.
    fn file_paths(env: &Env) -> Vec<(Result<PathBuf, Error>, BindingSource, bool)> {
        let mut paths = Vec::new();
        if let Some(named) = env.present("INFERENCE_STACK") {
            let path = Path::new(named);
            let path = if path.is_absolute() {
                Ok(path.to_path_buf())
            } else if let Some(dir) = &env.working_dir {
                Ok(dir.join(path))
            } else {
                Err(Error::StackFile {
                    path: path.to_path_buf(),
                    key: "INFERENCE_STACK".to_owned(),
                    message: "a relative path, and the environment has no working directory to \
                              read it from; set INFERENCE_STACK to an absolute path, or give the \
                              Env a working directory (Env::working_dir)"
                        .to_owned(),
                    source: None,
                })
            };
            paths.push((path, BindingSource::StackFile, true));
        }
        if let Some(dir) = &env.working_dir {
            paths.push((
                Ok(dir.join("inference.toml")),
                BindingSource::WorkingDirectoryFile,
                false,
            ));
        }
        if let Some(home) = &env.config_home {
            paths.push((
                Ok(home.join("inference").join("inference.toml")),
                BindingSource::UserConfigFile,
                false,
            ));
        }
        paths
    }

    /// The keys `INFERENCE_<ROLE>_<KEY>` of `role` but its adapter, lowercased and expanded.
    fn variables(role: Role, env: &Env) -> Result<BTreeMap<String, String>, Error> {
        let prefix = role.prefix();
        env.vars
            .iter()
            .filter_map(|(name, value)| {
                let key = name.strip_prefix(&prefix)?;
                (key != "ADAPTER" && !key.is_empty()).then(|| (key.to_lowercase(), value))
            })
            .map(|(key, value)| {
                expand(value, env)
                    .map(|value| (key.clone(), value))
                    .map_err(|message| Origin::Environment.refuse(role, &key, &message))
            })
            .collect()
    }

    /// Builds the role `role` through the registration of `adapter`; for `speech`, also the
    /// session it opens with: the registration's default, its voice the key `voice` when given.
    fn named(
        role: Role,
        adapter: &str,
        mut keys: BTreeMap<String, String>,
        origin: Origin<'_>,
        env: &Env,
    ) -> Result<(StackBinding, Option<SessionConfig>), Error> {
        let Some(descriptor) = registry::lookup(adapter) else {
            if let Some(feature) = registry::built_in_feature(adapter) {
                return Err(Error::AdapterNotCompiled {
                    name: adapter.to_owned(),
                    feature: feature.to_owned(),
                });
            }
            let refusal = AdapterNotRegistered {
                adapter: adapter.to_owned(),
            };
            return Err(match origin.refuse(role, "adapter", &refusal.to_string()) {
                Error::Layer { message, .. } => Error::Layer {
                    message,
                    source: Some(Box::new(refusal)),
                },
                Error::StackFile {
                    path, key, message, ..
                } => Error::StackFile {
                    path,
                    key,
                    message,
                    source: Some(Box::new(refusal)),
                },
                other => other,
            });
        };
        let Some(construct) = descriptor.constructor(role) else {
            let served: Vec<&str> = descriptor.roles().into_iter().map(Role::as_str).collect();
            return Err(origin.refuse(
                role,
                "adapter",
                &format!(
                    "the adapter \"{adapter}\" does not serve the role {role}; it serves {}",
                    served.join(", ")
                ),
            ));
        };
        if !keys.contains_key("model")
            && let Some(model) = descriptor.default_model(env)
        {
            keys.insert("model".to_owned(), model);
        }
        let built = construct(&keys)?;
        let value = match built.target {
            Target::Named { adapter: other, .. } => {
                return Err(Error::Layer {
                    message: format!(
                        "the registration \"{adapter}\" returned a binding by the name \
                         \"{other}\" for the role {role}; a constructor returns a typed \
                         binding such as Binding::{role}(..)"
                    ),
                    source: None,
                });
            }
            Target::Value(value) if value.role() != role => {
                return Err(Error::Layer {
                    message: format!(
                        "the registration \"{adapter}\" returned a {} binding for the role {role}",
                        value.role()
                    ),
                    source: None,
                });
            }
            Target::Value(value) => value,
        };
        let session = (role == Role::Speech).then(|| {
            let mut config = descriptor
                .default_session_config()
                .cloned()
                .unwrap_or_else(default_session_config);
            if let Some(voice) = keys.get("voice") {
                config.voice.clone_from(voice);
            }
            config
        });
        let binding = StackBinding {
            role,
            name: Some(adapter.to_owned()),
            source: origin.source(),
            model: keys.get("model").cloned().or(built.model),
            value,
        };
        Ok((binding, session))
    }

    /// Records a resolved binding and, for `speech`, its session.
    fn settle(&mut self, (binding, session): (StackBinding, Option<SessionConfig>)) {
        if binding.role == Role::Speech {
            self.session_config = session;
        }
        let index = binding.role.index();
        self.bindings[index] = Some(binding);
    }

    /// The binding of `role`, once the stack is resolved; none for a role no source names.
    #[must_use]
    pub fn adapter(&self, role: Role) -> Option<&StackBinding> {
        self.bindings[role.index()].as_ref()
    }

    /// The serving model name of `role`'s binding ([`StackBinding::model`]).
    #[must_use]
    pub fn model(&self, role: Role) -> Option<&str> {
        self.adapter(role).and_then(StackBinding::model)
    }

    /// One pair per bound role, in the order `Stt`, `Llm`, `Tts`, `Speech`.
    #[must_use]
    pub fn adapter_ids(&self) -> Vec<(Role, AdapterId)> {
        self.bindings
            .iter()
            .flatten()
            .map(|binding| (binding.role, binding.adapter().adapter_id()))
            .collect()
    }

    /// The id of the profile the stack declared.
    #[must_use]
    pub const fn profile_id(&self) -> ProfileId {
        self.profile.profile_id()
    }

    /// Where the profile came from; none when the default `offline_strict` applies.
    #[must_use]
    pub const fn profile_source(&self) -> Option<BindingSource> {
        self.profile_source
    }

    /// The session the `speech` binding opens with: its adapter's default, the voice the key
    /// `voice` names when given; none while `speech` is unbound.
    #[must_use]
    pub const fn session_config(&self) -> Option<&SessionConfig> {
        self.session_config.as_ref()
    }
}

impl Catalog for Stack {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.bindings
            .iter()
            .flatten()
            .map(StackBinding::adapter)
            .find(|adapter| adapter.adapter_id() == adapter_id)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;

    use inference_core::model::{
        AdapterDescriptor as ModelDescriptor, DeclareAdapter, ExecutionForm, ModelId,
    };

    use super::*;
    use crate::registry::{AdapterDescriptor, Registry};

    /// A recogniser a registration declared and did not open: it counts its opens.
    struct Deferred {
        adapter: Adapter,
        opens: Arc<AtomicUsize>,
        opened: bool,
    }

    impl BoundRecogniser for Deferred {
        fn adapter(&self) -> &Adapter {
            &self.adapter
        }

        fn open(&mut self) -> Result<(), Error> {
            if !self.opened {
                self.opens.fetch_add(1, Ordering::SeqCst);
                self.opened = true;
            }
            Ok(())
        }
    }

    fn within<T: Send + 'static>(case: impl FnOnce() -> T + Send + 'static) -> T {
        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            let _ = done.send(case());
        });
        finished
            .recv_timeout(Duration::from_secs(20))
            .expect("the case finishes within 20 s without panicking")
    }

    #[test]
    fn a_deferred_recogniser_opens_when_its_session_starts_not_at_resolve() {
        within(|| {
            let opens = Arc::new(AtomicUsize::new(0));
            let adapter = Adapter::declare(DeclareAdapter {
                model_id: ModelId::generate(),
                backend: "stack unit deferred".to_owned(),
                contract_revision: "1".to_owned(),
                execution_form: ExecutionForm::NativeFfi,
                locality: Locality::SameProcess,
                capabilities: Vec::new(),
                max_concurrency: 1,
                descriptor: ModelDescriptor::default(),
            })
            .expect("a concurrency of one is positive")
            .0;
            let declared = adapter.adapter_id();
            let counted = Arc::clone(&opens);
            Registry::register(
                AdapterDescriptor::new("stack-unit-deferred", Locality::SameProcess).role(
                    Role::Stt,
                    move |_| {
                        Ok(Binding::deferred_stt(Deferred {
                            adapter: adapter.clone(),
                            opens: Arc::clone(&counted),
                            opened: false,
                        })
                        .into())
                    },
                ),
            )
            .expect("registers");
            let mut stack = Stack::new()
                .resolve_with(&Env::new().var("INFERENCE_STT_ADAPTER", "stack-unit-deferred"))
                .expect("resolves");
            assert_eq!(
                opens.load(Ordering::SeqCst),
                0,
                "resolving opened the recogniser"
            );
            let binding = stack.bindings[Role::Stt.index()]
                .as_mut()
                .expect("stt is bound");
            assert_eq!(binding.adapter().adapter_id(), declared);
            let Value::Stt(recogniser) = &mut binding.value else {
                panic!("stt holds a recogniser");
            };
            recogniser.open().expect("opens");
            recogniser.open().expect("opens again as a no-op");
            assert_eq!(opens.load(Ordering::SeqCst), 1);
            assert_eq!(recogniser.adapter().adapter_id(), declared);
        });
    }
}
