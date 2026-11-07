//! The SDK's one error (`docs/design/sdk.md` §9): one [`Error`] whose variants map one to one onto
//! what the user can change, each the user can fix naming the fix in its `Display`, and
//! [`report`], which writes an error and its causes the way a program's end shows them.

use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use inference_core::model::Locality;
use inference_core::policy::ProfileKind;
use inference_pipeline::LocalityDenied;

/// What a call of the SDK returns.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Every failure of the SDK. Its `Display` names the fix where the user can make one: the feature
/// to enable, the variable to set, the file and key to change.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A stack names the adapter `name`, whose feature `feature` of this crate is off.
    AdapterNotCompiled { name: String, feature: String },
    /// A recipe needs the role `role` (`stt`, `llm`, `tts`, `speech`), which the stack does not
    /// bind.
    RoleUnbound { role: String },
    /// The adapter `adapter` bound to the role `role` runs at `locality`, which the stack's
    /// profile `profile` does not admit; `source` is planning's refusal.
    ProfileDenies {
        role: String,
        adapter: String,
        locality: Locality,
        profile: ProfileKind,
        source: LocalityDenied,
    },
    /// The stack file `path` has a wrong or missing value at `key`; `source`, when present, is the
    /// typed refusal behind it, such as an unregistered adapter.
    StackFile {
        path: PathBuf,
        key: String,
        message: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// The audio device `selector` could not be opened; `devices` are the names the host reports.
    Device {
        selector: String,
        devices: Vec<String>,
        message: String,
    },
    /// The provider behind the adapter `adapter` failed.
    Provider {
        adapter: String,
        message: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// The adapter `adapter` bound to the role `role` does not claim `concurrent_input_output`,
    /// which the run's model node needs: it keeps one generation for the whole run and offers it
    /// every input.
    ConcurrencyUnclaimed { role: String, adapter: String },
    /// `inference.sdk.RecipeNameTaken`: a built-in recipe or an earlier registration of this
    /// process holds the recipe name `name`; nothing was registered.
    RecipeNameTaken { name: String },
    /// A cost was asked for and no price document is named.
    NoPrices,
    /// The handle or session is closed.
    Closed { message: String },
    /// An error of the layer beneath that the SDK does not classify.
    Layer {
        message: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

/// Which [`Error`] variant an error is, one value per variant, named as it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    AdapterNotCompiled,
    RoleUnbound,
    ProfileDenies,
    StackFile,
    Device,
    Provider,
    ConcurrencyUnclaimed,
    RecipeNameTaken,
    NoPrices,
    Closed,
    Layer,
}

impl Error {
    /// The variant this error is.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::AdapterNotCompiled { .. } => ErrorKind::AdapterNotCompiled,
            Self::RoleUnbound { .. } => ErrorKind::RoleUnbound,
            Self::ProfileDenies { .. } => ErrorKind::ProfileDenies,
            Self::StackFile { .. } => ErrorKind::StackFile,
            Self::Device { .. } => ErrorKind::Device,
            Self::Provider { .. } => ErrorKind::Provider,
            Self::ConcurrencyUnclaimed { .. } => ErrorKind::ConcurrencyUnclaimed,
            Self::RecipeNameTaken { .. } => ErrorKind::RecipeNameTaken,
            Self::NoPrices => ErrorKind::NoPrices,
            Self::Closed { .. } => ErrorKind::Closed,
            Self::Layer { .. } => ErrorKind::Layer,
        }
    }
}

fn locality_name(locality: Locality) -> &'static str {
    match locality {
        Locality::SameProcess => "same_process",
        Locality::LocalWorker => "local_worker",
        Locality::SelfHostedCluster => "self_hosted_cluster",
        Locality::ExternalProvider => "external_provider",
    }
}

