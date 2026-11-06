//! Adversary pass 1 on story:sdk-crate: what `inference::Error` promises beyond the cases the
//! unit's own tests build, and the paths later SDK stories name through this crate's `lib.rs`.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use inference::core::id::NodeId;
use inference::core::model::{AdapterId, Locality};
use inference::core::policy::{ProfileId, ProfileKind};
use inference::pipeline::LocalityDenied;
use inference::{Error, ErrorKind};

fn within<T: Send + 'static>(case: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(case());
    });
    finished
        .recv_timeout(Duration::from_secs(20))
        .expect("the case finishes within 20 s without panicking")
}

fn serde_name<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .expect("serializes")
        .as_str()
        .expect("a unit variant serializes as a string")
        .to_owned()
}

fn denial(locality: Locality, profile: ProfileKind) -> Error {
    Error::ProfileDenies {
        role: "llm".to_owned(),
        adapter: "remote".to_owned(),
        locality,
        profile,
        source: LocalityDenied {
            node_id: NodeId::generate(),
            adapter_id: AdapterId::generate(),
            locality,
            profile_id: ProfileId::generate(),
        },
    }
}

#[test]
fn every_locality_and_profile_is_written_as_its_serde_name() {
    within(|| {
        for locality in Locality::ALL {
            for profile in ProfileKind::ALL {
                let shown = denial(locality, profile).to_string();
                let at = format!("which runs at {},", serde_name(locality));
                let under = format!("the profile {} does not admit", serde_name(profile));
                assert!(shown.contains(&at), "{at:?} missing from {shown}");
                assert!(shown.contains(&under), "{under:?} missing from {shown}");
            }
        }
    });
}

#[test]
fn a_role_unbound_names_the_variable_and_table_of_every_role() {
    within(|| {
        for (role, variable) in [
            ("stt", "INFERENCE_STT_ADAPTER"),
            ("llm", "INFERENCE_LLM_ADAPTER"),
            ("tts", "INFERENCE_TTS_ADAPTER"),
            ("speech", "INFERENCE_SPEECH_ADAPTER"),
        ] {
            let shown = Error::RoleUnbound {
                role: role.to_owned(),
            }
            .to_string();
            assert!(shown.contains(variable), "{variable} missing from {shown}");
            assert!(
                shown.contains(&format!("[{role}]")),
                "[{role}] missing from {shown}"
            );
        }
    });
}

#[test]
fn the_error_crosses_threads_and_boxes_as_a_std_error() {
    fn shareable<T: Send + Sync + 'static + std::error::Error>() {}
    shareable::<Error>();
    within(|| {
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(Error::NoPrices);
        assert!(
            boxed.to_string().starts_with("no price document is named"),
            "{boxed}"
        );
        let kind: ErrorKind = Error::NoPrices.kind();
        let copied = kind;
        assert_eq!(kind, copied);
        assert_eq!(copied, ErrorKind::NoPrices);
    });
}

#[test]
fn report_of_a_profile_denial_writes_the_planning_refusal_as_its_one_cause() {
    within(|| {
        let error = denial(Locality::ExternalProvider, ProfileKind::OfflineStrict);
        let Error::ProfileDenies { source, .. } = &error else {
            unreachable!("built as ProfileDenies")
        };
        let cause = format!("caused by: {source}");
        let mut out = Vec::new();
        let code = inference::report(Err(error), &mut out);
        let written = String::from_utf8(out).expect("utf-8");
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), 2, "{written}");
        assert!(
            lines[0].starts_with("error: the role llm is bound"),
            "{written}"
        );
        assert_eq!(lines[1], cause);
        assert_eq!(code, std::process::ExitCode::FAILURE);
    });
}

#[derive(Debug)]
struct Reset;

impl std::fmt::Display for Reset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("connection reset by peer")
    }
}

impl std::error::Error for Reset {}

#[test]
fn a_provider_error_returns_its_source_and_report_writes_it() {
    within(|| {
        let error = Error::Provider {
            adapter: "openai".to_owned(),
            message: "the session ended".to_owned(),
            source: Some(Box::new(Reset)),
        };
        let source =
            std::error::Error::source(&error).expect("a Provider with a source returns it");
        assert!(source.is::<Reset>());
        let mut out = Vec::new();
        let _ = inference::report(Err(error), &mut out);
        assert_eq!(
            String::from_utf8(out).expect("utf-8"),
            "error: the provider of the adapter \"openai\" failed: the session ended\n\
             caused by: connection reset by peer\n"
        );
        let without = Error::Provider {
            adapter: "openai".to_owned(),
            message: "the session ended".to_owned(),
            source: None,
        };
        assert!(std::error::Error::source(&without).is_none());
    });
}

#[test]
fn the_paths_later_sdk_stories_name_resolve_through_this_crate() {
    #[allow(
        dead_code,
        clippy::too_many_arguments,
        reason = "a compile-time check of paths, one argument per path, never called"
    )]
    fn names(
        _: &dyn inference::pipeline::Catalog,
        _: inference::pipeline::Declaration,
        _: inference::pipeline::PipelineId,
        _: inference::core::policy::ProfileId,
        _: inference::core::model::AdapterId,
        _: inference::core::model::Capability,
        _: inference::tools::Tool,
        _: Box<dyn inference::tools::ToolExecutor>,
        _: inference::tools::SideEffectClass,
        _: inference::text::Span,
        _: inference::facade::live::LiveSessions,
        _: inference::llm::llm_core::Usage,
        _: inference::__private::serde_json::Value,
        _: inference::__private::tokio::runtime::Builder,
    ) {
    }
    within(|| {
        let code = inference::__private::exit(Ok(()));
        assert_eq!(code, std::process::ExitCode::SUCCESS);
    });
}
