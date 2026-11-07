//! The adapter registry: a constructor per adapter name, registered for each adapter whose feature
//! is on, and open to an application's own adapters (`docs/design/sdk.md` §4.4).
//!
//! The table is process-wide and append-only, keyed by adapter name alone: one
//! [`AdapterDescriptor`] per name, listing every role it serves, with no removal. The built-in
//! names are reserved whatever the features, so an application never registers one.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use inference_core::model::Locality;
use inference_duplex::SessionConfig;

use crate::error::Error;
use crate::stack::{Binding, Env, Role};

#[cfg(feature = "aws")]
mod aws;
#[cfg(feature = "llama-server")]
mod llama_server;
#[cfg(feature = "testing")]
mod mock;
#[cfg(feature = "openai")]
mod openai;
#[cfg(feature = "sherpa-onnx")]
mod sherpa_onnx;
#[cfg(feature = "whispercpp")]
mod whispercpp;

/// The built-in adapter names and the feature of this crate each sits behind.
const BUILT_IN: [(&str, &str); 7] = [
    ("sherpa-onnx", "sherpa-onnx"),
    ("whispercpp", "whispercpp"),
    ("llama-server", "llama-server"),
    ("openai", "openai"),
    ("openai-live", "openai-live"),
    ("aws", "aws"),
    ("mock", "testing"),
];

/// What a registered constructor is: the role keys in, the role's [`Binding`] out.
type Construct = dyn Fn(&BTreeMap<String, String>) -> Result<Binding, Error> + Send + Sync;

/// Where a provider adapter reads its model name when the role key `model` is not given: a
/// variable of the stack's [`Env`], if any, else a fixed default.
#[derive(Clone, Copy, Debug)]
struct ModelDefault {
    variable: Option<&'static str>,
    default: &'static str,
}

/// One registration: the adapter's name, its locality, the feature it sits behind, one
/// constructor over role keys per role it serves and, for `speech`, its default session.
pub struct AdapterDescriptor {
    name: String,
    locality: Locality,
    feature: Option<&'static str>,
    roles: Vec<(Role, Arc<Construct>)>,
    session_config: Option<SessionConfig>,
    model_default: Option<ModelDefault>,
}

impl fmt::Debug for AdapterDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdapterDescriptor")
            .field("name", &self.name)
            .field("locality", &self.locality)
            .field("feature", &self.feature)
            .field("roles", &self.roles())
            .field("session_config", &self.session_config)
            .finish_non_exhaustive()
    }
}

impl AdapterDescriptor {
    /// A registration of the adapter `name`, running at `locality`, serving no role yet.
    #[must_use]
    pub fn new(name: &str, locality: Locality) -> Self {
        Self {
            name: name.to_owned(),
            locality,
            feature: None,
            roles: Vec::new(),
            session_config: None,
            model_default: None,
        }
    }

    /// The registration serving `role` through `construct`, which takes the role keys and returns
    /// a binding of that role, usually a typed `Bound<R>` converted with `.into()`. A second
    /// constructor for the same role replaces the first.
    #[must_use]
    pub fn role(
        mut self,
        role: Role,
        construct: impl Fn(&BTreeMap<String, String>) -> Result<Binding, Error> + Send + Sync + 'static,
    ) -> Self {
        self.roles.retain(|(served, _)| *served != role);
        self.roles.push((role, Arc::new(construct)));
        self
    }

    /// The registration behind the feature `feature` of the `inference` crate.
    #[must_use]
    pub const fn feature(mut self, feature: &'static str) -> Self {
        self.feature = Some(feature);
        self
    }

    /// The session a `speech` binding of this adapter opens with, before the stack's `voice` key.
    #[must_use]
    pub fn session_config(mut self, config: SessionConfig) -> Self {
        self.session_config = Some(config);
        self
    }

