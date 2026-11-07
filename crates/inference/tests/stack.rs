//! story:sdk-stack: a stack binds each role to one adapter from the builder, the environment or a
//! stack file, first found wins, under one deployment profile (`docs/design/sdk.md` §4).
//!
//! Every resolution builds its own [`Env`] (variables, a temporary working directory and config
//! home) and never reads or sets the process environment, so the cases run in parallel. The
//! adapter registry is process-wide and append-only: each case registers under names no other
//! case uses. Each case runs on a thread of its own and must finish within 20 s.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use inference::core::model::{
    Adapter, AdapterDescriptor as ModelDescriptor, AdapterId, DeclareAdapter, ExecutionForm,
    Locality, ModelId,
};
use inference::core::policy::ProfileKind;
use inference::llm::Generator;
use inference::llm::llm_core::{
    BoxFuture, Cancel, Capabilities, Id, Item, Model, Protocol, Provenance, StopReason, StreamSink,
    TurnObservation, TurnOutcome, TurnRequest,
};
use inference::pipeline::Catalog;
use inference::text::{AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued};
use inference::tts::{
    ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt, PlaybackRefused,
    SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};
use inference::{
    AdapterDescriptor, AdapterNotRegistered, Binding, BindingSource, Bound, Env, ErrorKind,
    Registry, Role, RoleBoundTwice, Stack, role,
};

fn within<T: Send + 'static>(case: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(case());
    });
    finished
        .recv_timeout(Duration::from_secs(20))
        .expect("the case finishes within 20 s without panicking")
}

fn declared(backend: &str, locality: Locality) -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: backend.to_owned(),
        contract_revision: "1".to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality,
        capabilities: Vec::new(),
        max_concurrency: 1,
        descriptor: ModelDescriptor::default(),
    })
    .expect("a concurrency of one is positive")
    .0
}

/// A text model of the test's own: every turn answers `own`.
struct OwnModel {
    provenance: Provenance,
    capabilities: Capabilities,
}

impl OwnModel {
    fn new() -> Self {
        let id = |value: &str| Id::new(value).expect("a printable identifier");
        Self {
            provenance: Provenance {
                protocol: Protocol::Responses,
                provider: id("stack-test"),
                account: id("own"),
                endpoint: id("in-process"),
                model: id("own"),
                binding_revision: id("1"),
            },
            capabilities: Capabilities::text(1 << 12, 1 << 10),
        }
    }
}

impl Model for OwnModel {
    fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn turn<'a>(
        &'a self,
        _request: &'a TurnRequest,
        _sink: &'a mut dyn StreamSink,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, inference::llm::llm_core::Error>> {
        Box::pin(async move {
            Ok(TurnOutcome {
                stop_reason: StopReason::EndTurn,
                items: vec![Item::assistant("own".to_owned())],
                observation: TurnObservation::new(self.provenance.clone()),
            })
        })
    }
}

/// A generator of the test's own over [`OwnModel`], its adapter running in the same process.
fn own_generator() -> Generator {
    Generator::new(
        declared("stack-test own generator", Locality::SameProcess),
        Arc::new(OwnModel::new()),
    )
    .expect("the adapter claims nothing the model does not declare")
}

/// A synthesiser of the test's own, declared with an adapter the test chooses.
struct OwnSynthesiser {
    adapter: Adapter,
}

/// Its synthesis: the contract's record, which every operation forwards to; it yields no event.
struct OwnSynthesis {
    output: SynthesisOutput,
}

impl Iterator for OwnSynthesis {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        None
    }
}

impl SynthesisHandle for OwnSynthesis {
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

impl Synthesiser for OwnSynthesiser {
    type Handle = OwnSynthesis;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(&mut self) -> Result<OwnSynthesis, ConcurrencyLimitReached> {
        Ok(OwnSynthesis {
            output: SynthesisOutput::start(&self.adapter),
        })
    }
}

fn own_synthesiser() -> OwnSynthesiser {
    OwnSynthesiser {
        adapter: declared("stack-test own synthesiser", Locality::SameProcess),
    }
}

/// Registers `name` serving `llm` with [`own_generator`].
fn register_llm(name: &str) {
    Registry::register(
        AdapterDescriptor::new(name, Locality::SameProcess)
            .role(Role::Llm, |_| Ok(Binding::llm(own_generator()).into())),
    )
    .unwrap_or_else(|error| panic!("{name} registers: {error}"));
}

/// Registers `name` serving `llm`, recording the keys each construction receives.
fn register_recording(name: &str) -> Arc<Mutex<Vec<BTreeMap<String, String>>>> {
    let received = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&received);
    Registry::register(AdapterDescriptor::new(name, Locality::SameProcess).role(
        Role::Llm,
        move |keys| {
            seen.lock().expect("unpoisoned").push(keys.clone());
            Ok(Binding::llm(own_generator()).into())
        },
    ))
    .unwrap_or_else(|error| panic!("{name} registers: {error}"));
    received
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the directory is created");
    }
    std::fs::write(path, text).expect("the file is written");
}