fn profile_name(profile: ProfileKind) -> &'static str {
    match profile {
        ProfileKind::OfflineStrict => "offline_strict",
        ProfileKind::SelfHostedLan => "self_hosted_lan",
        ProfileKind::SelfHostedCluster => "self_hosted_cluster",
        ProfileKind::HybridOptIn => "hybrid_opt_in",
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AdapterNotCompiled { name, feature } => write!(
                f,
                "the adapter \"{name}\" is not compiled in; enable its feature on the inference \
                 dependency in Cargo.toml: features = [\"{feature}\"]"
            ),
            Self::RoleUnbound { role } => write!(
                f,
                "no adapter is bound to the role {role}; set INFERENCE_{}_ADAPTER, or add a \
                 [{role}] table with an adapter key to the stack file",
                role.to_uppercase()
            ),
            Self::ProfileDenies {
                role,
                adapter,
                locality,
                profile,
                source: _,
            } => write!(
                f,
                "the role {role} is bound to the adapter \"{adapter}\", which runs at {}, a \
                 locality the profile {} does not admit; to admit it, set profile = \
                 \"hybrid_opt_in\" as the top-level key of the stack file, or \
                 INFERENCE_PROFILE=hybrid_opt_in in the environment, or \
                 Stack::profile(ProfileKind::HybridOptIn) on the stack",
                locality_name(*locality),
                profile_name(*profile)
            ),
            Self::StackFile {
                path, key, message, ..
            } => {
                write!(f, "the stack file {}, key {key}: {message}", path.display())
            }
            Self::Device {
                selector,
                devices,
                message,
            } => {
                write!(f, "the audio device \"{selector}\": {message}; ")?;
                if devices.is_empty() {
                    return f.write_str("the host reports no device");
                }
                f.write_str("the host reports these devices: ")?;
                for (at, name) in devices.iter().enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "\"{name}\"")?;
                }
                Ok(())
            }
            Self::Provider {
                adapter, message, ..
            } => write!(
                f,
                "the provider of the adapter \"{adapter}\" failed: {message}"
            ),
            Self::ConcurrencyUnclaimed { role, adapter } => write!(
                f,
                "the role {role} is bound to the adapter \"{adapter}\", which does not claim \
                 concurrent_input_output, and the run's model node keeps one generation for the \
                 whole run and offers it every input; bind the role {role} to an adapter whose \
                 capabilities include concurrent_input_output"
            ),
            Self::RecipeNameTaken { name } => write!(
                f,
                "the recipe name `{name}` is held by a built-in recipe or an earlier registration \
                 of this process; register the recipe under another name"
            ),
            Self::NoPrices => f.write_str(
                "no price document is named; set the top-level key prices = \"<path to a price \
                 document>\" in the stack file, or call Stack::prices on the stack",
            ),
            Self::Closed { message } => write!(f, "closed: {message}"),
            Self::Layer { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ProfileDenies { source, .. } => Some(source),
            Self::Provider { source, .. }
            | Self::StackFile { source, .. }
            | Self::Layer { source, .. } => source
                .as_deref()
                .map(|source| source as &(dyn std::error::Error + 'static)),
            _ => None,
        }
    }
}

