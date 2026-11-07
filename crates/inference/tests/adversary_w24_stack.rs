//! Adversary cases for story:sdk-stack: the resolution order when the builder binds onto a stack
//! that is already resolved, `${VAR}` expansion edges, and what an `Env` holds.
//!
//! Each case builds its own `Env`, registers names no other case uses and finishes within 20 s.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use inference::core::model::{
    Adapter, AdapterDescriptor as ModelDescriptor, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference::llm::Generator;
use inference::llm::llm_core::{
    BoxFuture, Cancel, Capabilities, Id, Item, Model, Protocol, Provenance, StopReason, StreamSink,
    TurnObservation, TurnOutcome, TurnRequest,
};
use inference::{AdapterDescriptor, Binding, BindingSource, Env, ErrorKind, Registry, Role, Stack};

fn within<T: Send + 'static>(case: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(case());
    });
    finished
        .recv_timeout(Duration::from_secs(20))
        .expect("the case finishes within 20 s without panicking")
}

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
                provider: id("adversary"),
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

fn own_generator() -> Generator {
    let (adapter, _) = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "adversary own generator".to_owned(),
        contract_revision: "1".to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: Vec::new(),
        max_concurrency: 1,
        descriptor: ModelDescriptor::default(),
    })
    .expect("a concurrency of one is positive");
    Generator::new(adapter, Arc::new(OwnModel::new()))
        .expect("the adapter claims nothing the model does not declare")
}

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

/// docs/design/sdk.md §4.2: the builder comes first per role, "whatever the environment says".
/// §3.2's program hands `Stack::from_env()` (a resolved stack) to `run`, and story:sdk-agent's
/// `Agent::voice_with(stack).llm(..)` binds through `Stack::bind` onto it. The builder binding
/// must win over the environment's, not be refused as a role "bound twice by the builder".
#[test]
fn a_builder_binding_onto_a_stack_resolved_from_the_environment_wins() {
    within(|| {
        register_recording("adv-w24-from-env");
        register_recording("adv-w24-from-builder");
        let env = Env::new().var("INFERENCE_LLM_ADAPTER", "adv-w24-from-env");
        let from_env = Stack::new().resolve_with(&env).expect("resolves");
        assert_eq!(
            from_env.adapter(Role::Llm).map(|binding| binding.source()),
            Some(BindingSource::Environment)
        );
        let stack = from_env
            .bind(Binding::named(
                Role::Llm,
                "adv-w24-from-builder",
                BTreeMap::new(),
            ))
            .resolve_with(&env)
            .unwrap_or_else(|error| {
                panic!(
                    "the builder binds llm over the environment's binding \
                     (docs/design/sdk.md §4.2), but resolving refused: {error}"
                )
            });
        let binding = stack.adapter(Role::Llm).expect("llm is bound");
        assert_eq!(binding.name(), Some("adv-w24-from-builder"));
        assert_eq!(binding.source(), BindingSource::Builder);
    });
}

/// `Env::new()` holds no working directory, and resolve reads "every source but the builder from
/// that Env only". A relative `INFERENCE_STACK` must not be read against the process's current
/// directory (under `cargo test`, the crate directory, which holds a `Cargo.toml`).
#[test]
fn a_relative_stack_file_is_not_read_from_the_process_directory() {
    within(|| {
        assert!(
            std::path::Path::new("Cargo.toml").is_file(),
            "the case runs in the crate directory"
        );
        let resolved = Stack::new().resolve_with(&Env::new().var("INFERENCE_STACK", "Cargo.toml"));
        let error = resolved.expect_err(
            "an Env without a working directory has no place to find a relative stack file; \
             resolve read the process's current directory instead",
        );
        assert_eq!(error.kind(), ErrorKind::StackFile);
    });
}

/// Expansion is one pass: a value that itself holds `${..}` is not expanded again, a `$` without a
/// brace stays, and an expanded value never adds a key or reaches another role.
#[test]
fn expansion_is_one_pass_and_injects_no_key() {
    within(|| {
        let received = register_recording("adv-w24-expand");
        let env = Env::new()
            .var("INFERENCE_LLM_ADAPTER", "adv-w24-expand")
            .var("INFERENCE_LLM_URL", "${A}|$B|${C}")
            .var("A", "${C}")
            .var("C", "x\"\n[stt]\nadapter = \"mock\"")
            .var("B", "never");
        let stack = Stack::new().resolve_with(&env).expect("resolves");
        let keys = received.lock().expect("unpoisoned")[0].clone();
        assert_eq!(
            keys.get("url").map(String::as_str),
            Some("${C}|$B|x\"\n[stt]\nadapter = \"mock\"")
        );
        assert_eq!(keys.keys().map(String::as_str).collect::<Vec<_>>(), ["url"]);
        assert!(stack.adapter(Role::Stt).is_none());
    });
}

/// An empty `INFERENCE_<ROLE>_ADAPTER` is unset: the file below it binds the role.
#[test]
fn an_empty_adapter_variable_gives_way_to_the_file() {
    within(|| {
        register_recording("adv-w24-empty-var");
        let root = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            root.path().join("inference.toml"),
            "[llm]\nadapter = \"adv-w24-empty-var\"\n",
        )
        .expect("the file is written");
        let stack = Stack::new()
            .resolve_with(
                &Env::new()
                    .working_dir(root.path())
                    .var("INFERENCE_LLM_ADAPTER", " "),
            )
            .expect("resolves");
        assert_eq!(
            stack.adapter(Role::Llm).map(|binding| binding.source()),
            Some(BindingSource::WorkingDirectoryFile)
        );
    });
}