/// Three empty directories: one for a stack file, a working directory and a config home.
struct Dirs {
    _root: tempfile::TempDir,
    stack_file: std::path::PathBuf,
    working: std::path::PathBuf,
    config: std::path::PathBuf,
}

impl Dirs {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("a temporary directory");
        let working = root.path().join("work");
        let config = root.path().join("config");
        std::fs::create_dir_all(&working).expect("the working directory");
        std::fs::create_dir_all(&config).expect("the config home");
        Self {
            stack_file: root.path().join("stack.toml"),
            working,
            config,
            _root: root,
        }
    }

    fn env(&self) -> Env {
        Env::new()
            .working_dir(&self.working)
            .config_home(&self.config)
    }

    fn working_file(&self) -> std::path::PathBuf {
        self.working.join("inference.toml")
    }

    fn user_file(&self) -> std::path::PathBuf {
        self.config.join("inference").join("inference.toml")
    }

    fn stack_path(&self) -> String {
        self.stack_file.to_str().expect("a UTF-8 path").to_owned()
    }
}

fn keys(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn roles_of(role: Role) -> &'static str {
    match role {
        Role::Stt => "stt",
        Role::Llm => "llm",
        Role::Tts => "tts",
        Role::Speech => "speech",
    }
}

#[test]
fn role_has_the_four_variants_of_the_specification() {
    within(|| {
        let names: Vec<&str> = [Role::Stt, Role::Llm, Role::Tts, Role::Speech]
            .into_iter()
            .map(roles_of)
            .collect();
        assert_eq!(names, ["stt", "llm", "tts", "speech"]);
        assert_eq!(Role::Llm.to_string(), "llm");
    });
}

#[test]
fn an_empty_stack_binds_nothing_and_resolves_with_every_role_unbound() {
    within(|| {
        assert!(Stack::new().adapter(Role::Llm).is_none());
        let stack = Stack::new()
            .resolve_with(&Env::new())
            .expect("an empty stack resolves");
        for role in [Role::Stt, Role::Llm, Role::Tts, Role::Speech] {
            assert!(stack.adapter(role).is_none(), "{role} is bound");
        }
        assert!(stack.adapter_ids().is_empty());
        assert!(stack.session_config().is_none());
    });
}

#[test]
fn a_stack_moves_between_threads() {
    fn send<T: Send + 'static>() {}
    send::<Stack>();
    send::<Binding>();
    send::<Env>();
}

#[test]
fn the_default_profile_is_offline_strict_admitting_local_compute() {
    within(|| {
        let stack = Stack::new()
            .resolve_with(&Env::new())
            .expect("an empty stack resolves");
        let profile = Catalog::profile(&stack, stack.profile_id()).expect("the stack's profile");
        assert_eq!(profile.kind(), ProfileKind::OfflineStrict);
        assert_eq!(
            profile.admitted(),
            [Locality::SameProcess, Locality::LocalWorker]
        );
        assert_eq!(stack.profile_source(), None);
    });
}

fn hybrid(stack: &Stack) {
    let profile = Catalog::profile(stack, stack.profile_id()).expect("the stack's profile");
    assert_eq!(profile.kind(), ProfileKind::HybridOptIn);
    assert_eq!(profile.admitted(), Locality::ALL);
}

#[test]
fn the_builder_profile_wins_over_the_environment() {
    within(|| {
        let stack = Stack::new()
            .profile(ProfileKind::HybridOptIn)
            .resolve_with(&Env::new().var("INFERENCE_PROFILE", "offline_strict"))
            .expect("resolves");
        hybrid(&stack);
        assert_eq!(stack.profile_source(), Some(BindingSource::Builder));
    });
}

#[test]
fn the_environment_profile_wins_over_the_working_directory_file() {
    within(|| {
        let dirs = Dirs::new();
        write(&dirs.working_file(), "profile = \"offline_strict\"\n");
        let stack = Stack::new()
            .resolve_with(&dirs.env().var("INFERENCE_PROFILE", "hybrid_opt_in"))
            .expect("resolves");
        hybrid(&stack);
        assert_eq!(stack.profile_source(), Some(BindingSource::Environment));
    });
}