/// Writes the end of a program: nothing for `Ok`, and for `Err(e)` the line `error: {e}` and one
/// `caused by: {source}` line per error of its `source()` chain, each through `Display`.
///
/// Returns [`ExitCode::SUCCESS`] for `Ok` and [`ExitCode::FAILURE`] for `Err`. A failed write is
/// ignored: the exit code is the program's answer either way.
pub fn report(result: Result<()>, out: &mut impl Write) -> ExitCode {
    let Err(error) = result else {
        return ExitCode::SUCCESS;
    };
    let _ = writeln!(out, "error: {error}");
    let mut cause = std::error::Error::source(&error);
    while let Some(source) = cause {
        let _ = writeln!(out, "caused by: {source}");
        cause = source.source();
    }
    let _ = out.flush();
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::fmt;
    use std::process::ExitCode;

    use inference_core::id::NodeId;
    use inference_core::model::{AdapterId, Locality};
    use inference_core::policy::{ProfileId, ProfileKind};
    use inference_pipeline::LocalityDenied;

    use super::{Error, ErrorKind, report};

    #[derive(Debug)]
    struct Beneath;

    impl fmt::Display for Beneath {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("the layer beneath failed")
        }
    }

    impl std::error::Error for Beneath {}

    fn denied() -> LocalityDenied {
        LocalityDenied {
            node_id: NodeId::generate(),
            adapter_id: AdapterId::generate(),
            locality: Locality::ExternalProvider,
            profile_id: ProfileId::generate(),
        }
    }

    fn not_compiled() -> Error {
        Error::AdapterNotCompiled {
            name: "whispercpp".to_owned(),
            feature: "whispercpp".to_owned(),
        }
    }

    fn profile_denies() -> Error {
        Error::ProfileDenies {
            role: "speech".to_owned(),
            adapter: "openai".to_owned(),
            locality: Locality::ExternalProvider,
            profile: ProfileKind::OfflineStrict,
            source: denied(),
        }
    }

    fn device() -> Error {
        Error::Device {
            selector: "USB Mic".to_owned(),
            devices: vec!["Built-in Microphone".to_owned(), "Headset".to_owned()],
            message: "no device of that name".to_owned(),
        }
    }

    fn every_variant() -> Vec<(Error, ErrorKind)> {
        vec![
            (not_compiled(), ErrorKind::AdapterNotCompiled),
            (
                Error::RoleUnbound {
                    role: "stt".to_owned(),
                },
                ErrorKind::RoleUnbound,
            ),
            (profile_denies(), ErrorKind::ProfileDenies),
            (
                Error::StackFile {
                    path: "inference.toml".into(),
                    key: "stt.adapter".to_owned(),
                    message: "expected a string".to_owned(),
                    source: None,
                },
                ErrorKind::StackFile,
            ),
            (device(), ErrorKind::Device),
            (
                Error::Provider {
                    adapter: "openai".to_owned(),
                    message: "the connection was reset".to_owned(),
                    source: None,
                },
                ErrorKind::Provider,
            ),
            (
                Error::ConcurrencyUnclaimed {
                    role: "llm".to_owned(),
                    adapter: "llama-server".to_owned(),
                },
                ErrorKind::ConcurrencyUnclaimed,
            ),
            (
                Error::RecipeNameTaken {
                    name: "chat".to_owned(),
                },
                ErrorKind::RecipeNameTaken,
            ),
            (Error::NoPrices, ErrorKind::NoPrices),
            (
                Error::Closed {
                    message: "the agent was closed before the call".to_owned(),
                },
                ErrorKind::Closed,
            ),
            (
                Error::Layer {
                    message: "the pipeline refused the declaration".to_owned(),
                    source: None,
                },
                ErrorKind::Layer,
            ),
        ]
    }

    #[test]
    fn every_variant_displays_its_snapshot() {
        let displayed: Vec<String> = every_variant()
            .into_iter()
            .map(|(error, _)| error.to_string())
            .collect();
        let snapshot = [
            r#"the adapter "whispercpp" is not compiled in; enable its feature on the inference dependency in Cargo.toml: features = ["whispercpp"]"#,
            "no adapter is bound to the role stt; set INFERENCE_STT_ADAPTER, or add a [stt] table with an adapter key to the stack file",
            r#"the role speech is bound to the adapter "openai", which runs at external_provider, a locality the profile offline_strict does not admit; to admit it, set profile = "hybrid_opt_in" as the top-level key of the stack file, or INFERENCE_PROFILE=hybrid_opt_in in the environment, or Stack::profile(ProfileKind::HybridOptIn) on the stack"#,
            "the stack file inference.toml, key stt.adapter: expected a string",
            r#"the audio device "USB Mic": no device of that name; the host reports these devices: "Built-in Microphone", "Headset""#,
            r#"the provider of the adapter "openai" failed: the connection was reset"#,
            r#"the role llm is bound to the adapter "llama-server", which does not claim concurrent_input_output, and the run's model node keeps one generation for the whole run and offers it every input; bind the role llm to an adapter whose capabilities include concurrent_input_output"#,
            "the recipe name `chat` is held by a built-in recipe or an earlier registration of this process; register the recipe under another name",
            r#"no price document is named; set the top-level key prices = "<path to a price document>" in the stack file, or call Stack::prices on the stack"#,
            "closed: the agent was closed before the call",
            "the pipeline refused the declaration",
        ];
        assert_eq!(displayed, snapshot);
    }

    #[test]
    fn a_device_error_on_a_host_without_devices_says_so() {
        let error = Error::Device {
            selector: "the default input".to_owned(),
            devices: Vec::new(),
            message: "no device of that name".to_owned(),
        };
        assert_eq!(
            error.to_string(),
            r#"the audio device "the default input": no device of that name; the host reports no device"#
        );
    }

    #[test]
    fn eight_variants_name_their_fix() {
        let shown = not_compiled().to_string();
        assert!(shown.contains(r#"features = ["whispercpp"]"#), "{shown}");

        let shown = Error::RoleUnbound {
            role: "llm".to_owned(),
        }
        .to_string();
        assert!(shown.contains("INFERENCE_LLM_ADAPTER"), "{shown}");
        assert!(shown.contains("[llm]"), "{shown}");

        let shown = profile_denies().to_string();
        let fix = shown
            .find(r#"profile = "hybrid_opt_in""#)
            .unwrap_or_else(|| panic!("no fix in {shown}"));
        for named in ["speech", "openai", "external_provider", "offline_strict"] {
            let at = shown
                .find(named)
                .unwrap_or_else(|| panic!("{named} missing from {shown}"));
            assert!(at < fix, "{named} comes after the fix in {shown}");
        }
        assert!(shown.contains("INFERENCE_PROFILE"), "{shown}");
        assert!(shown.contains("Stack::profile"), "{shown}");

        let shown = Error::StackFile {
            path: "config/stack.toml".into(),
            key: "speech.voice".to_owned(),
            message: "expected a string".to_owned(),
            source: None,
        }
        .to_string();
        assert!(shown.contains("config/stack.toml"), "{shown}");
        assert!(shown.contains("speech.voice"), "{shown}");

        let shown = device().to_string();
        for named in ["USB Mic", "Built-in Microphone", "Headset"] {
            assert!(shown.contains(named), "{named} missing from {shown}");
        }

        let shown = Error::ConcurrencyUnclaimed {
            role: "llm".to_owned(),
            adapter: "llama-server".to_owned(),
        }
        .to_string();
        for named in ["llm", "llama-server", "concurrent_input_output"] {
            assert!(shown.contains(named), "{named} missing from {shown}");
        }
        assert!(shown.contains("bind the role llm to an adapter"), "{shown}");

        let shown = Error::RecipeNameTaken {
            name: "ask".to_owned(),
        }
        .to_string();
        assert!(shown.contains("`ask`"), "{shown}");
        assert!(
            shown.contains("register the recipe under another name"),
            "{shown}"
        );

        let shown = Error::NoPrices.to_string();
        assert!(shown.contains("prices"), "{shown}");
    }

    #[test]
    fn every_variant_has_the_kind_of_its_name() {
        let pairs = every_variant();
        assert_eq!(pairs.len(), 11);
        for (error, kind) in pairs {
            assert_eq!(error.kind(), kind, "{error:?}");
            let variant = format!("{error:?}");
            let name = format!("{kind:?}");
            assert!(variant.starts_with(&name), "{variant} is not {name}");
        }
    }

    #[test]
    fn a_layer_error_returns_its_source() {
        let with = Error::Layer {
            message: "the runtime refused the run".to_owned(),
            source: Some(Box::new(Beneath)),
        };
        let source = with.source().expect("a Layer with a source returns it");
        assert!(source.is::<Beneath>());
        assert_eq!(source.to_string(), "the layer beneath failed");

        let without = Error::Layer {
            message: "the runtime refused the run".to_owned(),
            source: None,
        };
        assert!(without.source().is_none());
    }

    #[test]
    fn a_profile_denial_returns_the_planning_refusal() {
        let error = profile_denies();
        let source = error.source().expect("ProfileDenies has a source");
        assert!(source.is::<LocalityDenied>());
    }

    #[test]
    fn report_writes_the_error_with_its_fix_and_fails() {
        let mut out = Vec::new();
        let code = report(Err(not_compiled()), &mut out);
        let written = String::from_utf8(out).expect("utf-8");
        assert!(written.starts_with("error: "), "{written}");
        assert!(
            written.contains(r#"features = ["whispercpp"]"#),
            "{written}"
        );
        assert_eq!(code, ExitCode::FAILURE);
    }

    #[test]
    fn report_writes_each_cause_on_its_own_line() {
        let mut out = Vec::new();
        let error = Error::Layer {
            message: "the runtime refused the run".to_owned(),
            source: Some(Box::new(Beneath)),
        };
        let code = report(Err(error), &mut out);
        assert_eq!(
            String::from_utf8(out).expect("utf-8"),
            "error: the runtime refused the run\ncaused by: the layer beneath failed\n"
        );
        assert_eq!(code, ExitCode::FAILURE);
    }

    #[test]
    fn report_of_success_writes_nothing() {
        let mut out = Vec::new();
        let code = report(Ok(()), &mut out);
        assert!(out.is_empty());
        assert_eq!(code, ExitCode::SUCCESS);
    }
}
