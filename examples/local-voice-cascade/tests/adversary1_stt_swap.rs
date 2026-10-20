//! Adversary pass 1 on `story:stt-swap-by-configuration`: the configuration value at its edges,
//! the refusals of the whisper.cpp configuration before anything opens, what the lowered `stt`
//! node still refuses, and, with the checkpoints, that the recogniser bound under each value
//! claims what its planned record claims and behaves as it claims.

#![cfg(all(target_os = "linux", feature = "local"))]

use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{Adapter, Capability, DeclareAdapter, Locality};
use inference_pipeline::{Declaration, PlanPipelineError};
use inference_testkit::cascade::CascadeRefused;
use inference_testkit::stt::inference_stt::{
    Recogniser, RecognitionHandle, RecognitionRequest, TranscriptEvent,
};
use inference_text::TurnId;
use inference_tts::Synthesiser;
use local_voice_cascade::local::{
    LocalConfig, LocalModels, LocalRecogniser, LocalRefused, SERVER_MODEL_VARIABLE,
    SERVER_URL_VARIABLE, STT_DIR_VARIABLE, STT_VARIABLE, SttBackend, SttConfig,
    declare_local_adapters, server_adapter, synthesiser,
};

/// A loopback server; nothing is sent to it before the first turn.
const LOOPBACK: &str = "http://127.0.0.1:9/v1";

/// `lookup` answering `value` for [`STT_VARIABLE`] and, when `others`, every other variable.
fn answering(value: &str, others: bool) -> impl Fn(&str) -> Option<String> {
    let value = value.to_owned();
    move |name: &str| {
        if name == STT_VARIABLE {
            Some(value.clone())
        } else {
            others.then(|| format!("value of {name}"))
        }
    }
}

#[test]
fn an_empty_miscased_padded_or_aliased_backend_is_refused_naming_it_before_any_checkpoint() {
    for value in [
        "",
        " ",
        "Whispercpp",
        "WHISPERCPP",
        "Sherpa-Onnx",
        "SHERPA-ONNX",
        "whisper.cpp",
        "whisper-cpp",
        "whisper_cpp",
        "sherpa_onnx",
        "sherpaonnx",
        "sherpa",
        " whispercpp",
        "whispercpp ",
        "whispercpp\n",
        "sherpa-onnx,whispercpp",
    ] {
        for others in [false, true] {
            let refused = LocalConfig::from_lookup(answering(value, others))
                .expect_err("the value names no recogniser");
            assert_eq!(
                refused,
                LocalRefused::UnknownBackend {
                    value: value.to_owned()
                },
                "{value:?}, other variables set: {others}"
            );
            let said = refused.to_string();
            assert!(
                said.contains(STT_VARIABLE)
                    && said.contains(&format!("{value:?}"))
                    && SttBackend::ALL
                        .iter()
                        .all(|backend| said.contains(backend.name())),
                "{said}"
            );
        }
        assert!(value.parse::<SttBackend>().is_err(), "{value:?}");
    }
}

#[test]
fn the_default_names_the_sherpa_onnx_checkpoint_when_only_the_whispercpp_one_is_set() {
    let only_whispercpp = |name: &str| match name {
        STT_VARIABLE | STT_DIR_VARIABLE => None,
        _ => Some(format!("value of {name}")),
    };
    assert_eq!(
        LocalConfig::from_lookup(only_whispercpp),
        Err(LocalRefused::Missing(STT_DIR_VARIABLE))
    );
    for backend in SttBackend::ALL {
        let other = SttBackend::ALL
            .into_iter()
            .find(|other| *other != backend)
            .expect("two backends");
        let refused = LocalConfig::from_lookup(|name: &str| {
            if name == STT_VARIABLE {
                Some(backend.name().to_owned())
            } else if name == backend.variable() {
                None
            } else {
                Some(format!("value of {name}"))
            }
        })
        .expect_err("the selected backend's checkpoint is missing");
        assert_eq!(
            refused,
            LocalRefused::Missing(backend.variable()),
            "{backend} with only {} set",
            other.variable()
        );
    }
}

#[test]
fn under_whispercpp_a_server_off_loopback_is_refused_before_the_checkpoint_is_read() {
    let config = LocalConfig {
        stt: SttConfig::WhisperCpp {
            model: "no such checkpoint".into(),
        },
        server_url: "http://192.0.2.1:8080/v1".to_owned(),
        server_model: "qwen".to_owned(),
        max_output_tokens: 32,
    };
    let refused = LocalModels::open(&config).expect_err("refused");
    assert!(
        matches!(refused, LocalRefused::NotLoopback { .. }),
        "{refused}"
    );
}