#[test]
fn the_profile_of_the_named_stack_file() {
    within(|| {
        let dirs = Dirs::new();
        write(&dirs.stack_file, "profile = \"hybrid_opt_in\"\n");
        write(&dirs.working_file(), "profile = \"offline_strict\"\n");
        let stack = Stack::new()
            .resolve_with(&dirs.env().var("INFERENCE_STACK", dirs.stack_path()))
            .expect("resolves");
        hybrid(&stack);
        assert_eq!(stack.profile_source(), Some(BindingSource::StackFile));
    });
}

#[test]
fn the_profile_of_the_working_directory_file() {
    within(|| {
        let dirs = Dirs::new();
        write(&dirs.working_file(), "profile = \"hybrid_opt_in\"\n");
        write(&dirs.user_file(), "profile = \"offline_strict\"\n");
        let stack = Stack::new().resolve_with(&dirs.env()).expect("resolves");
        hybrid(&stack);
        assert_eq!(
            stack.profile_source(),
            Some(BindingSource::WorkingDirectoryFile)
        );
    });
}

#[test]
fn the_profile_of_the_user_config_file() {
    within(|| {
        let dirs = Dirs::new();
        write(&dirs.user_file(), "profile = \"hybrid_opt_in\"\n");
        let stack = Stack::new().resolve_with(&dirs.env()).expect("resolves");
        hybrid(&stack);
        assert_eq!(stack.profile_source(), Some(BindingSource::UserConfigFile));
    });
}

#[test]
fn each_source_of_a_role_gives_way_to_the_one_above_it() {
    within(|| {
        let names = [
            "stack-order-builder",
            "stack-order-environment",
            "stack-order-stack-file",
            "stack-order-working",
            "stack-order-user",
        ];
        for name in names {
            register_llm(name);
        }
        let dirs = Dirs::new();
        let table = |name: &str| format!("[llm]\nadapter = \"{name}\"\n");
        write(&dirs.stack_file, &table(names[2]));
        write(&dirs.working_file(), &table(names[3]));
        write(&dirs.user_file(), &table(names[4]));
        let full = dirs
            .env()
            .var("INFERENCE_LLM_ADAPTER", names[1])
            .var("INFERENCE_STACK", dirs.stack_path());

        let expect = |stack: Stack, name: &str, source: BindingSource| {
            let binding = stack.adapter(Role::Llm).expect("llm is bound");
            assert_eq!(binding.name(), Some(name));
            assert_eq!(binding.source(), source);
        };

        let builder = Stack::new().bind(Binding::named(Role::Llm, names[0], BTreeMap::new()));
        expect(
            builder.resolve_with(&full).expect("resolves"),
            names[0],
            BindingSource::Builder,
        );
        expect(
            Stack::new().resolve_with(&full).expect("resolves"),
            names[1],
            BindingSource::Environment,
        );
        let without_variable = dirs.env().var("INFERENCE_STACK", dirs.stack_path());
        expect(
            Stack::new()
                .resolve_with(&without_variable)
                .expect("resolves"),
            names[2],
            BindingSource::StackFile,
        );
        expect(
            Stack::new().resolve_with(&dirs.env()).expect("resolves"),
            names[3],
            BindingSource::WorkingDirectoryFile,
        );
        std::fs::remove_file(dirs.working_file()).expect("the working file is removed");
        expect(
            Stack::new().resolve_with(&dirs.env()).expect("resolves"),
            names[4],
            BindingSource::UserConfigFile,
        );
    });
}

#[test]
fn role_keys_reach_the_constructor_after_expansion() {
    within(|| {
        let received = register_recording("stack-keys");
        let env = Env::new()
            .var("INFERENCE_LLM_ADAPTER", "stack-keys")
            .var("INFERENCE_LLM_URL", "http://${HOST}:8080")
            .var("HOST", "localhost");
        Stack::new().resolve_with(&env).expect("resolves");
        assert_eq!(
            received.lock().expect("unpoisoned")[0]
                .get("url")
                .map(String::as_str),
            Some("http://localhost:8080")
        );

        let dirs = Dirs::new();
        write(
            &dirs.working_file(),
            "[llm]\nadapter = \"stack-keys\"\nurl = \"${HOST}/v1\"\n",
        );
        let stack = Stack::new()
            .resolve_with(&dirs.env().var("HOST", "localhost"))
            .expect("resolves");
        assert_eq!(
            stack.adapter(Role::Llm).map(|binding| binding.source()),
            Some(BindingSource::WorkingDirectoryFile)
        );
        assert_eq!(
            received.lock().expect("unpoisoned")[1]
                .get("url")
                .map(String::as_str),
            Some("localhost/v1")
        );
    });
}

