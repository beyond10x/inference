//! `story:adapter-descriptor-carrier`: `inference.model.Adapter` carries the
//! [`AdapterDescriptor`] its `DeclareAdapter` input names, and `ess/domains/model.yaml` declares
//! it on the input, the entity and the view `inference.model.Adapters`.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use inference_core::model::{
    Adapter, AdapterDescriptor, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality,
    ModelId, RateLimit,
};
use serde_json::json;
use uuid::Uuid;

const MODEL_DOMAIN: &str = include_str!("../../../ess/domains/model.yaml");

const DEADLINE: Duration = Duration::from_secs(20);

/// Runs `case` on its own thread and fails when it has not returned within [`DEADLINE`].
fn within(name: &str, case: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let runner = thread::spawn(move || {
        case();
        let _ = done.send(());
    });
    match finished.recv_timeout(DEADLINE) {
        Ok(()) => runner.join().expect("the case returned"),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            if let Err(panic) = runner.join() {
                std::panic::resume_unwind(panic);
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("{name} ran past {DEADLINE:?}"),
    }
}

fn descriptor() -> AdapterDescriptor {
    AdapterDescriptor {
        voices: vec!["alloy".to_owned(), "verse".to_owned()],
        codecs: vec!["pcm16".to_owned()],
        context_tokens: Some(32_000),
        max_input_duration_ms: Some(600_000),
        max_output_duration_ms: Some(120_000),
        token_budget: Some(4_096),
        state_scope: Some("session".to_owned()),
        rate_limits: vec![RateLimit {
            unit: "requests".to_owned(),
            limit: 60,
            window_ms: 60_000,
        }],
    }
}

#[test]
fn an_adapter_declared_with_a_descriptor_reads_it_back_and_carries_it_on_the_wire() {
    within("descriptor read back", || {
        let input = DeclareAdapter {
            model_id: ModelId::from(Uuid::from_u128(1)),
            backend: "realtime".to_owned(),
            contract_revision: "r1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: vec![Capability::OutputStreaming],
            max_concurrency: 2,
            descriptor: descriptor(),
        };
        let (adapter, _) = Adapter::declare_as(AdapterId::from(Uuid::from_u128(2)), input)
            .expect("positive concurrency");

        assert_eq!(adapter.descriptor(), &descriptor());
        let entity = serde_json::to_value(&adapter).expect("the entity serializes");
        assert_eq!(
            entity["descriptor"],
            serde_json::to_value(descriptor()).expect("the descriptor serializes")
        );
        assert_eq!(entity["descriptor"]["voices"], json!(["alloy", "verse"]));
    });
}

/// The declaration of `model.yaml` that starts at `header` and ends at the next top-level one.
fn block(header: &str) -> &'static str {
    MODEL_DOMAIN
        .split(header)
        .nth(1)
        .unwrap_or_else(|| panic!("model.yaml declares `{header}`"))
        .split("\n  - name: ")
        .next()
        .expect("a declaration")
}

#[test]
fn model_yaml_declares_the_descriptor_on_the_input_the_entity_and_the_view() {
    within("model.yaml carrier", || {
        let field = "      - name: descriptor\n        type: inference.model.AdapterDescriptor\n";
        for header in [
            "  - name: inference.model.DeclareAdapter\n",
            "  - name: inference.model.Adapter\n",
            "  - name: inference.model.Adapters\n",
        ] {
            assert!(
                block(header).contains(field),
                "`{}` carries `descriptor`",
                header.trim()
            );
        }
        assert!(
            block("  - name: inference.model.DeclareAdapter\n")
                .contains("          descriptor: input.descriptor\n"),
            "the declared outcome sets the descriptor from the input"
        );
        let declaration = MODEL_DOMAIN
            .split("  - name: inference.model.AdapterDescriptor\n")
            .next()
            .expect("text before the descriptor")
            .rsplit("\n\n")
            .next()
            .expect("the descriptor's comment");
        assert!(
            !declaration.contains("UNMAPPED"),
            "the descriptor's carrier is no longer unmapped: {declaration}"
        );
    });
}

/// A `DeclareAdapter` input on the wire, carrying `descriptor` as given.
fn declare_adapter_wire(descriptor: serde_json::Value) -> serde_json::Value {
    json!({
        "model_id": "00000000-0000-4000-8000-000000000001",
        "backend": "realtime",
        "contract_revision": "r1",
        "execution_form": "hosted_service",
        "locality": "external_provider",
        "capabilities": ["output_streaming"],
        "max_concurrency": 1,
        "descriptor": descriptor,
    })
}

#[test]
fn a_descriptor_without_its_lists_is_refused_as_ess_refuses_it() {
    within("descriptor lists required", || {
        assert!(
            serde_json::from_value::<AdapterDescriptor>(json!({})).is_err(),
            "an empty object leaves out the three lists model.yaml requires"
        );
        for list in ["voices", "codecs", "rate_limits"] {
            let mut descriptor = json!({"voices": [], "codecs": [], "rate_limits": []});
            descriptor.as_object_mut().expect("an object").remove(list);
            assert!(
                serde_json::from_value::<AdapterDescriptor>(descriptor).is_err(),
                "a descriptor without `{list}` is refused"
            );
        }
        assert!(
            serde_json::from_value::<DeclareAdapter>(declare_adapter_wire(json!({}))).is_err(),
            "DeclareAdapter refuses `\"descriptor\": {{}}`"
        );
        assert!(
            serde_json::from_value::<DeclareAdapter>(declare_adapter_wire(json!(null))).is_err(),
            "DeclareAdapter refuses a missing descriptor"
        );
        let declared: DeclareAdapter = serde_json::from_value(declare_adapter_wire(
            json!({"voices": [], "codecs": [], "rate_limits": []}),
        ))
        .expect("the three lists, each empty, with every optional part left out");
        assert_eq!(declared.descriptor, AdapterDescriptor::default());
    });
}