#[test]
fn each_backend_refuses_the_other_backend_s_checkpoint_as_its_own_adapter_s_refusal() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let a_file = manifest_dir.join("Cargo.toml");
    for (stt, whispercpp) in [
        (
            SttConfig::WhisperCpp {
                model: manifest_dir.clone(),
            },
            true,
        ),
        (
            SttConfig::WhisperCpp {
                model: manifest_dir.join("no such checkpoint"),
            },
            true,
        ),
        (
            SttConfig::WhisperCpp {
                model: a_file.clone(),
            },
            true,
        ),
        (SttConfig::SherpaOnnx { dir: a_file }, false),
    ] {
        let config = LocalConfig {
            stt: stt.clone(),
            server_url: LOOPBACK.to_owned(),
            server_model: "qwen".to_owned(),
            max_output_tokens: 32,
        };
        let refused = LocalModels::open(&config).expect_err("not a pinned checkpoint");
        assert_eq!(
            matches!(refused, LocalRefused::WhisperCpp(_)),
            whispercpp,
            "{stt:?}: {refused}"
        );
        assert_eq!(
            matches!(refused, LocalRefused::SherpaOnnx(_)),
            !whispercpp,
            "{stt:?}: {refused}"
        );
        assert!(
            std::error::Error::source(&refused).is_some(),
            "{stt:?}: the adapter's refusal is the source"
        );
    }
}

/// The adapter record of `backend`, otherwise as declared, at `locality`.
fn relocated(backend: SttBackend, locality: Locality) -> Adapter {
    let record = backend.adapter();
    Adapter::declare(DeclareAdapter {
        model_id: record.model_id(),
        backend: record.backend().to_owned(),
        contract_revision: record.contract_revision().to_owned(),
        execution_form: record.execution_form(),
        locality,
        capabilities: record.capabilities().to_vec(),
        max_concurrency: record.max_concurrency(),
    })
    .expect("a positive concurrency")
    .0
}

#[test]
fn the_lowered_stt_node_still_refuses_a_recogniser_at_a_locality_the_profile_does_not_admit() {
    let server = server_adapter(
        &LocalConfig {
            stt: SttConfig::WhisperCpp {
                model: PathBuf::new(),
            },
            server_url: LOOPBACK.to_owned(),
            server_model: "qwen".to_owned(),
            max_output_tokens: 32,
        }
        .server(),
    )
    .expect("the server's adapter is declared");
    for backend in SttBackend::ALL {
        for locality in [Locality::SelfHostedCluster, Locality::ExternalProvider] {
            let mut declaration = Declaration::new();
            let refused = declare_local_adapters(
                &mut declaration,
                [
                    relocated(backend, locality),
                    server.clone(),
                    synthesiser().adapter().clone(),
                ],
            )
            .expect_err("offline_strict admits neither");
            let stt = declaration
                .nodes()
                .find(|node| node.name() == "stt")
                .expect("the stt node is declared")
                .node_id();
            assert!(
                matches!(
                    &refused,
                    CascadeRefused::Unplannable(PlanPipelineError::LocalityDenied(denied))
                        if denied.node_id == stt && denied.locality == locality
                ),
                "{backend} at {locality:?}: {refused:?}"
            );
        }
        for locality in [Locality::SameProcess, Locality::LocalWorker] {
            declare_local_adapters(
                &mut Declaration::new(),
                [
                    relocated(backend, locality),
                    server.clone(),
                    synthesiser().adapter().clone(),
                ],
            )
            .unwrap_or_else(|refused| panic!("{backend} at {locality:?}: {refused}"));
        }
    }
}