#[test]
fn a_variable_a_key_names_and_the_env_lacks_is_refused_naming_it() {
    within(|| {
        register_llm("stack-unset");
        let env = Env::new()
            .var("INFERENCE_LLM_ADAPTER", "stack-unset")
            .var("INFERENCE_LLM_URL", "http://${NOT_SET_ANYWHERE}:8080");
        let error = Stack::new()
            .resolve_with(&env)
            .expect_err("an unset variable is refused");
        assert!(error.to_string().contains("NOT_SET_ANYWHERE"), "{error}");
    });
}

#[test]
fn a_stack_file_missing_not_toml_or_with_a_wrong_value_is_refused_naming_it() {
    within(|| {
        let dirs = Dirs::new();
        let missing = dirs.env().var("INFERENCE_STACK", dirs.stack_path());
        let error = Stack::new()
            .resolve_with(&missing)
            .expect_err("a named file that is missing is refused");
        assert_eq!(error.kind(), ErrorKind::StackFile);
        assert!(error.to_string().contains("stack.toml"), "{error}");

        write(&dirs.working_file(), "[llm\nadapter = ");
        let error = Stack::new()
            .resolve_with(&dirs.env())
            .expect_err("a file that is not TOML is refused");
        assert_eq!(error.kind(), ErrorKind::StackFile);
        assert!(error.to_string().contains("inference.toml"), "{error}");

        write(&dirs.working_file(), "profile = \"everywhere\"\n");
        let error = Stack::new()
            .resolve_with(&dirs.env())
            .expect_err("an unknown profile is refused");
        assert_eq!(error.kind(), ErrorKind::StackFile);
        assert!(error.to_string().contains("key profile"), "{error}");

        write(
            &dirs.working_file(),
            "[llm]\nurl = \"http://127.0.0.1:1\"\n",
        );
        let error = Stack::new()
            .resolve_with(&dirs.env())
            .expect_err("a role table without an adapter is refused");
        assert!(error.to_string().contains("llm.adapter"), "{error}");
    });
}

#[test]
fn a_named_builder_binding_passes_its_keys() {
    within(|| {
        let received = register_recording("stack-keys-builder");
        let given = keys(&[("url", "http://127.0.0.1:9")]);
        let stack = Stack::new()
            .bind(Binding::named(
                Role::Llm,
                "stack-keys-builder",
                given.clone(),
            ))
            .resolve_with(&Env::new())
            .expect("resolves");
        let binding = stack.adapter(Role::Llm).expect("llm is bound");
        assert_eq!(binding.source(), BindingSource::Builder);
        assert_eq!(received.lock().expect("unpoisoned")[0], given);
    });
}

#[test]
fn a_typed_value_of_the_applications_own_is_bound_by_the_builder() {
    within(|| {
        let synthesiser = own_synthesiser();
        let adapter_id: AdapterId = synthesiser.adapter.adapter_id();
        let bound: Bound<role::Tts> = Binding::tts(synthesiser);
        let named: Binding = Binding::named(Role::Llm, "stack-typed-unused", BTreeMap::new());
        assert_eq!(named.role(), Role::Llm);
        let stack = Stack::new()
            .bind(bound)
            .resolve_with(&Env::new())
            .expect("resolves");
        let binding = stack.adapter(Role::Tts).expect("tts is bound");
        assert_eq!(binding.adapter().adapter_id(), adapter_id);
        assert_eq!(binding.source(), BindingSource::Builder);
        assert_eq!(binding.name(), None);
        assert_eq!(stack.model(Role::Tts), None);
    });
}

#[test]
fn model_names_come_from_the_key_or_the_binding() {
    within(|| {
        register_llm("stack-model");
        let stack = Stack::new()
            .resolve_with(
                &Env::new()
                    .var("INFERENCE_LLM_ADAPTER", "stack-model")
                    .var("INFERENCE_LLM_MODEL", "qwen3-8b"),
            )
            .expect("resolves");
        assert_eq!(stack.model(Role::Llm), Some("qwen3-8b"));

        let dirs = Dirs::new();
        write(
            &dirs.working_file(),
            "[llm]\nadapter = \"stack-model\"\nmodel = \"qwen3-8b\"\n",
        );
        let stack = Stack::new().resolve_with(&dirs.env()).expect("resolves");
        assert_eq!(stack.model(Role::Llm), Some("qwen3-8b"));

        let stack = Stack::new()
            .bind(Binding::tts(own_synthesiser()).model("voice-1"))
            .resolve_with(&Env::new())
            .expect("resolves");
        assert_eq!(stack.model(Role::Tts), Some("voice-1"));
    });
}