    /// The model name a constructor is given without the key `model`: `variable` of the stack's
    /// environment when set and not empty, else `default`.
    #[cfg_attr(
        not(any(feature = "openai", feature = "aws")),
        allow(
            dead_code,
            reason = "only the hosted registrations read a model default"
        )
    )]
    #[must_use]
    pub(crate) const fn model_default(
        mut self,
        variable: Option<&'static str>,
        default: &'static str,
    ) -> Self {
        self.model_default = Some(ModelDefault { variable, default });
        self
    }

    /// The adapter's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the adapter runs.
    #[must_use]
    pub const fn locality(&self) -> Locality {
        self.locality
    }

    /// The feature of the `inference` crate it sits behind; none for an application's.
    #[must_use]
    pub const fn feature_name(&self) -> Option<&'static str> {
        self.feature
    }

    /// The roles it serves, in the order they were given.
    #[must_use]
    pub fn roles(&self) -> Vec<Role> {
        self.roles.iter().map(|(role, _)| *role).collect()
    }

    pub(crate) fn constructor(&self, role: Role) -> Option<Arc<Construct>> {
        self.roles
            .iter()
            .find(|(served, _)| *served == role)
            .map(|(_, construct)| Arc::clone(construct))
    }

    pub(crate) const fn default_session_config(&self) -> Option<&SessionConfig> {
        self.session_config.as_ref()
    }

    /// The model name a constructor is given without the key `model`, read from `env`.
    pub(crate) fn default_model(&self, env: &Env) -> Option<String> {
        let ModelDefault { variable, default } = self.model_default?;
        let named = variable
            .and_then(|variable| env.get(variable))
            .map(str::trim)
            .filter(|value| !value.is_empty());
        Some(named.unwrap_or(default).to_owned())
    }
}

/// The process-wide adapter registry.
#[derive(Debug)]
pub enum Registry {}

impl Registry {
    /// Adds `descriptor` to the table every stack resolves names from.
    ///
    /// # Errors
    ///
    /// [`Error::Layer`] naming the adapter when its name is a built-in's (reserved whatever the
    /// features) or already registered, or when it serves no role; nothing is added.
    pub fn register(descriptor: AdapterDescriptor) -> Result<(), Error> {
        let refuse = |message: String| Error::Layer {
            message,
            source: None,
        };
        let name = descriptor.name.clone();
        if built_in_feature(&name).is_some() {
            return Err(refuse(format!(
                "the adapter name \"{name}\" is a built-in adapter's and reserved; register the \
                 application's adapter under another name"
            )));
        }
        if descriptor.roles.is_empty() {
            return Err(refuse(format!(
                "the adapter \"{name}\" serves no role; give it a constructor with \
                 AdapterDescriptor::role"
            )));
        }
        let mut table = table().lock().unwrap_or_else(PoisonError::into_inner);
        if table.contains_key(&name) {
            return Err(refuse(format!(
                "the adapter name \"{name}\" is already registered; register the adapter under \
                 another name"
            )));
        }
        table.insert(name, Arc::new(descriptor));
        Ok(())
    }
}

/// The registration of `name`, if one is in the table.
pub(crate) fn lookup(name: &str) -> Option<Arc<AdapterDescriptor>> {
    table()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(name)
        .cloned()
}

/// The feature the built-in adapter `name` sits behind; none for a name no built-in carries.
pub(crate) fn built_in_feature(name: &str) -> Option<&'static str> {
    BUILT_IN
        .iter()
        .find(|(built_in, _)| *built_in == name)
        .map(|(_, feature)| *feature)
}

fn table() -> &'static Mutex<BTreeMap<String, Arc<AdapterDescriptor>>> {
    static TABLE: OnceLock<Mutex<BTreeMap<String, Arc<AdapterDescriptor>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(built_ins()))
}

/// The registrations of the built-in adapters whose feature is on.
fn built_ins() -> BTreeMap<String, Arc<AdapterDescriptor>> {
    let descriptors: Vec<AdapterDescriptor> = vec![
        #[cfg(feature = "sherpa-onnx")]
        sherpa_onnx::descriptor(),
        #[cfg(feature = "whispercpp")]
        whispercpp::descriptor(),
        #[cfg(feature = "llama-server")]
        llama_server::descriptor(),
        #[cfg(feature = "openai")]
        openai::descriptor(),
        #[cfg(feature = "openai-live")]
        openai::live_descriptor(),
        #[cfg(feature = "aws")]
        aws::descriptor(),
        #[cfg(feature = "testing")]
        mock::descriptor(),
    ];
    descriptors
        .into_iter()
        .map(|descriptor| (descriptor.name.clone(), Arc::new(descriptor)))
        .collect()
}