/// The 16 kHz mono 16-bit PCM samples of the WAV file at `path`, in 10 ms frames.
fn frames(path: &Path) -> Vec<AudioFrame> {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let le = |at: usize| {
        usize::try_from(u32::from_le_bytes(
            bytes[at..at + 4].try_into().expect("four bytes"),
        ))
        .expect("fits usize")
    };
    let mut at = 12;
    let data = loop {
        assert!(at + 8 <= bytes.len(), "{} has a data chunk", path.display());
        let size = le(at + 4);
        if &bytes[at..at + 4] == b"data" {
            let end = (at + 8 + size).min(bytes.len());
            break &bytes[at + 8..end - (end - at - 8) % 2];
        }
        at += 8 + size + size % 2;
    };
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("16000"),
        channels: NonZeroU16::new(1).expect("1"),
    };
    let mut position = 0_u64;
    data.chunks(320)
        .map(|chunk| {
            let sample_count = u32::try_from(chunk.len() / 2).expect("fits u32");
            let frame = AudioFrame {
                format,
                position: SampleClockPosition::new(position).expect("on the clock"),
                sample_count,
                data: chunk.to_vec(),
            };
            position += u64::from(sample_count);
            frame
        })
        .collect()
}

#[test]
#[ignore = "needs the clip in INFERENCE_SHERPA_ONNX_STT_DIR and the checkpoint of the recogniser INFERENCE_CASCADE_STT selects"]
fn the_bound_recogniser_claims_what_its_planned_record_claims_and_shows_it_before_the_endpoint() {
    let config = LocalConfig::from_lookup(|name: &str| match name {
        SERVER_URL_VARIABLE => Some(LOOPBACK.to_owned()),
        SERVER_MODEL_VARIABLE => Some("qwen".to_owned()),
        _ => std::env::var(name).ok(),
    })
    .unwrap_or_else(|refused| panic!("{refused}"));
    let backend = config.stt.backend();
    let mut recogniser =
        LocalRecogniser::open(&config.stt).unwrap_or_else(|refused| panic!("{refused}"));
    assert_eq!(recogniser.backend(), backend);
    let (opened, record) = (recogniser.adapter().clone(), backend.adapter());
    assert_eq!(
        (
            opened.capabilities(),
            opened.locality(),
            opened.execution_form(),
            opened.backend(),
            opened.contract_revision(),
            opened.max_concurrency(),
        ),
        (
            record.capabilities(),
            record.locality(),
            record.execution_form(),
            record.backend(),
            record.contract_revision(),
            record.max_concurrency(),
        ),
        "{backend}: the record the cascade is planned over without weights is the opened one's"
    );
    let streams = opened.capabilities().contains(&Capability::OutputStreaming);
    let clip = std::env::var_os(STT_DIR_VARIABLE)
        .map(|dir| PathBuf::from(dir).join("test_wavs/0.wav"))
        .unwrap_or_else(|| panic!("{STT_DIR_VARIABLE} is not set: the clip is in it"));

    let mut recognition = recogniser
        .start(RecognitionRequest::new(
            RunId::generate(),
            TurnId::generate(),
            StreamId::generate(),
            NodeId::generate(),
            "transcript",
        ))
        .expect("one recognition");
    let mut before = Vec::new();
    for frame in frames(&clip) {
        recognition.offer_frame(frame).expect("input is open");
        before.extend(recognition.by_ref().map(|envelope| envelope.payload));
    }
    recognition.end_input().expect("input ends once");
    let after: Vec<TranscriptEvent> = recognition
        .by_ref()
        .map(|envelope| envelope.payload)
        .collect();
    let names =
        |events: &[TranscriptEvent]| events.iter().map(TranscriptEvent::name).collect::<Vec<_>>();
    println!(
        "{backend}: claims {:?}; before the end of input {} events, {:?} first; after it {:?}",
        opened.capabilities(),
        before.len(),
        names(&before).first(),
        names(&after)
    );
    let hypotheses_before = before
        .iter()
        .filter(|event| matches!(event, TranscriptEvent::Hypothesis(_)))
        .count();
    if streams {
        assert!(
            hypotheses_before > 0,
            "{backend} claims output streaming and showed no hypothesis before the end of input"
        );
    } else {
        assert!(
            before.is_empty(),
            "{backend} claims no output streaming and delivered {:?} before the end of input",
            names(&before)
        );
    }
    let committed: Vec<&str> = before
        .iter()
        .chain(&after)
        .filter_map(|event| match event {
            TranscriptEvent::Committed(committed) => Some(committed.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        committed
            .iter()
            .any(|text| text.to_lowercase().contains("nightfall")),
        "{backend} committed the clip: {committed:?}"
    );
    assert!(
        matches!(after.last(), Some(TranscriptEvent::Usage(_))),
        "{backend} ends with its usage record: {:?}",
        names(&after)
    );
}