#[test]
fn a_role_the_builder_binds_twice_is_refused_naming_it() {
    within(|| {
        let error = Stack::new()
            .bind(Binding::tts(own_synthesiser()))
            .bind(Binding::tts(own_synthesiser()))
            .resolve_with(&Env::new())
            .expect_err("a role bound twice is refused");
        assert_eq!(error.kind(), ErrorKind::Layer);
        assert!(error.to_string().contains("tts"), "{error}");
        let source = std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<RoleBoundTwice>());
        assert_eq!(source, Some(&RoleBoundTwice { role: Role::Tts }));
    });
}

#[test]
fn an_adapter_no_registration_holds_is_refused_naming_it() {
    within(|| {
        let error = Stack::new()
            .resolve_with(&Env::new().var("INFERENCE_LLM_ADAPTER", "stack-nowhere"))
            .expect_err("an unregistered name is refused");
        assert_eq!(error.kind(), ErrorKind::Layer);
        assert!(error.to_string().contains("stack-nowhere"), "{error}");
        assert!(
            error.to_string().contains("INFERENCE_LLM_ADAPTER"),
            "{error}"
        );
        let source = std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<AdapterNotRegistered>());
        assert_eq!(
            source.map(|refusal| refusal.adapter.as_str()),
            Some("stack-nowhere")
        );

        let dirs = Dirs::new();
        write(&dirs.working_file(), "[llm]\nadapter = \"stack-nowhere\"\n");
        let error = Stack::new()
            .resolve_with(&dirs.env())
            .expect_err("an unregistered name is refused");
        assert_eq!(error.kind(), ErrorKind::StackFile);
        assert!(error.to_string().contains("llm.adapter"), "{error}");
        assert!(error.to_string().contains("stack-nowhere"), "{error}");
    });
}

#[test]
fn an_application_adapter_registers_once_and_built_in_names_are_reserved() {
    within(|| {
        let made: Arc<Mutex<Option<Adapter>>> = Arc::new(Mutex::new(None));
        let kept = Arc::clone(&made);
        Registry::register(
            AdapterDescriptor::new("stack-app", Locality::SameProcess).role(Role::Llm, move |_| {
                let generator = own_generator();
                *kept.lock().expect("unpoisoned") = Some(generator.adapter().clone());
                Ok(Binding::llm(generator).into())
            }),
        )
        .expect("a new name registers");
        let env = Env::new().var("INFERENCE_LLM_ADAPTER", "stack-app");
        let stack = Stack::new().resolve_with(&env).expect("resolves");
        let binding = stack.adapter(Role::Llm).expect("llm is bound");
        assert_eq!(binding.name(), Some("stack-app"));
        assert_eq!(
            Some(binding.adapter()),
            made.lock().expect("unpoisoned").as_ref()
        );

        let again = Registry::register(
            AdapterDescriptor::new("stack-app", Locality::SameProcess)
                .role(Role::Tts, |_| Ok(Binding::tts(own_synthesiser()).into())),
        )
        .expect_err("a held name is refused");
        assert_eq!(again.kind(), ErrorKind::Layer);
        assert!(again.to_string().contains("stack-app"), "{again}");

        let built_in = Registry::register(
            AdapterDescriptor::new("mock", Locality::SameProcess)
                .role(Role::Llm, |_| Ok(Binding::llm(own_generator()).into())),
        )
        .expect_err("a built-in name is refused");
        assert_eq!(built_in.kind(), ErrorKind::Layer);
        assert!(built_in.to_string().contains("mock"), "{built_in}");

        let stack = Stack::new().resolve_with(&env).expect("resolves");
        assert_eq!(
            stack.adapter(Role::Llm).and_then(|binding| binding.name()),
            Some("stack-app")
        );
        assert!(stack.adapter(Role::Tts).is_none());
    });
}

#[test]
fn a_constructor_returning_another_role_is_refused() {
    within(|| {
        Registry::register(
            AdapterDescriptor::new("stack-wrong-role", Locality::SameProcess)
                .role(Role::Llm, |_| Ok(Binding::tts(own_synthesiser()).into())),
        )
        .expect("registers");
        let error = Stack::new()
            .resolve_with(&Env::new().var("INFERENCE_LLM_ADAPTER", "stack-wrong-role"))
            .expect_err("a binding of another role is refused");
        assert_eq!(error.kind(), ErrorKind::Layer);
        let shown = error.to_string();
        assert!(shown.contains("stack-wrong-role"), "{shown}");
        assert!(shown.contains("llm"), "{shown}");
    });
}

#[test]
fn a_constructor_returning_a_named_binding_is_refused() {
    within(|| {
        Registry::register(
            AdapterDescriptor::new("stack-renamed", Locality::SameProcess).role(Role::Llm, |_| {
                Ok(Binding::named(
                    Role::Llm,
                    "stack-wrong-role",
                    BTreeMap::new(),
                ))
            }),
        )
        .expect("registers");
        let error = Stack::new()
            .resolve_with(&Env::new().var("INFERENCE_LLM_ADAPTER", "stack-renamed"))
            .expect_err("a named binding is refused");
        assert_eq!(error.kind(), ErrorKind::Layer);
        assert!(error.to_string().contains("stack-renamed"), "{error}");
    });
}

#[test]
fn the_adapter_the_stack_binds_is_in_its_catalog() {
    within(|| {
        register_llm("stack-catalog");
        let stack = Stack::new()
            .bind(Binding::tts(own_synthesiser()))
            .resolve_with(&Env::new().var("INFERENCE_LLM_ADAPTER", "stack-catalog"))
            .expect("resolves");
        let ids = stack.adapter_ids();
        assert_eq!(
            ids.iter().map(|(role, _)| *role).collect::<Vec<_>>(),
            [Role::Llm, Role::Tts]
        );
        for (role, id) in &ids {
            let bound = stack.adapter(*role).expect("bound").adapter();
            assert_eq!(bound.adapter_id(), *id);
            assert_eq!(Catalog::adapter(&stack, *id), Some(bound));
        }
        assert!(Catalog::adapter(&stack, AdapterId::generate()).is_none());
    });
}

#[cfg(not(feature = "whispercpp"))]
#[test]
fn a_built_in_adapter_whose_feature_is_off_names_the_feature() {
    within(|| {
        let dirs = Dirs::new();
        write(&dirs.working_file(), "[stt]\nadapter = \"whispercpp\"\n");
        let error = Stack::new()
            .resolve_with(&dirs.env())
            .expect_err("whispercpp is not compiled in");
        assert_eq!(error.kind(), ErrorKind::AdapterNotCompiled);
        assert!(
            error.to_string().contains(r#"features = ["whispercpp"]"#),
            "{error}"
        );
    });
}

#[cfg(feature = "testing")]
mod testing {
    use inference::core::id::{NodeId, RunId};
    use inference::duplex::SessionConfig;
    use inference::llm::llm_core::TurnRequest;
    use inference::llm::{GenerationEvent, GenerationRequest};
    use inference_testkit::duplex::mock::MockSpeechModel;
    use inference_testkit::llm::{MockConfiguration, MockGenerator};

    use super::*;

    fn mock_generator(script: &[&str]) -> MockGenerator {
        MockGenerator::new(
            MockConfiguration::StreamingOnly,
            ModelId::generate(),
            script.iter().copied(),
        )
    }

    fn first_reply(generator: &Generator) -> String {
        let mut generator = generator.clone();
        let mut handle = generator
            .start(GenerationRequest {
                run_id: RunId::generate(),
                node_id: NodeId::generate(),
                epoch: 1,
                turn: TurnRequest::new("mock", vec![Item::user("Greet the user.")]),
            })
            .expect("a fresh generator runs nothing else");
        handle
            .by_ref()
            .find_map(|event| match event {
                GenerationEvent::TextDelta(delta) => Some(delta.text),
                _ => None,
            })
            .expect("a reply")
    }

    #[test]
    fn the_mock_named_in_the_environment_is_an_environment_binding() {
        within(|| {
            let stack = Stack::new()
                .resolve_with(&Env::new().var("INFERENCE_LLM_ADAPTER", "mock"))
                .expect("resolves");
            let binding = stack.adapter(Role::Llm).expect("llm is bound");
            assert_eq!(binding.source(), BindingSource::Environment);
            assert_eq!(binding.name(), Some("mock"));
        });
    }

    #[test]
    fn a_role_the_builder_binds_keeps_it_and_the_others_fill_from_the_environment() {
        within(|| {
            register_llm("stack-keep");
            let env = Env::new()
                .var("INFERENCE_LLM_ADAPTER", "mock")
                .var("INFERENCE_STT_ADAPTER", "mock");
            let stack = Stack::new()
                .bind(Binding::named(Role::Llm, "stack-keep", BTreeMap::new()))
                .resolve_with(&env)
                .expect("resolves");
            let llm = stack.adapter(Role::Llm).expect("llm is bound");
            assert_eq!(llm.name(), Some("stack-keep"));
            assert_eq!(llm.source(), BindingSource::Builder);
            let stt = stack.adapter(Role::Stt).expect("stt is bound");
            assert_eq!(stt.source(), BindingSource::Environment);
        });
    }

    #[test]
    fn a_mock_bound_by_value_runs_its_own_script() {
        within(|| {
            let stack = Stack::new()
                .bind(Binding::llm(mock_generator(&["Ahoy", " there"])))
                .resolve_with(&Env::new())
                .expect("resolves");
            let generator = stack
                .adapter(Role::Llm)
                .and_then(|binding| binding.generator())
                .expect("llm is a generator");
            assert_eq!(first_reply(generator), "Ahoy");

            let by_name = Stack::new()
                .resolve_with(&Env::new().var("INFERENCE_LLM_ADAPTER", "mock"))
                .expect("resolves");
            let generator = by_name
                .adapter(Role::Llm)
                .and_then(|binding| binding.generator())
                .expect("llm is a generator");
            assert_ne!(first_reply(generator), "Ahoy");
        });
    }

    #[test]
    fn typed_and_named_mock_bindings_both_resolve() {
        within(|| {
            let stack = Stack::new()
                .bind(Binding::llm(mock_generator(&["one"])).model("m"))
                .resolve_with(&Env::new())
                .expect("resolves");
            assert_eq!(stack.model(Role::Llm), Some("m"));
            let stack = Stack::new()
                .bind(Binding::named(Role::Llm, "mock", keys(&[("model", "m")])))
                .resolve_with(&Env::new())
                .expect("resolves");
            assert_eq!(stack.model(Role::Llm), Some("m"));
        });
    }

    #[test]
    fn a_stack_file_naming_the_mock_for_every_role_resolves() {
        within(|| {
            let dirs = Dirs::new();
            write(
                &dirs.working_file(),
                "[stt]\nadapter = \"mock\"\n[llm]\nadapter = \"mock\"\n[tts]\nadapter = \"mock\"\n[speech]\nadapter = \"mock\"\n",
            );
            let stack = Stack::new().resolve_with(&dirs.env()).expect("resolves");
            for role in [Role::Stt, Role::Llm, Role::Tts, Role::Speech] {
                let binding = stack.adapter(role).expect("bound");
                assert_eq!(binding.name(), Some("mock"));
                assert_eq!(binding.source(), BindingSource::WorkingDirectoryFile);
                assert_eq!(binding.role(), role);
            }
            assert_eq!(
                stack.session_config(),
                Some(&inference::default_session_config())
            );
        });
    }

    #[test]
    fn the_stack_is_the_catalog_a_recipe_declares_into() {
        within(|| {
            let dirs = Dirs::new();
            write(
                &dirs.working_file(),
                "[stt]\nadapter = \"mock\"\n[llm]\nadapter = \"mock\"\n[tts]\nadapter = \"mock\"\n",
            );
            let stack = Stack::new().resolve_with(&dirs.env()).expect("resolves");
            let ids = stack.adapter_ids();
            assert_eq!(
                ids.iter().map(|(role, _)| *role).collect::<Vec<_>>(),
                [Role::Stt, Role::Llm, Role::Tts]
            );
            for (role, id) in &ids {
                assert_eq!(
                    stack
                        .adapter(*role)
                        .map(|binding| binding.adapter().adapter_id()),
                    Some(*id)
                );
                assert!(Catalog::adapter(&stack, *id).is_some());
            }
            assert!(Catalog::profile(&stack, stack.profile_id()).is_some());
            assert!(Catalog::adapter(&stack, AdapterId::generate()).is_none());
        });
    }

    fn voiced(config: &SessionConfig) -> SessionConfig {
        SessionConfig {
            voice: "cedar".to_owned(),
            ..config.clone()
        }
    }

    #[test]
    fn the_voice_key_overrides_the_voice_only() {
        within(|| {
            let dirs = Dirs::new();
            write(
                &dirs.working_file(),
                "[speech]\nadapter = \"mock\"\nvoice = \"cedar\"\n",
            );
            let stack = Stack::new().resolve_with(&dirs.env()).expect("resolves");
            assert_eq!(
                stack.session_config(),
                Some(&voiced(&inference::default_session_config()))
            );

            let stack = Stack::new()
                .resolve_with(
                    &Env::new()
                        .var("INFERENCE_SPEECH_ADAPTER", "mock")
                        .var("INFERENCE_SPEECH_VOICE", "cedar"),
                )
                .expect("resolves");
            assert_eq!(
                stack.session_config(),
                Some(&voiced(&inference::default_session_config()))
            );

            let stack = Stack::new()
                .bind(Binding::speech(MockSpeechModel::native(vec![])))
                .resolve_with(&Env::new())
                .expect("resolves");
            assert_eq!(
                stack.session_config(),
                Some(&inference::default_session_config())
            );
        });
    }
}

#[cfg(feature = "openai")]
mod openai {
    use super::*;

    #[test]
    fn openai_speech_has_its_default_session_and_model_and_reads_no_credential() {
        within(|| {
            let dirs = Dirs::new();
            write(
                &dirs.working_file(),
                "profile = \"hybrid_opt_in\"\n[speech]\nadapter = \"openai\"\n",
            );
            let stack = Stack::new().resolve_with(&dirs.env()).expect("resolves");
            assert_eq!(stack.model(Role::Speech), Some("gpt-realtime-2.1"));
            let config = stack.session_config().expect("a session config").clone();
            assert_eq!(config, inference_openai::duplex::default_session_config());
            assert_eq!(config.voice, "marin");
            assert_eq!(config.input_format.sample_rate.get(), 24_000);
            assert_eq!(config.output_format.sample_rate.get(), 24_000);
            assert_eq!(
                config.turn_detection,
                inference::duplex::TurnDetection::ServerVad
            );
            assert!(!config.auto_interrupt);
            assert!(config.auto_response);
            assert_eq!(config.idle_limit, Duration::from_secs(30));
            assert!(config.instructions.is_empty() && config.tools.is_empty());
            inference_openai::duplex::capabilities()
                .admit(&config)
                .expect("the provider admits its default");
            assert_eq!(
                stack
                    .adapter(Role::Speech)
                    .map(|binding| binding.adapter().locality()),
                Some(Locality::ExternalProvider)
            );
        });
    }

    #[cfg(feature = "openai-live")]
    #[test]
    fn gpt_live_speech_has_its_default_session_and_model() {
        within(|| {
            let dirs = Dirs::new();
            write(
                &dirs.working_file(),
                "[speech]\nadapter = \"openai-live\"\n",
            );
            let stack = Stack::new().resolve_with(&dirs.env()).expect("resolves");
            assert_eq!(stack.model(Role::Speech), Some("gpt-live-1"));
            let config = stack.session_config().expect("a session config").clone();
            assert_eq!(
                config,
                inference_openai::live_duplex::default_session_config()
            );
            assert_eq!(config.voice, "marin");
            assert_eq!(config.input_format.sample_rate.get(), 24_000);
            assert_eq!(config.output_format.sample_rate.get(), 24_000);
            assert_eq!(
                config.turn_detection,
                inference::duplex::TurnDetection::SemanticVad
            );
            assert!(config.auto_interrupt);
            assert!(config.auto_response);
            assert_eq!(config.idle_limit, Duration::from_secs(30));
            assert!(config.instructions.is_empty() && config.tools.is_empty());
            inference_openai::live_duplex::capabilities()
                .admit(&config)
                .expect("the provider admits its default");
        });
    }
}

#[cfg(feature = "aws")]
mod aws {
    use super::*;

    #[test]
    fn nova_sonic_resolves_with_no_aws_configuration_and_its_defaults() {
        within(|| {
            let dirs = Dirs::new();
            write(&dirs.stack_file, "[speech]\nadapter = \"aws\"\n");
            let env = dirs.env().var("INFERENCE_STACK", dirs.stack_path());
            let stack = Stack::new().resolve_with(&env).expect("resolves");
            let binding = stack.adapter(Role::Speech).expect("speech is bound");
            assert_eq!(binding.source(), BindingSource::StackFile);
            assert_eq!(stack.model(Role::Speech), Some("amazon.nova-2-sonic-v1:0"));
            let config = stack.session_config().expect("a session config").clone();
            assert_eq!(config, inference_aws::duplex::default_session_config());
            assert_eq!(config.voice, "matthew");
            assert_eq!(config.input_format.sample_rate.get(), 16_000);
            assert_eq!(config.output_format.sample_rate.get(), 24_000);
            assert_eq!(
                config.turn_detection,
                inference::duplex::TurnDetection::ServerVad
            );
            assert!(config.auto_interrupt);
            assert!(config.auto_response);
            assert_eq!(config.idle_limit, Duration::from_secs(30));
            assert!(config.instructions.is_empty() && config.tools.is_empty());
            inference_aws::duplex::capabilities(inference_aws::nova_sonic::DEFAULT_MODEL_ID)
                .admit(&config)
                .expect("the provider admits its default");

            let stack = Stack::new()
                .resolve_with(&env.var("INFERENCE_NOVA_SONIC_MODEL", "amazon.nova-sonic-v1:0"))
                .expect("resolves");
            assert_eq!(stack.model(Role::Speech), Some("amazon.nova-sonic-v1:0"));
        });
    }
}
